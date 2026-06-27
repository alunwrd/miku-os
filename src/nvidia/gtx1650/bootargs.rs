// GSP-RM boot arguments for TU116 (libos init args + GSP_ARGUMENTS_CACHED)
//
// This is the piece that was missing between "the SEC2 booter DMAs GSP-RM
// into WPR2" and "GSP-RM is actually running and talking to us". The booter
// only gets GSP-RM into WPR2 and starts the RISC-V core; the running GSP-RM
// firmware then reads a set of boot arguments out of sysmem to discover
//
//    where the host/GSP message rings live (so it can post events to us),
//    where to write its three log buffers (LOGINIT / LOGINTR / LOGRM), the cold-vs-resume power state
//
// The handoff path (matching nouveau drivers/gpu/drm/nouveau/nvkm/subdev/
// gsp/r535.c, firmware line 570.144):
//
//   1) Host builds a shared-memory region holding the CMDQ + MSGQ rings,
//      prefixed by a flat page-table-entry array describing its own pages
//      (r535_gsp_shared_init)
//   2) Host builds a GSP_ARGUMENTS_CACHED ("rmargs") block pointing at that
//      shared region (r535_gsp_rmargs_init)
//   3) Host builds three contiguous log regions, each carrying its own PTE
//      array at byte offset 8 (create_pte_array)
//   4) Host builds a LibosMemoryRegionInitArgument[4] table naming those
//      four regions ("LOGINIT","LOGINTR","LOGRM","RMARGS") so libos inside
//      GSP-RM can map them (r535_gsp_libos_init)
//   5) The sysmem address of that libos table is written to GSP falcon
//      MAILBOX0/1 (BAR0 PGSP+0x040/0x044) before the SEC2 booter runs;
//      after the booter, the GSP FALCON_OS register (PGSP+0x080) is set to
//      the bootloader app version (r535_gsp_init / r535_gsp_booter_load)
//
// All structures here are #[repr(C)] and byte-exact with the firmware ABI
// (cross-checked against open-gpu-kernel-modules 535.113.01 headers and the
// nova-core 570.144 bindings, which share these layouts on Turing)

#![allow(dead_code)]

use core::mem::size_of;

use crate::serial_println;

use super::dma_buf::{DmaBuffer, DmaBufError};

/// GSP page granularity (always 4 KiB on Turing, independent of host page size)
pub const GSP_PAGE: usize = 0x1000;
/// Each command/message ring is 256 KiB (r535 default: gsp->shm.{cmdq,msgq}.size)
pub const QUEUE_SIZE: usize = 0x40000;
/// Per-direction ring element size: one GSP page (matches msgqTxHeader.msgSize)
pub const ELEMENT_SIZE: usize = GSP_PAGE;
/// Byte offset of the first ring element within a queue region (msgqTxHeader.entryOff)
pub const ENTRY_OFF: usize = GSP_PAGE;
/// Each log region is 64 KiB (gsp->loginit/logintr/logrm)
pub const LOG_REGION_SIZE: usize = 0x10000;

/// LibosMemoryRegionKind: physically contiguous region
pub const LIBOS_MEMORY_REGION_CONTIGUOUS: u8 = 1;
/// LibosMemoryRegionLoc: lives in sysmem (not framebuffer)
pub const LIBOS_MEMORY_REGION_LOC_SYSMEM: u8 = 1;

/// GSP_INIT_DONE event (NV_VGPU_MSG_EVENT_GSP_INIT_DONE). FIRST_EVENT is
/// 0x1000; GSP-RM posts this on the MSGQ once its runtime is up
pub const NV_VGPU_MSG_EVENT_GSP_INIT_DONE: u32 = 0x1001;

/// rpc_message_header_v.signature: "VGPU" little-endian
pub const GSP_RPC_SIGNATURE: u32 = 0x4756_5055;

/// Bytes from the start of a MSGQ ring element to the embedded RPC header.
/// Layout of GSP_MSG_QUEUE_ELEMENT (570.144, non-confidential-compute): authTagBuffer[16] aadBuffer[16] checkSum(4) seqNum(4) elemCount(4) pad(4) = 48 bytes,
/// then rpc_message_header_v. The auth/aad buffers exist even on
/// Turing without CC; they are simply left zero
pub const GSP_MSG_HDR_SIZE: usize = 48;
/// rpc_message_header_v.function lies 12 bytes into the RPC header
pub const RPC_FUNCTION_OFFSET: usize = 12;

