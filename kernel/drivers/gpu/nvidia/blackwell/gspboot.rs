// Getting the GSP RISC-V core running, and the type vocabulary the rest of the
// Blackwell driver speaks. The COT/FSP handshake is in `sequence.rs`.


//                   GB206 GSP-RM boot sequence (615, `dev/gsp-615.md`)
// 1) Parse the FMC ELF32 image and stage the "image" section in sysmem
// 2) Parse gsp-<ver>.bin:
//       stage .fwimage
//       build radix-3 page table for firmware pages
//       stage .fwsignature_gb20x blob
// 3) Parse and stage bootloader-<ver>.bin
//    (RM_RISCV_UCODE_DESC container)
// 4) Stage ucodes-<ver>.bin behind a radix-3 table (GSP-RM's bindata)
// 5) Build the boot arguments: the rings, GSP_ARGUMENTS_CACHED, the logs;
//    our ring's header and QUEUE_HEAD = 0
// 6) Initialize GspFwWprMeta
//    On Blackwell, host provides only sysmem addresses and sizes
//    FB offsets remain zero because WPR placement is performed by FMC/ACR
// 7) Build GSP_FMC_BOOT_PARAMS
// 8) Submit NVDM_TYPE_COT to FSP and validate response
// 9) Wait for RISCV_BR_PRIV_LOCKDOWN release
//    MAILBOX0/1:
//        0xbadf41xx: FMC boot in progress
//        GSP_FMC_BOOT_PARAMS address: successful handoff
//        FMC error code: boot failure
// 10) Link GSP-RM's status queue, then GMCAPI GSP_INIT
// All staged sysmem remains pinned in `BOOT_IMAGE` for the lifetime of the GPU

use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

use spin::Mutex;

use crate::kcore::sched_lock::SchedMutex;

use crate::drivers::gpu::nvidia::gsp_common::bootargs::GspBootArgs;
use crate::mm::dma::DmaBuffer;
use crate::drivers::gpu::nvidia::gsp_common::gspfw::{
    GspFwWprMeta, Radix3,
};

use super::fmc::FmcError;
use super::fsp::FspError;
use super::regs::*;

pub const PAGE_SIZE: usize = 4096;

// The FB layout GSP-RM is asked for (`kgspCalculateFbLayout_GH100`)

// Every size here is a minimum request. On Hopper+ the FMC's ACR places the
// regions itself and patches the offsets into the WPR metadata, so the host
// decides only how much to ask for (`kernel_gsp_gh100.c:294`).

/// The non-WPR heap in front of the WPR: `kgspGetNonWprHeapSize` is
/// 2228224 on GB202-207 (`g_kernel_gsp_nvoc.h:1321`)
pub const HEAP_SIZE_NON_WPR: u64 = 0x22_0000;

/// PMU reservation, `kpmuReservedMemorySizeGet_IMPL` (`kern_pmu.c:66`): every
/// GB20x PMU is self-init, so backing store 0x950000 + surfaces 0x1040000
/// (`gpuGetPmuReservedMemorySurfacesSize`, every chip before Rubin) + 4 KiB
/// misc, 128 KiB aligned
pub const RSVD_SIZE_PMU: u64 = align_up(0x095_0000 + 0x104_0000 + 0x1000, 0x2_0000);

/// libos3 baremetal carve-out, `kgspGetFwHeapParamOsCarveoutSize` (no vGPU)
const HEAP_OS_CARVEOUT: u64 = 22 << 20;
/// `fwHeapParamBaseSize`, Hopper+ including every GB20x
/// (`GSP_FW_HEAP_PARAM_BASE_RM_SIZE_GH100`)
const HEAP_BASE_RM_GH100: u64 = 18 << 20;
/// `kgspGetMinWprHeapSizeMB` for everything after GA100, no vGPU
/// (`gsp_fw_heap.h:101`). The Bullseye deltas (12 + 70) exist only in a
/// code-coverage build of GSP-RM, and `rmconfig.h:270` says this one is not
const HEAP_SIZE_MIN: u64 = 88 << 20;
/// `GSP_FW_HEAP_PARAM_SIZE_PER_GB`
const HEAP_PER_GB_FB: u64 = 96 << 10;
/// `GSP_FW_HEAP_PARAM_CLIENT_ALLOC_SIZE`: 2048 channels x 48 KiB
const HEAP_CLIENT_ALLOC: u64 = (48 << 10) * 2048;

