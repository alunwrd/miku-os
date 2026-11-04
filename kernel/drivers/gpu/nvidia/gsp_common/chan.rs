// A GPU channel, and the copy engine on it
//
// A desktop does not push pixels over the bus: the copy engine moves buffers
// and the CPU queues methods into a ring. This file is that ring.
//
// The order comes from nouveau's `rm/r535/fifo.c` and `ce.c`, the sequence any
// RM client follows, over RPC to GSP-RM:
//
//   1. an address space, whose page directory is ours: RM is told about it
//      (`IS_EXTERNALLY_OWNED` plus `NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY`).
//      VER3 format, six levels
//   2. memory for the instance block, USERD and the method buffer
//   3. the channel object itself, `BLACKWELL_CHANNEL_GPFIFO_B`
//   4. bind it to an engine and enable scheduling
//   5. the engine class on the channel, `BLACKWELL_DMA_COPY_B`
//
// Each step is a round trip that succeeds or names its own error.

use crate::drivers::gpu::nvidia::le;
use super::rmapi::{RmClient, RmError};
use super::rpc::GspRpc;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;

extern crate alloc;
use alloc::vec::Vec;

/// `FERMI_VASPACE_A`, `refsrc/nouveau/nvkm/subdev/gsp/rm/r535/nvrm/vmm.h`
pub const CLASS_VASPACE: u32 = 0x0000_90f1;
/// `BLACKWELL_CHANNEL_GPFIFO_B`, the channel class for GB20x, from
/// `gb20x_gpu.fifo.chan.class` in `rm/gb20x.c`
pub const CLASS_CHANNEL_GPFIFO: u32 = 0x0000_ca6f;

/// `KEPLER_CHANNEL_GROUP_A`, `cla06c.h:33`: a timeslice group.
///
/// In OpenRM a channel outside a group does not exist. When the parent of a
/// channel allocation is not one of these, `kchannelConstruct`
/// (`refsrc/openrm/src/nvidia/src/kernel/gpu/fifo/kernel_channel.c:365`)
/// allocates one and puts its handle in the channel's `hPhysChannelGroup`.
///
/// The group owns what a channel needs: `KERNEL_GRAPHICS_CONTEXT` is allocated
/// as the group's child (`kernel_channel_group_api.c:408`), and
/// `kgrctxFromKernelChannel` (`kernel_graphics_context.c:81`) answers
/// `NV_ERR_OBJECT_NOT_FOUND` when a channel has no graphics-context object on
/// itself or its group, the error a rebuilt compute channel's context promotion
/// gets
pub const CLASS_CHANNEL_GROUP: u32 = 0x0000_a06c;
/// `BLACKWELL_DMA_COPY_B`, `gb20x_gpu.ce.class`
pub const CLASS_DMA_COPY: u32 = 0x0000_cab5;
/// `BLACKWELL_USERMODE_A`: the doorbell aperture
#[allow(dead_code)] // RM class/control id this driver does not issue yet
pub const CLASS_USERMODE: u32 = 0x0000_c761;

/// `NV2080_ENGINE_TYPE_COPY0`, `cl2080_notification.h`
pub const ENGINE_TYPE_COPY0: u32 = 0x0000_0009;

pub const NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY: u32 = 0x0080_1813;
#[allow(dead_code)] // RM class/control id this driver does not issue yet
pub const NV0080_CTRL_CMD_DMA_UNSET_PAGE_DIRECTORY: u32 = 0x0080_1814;
pub const NVA06F_CTRL_CMD_GPFIFO_SCHEDULE: u32 = 0xa06f_0103;
pub const NVA06F_CTRL_CMD_BIND: u32 = 0xa06f_0104;

/// `NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE`. Asked on the internal
/// subdevice, as `r535_fifo_runl_ctor` asks it: the size is a property of the
/// chip's copy engines
pub const NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE: u32 = 0x2080_2a08;

/// How large a channel's method buffer has to be, asked on GSP-RM's own
/// subdevice, each time a channel is built: a remembered answer outlives the RM
/// it came from
pub fn method_buffer_size(rpc: &mut GspRpc, internal: &RmClient) -> Result<u64, RmError> {
    let b = internal.control_query(rpc, internal.h_subdevice, NV2080_CTRL_CMD_CE_GET_FAULT_METHOD_BUFFER_SIZE, 4)?;
    if b.len() < 4 {
        return Err(RmError::ShortReply);
    }
    Ok(crate::drivers::gpu::nvidia::le::u32_at(&b, 0) as u64)
}

/// `NVC36F_CTRL_CMD_GPFIFO_GET_WORK_SUBMIT_TOKEN`, asked on the channel itself.
///
/// `gb202_chan_doorbell_handle` builds the doorbell value from
/// `BIT(30) | (runlist << 16) | chid`, where the runlist id comes from walking
/// the device info table
pub const NVC36F_CTRL_CMD_GPFIFO_GET_WORK_SUBMIT_TOKEN: u32 = 0xc36f_0108;

/// `NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE`, on the internal subdevice.
///
/// Under 570 the work submit token was not the doorbell value:
/// `kfifoGenerateWorkSubmitTokenHal_GB202` built the real one (`RUNLIST_ID`,
/// `VECTOR`, `RUNLIST_DOORBELL`) only in its `!RMCFG_FEATURE_PLATFORM_GSP`
/// branch, and our control is answered by GSP-RM, so it came back as the bare
/// channel id. 615 builds the doorbell value on GSP as well; the doorbell is
/// computed here as nouveau's `gb202_chan_doorbell_handle` does, and this gives
/// the runlist the copy engine is on
pub const NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE: u32 = 0x2080_1112;

/// `NV_VIRTUAL_FUNCTION_DOORBELL`, 0x30090 inside the virtual-function window
/// at BAR0 0xb80000 (`blackwell/gb202/dev_vm.h`; nouveau reaches the same
/// address as `vfn->addr.user + 0x90`). VECTOR is 11:0, RUNLIST_ID 22:16,
/// RUNLIST_DOORBELL 30:30, the token's layout
pub const DOORBELL: u32 = 0x00bb_0090;

/// USERD, `Nvca6fControl`: GPGet at 0x88, GPPut at 0x8c
const USERD_GP_GET: u64 = 0x88;
const USERD_GP_PUT: u64 = 0x8c;

// Handles. The high half is a tag so a stray handle in a log says what it was
pub const H_VASPACE: u32 = 0x9a50_0000;
/// One address space per process running its own programs, numbered from here.
/// The kernel's is `H_VASPACE`
pub const H_VASPACE_CLIENT: u32 = 0x9a51_0000;
/// `NVKM_RM_CHAN(chid)`: the driver's own handle for a channel, chid in the low
/// bits. Identical to nouveau's, so a handle in a GSP-RM log line means the
/// same
pub const H_CHANNEL: u32 = 0xf1f0_0000;
/// The timeslice group a channel hangs off, one per channel, tagged by chid
/// like the rest
pub const H_CHANGRP: u32 = 0xa06c_0000;
pub const H_COPY: u32 = 0xce00_0000;

/// Channel ids this driver has taken, one bit each.
///
/// Bit 0 starts set: `r570_fifo.rsvd_chids` is 1, and RM's golden context
/// channel takes that many from the bottom.
///
/// Two channels sharing an id share a USERD slot and an instance block,
/// silently until something runs. Ids go back on teardown.
///
/// Sixty-four is far fewer than the chip has and far more than this driver
/// opens, not a hardware limit
static CHIDS: spin::Mutex<u64> = spin::Mutex::new(1);

/// Whose work a channel carries: what a robust-channel report or an Xid names
/// by number, said in words (`kgsp::events`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// No channel of ours has this id: GSP-RM's own (the golden context channel
    /// is chid 0), or one already given back
    None,
    /// The kernel's copy-engine channel: the console and every frame
    Copy,
    /// The kernel's compute channel: the desktop's effects and the programs
    /// processes run without a GPU of their own
    Compute,
    /// A process's own compute channel (`gpu_program`)
    Process(u64),
}

impl Owner {
    pub fn describe(self) -> alloc::string::String {
        match self {
            Owner::None => alloc::string::String::from("not ours - GSP-RM's own"),
            Owner::Copy => alloc::string::String::from("the kernel's copy engine"),
            Owner::Compute => alloc::string::String::from("the kernel's compute channel"),
            Owner::Process(pid) => alloc::format!("pid {}'s own compute channel", pid),
        }
    }
}

static OWNERS: spin::Mutex<[Owner; 64]> = spin::Mutex::new([Owner::None; 64]);

/// Take the lowest free channel id for `owner`, or `None` when they are all out
pub fn alloc_chid(owner: Owner) -> Option<u32> {
    let mut m = CHIDS.lock();
    let n = m.trailing_ones();
    if n >= 64 {
        return None;
    }
    *m |= 1 << n;
    OWNERS.lock()[n as usize] = owner;
    Some(n)
}

/// Say whose a channel is, once that is known better than at allocation: a
/// process's channel is built by the same code as the kernel's
pub fn set_owner(chid: u32, owner: Owner) {
    if let Some(o) = OWNERS.lock().get_mut(chid as usize) {
        *o = owner;
    }
}

/// Whose channel `chid` is
pub fn owner_of(chid: u32) -> Owner {
    OWNERS.lock().get(chid as usize).copied().unwrap_or(Owner::None)
}

/// Give a channel id back. Idempotent, so a teardown may run twice
pub fn free_chid(chid: u32) {
    if chid == 0 || chid >= 64 {
        return;
    }
    *CHIDS.lock() &= !(1u64 << chid);
    OWNERS.lock()[chid as usize] = Owner::None;
}

/// The engine context pointer as GSP-RM wrote it, kept from the first binding
/// of a boot and replayed on every later one.
///
/// It points past the end of the main context buffer, at the per-subcontext
/// headers; see `bind_gr_context`, which has the measurement
pub static GR_ECTX: spin::Mutex<Option<(u32, u32)>> = spin::Mutex::new(None);

/// The instance block of the first compute channel that worked, word for word,
/// to diff a rebuilt channel against
pub static INST_SNAPSHOT: spin::Mutex<Option<(u32, u64, Vec<u32>)>> = spin::Mutex::new(None);

/// What the engine context words held before and after each binding.
///
/// GSP firmware writes them on a first promotion, so what it leaves is the only
/// ground truth for what they should contain. Printed by `nvidia rm-tree`
pub static ECTX_LOG: spin::Mutex<Vec<alloc::string::String>> = spin::Mutex::new(Vec::new());

/// Where the channel's engine state word lives in its instance block.
///
/// Not a published offset: nouveau does not write it and the swref headers have
/// no dword 20 of this page. It was measured; `bind_gr_context` has the
/// readings
pub const INST_ENGINE_STATE: u64 = 0x050;

/// "The engine context for this channel is installed."
///
/// Set by `NV2080_CTRL_CMD_GPU_PROMOTE_CTX` and nothing else in a channel's
/// bring-up, only on a channel with a graphics context
pub const INST_ENGINE_CTX_INSTALLED: u32 = 0x0000_0002;

/// Read one word of a channel's instance block and say what it holds.
///
/// Finds which step writes a field, which names it. The word at +0x050: its top
/// half is the GPFIFO size in bytes (`0x1000` here, and `GPFIFO_BYTES` is
/// `0x1000`), its bottom half flags, and a working channel has bit 1 where a
/// rebuilt one does not
pub fn probe_inst(bar0: &MmioRegion, inst: u64, chid: u32, stage: &str, say: &dyn Fn(&str)) {
    let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
    let v = w.read32(inst + INST_ENGINE_STATE);
    let line = alloc::format!("probe chid {} {:<14} +0x050 = {:#010x}", chid, stage, v);
    say(&line);
    push_log(line);
}

/// Keep only the last few: `chan-cycle 64` would otherwise grow this without
/// bound
fn push_log(line: alloc::string::String) {
    let mut l = ECTX_LOG.lock();
    if l.len() >= 32 {
        l.remove(0);
    }
    l.push(line);
}

