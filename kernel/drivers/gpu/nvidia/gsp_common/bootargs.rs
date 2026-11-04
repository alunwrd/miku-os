// GSP-RM's boot arguments: the rings, GSP_ARGUMENTS_CACHED, the log buffers and
// the libos table naming them all (615)
//
// Whatever boots the GSP's RISC-V core (SEC2's booter on Turing, the FSP and
// GSP-FMC on Blackwell), GSP-RM finds its world through one table: the libos
// init arguments, `LibosMemoryRegionInitArgument` entries naming memory regions
// by an eight-character id. OpenRM builds it in `kgspSetupLibosInitArgs_IMPL`,
// and this follows its order (`dev/gsp-615.md`, section 5):
//
//   LOGINIT LOGINTR LOGRM LOGMNOC LOGDBG   GSP-RM's task logs, 64 KiB each
//   LOGROOT LOGRMON                        4 KiB each
//   LOGKRNL                                libos's own log, 64 KiB
//   RMARGS                                 GSP_ARGUMENTS_CACHED
//
// A log buffer holds its put pointer in its first word and its own physical
// address in its second (`_setupLogBufferBaremetal`); the records follow.
//
// GSP_ARGUMENTS_CACHED (184 bytes, `gsp_init_args.h`; offsets compiled from the
// header):
//
//   0  magic "GSP "  4 size  8 flags (STACK_IN_DMEM on a normal boot)
//   16 MESSAGE_QUEUE_INIT_ARGUMENTS: 16 flags, 24 shared memory, 32 command
//      queue offset, 40 status queue offset, 48 element header size, 52 slot,
//      56 largest element, 60 header alignment, 64 slot alignment (log2), 68
//      PTEs
//   80 SR arguments, 100 gpuInstance, 104 profiler, 120 sysmem heap,
//   136 RM state monitor buffer, 152 bindata (radix3 of ucodes-<ver>.bin),
//   168 VBIOS override
//
// The shared memory is a flat PTE array describing itself, then our command
// queue and GSP-RM's status queue, 256 KiB each (`GspMsgQueuesInit`). Each
// queue starts with its producer's MSGQ 2.0 header; we write ours, GSP-RM
// writes its own when it comes up (`rpc::queue`).

use crate::drivers::gpu::nvidia::le;
use core::mem::size_of;

use crate::serial_println;

use crate::mm::dma::{DmaBuffer, DmaBufError};

use super::rpc::{QueueState, RxGeometry, MSGQ_HEADER_SIZE, MSGQ_VERSION_MAJOR, MSG_HDR_SIZE, MSG_MAX_SIZE};

/// GSP page granularity
pub const GSP_PAGE: usize = 0x1000;
/// Each queue is 256 KiB (`defaultCommandQueueSize`, `defaultStatusQueueSize`)
pub const QUEUE_SIZE: usize = 0x40000;
/// Slot size (`queueElementSizeMin`)
pub const ELEMENT_SIZE: usize = GSP_PAGE;
/// Where the slots start in a queue: the header, aligned to a slot
/// (`msgqTxCreate`: `NV_ALIGN_UP(sizeof(msgqTxHeader), 1 << entryAlign)`)
pub const ENTRY_OFF: usize = GSP_PAGE;
/// `queueHeaderAlign` and `queueElementAlign` (log2)
const HEADER_ALIGN: u32 = 4;
const ELEMENT_ALIGN: u32 = 12;

/// LibosMemoryRegionKind: physically contiguous region
pub const LIBOS_MEMORY_REGION_CONTIGUOUS: u8 = 1;
/// LibosMemoryRegionLoc: lives in sysmem (not framebuffer)
pub const LIBOS_MEMORY_REGION_LOC_SYSMEM: u8 = 1;

/// The log regions, in the order OpenRM registers them, and their sizes
/// (`logInitValues`, release-build defaults; then LOGKRNL)
const LOGS: [(&str, usize); 8] = [
    ("LOGINIT", 0x10000),
    ("LOGINTR", 0x10000),
    ("LOGRM", 0x10000),
    ("LOGMNOC", 0x10000),
    ("LOGDBG", 0x10000),
    ("LOGROOT", 0x1000),
    ("LOGRMON", 0x1000),
    ("LOGKRNL", 0x10000),
];

