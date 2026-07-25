// GB206 GSP-RM boot: phases 1-3 of docs/Nvidia_gb206.md
//
// Mirrors nouveau gh100_gsp_oneinit + gh100_gsp_init (r570 line, which
// gb202.c reuses verbatim for all GB20x):
//
//   1) parse the FMC ELF32 and stage its "image" section in sysmem
//   2) parse gsp-570.144.bin, stage .fwimage, build the radix3 table over
//      its pages, stage the .fwsignature_gb20x blob
//   3) parse + stage bootloader-570.144.bin (RM_RISCV_UCODE_DESC container)
//   4) build the libos/CMDQ-MSGQ boot-args set (shared with Turing)
//   5) fill a GspFwWprMeta: on Blackwell only the sysmem pointers and the
//      sizes are host-filled; every FB offset is left 0 because the FMC
//      places the WPR itself (nouveau: wpr->offset_set_by_acr)
//   6) fill GSP_FMC_BOOT_PARAMS (desc -> wpr-meta, bootArgs -> libos table)
//   7) send NVDM_TYPE_COT to the FSP and parse the response
//   8) poll the GSP falcon for RISCV_BR_PRIV_LOCKDOWN release; MAILBOX0/1
//      meanwhile must hold either 0xbadf41xx (busy), the boot-params
//      address (ok), or an FMC error code
//   9) poll the MSGQ for GSP-RM's first message (GSP_INIT_DONE)
//
// All staged sysmem stays pinned in GB206_BOOT for the GPU's lifetime

use core::mem::size_of;

use spin::Mutex;

use crate::drivers::gpu::nvidia::gtx1650::bootargs::{GspBootArgs, NV_VGPU_MSG_EVENT_GSP_INIT_DONE};
use crate::drivers::gpu::nvidia::gtx1650::dma_buf::DmaBuffer;
use crate::drivers::gpu::nvidia::gtx1650::gsprm::{
    parse_gsp_bootloader, parse_gsp_rm_sig, GspFwWprMeta, Radix3,
    GSP_FW_WPR_META_MAGIC, GSP_FW_WPR_META_REVISION,
};
use crate::serial_println;

use super::fmc::{FmcError, FmcImage, NvdmPayloadCot};
use super::fsp::{Fsp, FspError};
use super::init::{FW_FMC_PATH, FW_GSP_PATH};
use super::regs::*;

pub const FW_BL_PATH: &str = "nvidia/gb206/gsp/bootloader-570.144.bin";

/// Signature section for the Blackwell consumer chip group (gsp/gb202.c)
pub const GSP_SIGNATURE_SECTION_GB20X: &str = ".fwsignature_gb20x";

const PAGE_SIZE: usize = 4096;
//////////////////////////////////////////////////////////////////////////////////////////////////////////////
// r570 WPR parameters for GB20x (nouveau rm/r570/rm.c: r570_wpr_libos3_gb20x  + nvrm/gsp.h heap constants) //
//////////////////////////////////////////////////////////////////////////////////////////////////////////////

/// Non-WPR heap directly below the WPR (heap_size_non_wpr)
const HEAP_SIZE_NON_WPR: u64 = 0x22_0000;
/// PMU reservation above FRTS: ALIGN(8M + 16M + 4K, 128K)
const RSVD_SIZE_PMU: u64 = align_up(0x080_0000 + 0x100_0000 + 0x1000, 0x2_0000);
/// libos3 baremetal OS carve-out inside the WPR heap
const HEAP_OS_CARVEOUT: u64 = 22 << 20;
/// RM base size, Hopper+
const HEAP_BASE_RM_GH100: u64 = 14 << 20;
/// Minimum WPR heap: 88 MiB + Bullseye deltas (12 + 70)
const HEAP_SIZE_MIN: u64 = (88 + 12 + 70) << 20;
/// Per-GiB-of-FB heap growth
const HEAP_PER_GB_FB: u64 = 96 << 10;
/// Client allocation reserve (2048 channels x 48 KiB)
const HEAP_CLIENT_ALLOC: u64 = (48 << 10) * 2048;