/// What boot A asked for, for the `gspbase` escape hatch and the line showing
/// the change: the minimum with the Bullseye deltas
const HEAP_SIZE_MIN_BASE: u64 = (88 + 12 + 70) << 20;

pub const fn align_up(v: u64, a: u64) -> u64 { (v + a - 1) & !(a - 1) }

/// The WPR heap GSP-RM gets, `_kgspCalculateFwHeapSize` (`kernel_gsp.c:5021`).
///
/// Clamped to the minimum only. The 280 MiB maximum in `gsp_fw_heap.h` bounds a
/// registry override (`kgspGetFwHeapSize_IMPL`); the calculated size is bounded
/// by the pre-scrubbed area, which Hopper+ does not have
/// (`kgspGetFwHeapSize(pGpu, pKernelGsp, 0, 0)` in
/// `kgspCalculateFbLayout_GH100`)
pub fn wpr_heap_size(fb_size: u64) -> u64 {
    wpr_heap_size_for(fb_size, super::bringup::base_init())
}

/// The same, for either minimum: `base` is boot A's, for `gspbase` and the log
/// line showing the change
pub fn wpr_heap_size_for(fb_size: u64, base: bool) -> u64 {
    wpr_heap_calculated(fb_size).max(if base { HEAP_SIZE_MIN_BASE } else { HEAP_SIZE_MIN })
}

fn wpr_heap_calculated(fb_size: u64) -> u64 {
    let fb_gb = fb_size.div_ceil(1 << 30);
    HEAP_OS_CARVEOUT
        + HEAP_BASE_RM_GH100
        + align_up(HEAP_PER_GB_FB * fb_gb, 1 << 20)
        + align_up(HEAP_CLIENT_ALLOC, 1 << 20)
}

/// `VGA_WORKSPACE_SIZE`: what sits at the very end of FB, below which the PMU
/// reservation and FRTS go
const VGA_WORKSPACE_SIZE: u64 = 128 << 10;
/// `kfspGetExtraReservedMemorySize_GH100`: 4 KiB after FRTS (bug 3763996)
const FSP_EXTRA_RESERVED: u64 = 4 << 10;
/// `WPR_ALIGNMENT`, 128 KiB (`gsp_init_args.h`)
const WPR_ALIGNMENT: u64 = 128 << 10;

/// Where FRTS goes, as an offset from the end of FB: the `frtsVidmemOffset` of
/// the COT (`kfspSendBootCommands_GH100` and its preparation in 615).
///
/// Layout `0 | ... | FRTS | PMU | VGA workspace | END`, aligned to the WPR
/// granularity. The WPR end margin is added only on a boot retry after an ECC
/// failure, which this driver does not do
pub fn frts_vidmem_offset() -> u64 {
    align_up(VGA_WORKSPACE_SIZE + FSP_EXTRA_RESERVED + RSVD_SIZE_PMU, WPR_ALIGNMENT)
}

// GSP_FMC_BOOT_PARAMS (615 `gspifpub.h`), 256 bytes. Offsets compiled from the
// header (`dev/gsp-615.md`, section 5):
//   0 magic "FMC "  4 size  32 initParams (regkeys)
//   64 bootGspRmParams: target, descSize, descOffset, wprCarveoutOffset,
//      wprCarveoutSize, bIsGspRmBoot, bInstInSysMode, bIcuEnabled, bScrubCbcSr
//   128 gspRmParams: target, bootArgsOffset
//   160 gspSpdmParams (no SPDM)   192 gspRmMemParams (confidential compute only)
// The three DMA targets are one enum from the header; the boot uses sysmem
#[allow(dead_code)]
pub const GSP_DMA_TARGET_LOCAL_FB: u32 = 0;
pub const GSP_DMA_TARGET_COHERENT_SYSTEM: u32 = 1;
pub const GSP_DMA_TARGET_NONCOHERENT_SYSTEM: u32 = 2;