/// A different group handle for every channel ever built
static GROUP_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn next_group() -> u32 {
    GROUP_GEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed) & 0xff
}

/// Keep bare channels, with no timeslice group.
///
/// `nochangrp` on the GRUB line
pub fn bare_channels() -> bool {
    crate::arch::x86_64::grub::cmdline_flag("nochangrp")
}

/// Every channel built gets a different one of these.
///
/// A fence leaving the kernel is `(epoch, seq)`, and the epoch stops a client
/// holding one across a teardown from comparing it against a restarted counter,
/// the mistake the signed comparison in `fence_passed` guards one level down
static EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

fn next_epoch() -> u32 {
    EPOCH.fetch_add(1, core::sync::atomic::Ordering::Relaxed)
}

/// Frames from channels torn down.
///
/// The rule of `shm::RETIRED`: RM has freed the channel, but the frames were
/// busmastered until a moment ago. The compositor drains this where it drains
/// the other, after its wait has proved the engines are finished with the frame
/// in flight
static RETIRED_BUFS: spin::Mutex<alloc::vec::Vec<crate::mm::dma::DmaBuffer>> =
    spin::Mutex::new(alloc::vec::Vec::new());

fn retire_buffers(bufs: alloc::vec::Vec<crate::mm::dma::DmaBuffer>) {
    if bufs.is_empty() {
        return;
    }
    RETIRED_BUFS.lock().extend(bufs);
}

/// Retire memory reachable by the GPU until a moment ago, for anything else a
/// teardown releases: a graphics context, a set of programs. Public since the
/// compute channel's context is owned a layer up
pub fn retire_dma(bufs: alloc::vec::Vec<crate::mm::dma::DmaBuffer>) {
    retire_buffers(bufs);
}

/// GSP-RM was unloaded. The channel ids go back to the one RM keeps for itself,
/// what was learned from that boot's first binding goes, and the retired frames
/// are freed: the engines that might read them stopped with GSP-RM
pub fn forget() -> usize {
    *CHIDS.lock() = 1;
    GR_ECTX.lock().take();
    INST_SNAPSHOT.lock().take();
    reap_buffers()
}

/// Free every frame a torn-down channel left behind. Returns how many
pub fn reap_buffers() -> usize {
    let taken = core::mem::take(&mut *RETIRED_BUFS.lock());
    let n = taken.len();
    drop(taken);
    n
}

/// How many frames are waiting for the engines. Should be zero between frames
pub fn retired_buffers() -> usize {
    RETIRED_BUFS.lock().len()
}

/// Which channel ids are taken, for `nvidia intr` and the teardown test
pub fn chids_in_use() -> u64 {
    *CHIDS.lock()
}
/// `CHID_PER_USERD`, `rm/r570/fifo.c`
const CHID_PER_USERD: u32 = 8;

/// Instance block size, `gf100_chan_inst.size`
pub const INST_SIZE: u64 = 0x1000;
/// USERD size, `gv100_chan_userd.size`
pub const USERD_SIZE: u64 = 0x200;
/// Instance block, then USERD one page above it, as the golden channel in
/// `r535_gr_oneinit`: one allocation, instance block at +0, USERD at +0x1000.
/// USERD is page aligned, since the flags address it as a page index plus a
/// slot inside the page
pub const CHAN_MEM_SIZE: u64 = 0x2000;

/// `NV_VASPACE_ALLOCATION_PARAMETERS`, 56 bytes in 615 (`pasid` at 48, then
/// padding). `vaSize` is 8-aligned
fn vaspace_params(externally_owned: bool) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 56];
    // index: NV_VASPACE_ALLOCATION_INDEX_GPU_NEW
    le::put_u32(&mut p, 0, 0u32);
    // flags: BIT(3) is IS_EXTERNALLY_OWNED: "this address space has a page
    // directory the client built and RM must not allocate one"
    let flags: u32 = if externally_owned { 1 << 3 } else { 0 };
    le::put_u32(&mut p, 4, flags);
    // vaSize, vaStartInternal, vaLimitInternal, bigPageSize, vaBase all zero:
    // an externally owned space has no size of its own, the page directory
    // decides what is reachable
    p
}

/// `NV0080_CTRL_DMA_SET_PAGE_DIRECTORY_PARAMS`, 32 bytes with the leading
/// `physAddress` 8-aligned
fn set_page_directory_params(phys: u64, entries: u32, vidmem: bool, h_vaspace: u32) -> Vec<u8> {
    let mut p = alloc::vec![0u8; 32];
    le::put_u64(&mut p, 0, phys);
    le::put_u32(&mut p, 8, entries);
    // APERTURE is 1:0: VIDMEM 0, SYSMEM_COH 1. Nothing else is set:
    // PRESERVE_PDES would keep RM's own entries, and a fresh directory has none
    let flags: u32 = if vidmem { 0 } else { 1 };
    le::put_u32(&mut p, 12, flags);
    le::put_u32(&mut p, 16, h_vaspace);
    // chId, subDeviceId, pasid stay zero
    p
}

/// An address space RM knows about and we own the page tables of
pub struct ChanVaSpace {
    pub handle: u32,
    /// Physical address of the root page directory, in video memory. Two
    /// entries of 8 bytes: VER3's top level is one bit of virtual address
    pub pdb: u64,
}

impl ChanVaSpace {
    /// Root page directory size. `gh100_vmm_desc_16[5]` is
    /// `{ PGD, 1, 8, 0x1000 }`: one bit of VA, 8-byte entries, in a 4 KiB page,
    /// so two entries, what `numEntries` says
    pub const ROOT_ENTRIES: u32 = 2;

    /// Hand RM a page directory and get an address space walking it.
    ///
    /// `pdb` must be 4 KiB of zeroed video memory. It stays ours: `gmmu` builds
    /// every level below it and RM never writes into it, as
    /// `IS_EXTERNALLY_OWNED` provides and as nouveau does for every channel
    /// under GSP-RM. `handle` is the caller's: the kernel's space is
    /// `H_VASPACE`, and a process with its own programs gets its own
    pub fn create(
        rpc: &mut GspRpc,
        rm: &RmClient,
        pdb: u64,
        handle: u32,
    ) -> Result<Self, RmError> {
        rm.alloc(
            rpc,
            rm.h_device,
            handle,
            CLASS_VASPACE,
            &vaspace_params(true),
        )?;
        rm.control(
            rpc,
            rm.h_device,
            NV0080_CTRL_CMD_DMA_SET_PAGE_DIRECTORY,
            &set_page_directory_params(pdb, Self::ROOT_ENTRIES, true, handle),
        )?;
        Ok(ChanVaSpace { handle, pdb })
    }

    /// Give the directory back before freeing the space, the order of
    /// `r535_mmu_vaspace_del`: RM holds a reference until told otherwise.
    ///
    /// A process's space is torn down when it exits; the kernel's lives until
    /// the machine is switched off
    pub fn destroy(self, rpc: &mut GspRpc, rm: &RmClient) {
        let mut p = alloc::vec![0u8; 8];
        le::put_u32(&mut p, 0, self.handle);
        let _ = rm.control(rpc, rm.h_device, NV0080_CTRL_CMD_DMA_UNSET_PAGE_DIRECTORY, &p);
        let _ = rm.free(rpc, self.handle);
    }
}

/// Write the channel's instance block.
///
/// The host's job, not RM's: `nvkm_chan_new_` calls `nvkm_vmm_join` before it
/// asks RM for anything, and for Volta and later that is `gv100_vmm_join` ->
/// `gp100_vmm_join` -> `gf100_vmm_join_`. RM is told where the block is,
/// through `instanceMem`. So the page directory reaches the hardware by two
/// routes, a number in an RPC and bytes in this page, which must agree.
///
/// Only PRAMIN can write it
fn write_inst_block(bar0: &MmioRegion, inst: u64, pdb: u64, va_limit: u64) {
    // `gf100_vmm_join_`: VER2 in bit 10, 64 KiB big pages in bit 11, the target
    // in bits 1:0, zero for a page directory in video memory, and the address
    // itself, whose low bits are the flag bits
    let base = (1u64 << 10) | (1u64 << 11) | pdb;

    let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
    // `gf100_chan_inst.zero` is true: RM reads fields here that nothing below
    // writes, so stale video memory would be read as those fields. Always, on a
    // rebuild too: otherwise the new channel inherits the previous one's USERD,
    // `gpget` and `gpput` included
    for off in (0..INST_SIZE).step_by(4) {
        w.write32(inst + off, 0);
    }
    w.write32(inst + 0x200, base as u32);
    w.write32(inst + 0x204, (base >> 32) as u32);
    w.write32(inst + 0x208, va_limit as u32);
    w.write32(inst + 0x20c, (va_limit >> 32) as u32);

    // `gv100_vmm_join` adds the subcontext table. One valid subcontext, index
    // zero, holding the same page directory; every other slot is 0x00000001,
    // the encoding for "no address space here", since a slot of zeroes names
    // page directory zero.
    //
    // For BAR1 the subcontext id is `FIFO_PDB_IDX_BASE` and RM reads the legacy
    // field at 0x200. For a channel both are read, so both are written
    w.write32(inst + 0x21c, 0);
    for i in 0..64u64 {
        if i == 0 {
            w.write32(inst + 0x2a0 + i * 0x10, base as u32);
            w.write32(inst + 0x2a4 + i * 0x10, (base >> 32) as u32);
        } else {
            w.write32(inst + 0x2a0 + i * 0x10, 1);
            w.write32(inst + 0x2a4 + i * 0x10, 1);
        }
        w.write32(inst + 0x2a8 + i * 0x10, 0);
    }
    // The mask of valid subcontexts: bit 0 only
    w.write32(inst + 0x298, 1);
    w.write32(inst + 0x29c, 0);
    drop(w);

    // USERD is read by the host unit as soon as the channel is scheduled;
    // `gf100_chan_userd_clear` zeroes it first
    let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
    for off in (0..USERD_SIZE).step_by(4) {
        w.write32(inst + INST_SIZE + off, 0);
    }
}

/// One `NV_MEMORY_DESC_PARAMS`: base, size, addressSpace, cacheAttrib, 24
/// bytes, the u64s 8-aligned. `addressSpace` is 2 for video memory and 1 for
/// system memory, as `r570_chan_alloc` writes it
fn mem_desc(p: &mut [u8], at: usize, base: u64, size: u64, address_space: u32, cache: u32) {
    le::put_u64(p, at, base);
    p[at + 8..at + 16].copy_from_slice(&size.to_le_bytes());
    p[at + 16..at + 20].copy_from_slice(&address_space.to_le_bytes());
    p[at + 20..at + 24].copy_from_slice(&cache.to_le_bytes());
}

/// `RM_ENGINE_TYPE_GR0`
pub const RM_ENGINE_TYPE_GR0: u32 = 1;

/// Every engine's `RM_ENGINE_TYPE` beside its index in the interrupt table,
/// from the device-info table `pick_copy_engine` reads
static ENGINE_MC: spin::Mutex<Vec<(u32, u32)>> = spin::Mutex::new(Vec::new());

/// The interrupt-table index (`MC_ENGINE_IDX_*`) of an engine, by its
/// `RM_ENGINE_TYPE`: what a non-stall vector is looked up by
pub fn mc_index(rm_engine_type: u32) -> Option<u16> {
    ENGINE_MC.lock().iter().find(|(t, _)| *t == rm_engine_type).map(|(_, m)| *m as u16)
}

/// A copy engine and the runlist it runs on
#[derive(Clone, Copy, Debug)]
pub struct CopyEngine {
    /// `NV2080_ENGINE_TYPE_COPY0 + n`, also the RM_ENGINE_TYPE value: the two
    /// enums agree on 9..0x12 for the copy engines and on 1 for GR0
    pub engine_type: u32,
    pub runlist: u32,
}