const fn align_up(v: u64, a: u64) -> u64 { (v + a - 1) & !(a - 1) }

//////////////////////////////////////////////////////////
/// tu102_gsp_wpr_heap_size for the GB20x parameter set //
/////////////////////////////////////////////////////////
fn wpr_heap_size(fb_size: u64) -> u64 {
    let fb_gb = fb_size.div_ceil(1 << 30);
    let heap = HEAP_OS_CARVEOUT
        + HEAP_BASE_RM_GH100
        + align_up(HEAP_PER_GB_FB * fb_gb, 1 << 20)
        + align_up(HEAP_CLIENT_ALLOC, 1 << 20);
    heap.max(HEAP_SIZE_MIN)
}

/// Vidmem reservation the FMC carves above FRTS, and the FRTS offset-from-
/// end-of-FB the COT message carries (gh100_gsp_init + gh100_fsp_boot_gsp_fmc)
fn frts_vidmem_offset() -> u64 {
    let rsvd = align_up(HEAP_SIZE_NON_WPR + RSVD_SIZE_PMU, 0x20_0000);
    align_up(rsvd, 0x20_0000)
}

// GSP_FMC_BOOT_PARAMS (r570 nvrm/gsp.h), C layout with explicit padding
pub const GSP_DMA_TARGET_LOCAL_FB: u32 = 0;
pub const GSP_DMA_TARGET_COHERENT_SYSTEM: u32 = 1;
pub const GSP_DMA_TARGET_NONCOHERENT_SYSTEM: u32 = 2;

/// GSP_FMC_INIT_PARAMS + GSP_ACR_BOOT_GSP_RM_PARAMS + GSP_RM_PARAMS +
/// GSP_SPDM_PARAMS, 80 bytes. SPDM stays zero (no confidential compute)
#[repr(C)]
#[derive(Copy, Clone, Default)]
pub struct GspFmcBootParams {
    // GSP_FMC_INIT_PARAMS
    pub regkeys: u32,
    _pad0: u32,
    // GSP_ACR_BOOT_GSP_RM_PARAMS
    pub boot_target: u32,
    pub gsp_rm_desc_size: u32,
    pub gsp_rm_desc_offset: u64,
    pub wpr_carveout_offset: u64,
    pub wpr_carveout_size: u32,
    pub is_gsp_rm_boot: u8,
    _pad1: [u8; 3],
    // GSP_RM_PARAMS
    pub rm_target: u32,
    _pad2: u32,
    pub boot_args_offset: u64,
    // GSP_SPDM_PARAMS (unused: target/offset/size all zero)
    pub spdm_target: u32,
    _pad3: u32,
    pub spdm_payload_offset: u64,
    pub spdm_payload_size: u32,
    _pad4: u32,
}

const _: () = assert!(size_of::<GspFmcBootParams>() == 80);

impl GspFmcBootParams {
    /// gh100_gsp_init field fill: descriptor = the WPR meta block,
    /// boot args = the libos init-arg table
    pub fn new(wpr_meta_phys: u64, libos_phys: u64) -> Self {
        Self {
            boot_target: GSP_DMA_TARGET_COHERENT_SYSTEM,
            gsp_rm_desc_size: size_of::<GspFwWprMeta>() as u32,
            gsp_rm_desc_offset: wpr_meta_phys,
            is_gsp_rm_boot: 1,
            rm_target: GSP_DMA_TARGET_NONCOHERENT_SYSTEM,
            boot_args_offset: libos_phys,
            ..Default::default()
        }
    }

    pub fn write_bytes(&self, dst: &mut [u8]) {
        let n = size_of::<Self>();
        let src = unsafe {
            core::slice::from_raw_parts((self as *const Self).cast::<u8>(), n)
        };
        dst[..n].copy_from_slice(src);
    }
}