/// `GSP_ARGUMENTS_MAGIC_VALUE`, "GSP " read as a word
const GSP_ARGUMENTS_MAGIC: u32 = u32::from_le_bytes(*b"GSP ");
pub const GSP_ARGUMENTS_SIZE: usize = 184;
/// `GSP_ARGUMENTS_FLAG_STACK_IN_DMEM`: GSP-RM runs its init task on a DMEM
/// stack (the default unless paging puts the RM stack elsewhere)
const GSP_ARGUMENTS_FLAG_STACK_IN_DMEM: u64 = 0x02;

/// The RM state monitor buffer, one page (`kgspAllocRmStateMonitorBuffer`)
const RM_STATE_MONITOR_SIZE: usize = GSP_PAGE;

/// Result of `GspBootArgs::self_test`: the key addresses plus a pass flag
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

/// Pack an <=8-char ASCII name into a u64 as libos does: one byte at a time,
/// the first character highest (`_kgspGenerateInitArgId`)
fn libos_id8(name: &str) -> u64 {
    let mut id: u64 = 0;
    for &c in name.as_bytes().iter().take(8) {
        id = (id << 8) | c as u64;
    }
    id
}

/// A MSGQ 2.0 header (`msgqTxHeader`, 32 bytes)
fn write_msgq_header(dst: &mut [u8], size: u32, msg_size: u32, msg_count: u32, entry_off: u32) {
    le::put_u32(dst, 0, MSGQ_VERSION_MAJOR as u32); // versionMajor | versionMinor 0 << 16
    le::put_u32(dst, 4, size);
    le::put_u32(dst, 8, msg_size);
    le::put_u32(dst, 12, msg_count);
    le::put_u32(dst, 16, entry_off);
    // reserved[3], zero
}

// LibosMemoryRegionInitArgument: { u64 id8; u64 pa; u64 size; u8 kind; u8 loc;
// }
// padded to 32 bytes (8-aligned). The table is one page, 128 entries
const LIBOS_ARG_SIZE: usize = 32;

fn write_libos_arg(dst: &mut [u8], id8: u64, pa: u64, size: u64) {
    le::put_u64(dst, 0, id8);
    le::put_u64(dst, 8, pa);
    le::put_u64(dst, 16, size);
    dst[24] = LIBOS_MEMORY_REGION_CONTIGUOUS;
    dst[25] = LIBOS_MEMORY_REGION_LOC_SYSMEM;
}

/// Everything the host pins for the GSP-RM boot-arg handoff, alive for the GSP
/// session: GSP-RM reads these regions directly
pub struct GspBootArgs {
    /// Shared region: [PTE array | command queue | status queue]
    shared: DmaBuffer,
    pte_count: u32,
    cmdq_off: usize,
    msgq_off: usize,
    /// Slots in our command queue
    msg_count: u32,
    /// GSP_ARGUMENTS_CACHED
    rmargs: DmaBuffer,
    /// LibosMemoryRegionInitArgument table (its address goes to GSP-FMC)
    libos: DmaBuffer,
    /// The log buffers, in `LOGS` order
    logs: alloc::vec::Vec<DmaBuffer>,
    /// The RM state monitor buffer GSP-RM writes its state into
    rm_state_monitor: DmaBuffer,
    /// Both rings' counters and the RPC sequence number, with the rings: a
    /// `GspRpc` is rebuilt for every call
    state: QueueState,
}