/// Result of 'GspBootArgs::self_test': the key addresses plus a pass flag
/// for each structure verified against the firmware ABI
#[derive(Copy, Clone, Debug)]
pub struct BootArgsReport {
    pub shared_phys: u64,
    pub shared_pages: usize,
    pub pte_count: u32,
    pub cmdq_off: u64,
    pub msgq_off: u64,
    pub msg_count: u32,
    pub libos_phys: u64,
    pub rmargs_phys: u64,
    pub loginit_phys: u64,
    pub ptes_ok: bool,
    pub cmdq_ok: bool,
    pub rmargs_ok: bool,
    pub libos_ok: bool,
    pub log_ok: bool,
    pub ok: bool,
}

#[inline]
fn align_up(v: usize, a: usize) -> usize { (v + a - 1) & !(a - 1) }
#[inline]
fn div_round_up(v: usize, a: usize) -> usize { (v + a - 1) / a }

/// Pack an <=8-char ASCII name into a u64 the way libos does:
/// id = (id << 8) | c for each byte, MSB-first (r535_gsp_libos_id8)
fn libos_id8(name: &str) -> u64 {
    let mut id: u64 = 0;
    for &c in name.as_bytes().iter().take(8) {
        id = (id << 8) | c as u64;
    }
    id
}

/// msgqTxHeader (32 bytes). Field order is byte-exact with the firmware
/// struct; only the producer (cmdq = host, msgq = GSP) initializes its own
#[repr(C)]
#[derive(Copy, Clone, Default)]
struct MsgqTxHeader {
    version: u32,
    size: u32,
    msg_size: u32,
    msg_count: u32,
    write_ptr: u32,
    flags: u32,
    rx_hdr_off: u32,
    entry_off: u32,
}

