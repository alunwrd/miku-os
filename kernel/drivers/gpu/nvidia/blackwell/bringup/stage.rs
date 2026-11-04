// The GSP boot's first half: everything staged in system memory before the COT
// goes to the FSP
//
// (`dev/gsp-615.md`, section 5): the FSP's own secure boot waited for, the
// sysmem flush buffer and the GPU's time set (the head of `kgspInitRm_IMPL`),
// the images read and staged, the GSP-RM image and the ucodes container each
// behind a radix3 table, the WPR heap sized from the framebuffer, the boot
// arguments built and our ring announced (`msgqTxCreate`), GSP_INIT's system
// information encoded (`_kgspPrepareSystemInfo`, `_kgspPrepareRegistry`: they
// go out once GSP-RM is up), the WPR metadata, the FMC boot parameters, and the
// COT pointing the FSP at all of it. Nothing here touches the GSP but
// QUEUE_HEAD.
//
// `gspbase` on the boot line leaves out the flush buffer, the time,
// `bIsPrimary` from the machine, the heap size and the CrashCat queue, so the
// boot is told what card boot A was told (`bringup::base_init`).

use crate::drivers::gpu::nvidia::gsp_common::bootargs::GspBootArgs;
use crate::drivers::gpu::nvidia::gsp_common::gspfw::{
    parse_gsp_bootloader, parse_gsp_rm_sig, GspFwWprMeta, Radix3,
    GSP_FW_WPR_META_MAGIC, GSP_FW_WPR_META_REVISION,
};
use crate::mm::dma::DmaBuffer;
use crate::serial_println;

use super::super::chips::Chip;
use super::super::fmc::{FmcImage, NvdmPayloadCot};
use super::super::gspboot::{
    align_up, frts_vidmem_offset, phase, stage_blob, wpr_heap_size, wpr_heap_size_for, BootError,
    BootReport, GspFmcBootParams, HEAP_SIZE_NON_WPR, PAGE_SIZE, RSVD_SIZE_PMU,
};
use super::super::hal_gb20x;
use super::super::regs::*;

/// What staging leaves for the rest of the boot: the memory the GSP will be
/// pointed at, the boot arguments, and the COT that points it
pub(in super::super) struct Staged {
    pub fmc_image: DmaBuffer,
    pub bootloader: DmaBuffer,
    /// Written to FALCON_OS once the falcon is out of lockdown
    pub bl_app_version: u32,
    pub staged_fw: DmaBuffer,
    pub signature: DmaBuffer,
    pub radix3: Radix3,
    /// The ucodes container and its radix3, GSP-RM's bindata until GSP_INIT
    /// has answered (`pExternalBindata`)
    pub ucodes: (DmaBuffer, Radix3),
    pub bootargs: GspBootArgs,
    /// GSP_INIT's data: the system information and the registry, as NVKV
    pub gsp_init: alloc::vec::Vec<u8>,
    pub wpr_meta: DmaBuffer,
    pub boot_params: DmaBuffer,
    pub cot: NvdmPayloadCot,
}