impl GspBootArgs {
    /// Build every boot-argument structure in pinned system memory, as
    /// `GspMsgQueuesInit`, `_kgspInitLibosLoggingStructures`,
    /// `kgspAllocRmStateMonitorBuffer`, `kgspSetupLibosInitArgs` and
    /// `kgspPopulateGspRmInitArgs` build them before the boot
    pub fn build() -> Result<Self, DmaBufError> {
        // The PTE count covers both queues plus the pages of the PTE array,
        // which is page-aligned (`_getMsgQueueParams`)
        let ring_pages = (QUEUE_SIZE + QUEUE_SIZE) / GSP_PAGE;
        let pte_count = ring_pages + div_round_up(ring_pages * size_of::<u64>(), GSP_PAGE);
        let ptes_size = align_up(pte_count * size_of::<u64>(), GSP_PAGE);
        let total = ptes_size + QUEUE_SIZE + QUEUE_SIZE;

        let mut shared = DmaBuffer::alloc(total / GSP_PAGE)?;
        shared.zero();
        let base = shared.phys();
        let cmdq_off = ptes_size;
        let msgq_off = ptes_size + QUEUE_SIZE;
        let msg_count = ((QUEUE_SIZE - ENTRY_OFF) / ELEMENT_SIZE) as u32; // 63

        {
            let s = shared.as_mut_slice();
            for i in 0..pte_count {
                le::put_u64(s, i * 8, base + (i * GSP_PAGE) as u64);
            }
            // Our header (`msgqTxCreate`). GSP-RM's stays zero until it
            // writes it
            write_msgq_header(&mut s[cmdq_off..cmdq_off + MSGQ_HEADER_SIZE],
                QUEUE_SIZE as u32, ELEMENT_SIZE as u32, msg_count, ENTRY_OFF as u32);
        }
        DmaBuffer::write_barrier();

        let mut logs = alloc::vec::Vec::with_capacity(LOGS.len());
        for (_, size) in LOGS {
            logs.push(Self::alloc_log_region(size)?);
        }
        let mut rm_state_monitor = DmaBuffer::alloc(RM_STATE_MONITOR_SIZE / GSP_PAGE)?;
        rm_state_monitor.zero();

        let mut rmargs = DmaBuffer::alloc(1)?;
        rmargs.zero();
        {
            let a = rmargs.as_mut_slice();
            le::put_u32(a, 0, GSP_ARGUMENTS_MAGIC);
            a[4..6].copy_from_slice(&(GSP_ARGUMENTS_SIZE as u16).to_le_bytes());
            le::put_u64(a, 8, GSP_ARGUMENTS_FLAG_STACK_IN_DMEM);
            // MESSAGE_QUEUE_INIT_ARGUMENTS, no encryption
            le::put_u64(a, 24, base);
            le::put_u64(a, 32, cmdq_off as u64);
            le::put_u64(a, 40, msgq_off as u64);
            le::put_u32(a, 48, MSG_HDR_SIZE as u32);
            le::put_u32(a, 52, ELEMENT_SIZE as u32);
            le::put_u32(a, 56, MSG_MAX_SIZE as u32);
            le::put_u32(a, 60, HEADER_ALIGN);
            le::put_u32(a, 64, ELEMENT_ALIGN);
            le::put_u32(a, 68, pte_count as u32);
            // SR arguments zero for a cold boot; gpuInstance 0; no profiler,
            // no sysmem heap (this is not a zero-FB chip)
            le::put_u64(a, 136, rm_state_monitor.phys());
            le::put_u64(a, 144, rm_state_monitor.size() as u64);
            // bindata: `set_bindata`, once the ucodes are staged
        }
        DmaBuffer::write_barrier();

        let mut libos = DmaBuffer::alloc(1)?;
        libos.zero();
        {
            let s = libos.as_mut_slice();
            for (i, ((name, _), buf)) in LOGS.iter().zip(logs.iter()).enumerate() {
                let off = i * LIBOS_ARG_SIZE;
                write_libos_arg(&mut s[off..off + LIBOS_ARG_SIZE], libos_id8(name), buf.phys(), buf.size() as u64);
            }
            let off = LOGS.len() * LIBOS_ARG_SIZE;
            write_libos_arg(&mut s[off..off + LIBOS_ARG_SIZE], libos_id8("RMARGS"), rmargs.phys(), rmargs.size() as u64);
        }
        // GSP-FMC reads this table through GSP_DMA_TARGET_NONCOHERENT_SYSTEM
        // (`GspFmcBootParams::new`), as OpenRM tells it to, but OpenRM's table
        // is uncached memory and ours is write-back through the HHDM. With No
        // Snoop on the link (`chipset::init_device_control`) the read goes
        // straight to DRAM, so the table has to be there
        libos.flush_to_memory();

        serial_println!(
            "[bootargs] shared @ {:#x} ({} pages, {} PTEs) cmdq@{:#x} msgq@{:#x} depth={}",
            base, total / GSP_PAGE, pte_count, cmdq_off, msgq_off, msg_count
        );
        serial_println!(
            "[bootargs] rmargs @ {:#x}  libos @ {:#x}  {} log regions, rm state monitor @ {:#x}",
            rmargs.phys(), libos.phys(), logs.len(), rm_state_monitor.phys()
        );

        Ok(Self {
            shared,
            pte_count: pte_count as u32,
            cmdq_off,
            msgq_off,
            msg_count,
            rmargs,
            libos,
            logs,
            rm_state_monitor,
            state: QueueState::default(),
        })
    }