/// Pick a copy engine not sharing a runlist with graphics.
///
/// COPY0 on this chip is a GRCE, a copy engine sharing runlist 0 with graphics,
/// so a channel on it is a graphics channel to the host, and ours has no
/// graphics context: `ROBUST_CHANNEL_PBDMA_ERROR` attributed to GR, with NOCAT
/// saying `RC "GR"/"GR"` and a 523-byte `GR_STATUS` dump. nouveau's
/// `r535_fifo_runl_ctor` has the matching rule: "lone GRCEs don't appear to
/// function as async copy engines".
///
/// So: every engine from the device info table, the runlists carrying graphics
/// marked, and the first copy engine on none of them. An async LCE needs no
/// engine context: `r535_engn_ce` has neither a `ctor` nor a `bind`
pub fn pick_copy_engine(
    rpc: &mut GspRpc,
    internal: &RmClient,
    say: &dyn Fn(&str),
) -> Result<Option<CopyEngine>, RmError> {
    const HEADER: usize = 12;
    /// `NV2080_CTRL_FIFO_DEVICE_ENTRY`: 17 engine-data words in 615
    /// (`ENGINE_INFO_TYPE_IS_PREEMPTABLE` on the end), two PBDMA ids, two fault
    /// ids, a count and a 16-byte name
    const ENTRY: usize = 104;
    const MAX_ENTRIES: usize = 32;
    /// `ENGINE_INFO_TYPE_RM_ENGINE_TYPE`
    const COL_RM_ENGINE_TYPE: usize = 2;
    /// `ENGINE_INFO_TYPE_RUNLIST`
    const COL_RUNLIST: usize = 3;
    /// `ENGINE_INFO_TYPE_MC`: the engine's index in GSP-RM's interrupt table
    /// (`intr_vgpu.c:165` files it as the entry's `mcEngine`)
    const COL_MC: usize = 8;
    const RM_ENGINE_TYPE_GR_LAST: u32 = 8;
    const RM_ENGINE_TYPE_COPY9: u32 = 0x12;

    let b = internal.control_query(
        rpc,
        internal.h_subdevice,
        NV2080_CTRL_CMD_FIFO_GET_DEVICE_INFO_TABLE,
        HEADER + ENTRY * MAX_ENTRIES,
    )?;
    if b.len() < HEADER {
        return Err(RmError::ShortReply);
    }
    let n = (le::u32_at(&b, 4) as usize).min(MAX_ENTRIES);
    let word = |at: usize| le::u32_at(&b, at);

    let mut engines: Vec<(u32, u32)> = Vec::new();
    let mut mc: Vec<(u32, u32)> = Vec::new();
    for i in 0..n {
        let base = HEADER + i * ENTRY;
        if base + ENTRY > b.len() {
            break;
        }
        engines.push((
            word(base + COL_RM_ENGINE_TYPE * 4),
            word(base + COL_RUNLIST * 4),
        ));
        mc.push((word(base + COL_RM_ENGINE_TYPE * 4), word(base + COL_MC * 4)));
    }
    *ENGINE_MC.lock() = mc;

    // The map, once, one line per handful
    let mut line = alloc::string::String::new();
    for (t, r) in &engines {
        line.push_str(&alloc::format!(" {:#x}@{}", t, r));
    }
    say(&alloc::format!("RM: engines (type@runlist){}", line));

    let graphics_runlist = |r: u32| {
        engines
            .iter()
            .any(|(t, rr)| *rr == r && (RM_ENGINE_TYPE_GR0..=RM_ENGINE_TYPE_GR_LAST).contains(t))
    };
    let mut fallback = None;
    for (t, r) in &engines {
        if !(ENGINE_TYPE_COPY0..=RM_ENGINE_TYPE_COPY9).contains(t) {
            continue;
        }
        if !graphics_runlist(*r) {
            return Ok(Some(CopyEngine { engine_type: *t, runlist: *r }));
        }
        if fallback.is_none() {
            fallback = Some(CopyEngine { engine_type: *t, runlist: *r });
        }
    }
    // Every copy engine shares a runlist with graphics. The GRCE is all there
    // is, and the caller is told which
    if let Some(f) = fallback {
        say("RM: every copy engine shares a runlist with graphics - using a GRCE");
        return Ok(Some(f));
    }
    Ok(None)
}

/// One rectangle for the copy engine to move: `lines` rows of `line_len` bytes,
/// from a source of pitch `pitch_in` to a destination of pitch `pitch_out`.
/// Both addresses are physical.
///
/// Each copy says which memory each end is in: a frame reads windows from their
/// copies in video memory and the cursor patch from system memory
/// (`mayland-architecture.md`, "Window pixels the GPU reads live in video
/// memory"). OpenRM pushes `SET_SRC_PHYS_MODE` before every copy too
/// (`channelPushMemoryProperties`, `channel_utils.c:1018`)
#[derive(Clone, Copy)]
pub struct Copy2d {
    pub src: u64,
    pub dst: u64,
    pub pitch_in: u32,
    pub pitch_out: u32,
    pub line_len: u32,
    pub lines: u32,
    pub src_sysmem: bool,
    pub dst_sysmem: bool,
}

/// How many copies may go in one submission.
///
/// Twelve words each, fifteen where a copy's memory differs from the one
/// before, plus a short preamble, against a 64 KiB pushbuffer. A runaway list
/// is refused
pub const MAX_BATCH: usize = 48;

/// Words the longest possible submission writes: the preamble, the per-item
/// stream for `MAX_BATCH` items, then the fence.
///
/// Generous: compared against a 64 KiB buffer, twice the real figure costs
/// nothing and under it corrupts memory. `attach_ring` refuses a smaller
/// pushbuffer, which makes the unchecked `push.add(n)` writes in the submit
/// paths safe
pub const WORST_CASE_WORDS: usize = 66 + MAX_BATCH * 24;

/// At most this many submissions in flight on one channel: the pushbuffer is
/// cut into this many slots or as many as fit, whichever is fewer (3.5a)
pub const MAX_SLOTS: usize = 16;

/// `NVC9B5_LAUNCH_DMA_INTERRUPT_TYPE_NON_BLOCKING` in bits 6:5 (UVM's
/// `clc9b5.h:182`): the copy engine raises its non-stall interrupt when the
/// launch, its semaphore release included, is done
const LAUNCH_INTERRUPT_NON_BLOCKING: u32 = 2 << 5;

// Fences that sleep (`dev/gpu-memory-compute.md`, 3.5a)

/// A fence moved on some channel: the compute engine's or the copy engine's
/// non-stall interrupt. Every waiter looks at its own semaphore again
static FENCE: crate::kcore::isr_event::IsrEvent<8> = crate::kcore::isr_event::IsrEvent::new();
/// The two engines' non-stall vectors are open, and submissions ask for the
/// interrupt. Off until `blackwell::interrupts::fence_load` says so, and with
/// `nononstall` on the boot line
static FENCE_INTR: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Fence interrupts served, and how the sleeps of fence waits ended: woken by
/// an interrupt; the tick ran out with the work still running (a job longer
/// than a tick); the tick ran out and the fence had passed with no interrupt
/// (`MISSED`, an interrupt not arriving)
static FENCE_IRQS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FENCE_WOKEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FENCE_LONG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FENCE_MISSED: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// From the non-stall interrupt handler. Atomics and wakeups only
pub fn note_fence_interrupt() {
    FENCE_IRQS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    FENCE.notify();
}

pub fn set_fence_interrupts(on: bool) {
    FENCE_INTR.store(on, core::sync::atomic::Ordering::Release);
}

pub fn fence_interrupts() -> bool {
    FENCE_INTR.load(core::sync::atomic::Ordering::Acquire)
}

/// How fence waits went since boot, for `nvidia intr`
pub struct FenceStats {
    pub irqs: u64,
    /// Sleeps an interrupt ended
    pub woken: u64,
    /// Ticks that ran out with the work still running
    pub long: u64,
    /// Ticks that ran out on a fence that had passed with no interrupt. Growing
    /// with the frames means an interrupt not arriving
    pub missed: u64,
}

pub fn fence_stats() -> FenceStats {
    use core::sync::atomic::Ordering::Relaxed;
    FenceStats {
        irqs: FENCE_IRQS.load(Relaxed),
        woken: FENCE_WOKEN.load(Relaxed),
        long: FENCE_LONG.load(Relaxed),
        missed: FENCE_MISSED.load(Relaxed),
    }
}

/// How long a fence wait spins before it sleeps, and after every wake: most
/// frames' work is done by the time anyone looks, and an interrupt a hair ahead
/// of its semaphore's store must not cost a tick
const FENCE_SPIN_US: u64 = 50;

/// A fence, copied out from under its channel's lock so it can be waited for
/// without it. The kernel is not preempted: a thread sleeping with a spin lock
/// leaves others on its CPU spinning forever. The semaphore is in memory the
/// channel keeps until teardown, which retires it
#[derive(Clone, Copy)]
pub struct FenceHandle {
    sema: *const u32,
    value: u32,
}

unsafe impl Send for FenceHandle {}

impl FenceHandle {
    /// Reached or passed, by the same signed difference every fence check uses
    pub fn passed(&self) -> bool {
        !self.sema.is_null() && {
            let v = unsafe { core::ptr::read_volatile(self.sema) };
            v.wrapping_sub(self.value) as i32 >= 0
        }
    }

    /// Wait for it: spin a little, sleep until a fence interrupt or a tick,
    /// spin again, and so on. Only spins when it cannot be woken: interrupts
    /// off, the tree down, `nononstall`. `Err` carries the semaphore's value
    /// when time ran out
    pub fn wait(&self, timeout_us: u64) -> Result<(), u32> {
        if self.sema.is_null() {
            return Err(0);
        }
        let sw = crate::kcore::time::Stopwatch::start();
        let can_sleep = fences_wake() && x86_64::instructions::interrupts::are_enabled();
        let mut spin_until = FENCE_SPIN_US;
        loop {
            if self.passed() {
                return Ok(());
            }
            let el = sw.elapsed_us();
            if el >= timeout_us {
                return Err(unsafe { core::ptr::read_volatile(self.sema) });
            }
            if !can_sleep || el < spin_until {
                core::hint::spin_loop();
                continue;
            }
            use crate::kcore::isr_event::Woke;
            let seen = FENCE.seq();
            match FENCE.sleep_outcome(seen, 1, || self.passed()) {
                Woke::Moved => {
                    FENCE_WOKEN.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                // A whole tick and no interrupt. Passed means it came without
                // one; not passed means the engine is still at it
                Woke::TimedOut => {
                    let c = if self.passed() { &FENCE_MISSED } else { &FENCE_LONG };
                    c.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                }
                Woke::NotNeeded => {}
            }
            spin_until = sw.elapsed_us() + FENCE_SPIN_US;
        }
    }
}

/// Whether a fence can wake a sleeping waiter right now: the engines ask for
/// the interrupt and the tree is up
pub fn fences_wake() -> bool {
    fence_interrupts() && crate::drivers::gpu::nvidia::intr::active()
}

/// Sleep until the next fence interrupt or `ticks`, unless `ready`, for a
/// waiter watching more than one semaphore (`gpu_wait` watches a channel's
/// epoch too). Returns at once when fences are not interrupt-driven
pub fn sleep_for_fence(ticks: u64, ready: impl Fn() -> bool) {
    if !fences_wake() {
        return;
    }
    let seen = FENCE.seq();
    if !ready() {
        FENCE.sleep_unless(seen, ticks, ready);
    }
}

/// Why a channel was marked dead (`Channel::mark_failed`)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dead {
    /// GSP-RM killed it: a robust-channel report named its channel id
    RobustChannel,
    /// A frame waited for it past its budget, and the compositor let its fence
    /// go (`present::wait_for_engines` -> `unstick_engines`)
    FrameStall,
    /// Its process's own channel, killed by GSP-RM
    ClientKilled,
    /// It is being taken apart
    TornDown,
}