/// `GSP_FMC_BOOT_PARAMS_MAGIC_VALUE`, "FMC " read as a word
const GSP_FMC_BOOT_PARAMS_MAGIC: u32 = u32::from_le_bytes(*b"FMC ");
pub const GSP_FMC_BOOT_PARAMS_SIZE: usize = 256;

/// What `kgspSetupGspFmcArgs_GH100` fills for a normal boot of a GB20x: the
/// descriptor is the WPR metadata, the boot arguments the libos table. No
/// confidential compute, no NVLink, no ICU (Rubin only), and no CBC scrub
/// (`gpuRequiresCbcSrScrub` is GB20B/GB20C and Rubin)
#[derive(Copy, Clone, Debug)]
pub struct GspFmcBootParams {
    pub wpr_meta_phys: u64,
    pub libos_phys: u64,
}

impl GspFmcBootParams {
    pub fn new(wpr_meta_phys: u64, libos_phys: u64) -> Self {
        Self { wpr_meta_phys, libos_phys }
    }

    pub fn write_bytes(&self, dst: &mut [u8]) {
        let d = &mut dst[..GSP_FMC_BOOT_PARAMS_SIZE];
        d.fill(0);
        d[0..4].copy_from_slice(&GSP_FMC_BOOT_PARAMS_MAGIC.to_le_bytes());
        d[4..6].copy_from_slice(&(GSP_FMC_BOOT_PARAMS_SIZE as u16).to_le_bytes());
        // bootGspRmParams: the WPR metadata, cached system memory
        d[64..68].copy_from_slice(&GSP_DMA_TARGET_COHERENT_SYSTEM.to_le_bytes());
        d[68..72].copy_from_slice(&(size_of::<GspFwWprMeta>() as u32).to_le_bytes());
        d[72..80].copy_from_slice(&self.wpr_meta_phys.to_le_bytes());
        d[92] = 1; // bIsGspRmBoot
        // gspRmParams: the libos table, which OpenRM keeps uncached
        d[128..132].copy_from_slice(&GSP_DMA_TARGET_NONCOHERENT_SYSTEM.to_le_bytes());
        d[136..144].copy_from_slice(&self.libos_phys.to_le_bytes());
    }
}