    /// GSP-RM's bindata: the radix3 table of the ucodes file and its size
    /// (`GSP_ARGUMENTS_CACHED.bindataArgs`). Cleared with 0, 0 once `GSP_INIT`
    /// has answered and the ucodes are freed
    pub fn set_bindata(&mut self, radix3: u64, size: u64) {
        let a = self.rmargs.as_mut_slice();
        le::put_u64(a, 152, radix3);
        le::put_u64(a, 160, size);
        DmaBuffer::write_barrier();
    }

    /// One log region, its own physical address in its second word
    /// (`_setupLogBufferBaremetal`)
    fn alloc_log_region(size: usize) -> Result<DmaBuffer, DmaBufError> {
        let mut buf = DmaBuffer::alloc(size / GSP_PAGE)?;
        buf.zero();
        let base = buf.phys();
        le::put_u64(buf.as_mut_slice(), 8, base);
        DmaBuffer::write_barrier();
        Ok(buf)
    }

    /// The libos table's physical address, which goes to GSP-FMC
    pub fn libos_phys(&self) -> u64 { self.libos.phys() }

    /// The shared memory's physical address
    pub fn shared_phys(&self) -> u64 { self.shared.phys() }

    pub fn queue_state(&self) -> &QueueState { &self.state }
    pub fn queue_state_mut(&mut self) -> &mut QueueState { &mut self.state }

    /// Slots in our command queue (63)
    pub fn cmdq_count(&self) -> u32 { self.msg_count }

    /// One command-queue slot
    pub fn cmdq_slot_mut(&mut self, slot: u32) -> &mut [u8] {
        let off = self.cmdq_off + ENTRY_OFF + (slot % self.msg_count) as usize * ELEMENT_SIZE;
        &mut self.shared.as_mut_slice()[off..off + ELEMENT_SIZE]
    }

    /// GSP-RM's header of the status queue, zero until it has written it
    pub fn msgq_header(&self) -> [u8; MSGQ_HEADER_SIZE] {
        let mut h = [0u8; MSGQ_HEADER_SIZE];
        h.copy_from_slice(&self.shared.as_slice()[self.msgq_off..self.msgq_off + MSGQ_HEADER_SIZE]);
        h
    }

    /// The status queue's size, as we gave it
    pub fn msgq_size(&self) -> u32 { QUEUE_SIZE as u32 }

    /// One status-queue slot, where GSP-RM's header put it
    pub fn msgq_slot(&self, rx: RxGeometry, slot: u32) -> &[u8] {
        let off = self.msgq_off + rx.entry_off as usize + (slot % rx.msg_count) as usize * rx.msg_size as usize;
        &self.shared.as_slice()[off..off + ELEMENT_SIZE]
    }

    pub fn msgq_slot_mut(&mut self, rx: RxGeometry, slot: u32) -> &mut [u8] {
        let off = self.msgq_off + rx.entry_off as usize + (slot % rx.msg_count) as usize * rx.msg_size as usize;
        &mut self.shared.as_mut_slice()[off..off + ELEMENT_SIZE]
    }

    /// A log region by name, empty if there is none of that name
    pub fn log_slice(&self, name: &str) -> &[u8] {
        LOGS.iter()
            .position(|(n, _)| *n == name)
            .map_or(&[][..], |i| self.logs[i].as_slice())
    }

    /// GSP-RM's three task logs, which `kgsp::logs` reads
    pub fn loginit_slice(&self) -> &[u8] { self.log_slice("LOGINIT") }
    pub fn logintr_slice(&self) -> &[u8] { self.log_slice("LOGINTR") }
    pub fn logrm_slice(&self) -> &[u8] { self.log_slice("LOGRM") }