impl Dead {
    pub fn describe(self) -> &'static str {
        match self {
            Dead::RobustChannel => "GSP-RM killed it: robust-channel report",
            Dead::FrameStall => "a frame waited for it past its budget",
            Dead::ClientKilled => "GSP-RM killed its process's channel",
            Dead::TornDown => "taken down",
        }
    }
}

/// A GPFIFO channel with the copy engine bound to it.
///
/// `gpfifo_va` is where the ring was placed in the channel's address space: the
/// hardware follows it, the CPU writes the ring through PRAMIN
#[allow(dead_code)]
pub struct Channel {
    pub handle: u32,
    /// The timeslice group this channel hangs off, or zero under `nochangrp`.
    /// Freed after the channel: it is the parent
    pub group: u32,
    /// Did this channel make that group, or inherit one that outlives it?
    ///
    /// A group from a previous build carries RM's graphics context, and is not
    /// freed on any path
    pub owns_group: bool,
    pub chid: u32,
    /// Video memory: instance block at +0, USERD at +`INST_SIZE`
    pub inst: u64,
    /// Where the ring lives in the channel's own address space
    pub gpfifo_va: u64,
    pub entries: u32,
    /// What to write into the doorbell to tell the host unit there is work.
    /// Zero until `work_submit_token` has been asked
    pub doorbell: u32,

    // The ring, once `attach_ring` has been given it
    push: *mut u32,
    push_va: u64,
    gpfifo: *mut u32,
    sema: *mut u32,
    sema_va: u64,
    /// The same semaphore as a physical address.
    ///
    /// The copy engine reaches it through the channel's address space; the
    /// display reaches it physically, as `SET_SURFACE_ADDRESS_LO_*` on a window
    /// channel takes a physical address and an aperture
    sema_phys: u64,
    /// Next GPFIFO slot to write, modulo `entries`
    put: u32,
    /// The pushbuffer as a ring of `slots` submissions, each `slot_words` long,
    /// and the fence each one's last user releases: a submission waits only for
    /// the slot it is about to overwrite (3.5a)
    slot_words: usize,
    slots: usize,
    next_slot: usize,
    slot_fence: [u32; MAX_SLOTS],
    /// Semaphore payload of the last submit. Monotonic, so a stale word is
    /// never taken for this submit's completion
    seq: u32,
    /// GSP-RM has killed this channel, or the host gave up waiting on it.
    ///
    /// The engine will never run another method on it, so a submit is lost.
    /// Once this is set every entry point refuses, and a fence from it can
    /// never be waited on. Kept with why, for a rebuild
    failed: Option<Dead>,
    /// Fences the processor wrote on this channel because the engine would not
    forced: u32,
    /// Bumped every time a channel is built, so a fence handed to ring 3 before
    /// a teardown cannot be taken for one from its replacement. The sequence
    /// counter restarts; this does not
    epoch: u32,
    /// The ring, the pushbuffer, the semaphore page and the method buffer.
    /// `DmaBuffer` frees its frames on drop, and the frames live as long as the
    /// channel
    bufs: alloc::vec::Vec<crate::mm::dma::DmaBuffer>,
    /// USERD through BAR1, uncached, where GP_PUT is stored when BAR1 is
    /// physical (3.5b), or null, and the `USERD_EPOCH` it was handed out at. A
    /// window from another epoch is never used: BAR1 has been, or is being,
    /// bound back
    userd_cpu: *mut u8,
    userd_epoch: u32,
}

// The pointers are into DMA buffers the channel owns (`bufs`), which go on the
// retired list at teardown, so they outlive the channel; only the GPU also
// touches them. `userd_cpu` is an MMIO window that outlives everything, used
// only in its epoch
unsafe impl Send for Channel {}

/// The epoch of every USERD window handed out (3.5b,
/// `Channel::set_userd_window`).
///
/// Raised when BAR1 stops being physical, before it is bound back. A rebind
/// holds every channel's lock for at most 100 ms and goes on without one it
/// could not get; the epoch stops that channel's next submission from storing
/// into BAR1: an atomic, read under the channel's own lock
static USERD_EPOCH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(1);

/// BAR1 is about to stop reaching video memory: every USERD window is stale
/// from this call on
pub fn userd_windows_invalidate() {
    USERD_EPOCH.fetch_add(1, core::sync::atomic::Ordering::SeqCst);
}

/// The epoch a window handed out now would carry. Read before asking whether
/// BAR1 is physical; see `blackwell::channel::attach_userd`
pub fn userd_epoch() -> u32 {
    USERD_EPOCH.load(core::sync::atomic::Ordering::SeqCst)
}

/// GP_PUT stores, through BAR1 and through PRAMIN, for `nvidia intr`, and
/// BAR1 stores whose read-back did not say what was written
static GP_PUT_BAR1: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static GP_PUT_PRAMIN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static GP_PUT_READ_WRONG: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// `(through BAR1, through PRAMIN, BAR1 read back wrong)` since boot
pub fn gp_put_counts() -> (u64, u64, u64) {
    use core::sync::atomic::Ordering::Relaxed;
    (GP_PUT_BAR1.load(Relaxed), GP_PUT_PRAMIN.load(Relaxed), GP_PUT_READ_WRONG.load(Relaxed))
}

impl Channel {
    /// `NV_CHANNEL_GROUP_ALLOCATION_PARAMETERS`, `nvos.h`: 28 bytes in 615
    /// (`reserved` and `internalFlags` after the vGPU flag).
    ///
    /// Two fields carry anything: the address space every channel in the group
    /// shares, and the engine they run on
    fn group_params(h_vaspace: u32, engine_type: u32) -> Vec<u8> {
        let mut p = alloc::vec![0u8; 0x1c];
        le::put_u32(&mut p, 0x08, h_vaspace);
        le::put_u32(&mut p, 0x0c, engine_type);
        p
    }

    /// `NV_CHANNEL_ALLOC_PARAMS` for the 615 headers, 0x178 bytes.
    ///
    /// Written by hand: the C struct is full of `NV_DECLARE_ALIGNED(..., 8)`,
    /// and its holes are part of the wire format. Field offsets, compiled from
    /// `alloc_channel.h`:
    ///
    /// ```text
    ///   0x000 hObjectError      0x004 hObjectBuffer
    ///   0x008 gpFifoOffset:u64  0x010 gpFifoEntries   0x014 flags
    ///   0x018 hContextShare     0x01c hVASpace        0x020 hHandleVASpace
    ///   0x024 hUserdMemory[8]   0x048 userdOffset[8]:u64
    ///   0x088 engineType        0x08c cid   0x090 subDeviceId  0x094 hObjectEccError
    ///   0x098 instanceMem  0x0b0 userdMem  0x0c8 ramfcMem  0x0e0 mthdbufMem
    ///   0x0f8 hPhysChannelGroup 0x0fc internalFlags
    ///   0x100 errorNotifierMem  0x118 eccErrorNotifierMem
    ///   0x130 ProcessID 0x134 SubProcessID
    ///   0x138 encryptIv[3] 0x144 decryptIv[3] 0x150 hmacNonce[8] 0x170 tpcConfigID
    /// ```
    ///
    /// `hHandleVASpace` is read only by GSP-RM, and zero leaves the address
    /// space to `hVASpace`
    fn alloc_params(
        h_vaspace: u32,
        inst: u64,
        mthdbuf: u64,
        mthdbuf_size: u64,
        gpfifo_va: u64,
        entries: u32,
        engine_type: u32,
        chid: u32,
        group: u32,
    ) -> Vec<u8> {
        let mut p = alloc::vec![0u8; 0x178];
        le::put_u64(&mut p, 0x08, gpfifo_va);
        le::put_u32(&mut p, 0x10, entries);

        // The flags, one field at a time, from `r570_chan_alloc`. What it sets
        // to FALSE or DEFAULT is zero; what is left:
        //   CHANNEL_TYPE 1:0          PHYSICAL = 0
        //   PRIVILEGED_CHANNEL 5:5    TRUE: a kernel channel, the copy engine in physical mode
        //   USERD_INDEX_VALUE 10:8    chid within its USERD page
        //   USERD_INDEX_PAGE_VALUE 20:12  which USERD page
        //   USERD_INDEX_PAGE_FIXED 21:21  TRUE
        let userd_i = chid % CHID_PER_USERD;
        let userd_p = chid / CHID_PER_USERD;
        let flags: u32 = (1 << 5) | (userd_i << 8) | (userd_p << 12) | (1 << 21);
        le::put_u32(&mut p, 0x14, flags);

        // `hVASpace` at 0x1c, mutually exclusive with a group.
        // `kchannelConstruct` (`kernel_channel.c:428`) refuses a channel in a
        // group that also names an address space:
        //
        //     // TSG channel should specify a context share object, rather
        //     // than vaspace directly
        //     if (pChannelGpfifoParams->hVASpace != NV01_NULL_OBJECT)
        //         status = NV_ERR_INVALID_ARGUMENT;
        //
        // The group carries the address space, and the channel reaches it
        // through a context share. `hContextShare` at 0x18 stays zero: with no
        // share named, RM puts the group into legacy mode and allocates the
        // SYNC share itself (`:652-675`), taking the address space from the
        // group. `pKernelChannel->pVAS` comes from that share
        le::put_u32(&mut p, 0x1c, if group != 0 { 0 } else { h_vaspace });
        le::put_u32(&mut p, 0x88, engine_type);

        // instanceMem and ramfcMem name the same page: RAMFC is the first 0x200
        // bytes of the instance block, and `r570_chan_alloc` passes `inst_addr`
        // for both
        mem_desc(&mut p, 0x098, inst, INST_SIZE, 2, 1);
        mem_desc(&mut p, 0x0b0, inst + INST_SIZE, USERD_SIZE, 2, 1);
        mem_desc(&mut p, 0x0c8, inst, 0x200, 2, 1);
        mem_desc(&mut p, 0x0e0, mthdbuf, mthdbuf_size, 1, 0);

        // PRIVILEGE 1:0 = ADMIN, ERROR_NOTIFIER_TYPE 3:2 = NONE (1),
        // ECC_ERROR_NOTIFIER_TYPE 5:4 = NONE (1). NONE is not zero here: zero
        // is UNKNOWN, and RM would look for a notifier we never allocated
        let internal_flags: u32 = 1 | (1 << 2) | (1 << 4);
        le::put_u32(&mut p, 0xfc, internal_flags);

        // `hPhysChannelGroup` at 0xf8. `kchannelConstruct` writes the handle of
        // the group it allocated here; it is never zero in the reference
        le::put_u32(&mut p, 0xf8, group);
        p
    }

