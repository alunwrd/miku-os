// GB206 bring-up orchestration
//
// Stages (each non-fatal; the card stays registered and inspectable):
//   1) BAR0 validation + map, memory decode + bus mastering, INTx off
//   2) chip identification: PMC_BOOT_42 names the chip (`chips.rs`), and
//      PMC_BOOT_0 is printed beside it
//   3) MSI/MSI-X capability walk (config reads only)
//   4) PTIMER liveness
//   5) FSP state: secure-boot scratch + ring pointers. On a VBIOS-posted
//      card the FSP finished its own secure boot long ago (scratch 0xFF)
//   6) firmware staging probe: FMC + GSP-RM blobs under /lib/firmware;
//      if the FMC is present it is parsed and its crypto material verified
//      against the GB20x COT v2 sizes
//
// The COT message is the GSP-RM boot proper (`sequence.rs`), which
// `nvidia::autoboot` runs next and `nvidia gb206-boot` can run by hand

use crate::drivers::gpu::nvidia::chip::{ChipId, PMC_BOOT_0};
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::bus::msi;
use crate::drivers::gpu::nvidia::pci::{self, GpuDevice};
use crate::serial_println;

use super::chips::Chip;
use super::device::{FwStatus, Gb20x};
use super::fmc::FmcImage;
use super::fsp::Fsp;
use super::regs::*;

pub fn init(gpu: &GpuDevice, gb: &'static Chip) -> Result<(), &'static str> {
    // 1) BAR0
    let bar0 = &gpu.bars[0];
    if bar0.is_io || bar0.phys == 0 || bar0.size == 0 {
        return Err("bar0 is not memory or not assigned by firmware");
    }
    pci::enable_memory_and_bus_master(gpu);
    pci::disable_intx(gpu);
    let bar0 = MmioRegion::new(bar0.phys, bar0.size);

    // 2) Identify
    let boot0 = bar0.read32(PMC_BOOT_0);
    let boot42 = bar0.read32(PMC_BOOT_42);
    let chip = ChipId::from_boot42(boot42);
    serial_println!(
        "[gb206] {:04x}:{:04x} PMC_BOOT_42={:#010x} -> {} ({}) rev {}.{}, fw {} (PMC_BOOT_0={:#010x})",
        gpu.vendor, gpu.device, boot42, gb.codename, chip.arch.name(),
        chip.major_rev, chip.minor_rev, gb.fw_dir, boot0
    );
    // Both registers carry the architecture and the implementation. If they
    // disagree, BOOT_42 stands: OpenRM builds the chip's HAL from it
    let b0 = ChipId::from_boot0(boot0);
    if (b0.arch_byte(), b0.implementation) != (chip.arch_byte(), chip.implementation) {
        serial_println!(
            "[gb206] warn: PMC_BOOT_0 says {}, PMC_BOOT_42 says {} - going by BOOT_42",
            b0.codename(), gb.codename
        );
    }

    // 3) MSI / MSI-X
    let caps = msi::read_caps(gpu.pci_fn());
    msi::log_capabilities("nvidia", &caps);

    // 4) PTIMER liveness (devinit indicator: FSP-posted cards tick)
    let t0 = bar0.read32(PTIMER_TIME_0);
    for _ in 0..5000 { core::hint::spin_loop(); }
    let t1 = bar0.read32(PTIMER_TIME_0);
    serial_println!(
        "[gb206] PTIMER {} ({} -> {})",
        if t1 != t0 { "alive" } else { "NOT advancing (devinit incomplete?)" },
        t0, t1
    );

    // 5) FSP state
    let fsp = Fsp::new(&bar0);
    let fsp_boot_status = fsp.boot_status();
    fsp.log_state("probe");
    if fsp_boot_status == FSP_BOOT_COMPLETE_SUCCESS {
        serial_println!("[gb206] FSP secure boot complete (scratch 0xff) - COT path is open");
    } else {
        serial_println!(
            "[gb206] FSP boot scratch = {:#04x} (not 0xff): card may not be POSTed or scratch offset needs verification",
            fsp_boot_status
        );
    }

    // 6) Firmware staging probe
    let fw = probe_firmware(gb);

    let dev = Gb20x {
        pci: gpu.clone(),
        bar0,
        chip,
        gb,
        caps,
        boot42,
        fw,
        fsp_boot_status,
    };
    crate::drivers::gpu::nvidia::set_active_gb20x(dev);
    serial_println!(
        "[gb206] registered {} ({}) - host-side + FSP transport; GSP-RM boot is next",
        super::model_name(gpu.device), gb.codename
    );
    Ok(())
}

/// Probe (and if present, validate) the on-disk firmware blobs
fn probe_firmware(gb: &Chip) -> FwStatus {
    let mut fw = FwStatus {
        fmc_size: crate::kcore::firmware::load::size_of(gb.fmc),
        gsp_size: crate::kcore::firmware::load::size_of(gb.gsp),
        bl_size: crate::kcore::firmware::load::size_of(gb.bootloader),
        ucodes_size: crate::kcore::firmware::load::size_of(gb.ucodes),
    };
    // None of the four is in linux-firmware for 615: `fw_extract.py` takes them
    // from the driver package (`dev/gsp-615.md`, "Firmware")
    let ver = crate::drivers::gpu::nvidia::gsp_common::GSP_RM_VERSION;

    match fw.fmc_size {
        Some(sz) => {
            serial_println!("[gb206] FMC blob present: {} ({} KB)", gb.fmc, sz / 1024);
            // Parse + verify crypto sizes now, so a wrong blob is caught at
            // boot, not at COT-send time
            if let Ok(blob) = crate::kcore::firmware::load::request(gb.fmc) {
                match FmcImage::parse(blob.bytes()) {
                    Ok(img) => {
                        img.log();
                        if img.verify_gb20x().is_err() {
                            crate::kwarn!("[gb206] warn: FMC crypto sizes wrong for GB20x COT v2; blob rejected");
                            fw.fmc_size = None;
                        }
                    }
                    Err(e) => {
                        crate::kwarn!("[gb206] warn: FMC ELF parse failed: {:?}; blob rejected", e);
                        fw.fmc_size = None;
                    }
                }
            }
        }
        None => serial_println!(
            "[gb206] FMC blob missing: /lib/firmware/{} (built by dev/nvidia/fw_extract.py from the {} driver package)",
            gb.fmc, ver
        ),
    }

    match fw.gsp_size {
        Some(sz) => serial_println!("[gb206] GSP-RM blob present: {} ({} MB)", gb.gsp, sz / (1024 * 1024)),
        None => serial_println!(
            "[gb206] GSP-RM blob missing: /lib/firmware/{} (built by dev/nvidia/fw_extract.py from the {} driver package)",
            gb.gsp, ver
        ),
    }

    match fw.bl_size {
        Some(sz) => serial_println!("[gb206] bootloader blob present: {} ({} KB)", gb.bootloader, sz / 1024),
        None => serial_println!(
            "[gb206] bootloader blob missing: /lib/firmware/{} (built by dev/nvidia/fw_extract.py from the {} driver package)",
            gb.bootloader, ver
        ),
    }

    match fw.ucodes_size {
        Some(sz) => serial_println!("[gb206] ucodes blob present: {} ({} MB)", gb.ucodes, sz / (1024 * 1024)),
        None => serial_println!(
            "[gb206] ucodes blob missing: /lib/firmware/{} (built by dev/nvidia/fw_extract.py from the {} driver package)",
            gb.ucodes, ver
        ),
    }

    fw
}