pub(super) fn stage(
    bar0: &crate::drivers::gpu::nvidia::mmio::MmioRegion,
    gpu: &crate::drivers::gpu::nvidia::pci::GpuDevice,
    chip: &'static Chip,
    not_primary: bool,
    disp_vidmem: bool,
    report: &mut BootReport,
) -> Result<Staged, BootError> {
    // 0) What `kgspInitRm_IMPL` does before it stages anything. The FSP must
    //    have finished its own secure boot (`kgspWaitForGfwBootOk_GH100`); on a
    //    POSTed card the wait returns at once.
    //
    // The flush buffer is programmed after that wait, as FBHUB is written here
    // and nothing is written before the FSP says it is done
    let fsp_ms = hal_gb20x::wait_secure_boot(bar0)?;
    let base = super::base_init();
    let (flush, time_offset) = if base {
        (0, 0)
    } else {
        (super::super::memsys::init_flush_buffer(bar0), hal_gb20x::set_current_time(bar0))
    };
    report.flush_buffer = flush;
    report.time_offset_ns = time_offset;
    crate::println!(
        "    fsp secure boot {} ms, flush buffer {}, gpu time {}",
        fsp_ms,
        if base { "not set (gspbase)".into() } else if flush == 0 { "NONE".into() } else { alloc::format!("{:#x}", flush) },
        if base { "not set" } else if time_offset == 0 { "not set (see serial)" } else { "set to wall clock" },
    );

    // Blob read order matters: the small files first, the 60 MB gsp container
    // inside its own scope. The kernel heap is 128 MiB; holding the gsp Vec
    // (plus the ext2 block cache the read populates) while requesting another
    // file starves try_reserve, and the load fails with NoSpace
    let fw_err = |p: &'static str| move |e: crate::kcore::firmware::load::FwError| match e {
        crate::kcore::firmware::load::FwError::NotFound | crate::kcore::firmware::load::FwError::NotMounted =>
            BootError::FirmwareMissing(p),
        other => BootError::FirmwareRead(p, other),
    };

    // 1) FMC: parse ELF32, verify GB20x crypto sizes, stage "image"
    // fmc_fw stays alive to the COT build (hash/pkey/sig borrow it)
    phase(1, "loading FMC...");
    let fmc_fw = crate::kcore::firmware::load::request(chip.fmc).map_err(fw_err(chip.fmc))?;
    let fmc = FmcImage::parse(fmc_fw.bytes())?;
    fmc.verify_gb20x()?;
    let fmc_image = stage_blob(fmc.image)?;
    report.fmc_image_phys = fmc_image.phys();
    serial_println!(
        "[gb206/boot] FMC image staged @ {:#x} ({} B)", fmc_image.phys(), fmc.image.len()
    );

    // 2) Bootloader (RM RISC-V monitor): stage the image, keep the desc
    // offsets, drop the file buffer
    phase(2, "loading bootloader...");
    let (bootloader, bl_size, bl_code, bl_data, bl_manifest, bl_app_version) = {
        let bl_fw = crate::kcore::firmware::load::request(chip.bootloader).map_err(fw_err(chip.bootloader))?;
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
            bl.app_version,
        )
    };

    // 3) GSP-RM: parse the container, stage .fwimage + signature, then
    // free the 60 MB heap buffer before anything else allocates
    phase(3, "loading gsp-rm (60 MB, slow)...");
    let (staged_fw, fwimage_len, signature, sig_len) = {
        let gsp_fw = crate::kcore::firmware::load::request(chip.gsp).map_err(fw_err(chip.gsp))?;
        let rm = parse_gsp_rm_sig(gsp_fw.bytes(), chip.sig_section)
            .map_err(|e| { crate::kerror!("[gb206/boot] gsp elf: {:?}", e); BootError::GspElf })?;
        serial_println!(
            "[gb206/boot] gsp-rm: fwimage={} MB sig={} B version={}",
            rm.fwimage.len() >> 20, rm.signature.len(),
            core::str::from_utf8(rm.fwversion).unwrap_or("?")
        );
        let staged = stage_blob(rm.fwimage)?;
        let signature = stage_blob(rm.signature)?;
        (staged, rm.fwimage.len() as u64, signature, rm.signature.len() as u64)
    };
    phase(4, "radix3 + ucodes + boot args...");
    let pages_of = |b: &DmaBuffer| (0..b.pages()).map(|i| b.phys() + (i * PAGE_SIZE) as u64).collect::<alloc::vec::Vec<_>>();
    let radix3 = Radix3::build(&pages_of(&staged_fw)).map_err(|_| BootError::Alloc)?;
    report.radix3_root_phys = radix3.root_phys();

    // 3b) The ucodes container, whole, behind a radix3 of its own: GSP-RM takes
    // the microcode it needs by index (`_kgspPrepareGspRmBinaryImage`,
    // `kgspCreateRadix3`). Read after the GSP-RM image has been let go, for the
    // same reason as the order above
    let (ucodes, ucodes_size) = {
        let f = crate::kcore::firmware::load::request(chip.ucodes).map_err(fw_err(chip.ucodes))?;
        if f.bytes().get(..8) != Some(&b"NVUCODES"[..]) {
            crate::kerror!("[gb206/boot] {} is not an NVUCODES container", chip.ucodes);
            return Err(BootError::GspElf);
        }
        (stage_blob(f.bytes())?, f.bytes().len() as u64)
    };
    let ucodes_radix3 = Radix3::build(&pages_of(&ucodes)).map_err(|_| BootError::Alloc)?;
    serial_println!(
        "[gb206/boot] ucodes staged @ {:#x} ({} MiB), radix3 @ {:#x}",
        ucodes.phys(), ucodes_size >> 20, ucodes_radix3.root_phys()
    );

    // 4) `_kgspBootGspRm` starts by refusing a WPR2 that is up: a GSP-RM booted
    //    and never unloaded. Logged and not refused (counters before
    //    enforcement)
    let wpr2 = hal_gb20x::wpr2_hi(bar0);
    report.wpr2_hi = wpr2;
    if hal_gb20x::is_wpr2_up(wpr2) {
        serial_println!(
            "[gb206/boot] WPR2 is already up (hi {:#010x}) - OpenRM refuses to boot here; the GPU may need a reset",
            wpr2
        );
    }

    // FB size (for the WPR heap sizing only; the FMC does the placement)
    let fb_size = (bar0.read32(FB_USABLE_SIZE_MB) as u64) << 20;
    let heap_size = wpr_heap_size(fb_size);
    let heap_base = wpr_heap_size_for(fb_size, true);
    report.fb_size = fb_size;
    report.heap_size = heap_size;
    serial_println!(
        "[gb206/boot] FB {} MiB -> WPR heap {} MiB (boot A asked {} MiB), FRTS offset-from-end {:#x}, WPR2 hi {:#010x}",
        fb_size >> 20, heap_size >> 20, heap_base >> 20, frts_vidmem_offset(), wpr2
    );
    // On screen too: FB size lives in the same PGC6 AON scratch window as the
    // GFW progress byte. 0 MiB here means the whole window reads back as zero,
    // unlike devinit not having run
    crate::println!(
        "    fb {} MiB -> wpr heap {} MiB (was {}), wpr2 {}",
        fb_size >> 20, heap_size >> 20, heap_base >> 20,
        if hal_gb20x::is_wpr2_up(wpr2) { "UP - see serial" } else { "down" }
    );

    // 5) The boot arguments, GSP-RM's bindata in them, and our ring
    // announced: its header is in place and QUEUE_HEAD reads 0
    // (`GspMsgQueuesInit` -> `msgqTxCreate`)
    let mut bootargs = GspBootArgs::build().map_err(|_| BootError::Alloc)?;
    bootargs.set_bindata(ucodes_radix3.root_phys(), ucodes_size);
    report.libos_phys = bootargs.libos_phys();
    crate::drivers::gpu::nvidia::gsp_common::rpc::GspRpc::new(bar0, &mut bootargs).tx_create();

    // 5b) What GSP_INIT will carry, prepared now, as OpenRM prepares it
    // while it still holds the locks it needs (`kgspInitRm_IMPL`)
    let gsp_init = {
        use crate::drivers::gpu::nvidia::gsp_common::sysinfo::SystemInfo;
        let mut info = SystemInfo::for_gpu(gpu);
        // What `tmrSetCurrentTime` measured, and whether this GPU is the
        // primary device. `gspbase` sends boot A's: no offset, and primary on
        // every card
        info.sys_timer_offset_ns = time_offset;
        if base {
            info.is_primary = true;
        }
        // The chipset and its ports, as `clSyncWithGsp` gives them (step 1.17).
        // `gspbase` sends none
        if !base {
            if let Some(cl) = crate::drivers::gpu::nvidia::chipset::last() {
                crate::drivers::gpu::nvidia::chipset::fill(&cl, &mut info);
            }
        }
        if not_primary {
            info.is_primary = false;
            crate::println!("    primary forced off (GSP-RM should not inherit the console)");
        }
        // The console size changes what GSP-RM does to the aperture the
        // firmware console scans, so it goes on screen: 0 means GSP-RM treats
        // the boot framebuffer as free vidmem
        crate::println!(
            "    console reserve: {} KiB{}, primary {}",
            info.console_mem_size >> 10,
            if info.console_mem_size == 0 { " (boot fb not in this GPU's BAR1)" } else { "" },
            info.is_primary
        );
        // The registry nouveau calls "required for GSP-RM". RMDevidCheckIgnore
        // lets GSP-RM boot when the PCI device ID is not in its product-name
        // database
        let mut keys = alloc::vec![
            ("RMSecBusResetEnable", 1u32),
            ("RMForcePcieConfigSave", 1),
            ("RMDevidCheckIgnore", 1),
        ];
        if disp_vidmem {
            // NV_REG_STR_RM_INST_LOC_DISPLAY is bits 15:14 and
            // NV_REG_STR_RM_INST_LOC_4_DISP_SC is bits 1:0; _VID is 3 in both.
            // Every other field stays DEFAULT (0), so this changes only the two
            // display allocations
            keys.push(("RMInstLoc", 3 << 14));
            keys.push(("RMInstLoc4", 3));
            crate::println!("    display instmem + state cache forced to vidmem");
        }
        let data = info.encode(&keys);
        serial_println!(
            "[gb206/boot] GSP_INIT prepared: system info and {} registry keys, {} B of NVKV",
            keys.len(), data.len()
        );
        data
    };

    // 6) WPR meta: sysmem pointers + sizes only; FB offsets are FMC-owned. With
    //    them the CrashCat queue, as OpenRM gives GB202-207 one
    //    (`kernel_gsp_gh100.c:363-370`)
    let crash_queue = {
        use crate::drivers::gpu::nvidia::gsp_common::kgsp::crashcat;
        if base {
            crashcat::release(false);
            None
        } else {
            crashcat::configure()
        }
    };
    report.crash_queue = crash_queue.map_or(0, |(phys, _)| phys);
    serial_println!("[gb206/boot] crashcat queue: {}", match crash_queue {
        Some((phys, size)) => alloc::format!("{:#x} + {:#x}", phys, size),
        None if base => "none (gspbase)".into(),
        None => "none".into(),
    });
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
        // nouveau allocates the signature buffer as ALIGN(size, 256) and
        // reports that padded size; the ACR inside the FMC DMAs sizeOfSignature
        // bytes and expects the 256-byte granularity (gh100_gsp_oneinit ->
        // nvkm_gsp_mem_ctor)
        size_of_signature: align_up(sig_len, 256),
        non_wpr_heap_size: HEAP_SIZE_NON_WPR,
        gsp_fw_heap_size: heap_size,
        frts_size: FRTS_SIZE,
        vga_workspace_size: 128 * 1024,
        pmu_reserved_size: RSVD_SIZE_PMU as u32,
        sysmem_addr_of_crash_report_queue: crash_queue.map_or(0, |(phys, _)| phys),
        size_of_crash_report_queue: crash_queue.map_or(0, |(_, size)| size),
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

    Ok(Staged {
        fmc_image, bootloader, bl_app_version, staged_fw, signature, radix3,
        ucodes: (ucodes, ucodes_radix3), bootargs, gsp_init, wpr_meta, boot_params, cot,
    })
}