// Errors / report
#[derive(Debug, Clone, Copy)]
pub enum BootError {
    /// FSP never reported secure boot complete (scratch != 0xff)
    SecureBootTimeout(u32),
    /// A firmware blob is missing from /lib/firmware
    FirmwareMissing(&'static str),
    /// The blob exists but reading it failed (NoSpace = kernel heap could
    /// not hold the buffer; Io = ext2 read error)
    FirmwareRead(&'static str, crate::kcore::firmware::load::FwError),
    /// FMC ELF parse / crypto-size failure
    Fmc(FmcError),
    /// gsp-570.144.bin section parse failure
    GspElf,
    /// bootloader-570.144.bin container parse failure
    Bootloader,
    /// sysmem allocation failure (blob too large / fragmentation)
    Alloc,
    /// FSP rejected or never answered the COT message
    Fsp(FspError),
    /// RISCV_BR_PRIV_LOCKDOWN never cleared after COT accept
    LockdownTimeout { mbox0: u32, hwcfg2: u32 },
    /// FMC posted an error code in MAILBOX0/1
    FmcFailed { mbox0: u32, mbox1: u32 },
    /// GFW (devinit) never reached PROGRESS=0xFF: GSP-RM would assert on
    /// NV_PGC6_AON_SECURE_SCRATCH_GROUP_05_0_GFW_BOOT. GFW only runs at
    /// cold power-on; a failed FMC boot in an earlier session leaves this
    /// at 0 until the machine is fully powered off
    GfwNotCompleted(u32),
}

impl From<FmcError> for BootError { fn from(e: FmcError) -> Self { BootError::Fmc(e) } }
impl From<FspError> for BootError { fn from(e: FspError) -> Self { BootError::Fsp(e) } }

#[derive(Debug, Clone, Copy, Default)]
pub struct BootReport {
    pub fb_size: u64,
    pub heap_size: u64,
    pub frts_offset: u64,
    pub fmc_image_phys: u64,
    pub radix3_root_phys: u64,
    pub wpr_meta_phys: u64,
    pub boot_params_phys: u64,
    pub libos_phys: u64,
    pub cot_response_words: usize,
    pub lockdown_released: bool,
    pub mbox0: u32,
    /// First MSGQ message from GSP-RM: (function, signature), if any arrived
    pub gsp_first_msg: Option<(u32, u32)>,
    pub init_done: bool,
}

////////////////////////////////////////////////////////
//              Pinned boot state                     //
////////////////////////////////////////////////////////

/// Every sysmem region GSP-RM/FMC keeps referencing after the COT. Dropping
/// this while the GSP is alive would hand freed pages to a busmastering GPU
pub struct Gb206BootState {
    pub fmc_image: DmaBuffer,
    pub staged_fw: DmaBuffer,
    pub radix3: Radix3,
    pub bootloader: DmaBuffer,
    pub signature: DmaBuffer,
    pub wpr_meta: DmaBuffer,
    pub boot_params: DmaBuffer,
    pub bootargs: GspBootArgs,
    pub report: BootReport,
}

static GB206_BOOT: Mutex<Option<Gb206BootState>> = Mutex::new(None);

pub fn is_booted() -> bool { GB206_BOOT.lock().is_some() }

pub fn with_boot_state<R>(f: impl FnOnce(&Gb206BootState) -> R) -> Option<R> {
    GB206_BOOT.lock().as_ref().map(f)
}

/// Mutable access for the post-boot RPC layer (ring pointers live inside
/// the pinned shared region)
pub fn with_boot_state_mut<R>(f: impl FnOnce(&mut Gb206BootState) -> R) -> Option<R> {
    GB206_BOOT.lock().as_mut().map(f)
}

/////////////////////////////////////////////////////////////////////////////
//                          Boot orchestration                             //
/////////////////////////////////////////////////////////////////////////////

/// Wall-clock timeouts via PTIMER (ns). nouveau uses 4000 ms for both the
/// secure-boot wait and the lockdown-release poll
const LOCKDOWN_TIMEOUT_NS: u32 = 4_000_000_000;
const INIT_DONE_TIMEOUT_NS: u64 = 4_000_000_000;

fn stage_blob(bytes: &[u8]) -> Result<DmaBuffer, BootError> {
    let pages = bytes.len().div_ceil(PAGE_SIZE).max(1);
    let mut buf = DmaBuffer::alloc(pages).map_err(|_| BootError::Alloc)?;
    buf.zero();
    buf.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
    DmaBuffer::write_barrier();
    Ok(buf)
}

/// gh100_gsp_lockdown_released, one poll step.
/// Returns Ok(true) = released, Ok(false) = still busy, Err = FMC error
fn lockdown_step(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    boot_params_phys: u64,
) -> Result<(bool, u32), BootError> {
    let mbox0 = bar0.read32(PGSP_FALCON_BASE + FALCON_MAILBOX0);

    // 0xbadf41xx = GSP PRI access still fenced off, keep waiting
    if mbox0 != 0 && (mbox0 & 0xffff_ff00) == 0xbadf_4100 {
        return Ok((false, mbox0));
    }

    // Any non-zero value that is not the boot-params address is an FMC error
    if mbox0 != 0 {
        let mbox1 = bar0.read32(PGSP_FALCON_BASE + FALCON_MAILBOX1);
        if ((mbox1 as u64) << 32 | mbox0 as u64) != boot_params_phys {
            return Err(BootError::FmcFailed { mbox0, mbox1 });
        }
    }

    let hwcfg2 = bar0.read32(PGSP_FALCON_BASE + FALCON_HWCFG2);
    Ok((hwcfg2 & HWCFG2_RISCV_BR_PRIV_LOCKDOWN == 0, mbox0))
}

/// Full COT boot. Non-destructive up to the send_sync: everything before it
/// is sysmem staging and BAR0 reads. 'gpu' feeds the SET_SYSTEM_INFO RPC
/// that must sit in the cmdq before GSP-RM comes up
pub fn boot(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
) -> Result<BootReport, BootError> {
    let mut report = BootReport::default();

    // 0) FSP must have finished its own secure boot (POSTed card => 0xff)
    let fsp = Fsp::new(bar0);
    let scratch = fsp.boot_status();
    if scratch != FSP_BOOT_COMPLETE_SUCCESS {
        return Err(BootError::SecureBootTimeout(scratch));
    }

    // Blob read order matters: the small files go first and the 60 MB
    // gsp container is read inside its own scope. The kernel heap is
    // 128 MiB; holding the gsp Vec (plus the ext2 block cache the read
    // populates) while requesting another file starves try_reserve and
    // the load fails with NoSpace
    let fw_err = |p: &'static str| move |e: crate::kcore::firmware::load::FwError| match e {
        crate::kcore::firmware::load::FwError::NotFound | crate::kcore::firmware::load::FwError::NotMounted =>
            BootError::FirmwareMissing(p),
        other => BootError::FirmwareRead(p, other),
    };

    // 1) FMC: parse ELF32, verify GB20x crypto sizes, stage "image"
    // fmc_fw stays alive to the COT build (hash/pkey/sig borrow it)
    crate::println!("    [1/7] loading FMC...");
    let fmc_fw = crate::kcore::firmware::load::request(FW_FMC_PATH).map_err(fw_err(FW_FMC_PATH))?;
    let fmc = FmcImage::parse(fmc_fw.bytes())?;
    fmc.verify_gb20x()?;
    let fmc_image = stage_blob(fmc.image)?;
    report.fmc_image_phys = fmc_image.phys();
    serial_println!(
        "[gb206/boot] FMC image staged @ {:#x} ({} B)", fmc_image.phys(), fmc.image.len()
    );

    // 2) Bootloader (RM RISC-V monitor): stage the image, keep the desc
    // offsets, drop the file buffer
    crate::println!("    [2/7] loading bootloader...");
    let (bootloader, bl_size, bl_code, bl_data, bl_manifest) = {
        let bl_fw = crate::kcore::firmware::load::request(FW_BL_PATH).map_err(fw_err(FW_BL_PATH))?;
        let bl = parse_gsp_bootloader(bl_fw.bytes()).ok_or(BootError::Bootloader)?;
        let staged = stage_blob(bl.image)?;
        serial_println!(
            "[gb206/boot] bootloader staged @ {:#x} ({} B) code={:#x} data={:#x} manifest={:#x}",
            staged.phys(), bl.image.len(),
            bl.monitor_code_offset, bl.monitor_data_offset, bl.manifest_offset
        );
        (
            staged,
            bl.image.len() as u64,
            bl.monitor_code_offset as u64,
            bl.monitor_data_offset as u64,
            bl.manifest_offset as u64,
        )
    };

    // 3) GSP-RM: parse the container, stage .fwimage + signature, then
    // free the 60 MB heap buffer before anything else allocates
    crate::println!("    [3/7] loading gsp-rm (60 MB, this takes a while)...");
    let (staged_fw, fwimage_len, signature, sig_len) = {
        let gsp_fw = crate::kcore::firmware::load::request(FW_GSP_PATH).map_err(fw_err(FW_GSP_PATH))?;
        let rm = parse_gsp_rm_sig(gsp_fw.bytes(), GSP_SIGNATURE_SECTION_GB20X)
            .map_err(|e| { serial_println!("[gb206/boot] gsp elf: {:?}", e); BootError::GspElf })?;
        serial_println!(
            "[gb206/boot] gsp-rm: fwimage={} MB sig={} B version={}",
            rm.fwimage.len() >> 20, rm.signature.len(),
            core::str::from_utf8(rm.fwversion).unwrap_or("?")
        );
        let staged = stage_blob(rm.fwimage)?;
        let signature = stage_blob(rm.signature)?;
        (staged, rm.fwimage.len() as u64, signature, rm.signature.len() as u64)
    };
    crate::println!("    [4/7] building radix3 + boot args...");
    let mut data_phys = alloc::vec::Vec::with_capacity(staged_fw.pages());
    for i in 0..staged_fw.pages() {
        data_phys.push(staged_fw.phys() + (i * PAGE_SIZE) as u64);
    }
    let radix3 = Radix3::build(&data_phys).map_err(|_| BootError::Alloc)?;
    report.radix3_root_phys = radix3.root_phys();

    // 4) FB size (for the WPR heap sizing only; the FMC does the placement)
    let fb_size = (bar0.read32(FB_USABLE_SIZE_MB) as u64) << 20;
    let heap_size = wpr_heap_size(fb_size);
    report.fb_size = fb_size;
    report.heap_size = heap_size;
    serial_println!(
        "[gb206/boot] FB {} MiB -> WPR heap {} MiB, FRTS offset-from-end {:#x}",
        fb_size >> 20, heap_size >> 20, frts_vidmem_offset()
    );

    // 5) libos / CMDQ-MSGQ boot args (same ABI as Turing r535/r570)
    let mut bootargs = GspBootArgs::build().map_err(|_| BootError::Alloc)?;
    report.libos_phys = bootargs.libos_phys();

    // 5b) Prefill the cmdq with GSP_SET_SYSTEM_INFO + SET_REGISTRY BEFORE
    // the GSP boots. GSP-RM consumes both from the queue during its own
    // init and will not send GSP_INIT_DONE without them (nouveau does this
    // in r535_gsp_oneinit, tinygrad comments it "Prefill cmd queue with
    // info for gsp to start"). The doorbell write inside send() is a no-op
    // while the falcon is still locked down; GSP-RM reads writePtr from
    // the queue header in sysmem when it comes up
    {
        use crate::drivers::gpu::nvidia::gsp_common::rpc::{GspRpc, FN_GSP_SET_SYSTEM_INFO, FN_SET_REGISTRY};
        use crate::drivers::gpu::nvidia::gsp_common::sysinfo::{build_registry, GspSystemInfo};
        let mut rpc = GspRpc::new(bar0, &mut bootargs);
        let info = GspSystemInfo::for_gpu_gh100(gpu);
        rpc.send(FN_GSP_SET_SYSTEM_INFO, info.as_bytes(), 1_000_000_000)
            .map_err(|_| BootError::Alloc)?;
        let registry = build_registry(&[
            ("RMSecBusResetEnable", 1),
            ("RMForcePcieConfigSave", 1),
        ]);
        rpc.send(FN_SET_REGISTRY, &registry, 1_000_000_000)
            .map_err(|_| BootError::Alloc)?;
        DmaBuffer::write_barrier();
        serial_println!(
            "[gb206/boot] cmdq prefilled: SET_SYSTEM_INFO ({} B) + SET_REGISTRY ({} B)",
            size_of::<GspSystemInfo>(), registry.len()
        );
        crate::println!("    cmdq prefill: SET_SYSTEM_INFO + SET_REGISTRY (pre-boot, nouveau order)");
    }

    // 6) WPR meta: sysmem pointers + sizes only; FB offsets are FMC-owned
    let meta = GspFwWprMeta {
        magic: GSP_FW_WPR_META_MAGIC,
        revision: GSP_FW_WPR_META_REVISION,
        sysmem_addr_of_radix3_elf: radix3.root_phys(),
        size_of_radix3_elf: fwimage_len,
        sysmem_addr_of_bootloader: bootloader.phys(),
        size_of_bootloader: bl_size,
        bootloader_code_offset: bl_code,
        bootloader_data_offset: bl_data,
        bootloader_manifest_offset: bl_manifest,
        sysmem_addr_of_signature: signature.phys(),
        size_of_signature: sig_len,
        non_wpr_heap_size: HEAP_SIZE_NON_WPR,
        gsp_fw_heap_size: heap_size,
        frts_size: FRTS_SIZE,
        vga_workspace_size: 128 * 1024,
        pmu_reserved_size: RSVD_SIZE_PMU as u32,
        ..Default::default()
    };
    let mut wpr_meta = DmaBuffer::alloc(1).map_err(|_| BootError::Alloc)?;
    wpr_meta.zero();
    meta.write_bytes(wpr_meta.as_mut_slice());
    DmaBuffer::write_barrier();
    report.wpr_meta_phys = wpr_meta.phys();

    // 7) GSP_FMC_BOOT_PARAMS
    let params = GspFmcBootParams::new(wpr_meta.phys(), bootargs.libos_phys());
    let mut boot_params = DmaBuffer::alloc(1).map_err(|_| BootError::Alloc)?;
    boot_params.zero();
    params.write_bytes(boot_params.as_mut_slice());
    DmaBuffer::write_barrier();
    report.boot_params_phys = boot_params.phys();
    serial_println!(
        "[gb206/boot] wpr-meta @ {:#x}, boot-params @ {:#x}, libos @ {:#x}",
        wpr_meta.phys(), boot_params.phys(), bootargs.libos_phys()
    );

    // 8) COT
    let cot = NvdmPayloadCot::new_gb20x(
        fmc_image.phys(),
        frts_vidmem_offset(),
        FRTS_SIZE as u32,
        &fmc,
        boot_params.phys(),
    );
    report.frts_offset = frts_vidmem_offset();

    // GFW (devinit) boot progress gate. GSP-RM asserts on this scratch
    // during its own init (seen on live silicon), so surface the value
    // before the COT and give a slow GFW a few seconds to finish
    // (OpenRM kgspWaitForGfwBootOk does the same wait)
    let plm = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_PLM);
    let mut gfw = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_0);
    if gfw & 0xFF != GFW_BOOT_PROGRESS_COMPLETED {
        // Short grace only: on GB206 consumer silicon this scratch stays 0
        // even on boots where GSP-RM comes up fine (tinygrad never reads it
        // at all on the COT path), so don't burn 4 s here
        let sw = crate::kcore::time::Stopwatch::start();
        while sw.elapsed_ms() < 200 {
            gfw = bar0.read32(PGC6_AON_SECURE_SCRATCH_GROUP_05_0);
            if gfw & 0xFF == GFW_BOOT_PROGRESS_COMPLETED {
                break;
            }
            core::hint::spin_loop();
        }
    }
    crate::println!(
        "    gfw boot: progress={:#04x} (raw {:#010x}, plm={:#010x}){}",
        gfw & 0xFF, gfw, plm,
        if gfw & 0xFF == GFW_BOOT_PROGRESS_COMPLETED { " - COMPLETED" }
        else { " - not 0xff (non-fatal: GSP-RM logs a NOCAT assert but continues)" }
    );
    serial_println!(
        "[gb206/boot] GFW_BOOT_PROGRESS={:#010x} PLM={:#010x}", gfw, plm
    );
    // Note: NOT a gate. OpenRM's kgspWaitForGfwBootOk_GH100 reduces the
    // whole "GFW ok" check to "FSP secure boot done" on FSP-managed chips;
    // the PGC6 progress scratch legitimately reads 0 here pre-COT. GSP-RM
    // asserting on it later is a separate question under investigation

    crate::println!("    [5/7] sending COT to FSP...");
    crate::println!("    (note: the screen may freeze or blank while GSP-RM inits the");
    crate::println!("     display engine - the OS keeps running, wait ~30 s)");
    fsp.log_state("pre-COT");
    let resp = match fsp.send_sync(NVDM_TYPE_COT, &cot.as_words()) {
        Ok(r) => r,
        Err(e) => {
            fsp.log_state("post-COT-fail");
            // Queue pointers tell the story on-screen too: cmdq h==t means
            // the FSP consumed the command; msgq h==t means it never answered
            crate::println!(
                "    FSP after failure: scratch={:#04x} cmdq h/t={:#x}/{:#x} msgq h/t={:#x}/{:#x}",
                fsp.boot_status(),
                bar0.read32(PFSP_QUEUE_HEAD0), bar0.read32(PFSP_QUEUE_TAIL0),
                bar0.read32(PFSP_MSGQ_HEAD0), bar0.read32(PFSP_MSGQ_TAIL0),
            );
            return Err(e.into());
        }
    };
    report.cot_response_words = resp.len_words;
    serial_println!(
        "[gb206/boot] FSP accepted COT (response type {:#x}, {} words)",
        resp.nvdm_type, resp.len_words
    );

    // 9) Lockdown release poll (TSC-bounded, 4 s)
    crate::println!("    [6/7] waiting for RISC-V lockdown release...");
    let sw = crate::kcore::time::Stopwatch::start();
    let (released, last_mbox0) = loop {
        match lockdown_step(bar0, boot_params.phys())? {
            (true, m) => break (true, m),
            (false, m) => {
                if sw.elapsed_ms() as u64 * 1_000_000 >= LOCKDOWN_TIMEOUT_NS as u64 {
                    break (false, m);
                }
                core::hint::spin_loop();
            }
        }
    };
    report.lockdown_released = released;
    report.mbox0 = last_mbox0;
    if !released {
        let hwcfg2 = bar0.read32(PGSP_FALCON_BASE + FALCON_HWCFG2);
        return Err(BootError::LockdownTimeout { mbox0: last_mbox0, hwcfg2 });
    }
    serial_println!(
        "[gb206/boot] RISC-V lockdown released (mbox0={:#x}) - GSP-RM is executing",
        last_mbox0
    );

    // 10) Handshake: consume GSP-RM's boot-time events until GSP_INIT_DONE.
    // The first messages are diagnostics (GSP_POST_NOCAT_RECORD 0x1020,
    // libos prints); INIT_DONE arrives behind them, so each element must be
    // consumed, not just peeked. Budget one PTIMER window (~4 s) total
    {
        use crate::drivers::gpu::nvidia::gsp_common::rpc::{
            event_name, GspRpc, RpcError, EV_GSP_INIT_DONE, EV_GSP_POST_NOCAT_RECORD,
        };
        let mut rpc = GspRpc::new(bar0, &mut bootargs);
        // Budget: up to 240 half-second receive slices (~2 min of init
        // progress), bail after 20 consecutive silent slices (~10 s of
        // nothing). All timing is CPU TSC; this loop never touches PRI,
        // which matters because GSP-RM cycles PRI lockdown windows during
        // init (the GSP_LOCKDOWN_NOTICE events)
        let mut quiet_slices = 0u32;
        let mut event_counts: alloc::vec::Vec<(u32, u32)> = alloc::vec::Vec::new();
        let mut nocat_count = 0u32;
        let mut nocat_seq_first = 0u32;
        let mut nocat_seq_last = 0u32;
        // Distinct assert/diag texts already shown, to keep the screen
        // readable while still surfacing every unique complaint
        let mut seen_texts: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
        for _ in 0..240 {
            match rpc.recv(500_000_000) {
                Ok(m) => {
                    quiet_slices = 0;
                    serial_println!(
                        "[gb206/boot] GSP event {:#x} ({}) len={}",
                        m.function, event_name(m.function), m.payload.len()
                    );
                    if m.function == EV_GSP_POST_NOCAT_RECORD {
                        // Diagnostic record. Keep sequence bookkeeping so a
                        // stuck read pointer (same record re-read forever) is
                        // distinguishable from a genuine stream, and put the
                        // first record on screen - no serial needed
                        nocat_count += 1;
                        nocat_seq_last = m.sequence;
                        if nocat_count == 1 { nocat_seq_first = m.sequence; }
                        // Pull the ASCII runs out of the record and show any
                        // text not seen before (each unique complaint once)
                        let mut run = alloc::string::String::new();
                        let mut texts: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
                        for &b in m.payload.iter().chain(core::iter::once(&0u8)) {
                            if (0x20..0x7f).contains(&b) {
                                run.push(b as char);
                            } else {
                                if run.len() >= 6 { texts.push(core::mem::take(&mut run)); }
                                run.clear();
                            }
                        }
                        let is_new = texts.iter().any(|t| !seen_texts.contains(t));
                        if is_new && seen_texts.len() < 24 {
                            crate::println!("      nocat #{} (seq {}):", nocat_count, m.sequence);
                            for t in &texts {
                                if !seen_texts.contains(t) {
                                    crate::println!("        \"{}\"", t);
                                    seen_texts.push(t.clone());
                                }
                            }
                        } else if nocat_count % 20 == 0 {
                            crate::println!("      ... {} nocat records so far", nocat_count);
                        }
                        let mut head = [0u32; 8];
                        for (i, w) in head.iter_mut().enumerate() {
                            let o = i * 4;
                            if o + 4 <= m.payload.len() {
                                *w = u32::from_le_bytes(
                                    m.payload[o..o + 4].try_into().unwrap(),
                                );
                            }
                        }
                        serial_println!(
                            "[gb206/boot] nocat[{}] seq={}: {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x} {:08x}",
                            nocat_count, m.sequence, head[0], head[1], head[2], head[3],
                            head[4], head[5], head[6], head[7]
                        );
                    } else {
                        // Repeating events (LOCKDOWN_NOTICE, LIBOS_PRINT)
                        // flood during init; show the first of each kind
                        // and then every 50th
                        let n = match event_counts.iter_mut().find(|(f, _)| *f == m.function) {
                            Some((_, c)) => { *c += 1; *c }
                            None => { event_counts.push((m.function, 1)); 1 }
                        };
                        if n == 1 || n % 50 == 0 {
                            crate::println!(
                                "      gsp event: {:#x} {} (x{})",
                                m.function, event_name(m.function), n
                            );
                        }
                    }
                    if report.gsp_first_msg.is_none() {
                        report.gsp_first_msg = Some((m.function, m.sequence));
                    }
                    if m.function == EV_GSP_INIT_DONE {
                        report.init_done = true;
                        break;
                    }
                }
                Err(RpcError::RecvTimeout) => {
                    quiet_slices += 1;
                    if quiet_slices >= 20 {
                        serial_println!("[gb206/boot] msgq quiet for 10 s, giving up on INIT_DONE");
                        break;
                    }
                }
                Err(e) => {
                    serial_println!("[gb206/boot] handshake recv error: {:?}", e);
                    break;
                }
            }
        }
        if nocat_count > 0 {
            crate::println!(
                "      ({} NOCAT records, rpc seq {}..{}; try nvidia gb206-log for the GSP log)",
                nocat_count, nocat_seq_first, nocat_seq_last
            );
        }
    }

    *GB206_BOOT.lock() = Some(Gb206BootState {
        fmc_image,
        staged_fw,
        radix3,
        bootloader,
        signature,
        wpr_meta,
        boot_params,
        bootargs,
        report,
    });

    Ok(report)
}