    /// Build the channel: instance block, then the object, then bind, then
    /// schedule.
    ///
    /// `inst` is `CHAN_MEM_SIZE` bytes of video memory. `mthdbuf` is system
    /// memory of the size RM asked for. `gpfifo_va` must be mapped in `vas`,
    /// since RM hands the address to the host unit
    pub fn create(
        rpc: &mut GspRpc,
        rm: &RmClient,
        bar0: &MmioRegion,
        vas: &ChanVaSpace,
        inst: u64,
        mthdbuf: u64,
        mthdbuf_size: u64,
        gpfifo_va: u64,
        gpfifo_bytes: u64,
        engine_type: u32,
        chid: u32,
        reuse_group: Option<u32>,
        say: &dyn Fn(&str),
    ) -> Result<Self, RmError> {
        // The address space reaches as far as one level-4 table does. RM on
        // Ampere and later never reads this back
        // (`kgmmuInstBlkVaLimitGet_f03539` returns zero), but nouveau writes
        // it; a limit too small silently truncates
        write_inst_block(bar0, inst, vas.pdb, (1u64 << 47) - 1);
        probe_inst(bar0, inst, chid, "zeroed", say);

        let handle = H_CHANNEL | chid;
        let entries = (gpfifo_bytes / 8) as u32;

        // The group first, and the channel hangs off it. `hPhysChannelGroup` in
        // the parameters is what RM records, and the parent decides whether
        // `kchannelConstruct` allocates a group of its own
        // (`kernel_channel.c:360`) or adopts the parent (`:415`). The explicit
        // allocation is the form sent to GSP-RM over RPC: an internally
        // allocated group carries `SKIP_RPC`.
        //
        // A group can outlive its channel, and for the graphics engine has to.
        // RM's `KernelGraphicsContext` is the group's child
        // (`kernel_channel_group_api.c:408`), while the engine's record of
        // which context buffers it initialised belongs to `KernelGraphics`. In
        // RM's model channels come and go inside a group, so the compute
        // channel hands its group back in here
        let group = match reuse_group {
            Some(g) => g,
            None if bare_channels() => 0,
            None => {
                // A new handle every time, not `H_CHANGRP | chid`.
                //
                // RM's `KernelGraphicsContext` is allocated in the group's
                // constructor and carries `bKGrMainCtxBufferInitialized` and
                // the engine context descriptor. A rebuild needs a fresh one,
                // and GSP-RM may not consider a just-freed handle fresh
                let g = H_CHANGRP | (next_group() << 8) | chid;
                rm.alloc(
                    rpc,
                    rm.h_device,
                    g,
                    CLASS_CHANNEL_GROUP,
                    &Self::group_params(vas.handle, engine_type),
                )?;
                g
            }
        };
        let fresh_group = reuse_group.is_none();
        let parent = if group != 0 { group } else { rm.h_device };

        if let Err(e) = rm.alloc(
            rpc,
            parent,
            handle,
            CLASS_CHANNEL_GPFIFO,
            &Self::alloc_params(
                vas.handle, inst, mthdbuf, mthdbuf_size, gpfifo_va, entries, engine_type, chid,
                group,
            ),
        ) {
            // Only a group made here is ours to take back; a reused one belongs
            // to the caller
            if group != 0 && fresh_group {
                let _ = rm.free(rpc, group);
            }
            return Err(e);
        }

        // Bind to the engine, then let it be scheduled: controls on the channel
        // object, in the order of `r535_chan_ramfc_write`. A channel scheduled
        // before it is bound has no engine to run on
        let mut bind = alloc::vec![0u8; 4];
        le::put_u32(&mut bind, 0, engine_type);
        // `NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS` is three NvBools in 615:
        // bEnable, bSkipSubmit, bSkipEnable
        let sched = alloc::vec![1u8, 0, 0];
        probe_inst(bar0, inst, chid, "alloc", say);
        let started = rm
            .control(rpc, handle, NVA06F_CTRL_CMD_BIND, &bind)
            .inspect(|_| probe_inst(bar0, inst, chid, "bind", say))
            .and_then(|_| rm.control(rpc, handle, NVA06F_CTRL_CMD_GPFIFO_SCHEDULE, &sched));
        if let Err(e) = started {
            // Freeing the channel takes it out of the group, and only then can
            // the group go: RM will not free a parent with a live child
            let _ = rm.free(rpc, handle);
            if group != 0 && fresh_group {
                let _ = rm.free(rpc, group);
            }
            return Err(e);
        }

        probe_inst(bar0, inst, chid, "scheduled", say);
        Ok(Channel {
            handle, group, owns_group: fresh_group, chid, inst, gpfifo_va, entries, doorbell: 0,
            push: core::ptr::null_mut(), push_va: 0,
            gpfifo: core::ptr::null_mut(),
            sema: core::ptr::null_mut(), sema_va: 0, sema_phys: 0,
            put: 0, slot_words: 0, slots: 0, next_slot: 0, slot_fence: [0; MAX_SLOTS],
            seq: 0, failed: None, forced: 0, epoch: next_epoch(),
            bufs: alloc::vec::Vec::new(),
            userd_cpu: core::ptr::null_mut(), userd_epoch: 0,
        })
    }

    /// Put the copy engine on the channel.
    ///
    /// `NVC0B5_ALLOCATION_PARAMETERS` is two words, version 1 and the engine,
    /// and the class is the chip's, `gb20x_gpu.ce.class`. Without it the
    /// channel can be scheduled but cannot execute methods
    pub fn alloc_copy_engine(
        &self,
        rpc: &mut GspRpc,
        rm: &RmClient,
        engine_type: u32,
    ) -> Result<(), RmError> {
        let mut p = alloc::vec![0u8; 8];
        le::put_u32(&mut p, 0, 1u32);
        le::put_u32(&mut p, 4, engine_type);
        // Derived from the chid: two channels allocating one engine handle
        // under one client are one object
        rm.alloc(rpc, self.handle, H_COPY | self.chid, CLASS_DMA_COPY, &p)
    }

    /// `gb202_chan_doorbell_handle`: `BIT(30) | (runlist << 16) | chid`, lining
    /// up with `NV_VIRTUAL_FUNCTION_DOORBELL`: RUNLIST_DOORBELL 30:30,
    /// RUNLIST_ID 22:16, VECTOR 11:0. Blackwell needs bit 30
    pub fn doorbell_handle(&self, runlist: u32) -> u32 {
        (1 << 30) | (runlist << 16) | self.chid
    }

    /// What GSP-RM calls this channel's work submit token. Cheap, and says
    /// whether RM knows the channel; from 615 on it should equal
    /// `doorbell_handle`
    pub fn work_submit_token(&self, rpc: &mut GspRpc, rm: &RmClient) -> Result<u32, RmError> {
        let b = rm.control_query(
            rpc, self.handle, NVC36F_CTRL_CMD_GPFIFO_GET_WORK_SUBMIT_TOKEN, 4,
        )?;
        if b.len() < 4 {
            return Err(RmError::ShortReply);
        }
        Ok(le::u32_at(&b, 0))
    }

    /// Spin until the semaphore has reached `payload`, or the budget runs out.
    ///
    /// Reached, not equals: the fence counter only goes up, and a waiter
    /// wanting the exact value misses it once the counter passes. The signed
    /// difference survives the counter wrapping, the rule of the hardware's own
    /// `ACQ_CIRC_GEQ`.
    ///
    /// A tight spin against the TSC: a console line moves in tens of
    /// microseconds
    fn wait_sema(sema: *const u32, payload: u32, timeout_us: u64) -> Result<(), u32> {
        let khz = crate::kcore::time::tsc_khz().max(1);
        let t0 = crate::kcore::time::rdtsc();
        loop {
            let v = unsafe { core::ptr::read_volatile(sema) };
            if v.wrapping_sub(payload) as i32 >= 0 {
                return Ok(());
            }
            let us = crate::kcore::time::rdtsc().wrapping_sub(t0) * 1000 / khz;
            if us >= timeout_us {
                return Err(v);
            }
            core::hint::spin_loop();
        }
    }