    /// Build a boot-arg set, read every structure back from memory, and confirm
    /// each field against the firmware ABI. Starts no falcon, so safe on a live
    /// card. The buffers go when the value drops
    pub fn self_test() -> Result<BootArgsReport, DmaBufError> {
        let ba = Self::build()?;
        let base = ba.shared.phys();
        let s = ba.shared.as_slice();
        let a = ba.rmargs.as_slice();
        let l = ba.libos.as_slice();

        let ptes_ok = le::u64_at(s, 0) == base
            && le::u64_at(s, 8) == base + GSP_PAGE as u64
            && le::u64_at(s, (ba.pte_count as usize - 1) * 8) == base + (ba.pte_count as u64 - 1) * GSP_PAGE as u64;

        let tx = ba.cmdq_off;
        let cmdq_ok = le::u16_at(s, tx) == 2 && le::u16_at(s, tx + 2) == 0
            && le::u32_at(s, tx + 4) == QUEUE_SIZE as u32
            && le::u32_at(s, tx + 8) == ELEMENT_SIZE as u32
            && le::u32_at(s, tx + 12) == ba.msg_count
            && le::u32_at(s, tx + 16) == ENTRY_OFF as u32
            && le::u32_at(s, ba.msgq_off) == 0;

        let rmargs_ok = le::u32_at(a, 0) == GSP_ARGUMENTS_MAGIC
            && le::u16_at(a, 4) == GSP_ARGUMENTS_SIZE as u16
            && le::u64_at(a, 8) == GSP_ARGUMENTS_FLAG_STACK_IN_DMEM
            && le::u64_at(a, 16) == 0
            && le::u64_at(a, 24) == base
            && le::u64_at(a, 32) == ba.cmdq_off as u64
            && le::u64_at(a, 40) == ba.msgq_off as u64
            && le::u32_at(a, 48) == 24 && le::u32_at(a, 52) == 4096 && le::u32_at(a, 56) == 65536
            && le::u32_at(a, 60) == 4 && le::u32_at(a, 64) == 12
            && le::u32_at(a, 68) == ba.pte_count
            && le::u64_at(a, 136) == ba.rm_state_monitor.phys()
            && le::u64_at(a, 144) == GSP_PAGE as u64;

        let mut libos_ok = true;
        let names = LOGS.iter().map(|(n, _)| *n).chain(core::iter::once("RMARGS"));
        let pas = ba.logs.iter().map(|b| b.phys()).chain(core::iter::once(ba.rmargs.phys()));
        for (i, (name, pa)) in names.zip(pas).enumerate() {
            let off = i * LIBOS_ARG_SIZE;
            libos_ok &= le::u64_at(l, off) == libos_id8(name)
                && le::u64_at(l, off + 8) == pa
                && l[off + 24] == LIBOS_MEMORY_REGION_CONTIGUOUS
                && l[off + 25] == LIBOS_MEMORY_REGION_LOC_SYSMEM;
        }
        // Nothing after RMARGS
        libos_ok &= le::u64_at(l, (LOGS.len() + 1) * LIBOS_ARG_SIZE) == 0;

        let log_ok = ba.logs.iter().all(|b| le::u64_at(b.as_slice(), 0) == 0 && le::u64_at(b.as_slice(), 8) == b.phys());

        Ok(BootArgsReport {
            shared_phys: base,
            shared_pages: ba.shared.size() / GSP_PAGE,
            pte_count: ba.pte_count,
            cmdq_off: ba.cmdq_off as u64,
            msgq_off: ba.msgq_off as u64,
            msg_count: ba.msg_count,
            libos_phys: ba.libos.phys(),
            rmargs_phys: ba.rmargs.phys(),
            loginit_phys: ba.logs[0].phys(),
            ptes_ok,
            cmdq_ok,
            rmargs_ok,
            libos_ok,
            log_ok,
            ok: ptes_ok && cmdq_ok && rmargs_ok && libos_ok && log_ok,
        })
    }

    /// Poll for GSP-RM's status-queue header, the first thing a GSP-RM writes,
    /// until `timeout_ns` of PTIMER time. Returns its (version, slot count).
    /// For the Turing shell path, with no transport of its own
    pub fn poll_status_queue(&self, read_ptimer: impl Fn() -> u32, timeout_ns: u64) -> Option<(u16, u32)> {
        let start = read_ptimer();
        let budget = timeout_ns.min(u32::MAX as u64) as u32;
        loop {
            core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);
            let h = self.msgq_header();
            let major = le::u16_at(&h, 0);
            if major != 0 {
                return Some((major, le::u32_at(&h, 12)));
            }
            if read_ptimer().wrapping_sub(start) >= budget {
                return None;
            }
            core::hint::spin_loop();
        }
    }
}