// Errors / report
#[derive(Debug, Clone, Copy)]
/// Every variant carries what the hardware said when it gave up, read through
/// `Debug` when printed
#[allow(dead_code)]
pub enum BootError {
    /// FSP never reported secure boot complete (scratch != 0xff)
    SecureBootTimeout(u32),
    /// A firmware blob is missing from /lib/firmware
    FirmwareMissing(&'static str),
    /// The blob exists but reading it failed (NoSpace = kernel heap could not
    /// hold the buffer; Io = ext2 read error)
    FirmwareRead(&'static str, crate::kcore::firmware::load::FwError),
    /// FMC ELF parse / crypto-size failure
    Fmc(FmcError),
    /// gsp-<ver>.bin section parse failure
    GspElf,
    /// bootloader-<ver>.bin container parse failure
    Bootloader,
    /// sysmem allocation failure (blob too large / fragmentation)
    Alloc,
    /// FSP rejected or never answered the COT message
    Fsp(FspError),
    /// RISCV_BR_PRIV_LOCKDOWN never cleared after COT accept
    LockdownTimeout { mbox0: u32, hwcfg2: u32 },
    /// FMC posted an error code in MAILBOX0/1
    FmcFailed { mbox0: u32, mbox1: u32 },
    /// The GSP PRI target mask (BAR0 decoupler) was never lifted, so every read
    /// of the NV_PGSP block still returns 0 / 0xbadf41xx. OpenRM
    /// kfspWaitForGspTargetMaskReleased_GH100 waits on this before looking at
    /// any GSP falcon register
    TargetMaskTimeout { hwcfg2: u32 },
    /// GSP-FMC wrote a fatal code to NV_PBUS_SW_SCRATCH_GSP_FMC_ERROR
    FmcErrorCode { raw: u32, decoded: FmcErrorCode },
    /// GFW (devinit) never reached PROGRESS=0xFF: GSP-RM would assert on
    /// NV_PGC6_AON_SECURE_SCRATCH_GROUP_05_0_GFW_BOOT. GFW runs only at cold
    /// power-on; a failed FMC boot in an earlier session leaves this at 0 until
    /// the machine is fully powered off.
    ///
    /// Nothing produces this. `sequence.rs` reads the scratch, gives it a 200ms
    /// grace and goes on regardless: on GB206 consumer silicon it stays 0 on
    /// boots where GSP-RM comes up fine, and OpenRM's Hopper+ path does not
    /// read it. The variant and the advice `nvidia_cmds.rs` prints stay
    GfwNotCompleted(u32),
    /// Lockdown released but the RISC-V core is not running
    /// (NV_PRISCV_RISCV_CPUCTL.ACTIVE_STAT == INACTIVE). OpenRM
    /// kgspBootstrap_GH100 asserts here with "Failed to boot GSP"
    RiscvNotActive { cpuctl: u32 },
}

impl From<FmcError> for BootError { fn from(e: FmcError) -> Self { BootError::Fmc(e) } }
impl From<FspError> for BootError { fn from(e: FspError) -> Self { BootError::Fsp(e) } }

#[derive(Debug, Clone, Copy, Default)]
pub struct BootReport {
    pub fb_size: u64,
    pub heap_size: u64,
    pub frts_offset: u64,
    /// `NV_PFB_PRI_MMU_WPR2_ADDR_HI` before the boot. Non-zero `_VAL` is a
    /// GSP-RM never unloaded
    pub wpr2_hi: u32,
    /// Where the sysmem flush buffer was programmed, 0 if it was not
    pub flush_buffer: u64,
    /// `sysTimerOffsetNs` as sent to GSP-RM, 0 if the time was not set
    pub time_offset_ns: u64,
    /// The CrashCat queue given to GSP-RM in the WPR metadata, 0 if none
    pub crash_queue: u64,
    pub fmc_image_phys: u64,
    pub radix3_root_phys: u64,
    pub wpr_meta_phys: u64,
    pub boot_params_phys: u64,
    pub libos_phys: u64,
    pub cot_response_words: usize,
    pub lockdown_released: bool,
    pub mbox0: u32,
    /// Raw NV_PBUS_SW_SCRATCH_GSP_FMC_ERROR after the boot attempt. 0 =
    /// GSP_FMC_OK
    pub fmc_error_raw: u32,
    /// NV_PRISCV_RISCV_CPUCTL.ACTIVE_STAT after lockdown release
    pub riscv_active: bool,
    /// First MSGQ message from GSP-RM: (function, signature), if any arrived
    pub gsp_first_msg: Option<(u32, u32)>,
    /// exceptType from the last OS_ERROR_LOG event: the Xid GSP-RM reported
    pub xid: Option<u32>,
    /// Whether GSP-RM published its own msgqTxHeader (entryOff/msgSize). False
    /// means the MSGQ side of the handshake never came up
    pub msgq_initialized: bool,
    pub init_done: bool,
    /// The NV01_ROOT -> NV01_DEVICE_0 -> NV20_SUBDEVICE_0 chain came up: GSP-RM
    /// is answering RmAlloc
    pub rm_client: bool,
    /// A control call round-tripped with a correct answer: the card is usable,
    /// not merely booted
    pub rm_control: bool,
}

// Pinned boot state

// The state of a booted GSP-RM, in three pieces with three locks, split by who
// needs what (`dev/gsp-foundation.md`, decision 8):
//
//   RM          the rings, every RPC. A `SchedMutex`: the one lock
//               anything may sleep under, never taken from an ISR
//   DISP_MEM    the display pushbuffers, modesets and the flip. Also a
//               `SchedMutex`, since `with_display` holds it across RPCs;
//               the flip takes only this one and never waits on GSP-RM
//   BOOT_IMAGE  what the GSP runs out of, boot and unload only
//
// Taken in that order when more than one is needed, only inside
// `control::with_rm` and `control::with_display`.

/// Every sysmem region GSP-RM/FMC keeps referencing after the COT. Dropping
/// this while the GSP is alive hands freed pages to a busmastering GPU. Held,
/// not read: every field is a physical address the GPU follows
#[allow(dead_code)]
pub struct BootImage {
    pub fmc_image: DmaBuffer,
    pub staged_fw: DmaBuffer,
    pub radix3: Radix3,
    pub bootloader: DmaBuffer,
    pub signature: DmaBuffer,
    pub wpr_meta: DmaBuffer,
    pub boot_params: DmaBuffer,
}

/// The transport's half of a booted GSP-RM: the shared region with both
/// rings, their counters and GSP-RM's log buffers
pub struct RmState {
    pub bootargs: GspBootArgs,
}

// Memory the display engine holds physical pointers into once a modeset has
// landed.
//
// All three outlive the function setting them up, as everything in `BootImage`:
// the hardware keeps reading from the addresses it was given. Freeing the
// framebuffer would hand the page allocator memory the GPU is scanning out
#[derive(Default)]
pub struct DisplayMemory {
    /// Core channel pushbuffer: modesets. One for the card: one core channel
    /// however many heads run, and every head's state is staged into it and
    /// armed by the same UPDATE
    pub core_push: Option<DmaBuffer>,
    /// Window channel pushbuffer: surface changes. One per head, as a window
    /// channel is per window and windows go two per head
    pub wndw_push: [Option<DmaBuffer>; MAX_HEADS],
    /// Window-immediate channel pushbuffer: window position. Per head, for the
    /// same reason
    pub wimm_push: [Option<DmaBuffer>; MAX_HEADS],
    /// The scanout surface, or the staging buffer it was copied from. Per head
    pub framebuffer: [Option<DmaBuffer>; MAX_HEADS],
}

/// Heads this driver keeps buffers for. The board reports four
/// (`disp::MAX_HEADS`), and the array matches
pub const MAX_HEADS: usize = crate::drivers::gpu::nvidia::gsp_common::disp::MAX_HEADS as usize;

pub static RM: SchedMutex<Option<RmState>> = SchedMutex::named("gsp-rm", None);
pub static DISP_MEM: SchedMutex<Option<DisplayMemory>> = SchedMutex::named("disp-mem", None);
pub static BOOT_IMAGE: Mutex<Option<BootImage>> = Mutex::new(None);

/// Set once all three are in place. An atomic: asking takes no lock and is safe
/// from anywhere
static BOOTED: AtomicBool = AtomicBool::new(false);

/// The display memory of an unloaded GSP-RM, until the next boot is installed.
///
/// A head that was scanning may keep fetching from these until a new display
/// engine replaces it, so the frames stay out of the page allocator until then
/// (`dev/gsp-foundation.md`, section 14)
static RETIRED_DISP: Mutex<Option<DisplayMemory>> = Mutex::new(None);

/// What an unload could not prove was let go of. A core that never halted may
/// still write to the rings; a WPR2 still up is the FMC and ACR guarding a
/// region they built from the boot image. Both stay pinned: only a reset frees
/// them, so they are never handed back
#[allow(dead_code)] // held, not read: the GPU may still be following them
struct Kept {
    image: Option<BootImage>,
    rings: RmState,
}
static KEPT: Mutex<alloc::vec::Vec<Kept>> = Mutex::new(alloc::vec::Vec::new());

pub fn is_booted() -> bool {
    BOOTED.load(Ordering::Acquire)
}

/// The end of a boot: everything the hardware points at, put where the rest of
/// the driver can reach it
pub(super) fn install(image: BootImage, rm: RmState, disp: DisplayMemory) {
    *BOOT_IMAGE.lock() = Some(image);
    *DISP_MEM.lock() = Some(disp);
    *RM.lock() = Some(rm);
    BOOTED.store(true, Ordering::Release);
    // A new display engine is up: whatever the last one was fetching from is
    // nobody's now
    RETIRED_DISP.lock().take();
}

/// The end of an unload: the display memory is retired, and the rings and the
/// boot image are freed, unless the unload could not show the GPU is done with
/// them. `rm` is the ring state the caller took out of `RM`. True when
/// everything was freed
pub(super) fn uninstall(rm: RmState, halted: bool, wpr2_down: bool) -> bool {
    BOOTED.store(false, Ordering::Release);
    if let Some(d) = DISP_MEM.lock().take() {
        *RETIRED_DISP.lock() = Some(d);
    }
    let image = BOOT_IMAGE.lock().take();
    if halted && wpr2_down {
        drop(image);
        drop(rm);
        return true;
    }
    KEPT.lock().push(Kept { image, rings: rm });
    false
}

/// The rings and log buffers, raw, for what is not an RPC: unload, which talks
/// to GSP-RM as it goes away, and the shell's log readers. An RPC goes through
/// `control::with_rm`
pub fn with_rm_state<R>(f: impl FnOnce(&mut RmState) -> R) -> Option<R> {
    RM.lock().as_mut().map(f)
}

/// The display pushbuffers on their own: all the flip may take
pub fn with_disp_mem<R>(f: impl FnOnce(&mut DisplayMemory) -> R) -> Option<R> {
    DISP_MEM.lock().as_mut().map(f)
}

// Boot orchestration

// Wall-clock timeout via PTIMER (ns). nouveau uses 4000 ms for both the
// secure-boot wait and the lockdown-release poll
pub const LOCKDOWN_TIMEOUT_NS: u32 = 4_000_000_000;

// Announce boot phase 'n' of 7, on console and serial.
//
// Not through the keyboard LEDs: `ps2::set_leds` writes 0xED and picks up the
// ACK with a bare port read while IRQ 1 is live and the shell's handler reads
// the same port. The ACK lands in the scancode decoder and desyncs the
// keyboard's command state machine
pub fn phase(n: u8, what: &str) {
    crate::println!("    [{}/7] {}", n, what);
}

pub fn stage_blob(bytes: &[u8]) -> Result<DmaBuffer, BootError> {
    let pages = bytes.len().div_ceil(PAGE_SIZE).max(1);
    let mut buf = DmaBuffer::alloc(pages).map_err(|_| BootError::Alloc)?;
    buf.zero();
    buf.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
    DmaBuffer::write_barrier();
    Ok(buf)
}

// gh100_gsp_lockdown_released, one poll step.
// Returns Ok(true) = released, Ok(false) = still busy, Err = FMC error
pub fn lockdown_step(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    boot_params_phys: u64,
) -> Result<(bool, u32), BootError> {
    let mbox0 = bar0.read32(PGSP_FALCON_BASE + FALCON_MAILBOX0);

    // 0xbadf41xx = GSP PRI access still fenced off, keep waiting
    if mbox0 != 0 && crate::drivers::gpu::nvidia::pri::is_target_locked(mbox0) {
        return Ok((false, mbox0));
    }

    // Any non-zero value that is not the boot-params address is an FMC error
    if mbox0 != 0 {
        let mbox1 = bar0.read32(PGSP_FALCON_BASE + FALCON_MAILBOX1);
        if ((mbox1 as u64) << 32 | mbox0 as u64) != boot_params_phys {
            return Err(BootError::FmcFailed { mbox0, mbox1 });
        }
    }

    // HWCFG2 carries real data only once the BAR0 decoupler is gone. While GSP
    // TARGET_MASK is applied, every read of the NV_PGSP block returns 0 or
    // 0xbadf41xx (OpenRM _kfspIsGspTargetMaskReleased), and a 0 reads
    // RISCV_BR_PRIV_LOCKDOWN as clear, so this guard keeps "lockdown released"
    // from firing when the COT is accepted
    let hwcfg2 = bar0.read32(PGSP_FALCON_BASE + FALCON_HWCFG2);
    if hwcfg2 == 0 || crate::drivers::gpu::nvidia::pri::is_target_locked(hwcfg2) {
        return Ok((false, mbox0));
    }

    Ok((hwcfg2 & HWCFG2_RISCV_BR_PRIV_LOCKDOWN == 0, mbox0))
}

// Read + decode NV_PBUS_SW_SCRATCH_GSP_FMC_ERROR. PBUS is host-side, so this is
// readable while the whole NV_PGSP window is dark, the FMC's only channel for a
// fatal error and why
pub fn fmc_error(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
) -> Option<(u32, FmcErrorCode)> {
    let raw = bar0.read32(PBUS_SW_SCRATCH_GSP_FMC_ERROR);
    decode_fmc_error(raw).map(|d| (raw, d))
}

pub fn print_fmc_error(raw: u32, decoded: FmcErrorCode) {
    crate::println!("      GSP-FMC error {:#010x}:", raw);
    match decoded {
        FmcErrorCode::Sk { partition, error, phase } => crate::println!(
            "        separation kernel: partition {:#x} error {:#x} phase {:#x}",
            partition, error, phase
        ),
        FmcErrorCode::Generic { partition, code, info } => crate::println!(
            "        partition {:#x} error {:#x} info {:#x}", partition, code, info
        ),
        FmcErrorCode::Unknown(v) => crate::println!("        unknown variant ({:#010x})", v),
    }
    crate::kerror!("[gb206/boot] GSP-FMC error {:#010x}: {:?}", raw, decoded);
}

// Knobs the shell exposes on a GSP-RM boot, grouped: four bare `true`/`false`
// arguments say nothing
#[derive(Clone, Copy, Default)]
pub struct BootFlags {
    /// Park the framebuffer console before the COT is sent, and leave it
    /// parked.
    ///
    /// The boot framebuffer sits in this GPU's BAR1, and GSP-RM's display init
    /// remaps that aperture. A `println!` after the COT is a CPU store into a
    /// BAR being reprogrammed, and it can wedge the PCIe link instead of being
    /// dropped. Parked, output still reaches serial.
    ///
    /// OpenRM owns the display path end to end: CPU-RM creates a BAR1 mapping
    /// for the console (`KernelBus.bPreserveBar1ConsoleEnabled`, set from the
    /// GOP framebuffer feeding `consoleMemSize`) so the console survives the
    /// remap. That needs BAR1 virtual-mode management
    pub detach_console: bool,
    pub log_verbose: bool,
    /// Toggle a keyboard LED on every heartbeat tick.
    ///
    /// LEDs changing while the screen is frozen mean the display is gone and
    /// the CPU fine; LEDs stopping mean the CPU wedged, and where tells when.
    /// Only with the `leds` flag: driving the LEDs desyncs the PS/2 command
    /// stream and costs the keyboard for the session
    pub led_heartbeat: bool,
    /// Send `GspSystemInfo.bIsPrimary = 0`.
    ///
    /// `bIsPrimary` gates GSP-RM's display-takeover path, including inheriting
    /// the firmware console; `consoleMemSize` only sizes a reservation. Forcing
    /// 0 misreports the machine, but isolates whether the ENGINE failure is
    /// tied to the takeover path
    pub not_primary: bool,
    /// Send `RMInstLoc`/`RMInstLoc4` forcing the display's instance memory and
    /// its state-cache buffer into video memory.
    ///
    /// The ENGINE record from the display subsystem has errorCode 0x27
    /// (`NV_ERR_INVALID_DMA_SPECIFIER` in `nvstatuscodes.h`), which a display
    /// engine reports when handed a memory descriptor in an aperture it cannot
    /// address. These two registry keys test whether those buffers land in
    /// sysmem
    pub disp_vidmem: bool,
}