impl MsgqTxHeader {
    const SIZE: usize = 32;
    fn write_to(&self, dst: &mut [u8]) {
        let words = [
            self.version, self.size, self.msg_size, self.msg_count,
            self.write_ptr, self.flags, self.rx_hdr_off, self.entry_off,
        ];
        for (i, w) in words.iter().enumerate() {
            dst[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
    }
}

// LibosMemoryRegionInitArgument: { u64 id8; u64 pa; u64 size; u8 kind; u8 loc; }
// padded to 32 bytes (8-aligned). Four of these fit in the 0x1000 libos page
const LIBOS_ARG_SIZE: usize = 32;

fn write_libos_arg(dst: &mut [u8], id8: u64, pa: u64, size: u64, kind: u8, loc: u8) {
    dst[0..8].copy_from_slice(&id8.to_le_bytes());
    dst[8..16].copy_from_slice(&pa.to_le_bytes());
    dst[16..24].copy_from_slice(&size.to_le_bytes());
    dst[24] = kind;
    dst[25] = loc;
    // bytes 26..32 are padding, already zero
}

/// GSP_ARGUMENTS_CACHED, 80 bytes (gsp_init_args.h):
///   MESSAGE_QUEUE_INIT_ARGUMENTS (48) + GSP_SR_INIT_ARGUMENTS (12) +
///   gpuInstance u32 (@60) + profilerArgs {u64 pa; u64 size} (@64)
/// MESSAGE_QUEUE_INIT_ARGUMENTS:
///   u64 sharedMemPhysAddr@0, u32 pageTableEntryCount@8, (pad@12),
///   u64 cmdQueueOffset@16, u64 statQueueOffset@24,
///   u64 locklessCmdQueueOffset@32, u64 locklessStatQueueOffset@40
fn write_rmargs(
    dst: &mut [u8],
    shared_mem_phys: u64,
    pte_count: u32,
    cmd_queue_offset: u64,
    stat_queue_offset: u64,
) {
    dst[0..8].copy_from_slice(&shared_mem_phys.to_le_bytes());
    dst[8..12].copy_from_slice(&pte_count.to_le_bytes());
    dst[16..24].copy_from_slice(&cmd_queue_offset.to_le_bytes());
    dst[24..32].copy_from_slice(&stat_queue_offset.to_le_bytes());
    // lockless queues unused (0); srInitArguments / gpuInstance / profilerArgs
    // all zero for a cold boot (already zeroed by the allocator)
}

/// Everything the host pins for the GSP-RM boot-arg handoff. Must stay alive
/// for the lifetime of the GSP session: GSP-RM reads these regions directly
pub struct GspBootArgs {
    /// Shared region: [PTE array | CMDQ (256 KiB) | MSGQ (256 KiB)]
    shared: DmaBuffer,
    /// Number of PTEs describing the shared region's own pages
    pte_count: u32,
    /// Byte offset of the CMDQ region within 'shared'
    cmdq_off: usize,
    /// Byte offset of the MSGQ region within 'shared'
    msgq_off: usize,
    /// Ring depth (elements per direction)
    msg_count: u32,
    /// GSP_ARGUMENTS_CACHED block
    rmargs: DmaBuffer,
    /// LibosMemoryRegionInitArgument[4] table (address handed to GSP MAILBOX)
    libos: DmaBuffer,
    /// GSP-RM log buffers
    loginit: DmaBuffer,
    logintr: DmaBuffer,
    logrm: DmaBuffer,
}

impl GspBootArgs {
    /// Build all boot-argument structures in pinned sysmem. Mirrors the
    /// host-side setup nouveau performs across r535_gsp_shared_init,
    /// r535_gsp_rmargs_init and r535_gsp_libos_init before the booter runs
    pub fn build() -> Result<Self, DmaBufError> {
        //           shared region (r535_gsp_shared_init)
        // PTE count covers both rings, plus the pages the PTE array itself
        // occupies, then page-aligned into its own leading region.
        let ring_pages = (QUEUE_SIZE + QUEUE_SIZE) / GSP_PAGE; // 128
        let mut pte_count = ring_pages + div_round_up(ring_pages * size_of::<u64>(), GSP_PAGE);
        let ptes_size = align_up(pte_count * size_of::<u64>(), GSP_PAGE);
        // The PTE region itself is 'ptes_size' bytes; recompute the count so it
        // describes every page of 'shared' (ptes + both rings)
        let total = ptes_size + QUEUE_SIZE + QUEUE_SIZE;
        pte_count = total / GSP_PAGE;

        let mut shared = DmaBuffer::alloc(total / GSP_PAGE)?;
        shared.zero();
        let base = shared.phys();

        let cmdq_off = ptes_size;
        let msgq_off = ptes_size + QUEUE_SIZE;
        let msg_count = ((QUEUE_SIZE - ENTRY_OFF) / ELEMENT_SIZE) as u32; // 63

        {
            let s = shared.as_mut_slice();
            // Flat PTE array describing shared's own pages: ptes[i] = base + i*page
            for i in 0..pte_count {
                let pte = base + (i * GSP_PAGE) as u64;
                s[i * 8..i * 8 + 8].copy_from_slice(&pte.to_le_bytes());
            }
            // CMDQ tx header (host is the producer). rxHdrOff points at the rx
            // readPtr that follows the 32-byte tx header in the same page
            let cmdq_tx = MsgqTxHeader {
                version: 0,
                size: QUEUE_SIZE as u32,
                msg_size: ELEMENT_SIZE as u32,
                msg_count,
                write_ptr: 0,
                flags: 1,
                rx_hdr_off: MsgqTxHeader::SIZE as u32,
                entry_off: ENTRY_OFF as u32,
            };
            cmdq_tx.write_to(&mut s[cmdq_off..cmdq_off + MsgqTxHeader::SIZE]);
            // MSGQ tx header is GSP-owned; GSP fills it on boot. Leave it zero :/
        }
        DmaBuffer::write_barrier();

        // rmargs (r535_gsp_rmargs_init)
        let mut rmargs = DmaBuffer::alloc(1)?;
        rmargs.zero();
        write_rmargs(
            rmargs.as_mut_slice(),
            base,
            pte_count as u32,
            cmdq_off as u64,
            msgq_off as u64,
        );
        DmaBuffer::write_barrier();

        // log regions, each with its own PTE array at offset 8
        let loginit = Self::alloc_log_region()?;
        let logintr = Self::alloc_log_region()?;
        let logrm = Self::alloc_log_region()?;

        // libos init args (r535_gsp_libos_init)
        let mut libos = DmaBuffer::alloc(1)?;
        libos.zero();
        {
            let s = libos.as_mut_slice();
            let entries = [
                ("LOGINIT", loginit.phys(), loginit.size() as u64),
                ("LOGINTR", logintr.phys(), logintr.size() as u64),
                ("LOGRM", logrm.phys(), logrm.size() as u64),
                ("RMARGS", rmargs.phys(), rmargs.size() as u64),
            ];
            for (i, (name, pa, size)) in entries.iter().enumerate() {
                let off = i * LIBOS_ARG_SIZE;
                write_libos_arg(
                    &mut s[off..off + LIBOS_ARG_SIZE],
                    libos_id8(name),
                    *pa,
                    *size,
                    LIBOS_MEMORY_REGION_CONTIGUOUS,
                    LIBOS_MEMORY_REGION_LOC_SYSMEM,
                );
            }
        }
        DmaBuffer::write_barrier();

        serial_println!(
            "[bootargs] shared @ {:#x} ({} pages, {} PTEs) cmdq@{:#x} msgq@{:#x} depth={}",
            base, total / GSP_PAGE, pte_count, cmdq_off, msgq_off, msg_count
        );
        serial_println!(
            "[bootargs] rmargs @ {:#x}  libos @ {:#x}  loginit @ {:#x} logintr @ {:#x} logrm @ {:#x}",
            rmargs.phys(), libos.phys(), loginit.phys(), logintr.phys(), logrm.phys()
        );

        Ok(Self {
            shared,
            pte_count: pte_count as u32,
            cmdq_off,
            msgq_off,
            msg_count,
            rmargs,
            libos,
            loginit,
            logintr,
            logrm,
        })
    }

    /// Allocate one 64 KiB log region and write its own contiguous PTE array
    /// starting at byte offset 8 (matching create_pte_array(data + 8, ...))
    fn alloc_log_region() -> Result<DmaBuffer, DmaBufError> {
        let pages = LOG_REGION_SIZE / GSP_PAGE;
        let mut buf = DmaBuffer::alloc(pages)?;
        buf.zero();
        let base = buf.phys();
        let s = buf.as_mut_slice();
        for i in 0..pages {
            let pte = base + (i * GSP_PAGE) as u64;
            let off = size_of::<u64>() + i * 8; // skip the leading u64
            s[off..off + 8].copy_from_slice(&pte.to_le_bytes());
        }
        DmaBuffer::write_barrier();
        Ok(buf)
    }

    /// Sysmem physical address of the libos init-args table. Goes into the
    /// GSP falcon MAILBOX0/1 before the booter runs
    pub fn libos_phys(&self) -> u64 { self.libos.phys() }

    /// Sysmem physical address of the shared CMDQ/MSGQ region
    pub fn shared_phys(&self) -> u64 { self.shared.phys() }

    pub fn pte_count(&self) -> u32 { self.pte_count }

    /// GSP-producer write pointer (msgq tx header writePtr @ +16)
    fn msgq_write_ptr(&self) -> u32 {
        let off = self.msgq_off + 16;
        let s = self.shared.as_slice();
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }

    /// Host-consumer read pointer (cmdq rx header readPtr @ cmdq_off + 32)
    /// nouveau wires gsp->msgq.rptr = &cmdq->rx.readPtr
    fn msgq_read_ptr(&self) -> u32 {
        let off = self.cmdq_off + MsgqTxHeader::SIZE;
        let s = self.shared.as_slice();
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }

    /// Read the rpc 'function' field of the MSGQ element at read-pointer 'rp'
    fn msgq_element_function(&self, rp: u32) -> u32 {
        let elem = self.msgq_off + ENTRY_OFF + (rp as usize % self.msg_count as usize) * ELEMENT_SIZE;
        let foff = elem + GSP_MSG_HDR_SIZE + RPC_FUNCTION_OFFSET;
        let s = self.shared.as_slice();
        u32::from_le_bytes(s[foff..foff + 4].try_into().unwrap())
    }

    /// Read the rpc 'signature' field of the MSGQ element at read-pointer 'rp'
    fn msgq_element_signature(&self, rp: u32) -> u32 {
        let elem = self.msgq_off + ENTRY_OFF + (rp as usize % self.msg_count as usize) * ELEMENT_SIZE;
        let soff = elem + GSP_MSG_HDR_SIZE + 4; // signature is rpc header word 1
        let s = self.shared.as_slice();
        u32::from_le_bytes(s[soff..soff + 4].try_into().unwrap())
    }

    // Read-back accessors used by the self-test to confirm the bytes that
    // landed in sysmem match the firmware ABI

    fn read_u32(buf: &DmaBuffer, off: usize) -> u32 {
        let s = buf.as_slice();
        u32::from_le_bytes(s[off..off + 4].try_into().unwrap())
    }
    fn read_u64(buf: &DmaBuffer, off: usize) -> u64 {
        let s = buf.as_slice();
        u64::from_le_bytes(s[off..off + 8].try_into().unwrap())
    }

    /// Build a boot-arg set, read every structure back out of sysmem, and
    /// confirm each field matches the value the firmware ABI requires. This
    /// exercises the exact code path 'boot' takes (real allocations, real
    /// byte layout) without starting any Falcon, so it is safe to run on a
    /// live card. All buffers are freed when the returned value drops
    pub fn self_test() -> Result<BootArgsReport, DmaBufError> {
        let ba = Self::build()?;
        let base = ba.shared.phys();

        // PTE array: identity-maps every page of the shared region
        let pte0 = Self::read_u64(&ba.shared, 0);
        let pte1 = Self::read_u64(&ba.shared, 8);
        let pte_last = Self::read_u64(&ba.shared, (ba.pte_count as usize - 1) * 8);
        let ptes_ok = pte0 == base
            && pte1 == base + GSP_PAGE as u64
            && pte_last == base + (ba.pte_count as u64 - 1) * GSP_PAGE as u64;

        // CMDQ tx header (host-owned producer)
        let tx = ba.cmdq_off;
        let cmdq_ok = Self::read_u32(&ba.shared, tx) == 0                         // version
            && Self::read_u32(&ba.shared, tx + 4) == QUEUE_SIZE as u32            // size
            && Self::read_u32(&ba.shared, tx + 8) == ELEMENT_SIZE as u32          // msgSize
            && Self::read_u32(&ba.shared, tx + 12) == ba.msg_count                // msgCount
            && Self::read_u32(&ba.shared, tx + 20) == 1                           // flags
            && Self::read_u32(&ba.shared, tx + 24) == MsgqTxHeader::SIZE as u32   // rxHdrOff
            && Self::read_u32(&ba.shared, tx + 28) == ENTRY_OFF as u32;           // entryOff

        // rmargs (GSP_ARGUMENTS_CACHED.messageQueueInitArguments)
        let rmargs_ok = Self::read_u64(&ba.rmargs, 0) == base                     // sharedMemPhysAddr
            && Self::read_u32(&ba.rmargs, 8) == ba.pte_count                      // pageTableEntryCount
            && Self::read_u64(&ba.rmargs, 16) == ba.cmdq_off as u64               // cmdQueueOffset
            && Self::read_u64(&ba.rmargs, 24) == ba.msgq_off as u64;              // statQueueOffset

        // libos init args: 4 named regions, kind=CONTIGUOUS loc=SYSMEM
        let names = ["LOGINIT", "LOGINTR", "LOGRM", "RMARGS"];
        let pas = [ba.loginit.phys(), ba.logintr.phys(), ba.logrm.phys(), ba.rmargs.phys()];
        let mut libos_ok = true;
        for (i, (name, pa)) in names.iter().zip(pas.iter()).enumerate() {
            let off = i * LIBOS_ARG_SIZE;
            libos_ok &= Self::read_u64(&ba.libos, off) == libos_id8(name)
                && Self::read_u64(&ba.libos, off + 8) == *pa
                && ba.libos.as_slice()[off + 24] == LIBOS_MEMORY_REGION_CONTIGUOUS
                && ba.libos.as_slice()[off + 25] == LIBOS_MEMORY_REGION_LOC_SYSMEM;
        }

        // log region PTE arrays start at byte offset 8 and identity-map
        let log_ok = Self::read_u64(&ba.loginit, 8) == ba.loginit.phys()
            && Self::read_u64(&ba.loginit, 8 + 8) == ba.loginit.phys() + GSP_PAGE as u64;

        Ok(BootArgsReport {
            shared_phys: base,
            shared_pages: ba.shared.size() / GSP_PAGE,
            pte_count: ba.pte_count,
            cmdq_off: ba.cmdq_off as u64,
            msgq_off: ba.msgq_off as u64,
            msg_count: ba.msg_count,
            libos_phys: ba.libos.phys(),
            rmargs_phys: ba.rmargs.phys(),
            loginit_phys: ba.loginit.phys(),
            ptes_ok,
            cmdq_ok,
            rmargs_ok,
            libos_ok,
            log_ok,
            ok: ptes_ok && cmdq_ok && rmargs_ok && libos_ok && log_ok,
        })
    }

    /// Poll the GSP-owned MSGQ until its write pointer advances past our read
    /// pointer (i.e. GSP-RM posted a message) or 'timeout_ns' of PTIMER wall
    /// clock elapses. On a message, returns its rpc (function, signature).
    /// 'bar0_ptimer' reads BAR0+0x9400 to bound the wait by real time
    pub fn poll_first_message(
        &self,
        read_ptimer: impl Fn() -> u32,
        timeout_ns: u64,
    ) -> Option<(u32, u32)> {
        let rp = self.msgq_read_ptr();
        let start = read_ptimer();
        let budget = timeout_ns.min(u32::MAX as u64) as u32;
        loop {
            // The producer header lives in sysmem GSP writes via DMA; re-read
            // each pass with an acquire fence so we observe its store ordering
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let wp = self.msgq_write_ptr();
            if wp != rp {
                let function = self.msgq_element_function(rp);
                let signature = self.msgq_element_signature(rp);
                return Some((function, signature));
            }
            if read_ptimer().wrapping_sub(start) >= budget {
                return None;
            }
            core::hint::spin_loop();
        }
    }
}