    /// Hand the channel the ring it submits through.
    ///
    /// `push`, `gpfifo` and `sema` are CPU pointers into system memory; the two
    /// `_va` values are the same memory in the channel's address space.
    /// `push_words` is how many u32 the pushbuffer holds.
    ///
    /// The submit paths bound their input by `MAX_BATCH` and write through a
    /// raw pointer with no further check. A pushbuffer too small for the worst
    /// case is refused here
    pub fn attach_ring(
        &mut self,
        push: *mut u32,
        push_va: u64,
        push_words: usize,
        gpfifo: *mut u32,
        sema: *mut u32,
        sema_va: u64,
        sema_phys: u64,
    ) -> Result<(), &'static str> {
        if push_words < WORST_CASE_WORDS {
            crate::serial_println!(
                "[chan] pushbuffer holds {} words, worst-case submission is {}",
                push_words, WORST_CASE_WORDS
            );
            return Err("pushbuffer too small for MAX_BATCH");
        }
        self.push = push;
        self.push_va = push_va;
        // Whole 64-byte lines per slot, as many as fit, at most `MAX_SLOTS`
        self.slot_words = WORST_CASE_WORDS.next_multiple_of(16);
        self.slots = (push_words / self.slot_words).clamp(1, MAX_SLOTS);
        self.next_slot = 0;
        self.slot_fence = [0; MAX_SLOTS];
        self.gpfifo = gpfifo;
        self.sema = sema;
        self.sema_va = sema_va;
        self.sema_phys = sema_phys;
        Ok(())
    }

    pub fn ring_attached(&self) -> bool {
        !self.push.is_null()
    }

    /// The next pushbuffer slot, once its last submission is done: where to
    /// write, where the engine reads it, and its index. The wait spins and is
    /// bounded, the caller holding the channel's lock; a submission would have
    /// to lap the ring to reach it
    fn take_slot(&mut self, timeout_us: u64) -> Result<(*mut u32, u64, usize), u32> {
        let s = self.next_slot;
        let owed = self.slot_fence[s];
        if owed != 0 {
            self.wait_fence(owed, timeout_us)?;
        }
        self.next_slot = (s + 1) % self.slots.max(1);
        let words = s * self.slot_words;
        Ok((unsafe { self.push.add(words) }, self.push_va + (words as u64) * 4, s))
    }

    /// `fence` on this channel, to be waited for once the channel's lock is
    /// released (`FenceHandle::wait`)
    pub fn fence_handle(&self, fence: u32) -> FenceHandle {
        FenceHandle { sema: self.sema, value: fence }
    }

    /// Everything submitted so far, as a handle. `None` when nothing was
    pub fn last_handle(&self) -> Option<FenceHandle> {
        (self.seq != 0 && !self.sema.is_null()).then(|| self.fence_handle(self.seq))
    }

    /// Will the engine still run what this channel is given?
    ///
    /// False once GSP-RM has posted a robust-channel error against it, or the
    /// host has forced a fence the engine never wrote. Work handed over then is
    /// lost silently, so it is refused
    pub fn healthy(&self) -> bool {
        self.failed.is_none()
    }

    /// Why this channel was marked dead, if it was
    pub fn dead_why(&self) -> Option<Dead> {
        self.failed
    }

    /// Take the channel apart, in the order making each step safe.
    ///
    /// The authority is RM, not the fence: a fence says the engine reached a
    /// value, not whether the host unit still fetches from the ring. RM takes
    /// the channel off the runlist and tears the instance block down inside
    /// GSP-RM, where the preempt happens (`r535_chan_ramfc_clear`).
    ///
    /// 1. refuse further submissions, then wait out what is queued;
    /// 2. off the runlist, so the host unit stops fetching;
    /// 3. free the engine object, then the channel object: RM preempts and
    ///    clears the instance block there;
    /// 4. only now take the ring, the pushbuffer and the semaphore out of the
    ///    address space, each ending in one invalidate;
    /// 5. the channel id goes back;
    /// 6. the frames go to a retire list, never straight to the allocator.
    ///
    /// The instance block is not freed: a permanent carve made once at boot and
    /// reused by the next channel with the same id, zeroed by
    /// `write_inst_block`. A busmastering host unit may still be reading it
    pub fn destroy(
        mut self,
        rpc: &mut GspRpc,
        rm: &RmClient,
        bar0: &MmioRegion,
        pdb: u64,
        engine_handle: u32,
        keep_group: bool,
        say: &dyn Fn(&str),
    ) {
        // 1. nothing new, and let what is queued finish. A budget: a channel
        //    being torn down may have stopped answering
        self.failed.get_or_insert(Dead::TornDown);
        if self.seq != 0 && !self.sema.is_null() {
            let _ = self.wait_last(200_000);
        }

        // 2. off the runlist. `NVA06F_CTRL_GPFIFO_SCHEDULE_PARAMS` is three
        //    NvBools: bEnable, bSkipSubmit, bSkipEnable, the control that put
        //    it on
        let sched = alloc::vec![0u8, 0, 0];
        if let Err(e) = rm.control(rpc, self.handle, NVA06F_CTRL_CMD_GPFIFO_SCHEDULE, &sched) {
            say(&alloc::format!(
                "chid {}: descheduling refused ({:?}) - carrying on", self.chid, e
            ));
        }

        // 3. the engine object first, then the channel it hangs off. RM will
        //    not free a parent with a live child.
        //
        // What RM says to each is reported. A refused free leaves the handle
        // registered, and the next allocation of that handle comes back as a
        // duplicate
        if engine_handle != 0 {
            match rm.free(rpc, engine_handle) {
                Ok(()) => say(&alloc::format!("chid {}: engine {:#x} freed", self.chid, engine_handle)),
                Err(e) => say(&alloc::format!(
                    "chid {}: RM refused to free engine {:#x}: {:?}", self.chid, engine_handle, e
                )),
            }
        }
        if let Err(e) = rm.free(rpc, self.handle) {
            say(&alloc::format!(
                "chid {}: RM refused to free channel {:#x}: {:?} - it stays registered, \
                 and the next allocation of that handle will be a duplicate",
                self.chid, self.handle, e
            ));
            // If RM still believes in this channel, taking its ring out of the
            // address space turns a fetch into a fault. Leak it
            core::mem::forget(self);
            return;
        }
        say(&alloc::format!("chid {}: channel {:#x} freed", self.chid, self.handle));

        // The group, now that its only child is gone. A group left registered
        // makes the next build's handle a duplicate
        if self.group != 0 && (keep_group || !self.owns_group) {
            say(&alloc::format!(
                "chid {}: group {:#x} kept - it owns the graphics context",
                self.chid, self.group));
        } else if self.group != 0 {
            match rm.free(rpc, self.group) {
                Ok(()) => say(&alloc::format!(
                    "chid {}: group {:#x} freed", self.chid, self.group)),
                Err(e) => say(&alloc::format!(
                    "chid {}: RM refused to free group {:#x}: {:?}", self.chid, self.group, e
                )),
            }
        }

        // 4. now the address space. RM has the channel; nothing fetches
        if pdb != 0 {
            for va in [self.gpfifo_va, self.push_va, self.sema_va] {
                if va != 0 {
                    crate::drivers::gpu::nvidia::vaspace::unmap(bar0, pdb, va);
                }
            }
        }

        // 5. the id, for whoever asks next
        free_chid(self.chid);

        // 6. and the frames, a frame later
        retire_buffers(core::mem::take(&mut self.bufs));
    }

    /// Point this channel's instance block at a graphics context.
    ///
    /// `gv100_ectx_bind` (`refsrc/nouveau/nvkm/engine/fifo/gv100.c:91`, used by
    /// Ampere too, `ga100.c:165`) writes this into the channel instance block
    /// when an engine context is attached:
    ///
    /// ```c
    /// addr  = cctx->vctx->vma->addr;
    /// addr |= 4ULL;
    /// nvkm_wo32(chan->inst, 0x210, lower_32_bits(addr));
    /// nvkm_wo32(chan->inst, 0x214, upper_32_bits(addr));
    /// nvkm_mo32(chan->inst, 0x0ac, 0x00010000, cctx ? 0x00010000 : 0);
    /// ```
    ///
    /// The address is the virtual address of the main context buffer in this
    /// channel's address space, with bit 2. Detaching writes zeroes, as
    /// OpenRM's `kchannelSetEngineContextMemDesc(..., NULL)`
    /// (`kernel_graphics_context.c:2695`) does.
    ///
    /// On a first promotion GSP-RM writes these words. On a rebuild the
    /// promotion is refused (`dev/gb206-display.md` section 14), and a channel
    /// with a zero pointer faults its first dispatch with
    /// `ROBUST_CHANNEL_PBDMA_ERROR` and a zero fault address.
    ///
    /// After the channel is allocated: 0x0ac is inside RAMFC, the first 0x200
    /// bytes, which RM fills during the allocation
    pub fn bind_gr_context(&self, bar0: &MmioRegion, gr_ctx_va: u64, say: &dyn Fn(&str)) {
        // The third thing, which nothing published names.
        //
        // `INST_ENGINE_STATE` bit 1 is set by `NV2080_CTRL_CMD_GPU_PROMOTE_CTX`
        // and nothing else. Measured on the card by reading the word after
        // every step of a channel's bring-up:
        //
        //   chid 1 (copy engine)  zeroed 0 -> alloc 0x11004 -> bind -> scheduled, never changes
        //   chid 2 (compute)      zeroed 0 -> alloc 0x01004 -> bind -> scheduled 0x01004
        //                         -> promoted 0x01006 -> class -> ring
        //
        // Without it the engine reads the context as absent and faults
        // `ROBUST_CHANNEL_PBDMA_ERROR` with a zero fault address. So: "the
        // engine context in this channel is installed", the hardware
        // counterpart of OpenRM's `pEngCtxDesc->pMemDesc != NULL` and of the
        // flag `gv100_ectx_bind` sets beside its pointer.
        let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);

        // What is there before this touches it. On a first promotion that is
        // what GSP-RM wrote, the only ground truth for these words. Printed
        let (was210, was214, was0ac) = (
            w.read32(self.inst + 0x210),
            w.read32(self.inst + 0x214),
            w.read32(self.inst + 0x0ac),
        );

        // What RM writes is authoritative, and it is not the start of the main
        // context buffer. Measured on the card: after the first promotion
        // GSP-RM leaves `0x01309004` here while the main buffer begins at
        // `0x01000000`. The difference, `0x309000`, is
        // `align_up(3107 KiB, 4096)`, the size RM reports for the main context,
        // so the pointer names what comes after it: the per-subcontext headers,
        // the `RM_PAGE_SIZE * maxSubctx` that `kgraphicsGetMainCtxBufferSize`
        // adds and this driver carries as `CtxBufSpec::extra`.
        //
        // So this remembers RM's value from the first binding and replays it:
        // the buffers do not move, as the group keeps the context across a
        // teardown.
        let mut saved = GR_ECTX.lock();
        if was210 != 0 || was214 != 0 {
            // RM has just written it. Leave it and keep it
            *saved = Some((was210, was214));
        } else if let Some((lo, hi)) = *saved {
            w.write32(self.inst + 0x210, lo);
            w.write32(self.inst + 0x214, hi);
        } else if gr_ctx_va != 0 {
            // Nothing measured and nothing there: the best guess is the
            // buffer's own address
            w.write32(self.inst + 0x210, (gr_ctx_va | 4) as u32);
            w.write32(self.inst + 0x214, (gr_ctx_va >> 32) as u32);
        }
        drop(saved);
        w.write32(
            self.inst + 0x0ac,
            if gr_ctx_va != 0 { was0ac | 0x0001_0000 } else { was0ac & !0x0001_0000 },
        );

        // Mark the context installed. A rebuild does not promote, so only this
        // sets it
        let state = w.read32(self.inst + INST_ENGINE_STATE);
        w.write32(
            self.inst + INST_ENGINE_STATE,
            if gr_ctx_va != 0 {
                state | INST_ENGINE_CTX_INSTALLED
            } else {
                state & !INST_ENGINE_CTX_INSTALLED
            },
        );

        let (now210, now214, now0ac) = (
            w.read32(self.inst + 0x210),
            w.read32(self.inst + 0x214),
            w.read32(self.inst + 0x0ac),
        );
        let line = alloc::format!(
            "chid {} inst {:#x}: 0x210 {:#010x}->{:#010x}  0x214 {:#010x}->{:#010x}  \
             0x0ac {:#010x}->{:#010x}",
            self.chid, self.inst, was210, now210, was214, now214, was0ac, now0ac
        );
        if crate::arch::x86_64::serial::tracing() {
            say(&line);
        }
        // Kept as well as said: the first of these lines is the only place
        // GSP-RM's own value for the engine context pointer can be read.
        // `nvidia rm-tree` prints them
        ECTX_LOG.lock().push(line);
    }

    /// Photograph the whole instance block and say how it differs from the last
    /// one.
    ///
    /// Between a working channel and a rebuilt one that faults, every
    /// difference in this page is a candidate.
    ///
    /// Taken when the channel is fully up, so it includes what RM wrote during
    /// the allocation and the context promotion
    pub fn snapshot_inst(&self, bar0: &MmioRegion, what: &str, restore: bool, say: &dyn Fn(&str)) {
        let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
        let mut now: Vec<u32> = Vec::with_capacity((INST_SIZE / 4) as usize);
        for off in (0..INST_SIZE).step_by(4) {
            now.push(w.read32(self.inst + off));
        }

        let mut prev = INST_SNAPSHOT.lock();
        // Restoring means something only against the same channel id and
        // instance block. A rebuild reuses both; otherwise it is a different
        // channel, which must not be overwritten
        let same = prev
            .as_ref()
            .map(|(c, i, _)| *c == self.chid && *i == self.inst)
            .unwrap_or(false);
        let restore = restore && same;
        match prev.as_ref().map(|(_, _, v)| v) {
            None => {
                let set = now.iter().filter(|v| **v != 0).count();
                let line = alloc::format!(
                    "inst {} ({}): {} non-zero word(s), kept for comparison",
                    what, self.chid, set);
                say(&line);
                push_log(line);
            }
            Some(old) => {
                let mut diffs = 0;
                for (i, (a, b)) in old.iter().zip(now.iter()).enumerate() {
                    if a != b {
                        diffs += 1;
                        // A page of words, only those that moved. Capped so a
                        // wholly different block cannot fill the screen
                        if diffs <= 24 {
                            let line = alloc::format!(
                                "inst {} ({}): +{:#05x} {:#010x} -> {:#010x}{}",
                                what, self.chid, i * 4, a, b,
                                if restore { " (restored)" } else { "" });
                            say(&line);
                            push_log(line);
                        }
                        // Make it identical to the one that works.
                        //
                        // A rebuilt compute channel has the same chid, instance
                        // block, context buffers at the same addresses, and
                        // rings, so a word that differs is one something did
                        // not write. Measured on the card: exactly one, at
                        // +0x050, only bit 1.
                        //
                        // RM writes the field and nothing published names it,
                        // so the value is copied
                        if restore {
                            w.write32(self.inst + (i as u64) * 4, *a);
                        }
                    }
                }
                let line = alloc::format!(
                    "inst {} ({}): {} word(s) differ from the working one", what, self.chid, diffs);
                say(&line);
                push_log(line);
            }
        }
        if prev.is_none() {
            *prev = Some((self.chid, self.inst, now));
        }
    }

    /// Take ownership of the system memory this channel runs out of.
    ///
    /// Called once, with the buffers the ring and the pushbuffer were carved
    /// from, after they are mapped and before the channel is used. The frames
    /// are released only by `Channel::destroy`, which knows the engine has
    /// stopped reading them
    pub fn own_buffers(&mut self, bufs: alloc::vec::Vec<crate::mm::dma::DmaBuffer>) {
        self.bufs.extend(bufs);
    }

    /// How many buffers this channel holds, for the teardown test
    pub fn buffer_count(&self) -> usize {
        self.bufs.len()
    }

    /// Which build of a channel this is: half of every fence leaving the kernel
    pub fn epoch(&self) -> u32 {
        self.epoch
    }

    /// Ready to be given work: a ring is attached and the engine still runs
    /// what it is given
    pub fn usable(&self) -> bool {
        // A dead GSP-RM stops every channel at once: OpenRM's
        // `kgspRcAndNotifyAllChannels`, done by asking
        self.ring_attached() && self.healthy() && !super::kgsp::fatal()
    }

    /// Record that this channel is dead, and why.
    ///
    /// One-way: a channel recovers by being torn down and rebuilt. The first
    /// reason stays
    pub fn mark_failed(&mut self, why: Dead) {
        if self.failed.is_none() {
            self.failed = Some(why);
            crate::kwarn!(
                "[nvidia] channel {:#x} (chid {}) marked dead ({}) - no further submissions",
                self.handle, self.chid, why.describe()
            );
        }
    }

    /// One line of what this channel was doing, for a stall report: the fence
    /// the last submission asked for, what the semaphore holds, and GP_GET /
    /// GP_PUT as USERD has them beside how many entries this side has put. GET
    /// behind PUT means the host never took the work (a doorbell that did not
    /// land, or a channel off its runlist); GET at PUT with the semaphore short
    /// means the engine took it and did not finish. USERD's GET can lag through
    /// PRAMIN, so a lag is a hint
    pub fn stall_line(&self, bar0: &MmioRegion) -> alloc::string::String {
        if self.sema.is_null() {
            return alloc::format!("chid {}: no ring", self.chid);
        }
        let v = unsafe { core::ptr::read_volatile(self.sema) };
        let (get, put) = self.userd_gp(bar0);
        alloc::format!("chid {}: fence {} wanted, semaphore at {}{}; GP_GET {} GP_PUT {} (ours {})",
            self.chid, self.seq, v,
            if v.wrapping_sub(self.seq) as i32 >= 0 { " (passed)" } else { "" },
            get, put, self.put)
    }

    /// One 2D copy, submitted and waited for.
    ///
    /// A rectangle of `lines` rows, `line_len` bytes each, from a source of
    /// pitch `pitch_in` to a destination of pitch `pitch_out`.
    /// `SET_{SRC,DST}_PHYS_MODE` names which side of the bus each end is on,
    /// and both addresses are physical; only the semaphore goes through the
    /// GMMU.
    ///
    /// Synchronous, so the pushbuffer can be reused from offset zero
    pub fn copy_2d(
        &mut self,
        bar0: &MmioRegion,
        src: u64,
        src_sysmem: bool,
        dst: u64,
        dst_sysmem: bool,
        pitch_in: u32,
        pitch_out: u32,
        line_len: u32,
        lines: u32,
        timeout_us: u64,
    ) -> Result<(), u32> {
        let fence = self.submit_2d(
            bar0, src, src_sysmem, dst, dst_sysmem, pitch_in, pitch_out, line_len, lines,
        )?;
        self.wait_fence(fence, timeout_us)
    }

    /// Several 2D copies in one submission, submitted and waited for once. See
    /// `submit_batch`
    pub fn copy_2d_batch(
        &mut self,
        bar0: &MmioRegion,
        copies: &[Copy2d],
        timeout_us: u64,
    ) -> Result<(), u32> {
        let fence = self.submit_batch(bar0, copies)?;
        self.wait_fence(fence, timeout_us)
    }

    /// Queue one 2D copy and ring the doorbell, without waiting.
    ///
    /// Returns the fence to pass to `wait_fence`. The engine reads the source
    /// while this returns, so nothing may rewrite it until the fence passes
    pub fn submit_2d(
        &mut self,
        bar0: &MmioRegion,
        src: u64,
        src_sysmem: bool,
        dst: u64,
        dst_sysmem: bool,
        pitch_in: u32,
        pitch_out: u32,
        line_len: u32,
        lines: u32,
    ) -> Result<u32, u32> {
        let one = [Copy2d { src, dst, pitch_in, pitch_out, line_len, lines, src_sysmem, dst_sysmem }];
        self.submit_batch(bar0, &one)
    }

    /// Queue a whole list of 2D copies as one pushbuffer, one gpfifo entry and
    /// one semaphore release, and ring the doorbell once.
    ///
    /// A composed frame arrives in one piece. One at a time, the preemptible
    /// compositor thread can lose the processor between the copy clearing where
    /// a window was and the copy putting it where it is, and the raster walks
    /// across the gap. Batched, the engine executes the list back to back.
    ///
    /// The copies are NON_PIPELINED, so they keep their order: the last written
    /// wins, the whole stacking model.
    ///
    /// The pushbuffer is reused from offset zero, so a submit first waits out
    /// the one before it
    pub fn submit_batch(
        &mut self,
        bar0: &MmioRegion,
        copies: &[Copy2d],
    ) -> Result<u32, u32> {
        self.submit_batch_after(bar0, copies, None)
    }

    /// The same batch, held until another channel's semaphore reaches a value.
    ///
    /// The other direction of `submit_compute_chain`'s `after`: the cursor is
    /// copied on top of a frame the compute engine is still blending, and the
    /// two engines run at once
    pub fn submit_batch_after(
        &mut self,
        bar0: &MmioRegion,
        copies: &[Copy2d],
        after: Option<(u64, u32)>,
    ) -> Result<u32, u32> {
        if self.failed.is_some() || self.push.is_null() || copies.is_empty() || copies.len() > MAX_BATCH {
            return Err(0);
        }
        if copies.iter().any(|c| c.lines == 0 || c.line_len == 0) {
            return Err(0);
        }
        let (slot_at, slot_va, ring_slot) = self.take_slot(200_000)?;

        fn hdr(method: u32, count: u32) -> u32 {
            (1 << 29) | (count << 16) | (0 << 13) | (method >> 2)
        }
        /// `SET_*_PHYS_MODE_TARGET`: LOCAL_FB 0, COHERENT_SYSMEM 1
        fn target(sysmem: bool) -> u32 {
            if sysmem { 1 } else { 0 }
        }
        /// Only the last copy in the batch releases the semaphore: one fence
        /// for the lot
        fn launch(multi_line: bool, release: bool) -> u32 {
            2                                                  // NON_PIPELINED
                | (1 << 2)                                     // FLUSH_ENABLE
                | (if release { 1 << 3 } else { 0 })           // RELEASE_ONE_WORD_SEMAPHORE
                | (1 << 7)                                     // SRC_MEMORY_LAYOUT PITCH
                | (1 << 8)                                     // DST_MEMORY_LAYOUT PITCH
                | (if multi_line { 1 << 9 } else { 0 })        // MULTI_LINE_ENABLE
                | (1 << 12)                                    // SRC_TYPE PHYSICAL
                | (1 << 13) // DST_TYPE PHYSICAL
        }

        // The counter goes up and the semaphore is never reset. Clearing it
        // before a submission deadlocks: the cursor batch clears the copy
        // channel's fence while the compute chain waits on the value the batch
        // before released. A monotonic counter with reached-or-passed waits
        // avoids it
        self.seq = self.seq.wrapping_add(1).max(1);
        let payload = self.seq;
        self.slot_fence[ring_slot] = payload;
        let interrupt = fence_interrupts();

        let (push, sema_va) = (slot_at, self.sema_va);
        let mut n = 0usize;
        let mut word = |w: u32| {
            unsafe { core::ptr::write_volatile(push.add(n), w) };
            n += 1;
        };

        // Before anything else, if another engine has to get there first.
        // `NVC56F_SEM_*` on subchannel zero: a host method, so the same five
        // words as on the compute channel
        if let Some((sema, value)) = after {
            word((1 << 29) | (5 << 16) | (0x005c >> 2));
            word(sema as u32);
            word((sema >> 32) as u32);
            word(value);
            word(0);
            word(3); // ACQ_CIRC_GEQ
        }

        // The object and the semaphore are set once for the whole batch; only
        // the rectangles repeat
        word(hdr(0x0000, 1));
        word(CLASS_DMA_COPY);
        word(hdr(0x0240, 3));
        word((sema_va >> 32) as u32);
        word(sema_va as u32);
        word(payload);

        // Which side of the bus each end is on, written before the first copy
        // and again only where a copy's pair differs from the one before. Three
        // words when it changes, inside the 24 a copy is budgeted
        // (`WORST_CASE_WORDS`) against the twelve it uses
        let mut mode: Option<(bool, bool)> = None;
        for (i, c) in copies.iter().enumerate() {
            if mode != Some((c.src_sysmem, c.dst_sysmem)) {
                word(hdr(0x0260, 2));
                word(target(c.src_sysmem));
                word(target(c.dst_sysmem));
                mode = Some((c.src_sysmem, c.dst_sysmem));
            }
            word(hdr(0x0400, 8));
            word((c.src >> 32) as u32);
            word(c.src as u32);
            word((c.dst >> 32) as u32);
            word(c.dst as u32);
            word(c.pitch_in);
            word(c.pitch_out);
            word(c.line_len);
            word(c.lines);
            word(hdr(0x0300, 1));
            let last = i + 1 == copies.len();
            // The last launch also raises the engine's non-stall interrupt,
            // after its own semaphore release: a host method after the launch
            // would not be ordered against the copy (3.5a)
            word(launch(c.lines > 1, last) | if last && interrupt { LAUNCH_INTERRUPT_NON_BLOCKING } else { 0 });
        }

        let slot = (self.put % self.entries) as usize;
        unsafe {
            core::ptr::write_volatile(self.gpfifo.add(slot * 2), slot_va as u32);
            core::ptr::write_volatile(
                self.gpfifo.add(slot * 2 + 1),
                (((slot_va >> 32) & 0xff) as u32) | ((n as u32) << 10),
            );
        }
        self.put = (self.put + 1) % self.entries;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        self.store_gp_put(bar0);
        crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
        bar0.write32(DOORBELL, self.doorbell);
        Ok(payload)
    }

    /// Launch one compute grid: the descriptor is in memory, the ring carries
    /// its address and an instruction to schedule it.
    ///
    /// Two kinds of method. The compute class's own (bind the object, take this
    /// descriptor, schedule it) go on subchannel 1, where every working example
    /// puts compute. The fence is a host method, the channel's semaphore
    /// release: it belongs to the FIFO, on subchannel 0, and `RELEASE_WFI`
    /// makes the host wait for the engine to fall idle before writing the
    /// value.
    ///
    /// A copy is one method that finishes, so the copy engine releases its own
    /// semaphore; a grid is a descriptor handed to a scheduler, and only the
    /// host waiting for idle knows it is done
    pub fn submit_compute(&mut self, bar0: &MmioRegion, qmd_va: u64) -> Result<u32, u32> {
        self.submit_compute_chain(bar0, &[qmd_va], None)
    }

    /// Several dispatches in one submission, in order.
    ///
    /// A blur is six launches, each reading what the one before wrote. In one
    /// submission the front end takes the descriptors back to back.
    ///
    /// Back to back is not one after the other. The front end hands a grid to
    /// the work distributor and goes on; a small grid's successor runs beside
    /// it, reading pixels not yet written. So between two launches the chain
    /// waits for idle and drops the data caches: `INVALIDATE_SHADER_CACHES`
    /// with `DATA` and `FLUSH_DATA`, which NVK emits for a barrier after shader
    /// storage writes, itself a wait for idle (`nvk_cmd_buffer.c`,
    /// `nvk_cmd_barrier`). tinygrad gets the same ordering by chaining one
    /// descriptor to the next as its dependent.
    ///
    /// `after` makes the whole chain wait for another channel's semaphore
    /// before its first dispatch, ordering two engines: the copy engine puts
    /// the backdrop down and releases its fence, and the compute engine reads
    /// only once the value appears. They run at the same time on separate
    /// runlists.
    ///
    /// The two channels share one page directory, so the copy channel's
    /// semaphore is at the same address in both
    pub fn submit_compute_chain(
        &mut self,
        bar0: &MmioRegion,
        qmds: &[u64],
        after: Option<(u64, u32)>,
    ) -> Result<u32, u32> {
        const SUBC_COMPUTE: u32 = 1;
        const SUBC_HOST: u32 = 0;
        const M_SET_OBJECT: u32 = 0x0000;
        const M_SEND_PCAS_A: u32 = 0x02b4;
        const M_SEND_SIGNALING_PCAS2_B: u32 = 0x02c0;
        const M_SET_SHARED_MEMORY_WINDOW_A: u32 = 0x02a0;
        const M_SET_LOCAL_MEMORY_WINDOW_A: u32 = 0x07b0;
        const SHARED_MEMORY_WINDOW: u64 = 0x7294_0000_0000;
        const LOCAL_MEMORY_WINDOW: u64 = 0x7293_0000_0000;
        /// `NVC56F_SEM_ADDR_LO`, and the four words that follow it
        const M_SEM_ADDR_LO: u32 = 0x005c;
        const SEM_OPERATION_RELEASE: u32 = 1;
        const SEM_OPERATION_ACQ_CIRC_GEQ: u32 = 3;
        const SEM_RELEASE_WFI: u32 = 1 << 20;
        /// `NVC96F_NON_STALL_INTERRUPT` (UVM's `clc96f.h:63`; the SDK's
        /// `clca6f.h` is trimmed)
        const M_NON_STALL_INTERRUPT: u32 = 0x0020;
        /// Drop the front end's cached copy of any previous descriptor, take
        /// this one, run it
        const PCAS_INVALIDATE_COPY_SCHEDULE: u32 = 3;
        /// `NVCEC0_INVALIDATE_SHADER_CACHES`: `DATA` (bit 4) and `FLUSH_DATA`
        /// (bit 2). Not the `_NO_WFI` method at 0x1698: the wait is needed
        const M_INVALIDATE_SHADER_CACHES: u32 = 0x021c;
        const INVALIDATE_DATA_FLUSH: u32 = (1 << 4) | (1 << 2);

        if self.failed.is_some() || self.push.is_null() || qmds.is_empty() || qmds.len() > MAX_BATCH {
            return Err(0);
        }
        let (slot_at, slot_va, ring_slot) = self.take_slot(500_000)?;

        fn hdr(subc: u32, method: u32, count: u32) -> u32 {
            (1 << 29) | (count << 16) | (subc << 13) | (method >> 2)
        }

        // The counter goes up and the semaphore is never reset, as in
        // `submit_batch`: clearing it before a submission deadlocks against a
        // chain waiting on the previous value
        self.seq = self.seq.wrapping_add(1).max(1);
        let payload = self.seq;
        self.slot_fence[ring_slot] = payload;
        let interrupt = fence_interrupts();

        let (push, sema_va) = (slot_at, self.sema_va);
        let mut n = 0usize;
        let mut word = |w: u32| {
            unsafe { core::ptr::write_volatile(push.add(n), w) };
            n += 1;
        };

        // Before anything else, if another engine has to get there first
        if let Some((sema, value)) = after {
            word(hdr(SUBC_HOST, M_SEM_ADDR_LO, 5));
            word(sema as u32);
            word((sema >> 32) as u32);
            word(value);
            word(0);
            // Circular greater-or-equal, not equality: the fence counter wraps,
            // and a channel waiting for a value already past waits for the
            // whole next lap
            word(SEM_OPERATION_ACQ_CIRC_GEQ);
        }

        word(hdr(SUBC_COMPUTE, M_SET_OBJECT, 1));
        word(super::compute::CLASS_COMPUTE);

        // Where generic addressing finds shared and local memory. Nothing is
        // mapped at either address: these are the two holes the machine punches
        // in the flat address space, and a pointer landing in one becomes a
        // shared or local access. The values are the reference's, far from
        // anything real
        word(hdr(SUBC_COMPUTE, M_SET_SHARED_MEMORY_WINDOW_A, 2));
        word((SHARED_MEMORY_WINDOW >> 32) as u32);
        word(SHARED_MEMORY_WINDOW as u32);
        word(hdr(SUBC_COMPUTE, M_SET_LOCAL_MEMORY_WINDOW_A, 2));
        word((LOCAL_MEMORY_WINDOW >> 32) as u32);
        word(LOCAL_MEMORY_WINDOW as u32);

        // Shifted by eight, so a descriptor starts on a 256-byte boundary. Each
        // launch after the first waits for the one before to finish and its
        // writes to land (see above)
        for (i, qmd_va) in qmds.iter().enumerate() {
            if i > 0 {
                word(hdr(SUBC_COMPUTE, M_INVALIDATE_SHADER_CACHES, 1));
                word(INVALIDATE_DATA_FLUSH);
            }
            word(hdr(SUBC_COMPUTE, M_SEND_PCAS_A, 1));
            word((qmd_va >> 8) as u32);
            word(hdr(SUBC_COMPUTE, M_SEND_SIGNALING_PCAS2_B, 1));
            word(PCAS_INVALIDATE_COPY_SCHEDULE);
        }

        // After the launch. The address is written low half first, unlike every
        // engine method in this file: the host's semaphore methods are ordered
        // that way
        word(hdr(SUBC_HOST, M_SEM_ADDR_LO, 5));
        word(sema_va as u32);
        word((sema_va >> 32) as u32);
        word(payload);
        word(0);
        word(SEM_OPERATION_RELEASE | SEM_RELEASE_WFI);
        // Tell the processor: the host has waited for idle and written the
        // value first, so the interrupt cannot beat the fence
        // (`mem_utils_gm107.c:2167`, tinygrad `ops_nv.py:176`)
        if interrupt {
            word(hdr(SUBC_HOST, M_NON_STALL_INTERRUPT, 1));
            word(0);
        }

        let slot = (self.put % self.entries) as usize;
        unsafe {
            core::ptr::write_volatile(self.gpfifo.add(slot * 2), slot_va as u32);
            core::ptr::write_volatile(
                self.gpfifo.add(slot * 2 + 1),
                (((slot_va >> 32) & 0xff) as u32) | ((n as u32) << 10),
            );
        }
        self.put = (self.put + 1) % self.entries;
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);

        self.store_gp_put(bar0);
        crate::drivers::gpu::nvidia::pramin::flush_fb(bar0);
        bar0.write32(DOORBELL, self.doorbell);
        Ok(payload)
    }

    /// Has the engine reached `fence`? Never blocks.
    ///
    /// Reached or passed, by the signed difference of `wait_sema`: the counter
    /// is monotonic and the semaphore never reset, so a strict `==` answers
    /// false for ever once the engine is one batch ahead. A flip gate latching
    /// false stops the screen
    pub fn fence_passed(&self, fence: u32) -> bool {
        !self.sema.is_null() && {
            let v = unsafe { core::ptr::read_volatile(self.sema) };
            v.wrapping_sub(fence) as i32 >= 0
        }
    }

    /// Block until `fence` passes
    pub fn wait_fence(&self, fence: u32, timeout_us: u64) -> Result<(), u32> {
        if self.sema.is_null() {
            return Err(0);
        }
        Self::wait_sema(self.sema, fence, timeout_us)
    }

    /// Wait for whatever was submitted last, if anything: `seq` is the payload
    /// the last submission asked for. Nothing submitted is not a failure
    pub fn wait_last(&self, timeout_us: u64) -> Result<(), u32> {
        if self.seq == 0 {
            return Ok(());
        }
        self.wait_fence(self.seq, timeout_us)
    }

    /// Write the fence the channel is owed, from the processor.
    ///
    /// The escape hatch for a GPU-side acquire that will never fire: an acquire
    /// is a channel spinning on a value in memory, with no timeout, and nothing
    /// revokes it. Writing the value by hand lets it go.
    ///
    /// Only right after a wait has timed out, having given running work two
    /// hundred milliseconds.
    ///
    /// This does not decide the channel is dead. It claims the work finished,
    /// without knowing which of the two channels was stuck, and killing both
    /// would take the copy engine, which puts the console and every frame on
    /// the panel. The caller decides, and this records that a fence came from
    /// the processor: `forced_fences`, printed by `nvidia intr`
    pub fn force_fence(&mut self) {
        if !self.sema.is_null() {
            unsafe { core::ptr::write_volatile(self.sema, self.seq) };
        }
        self.forced = self.forced.saturating_add(1);
        crate::serial_println!(
            "[nvidia] channel {} (chid {}) fence {} written by the processor - \
             the engine may not have done that work",
            self.handle, self.chid, self.seq
        );
    }

    /// How many fences on this channel the processor wrote because the engine
    /// would not. Above zero means some frame's completion was asserted, not
    /// observed
    pub fn forced_fences(&self) -> u32 {
        self.forced
    }

    /// Where this channel's fence lives, and what it stands at, for another
    /// channel to wait on: the address since both share a page directory, the
    /// value since a wait is for a number
    pub fn sema_va(&self) -> u64 {
        self.sema_va
    }

    /// The fence the last submission will reach. Pairs with `fence_passed`
    pub fn last_fence(&self) -> u32 {
        self.seq
    }

    /// Where that fence lives, as the display addresses it, and what the last
    /// submission will write there.
    ///
    /// `None` until a ring is attached. The value is the monotonic counter of
    /// `last_fence`, usable as a display acquire: the window compares for
    /// strict equality, and a counter that never repeats cannot match a stale
    /// prefetched value
    pub fn fence_target(&self) -> Option<(u64, u32)> {
        (self.sema_phys != 0).then_some((self.sema_phys, self.seq))
    }

    /// GP_GET and GP_PUT from USERD, for reporting only. Not a completion
    /// check: this read can be stale while the copies land
    pub fn userd_gp(&self, bar0: &MmioRegion) -> (u32, u32) {
        (self.read_userd(bar0, USERD_GP_GET), self.read_userd(bar0, USERD_GP_PUT))
    }

    /// Where USERD is in video memory: the page after the instance block
    pub fn userd_vram(&self) -> u64 {
        self.inst + INST_SIZE
    }

    /// Hand the channel a CPU window onto its USERD through BAR1, or take it
    /// away. `epoch` is `userd_epoch()` as read before the caller found BAR1
    /// physical, so a window handed out across a rebind is stale from the start
    pub fn set_userd_window(&mut self, cpu: Option<*mut u8>, epoch: u32) {
        self.userd_cpu = cpu.unwrap_or(core::ptr::null_mut());
        self.userd_epoch = epoch;
    }

    /// The window, if there is one and BAR1 has not moved since it was given
    fn userd_window(&self) -> Option<*mut u8> {
        (!self.userd_cpu.is_null() && self.userd_epoch == userd_epoch()).then_some(self.userd_cpu)
    }

    /// One word of USERD: through the window, an ordinary store, as OpenRM
    /// writes `pControlGPFifo->GPPut` through its BAR1 mapping
    /// (`channelFillGpFifo`), or through PRAMIN. True when it went through BAR1
    fn write_userd(&self, bar0: &MmioRegion, off: u64, val: u32) -> bool {
        match self.userd_window() {
            Some(p) => {
                // Uncached: the store is ordered before the doorbell without
                // the write-combining flush OpenRM needs for its mapping
                unsafe { core::ptr::write_volatile(p.add(off as usize) as *mut u32, val) };
                true
            }
            None => {
                let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
                w.write32(self.userd_vram() + off, val);
                false
            }
        }
    }

    fn read_userd(&self, bar0: &MmioRegion, off: u64) -> u32 {
        match self.userd_window() {
            Some(p) => unsafe { core::ptr::read_volatile(p.add(off as usize) as *const u32) },
            None => {
                let mut w = crate::drivers::gpu::nvidia::pramin::Window::new(bar0);
                w.read32(self.userd_vram() + off)
            }
        }
    }

    /// GP_PUT, after the GPFIFO entry it publishes, landed in video memory
    /// before the caller rings the doorbell.
    ///
    /// Through BAR1 the store is followed by a read of the same word through
    /// the same window, and that read is the flush. On Volta and later OpenRM
    /// flushes video memory so (`kbusFlushSingle_GV100`: "read the FB address 0
    /// in order to trigger a flush" through its CPU pointer): a read on the BAR
    /// path cannot pass the writes ahead of it. `flush_fb` reads the PRAMIN
    /// window, which flushes PRAMIN's writes only. Without the read the
    /// doorbell can beat GP_PUT, and the work waits for the next doorbell, a
    /// frame later.
    ///
    /// Through PRAMIN, `flush_fb`'s read of the window is that flush
    fn store_gp_put(&self, bar0: &MmioRegion) {
        use core::sync::atomic::Ordering::Relaxed;
        if self.write_userd(bar0, USERD_GP_PUT, self.put) {
            GP_PUT_BAR1.fetch_add(1, Relaxed);
            if self.read_userd(bar0, USERD_GP_PUT) != self.put {
                GP_PUT_READ_WRONG.fetch_add(1, Relaxed);
            }
        } else {
            GP_PUT_PRAMIN.fetch_add(1, Relaxed);
        }
    }
}
