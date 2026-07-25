// GB206 bring-up orchestration
//
// Stages (each non-fatal; the card stays registered and inspectable):
//   1) BAR0 validation + map, memory decode + bus mastering, INTx off
//   2) chip identification: expect PMC_BOOT_0 arch 0x1B impl 0x6 (GB206)
//   3) MSI/MSI-X capability walk (config reads only)
//   4) PTIMER liveness
//   5) FSP state: secure-boot scratch + ring pointers. On a VBIOS-posted
//      card the FSP finished its own secure boot long ago (scratch 0xFF)
//   6) firmware staging probe: FMC + GSP-RM blobs under /lib/firmware;
//      if the FMC is present it is parsed and its crypto material verified
//      against the GB20x COT v2 sizes
//
// What deliberately does NOT happen here: sending the COT message. That
// starts the real boot and needs FRTS/boot-args sysmem blocks staged first;
// it is exposed as an explicit shell command once the plumbing is complete
// (see docs/Nvidia_gb206.md, phases 2-3)

use crate::drivers::gpu::nvidia::chip::{Architecture, ChipId, PMC_BOOT_0};
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::msi;
use crate::drivers::gpu::nvidia::pci::{self, GpuDevice};
use crate::serial_println;

use super::device::{FwStatus, Rtx5060};
use super::fmc::FmcImage;
use super::fsp::Fsp;
use super::regs::*;

pub const FW_FMC_PATH: &str = "nvidia/gb206/gsp/fmc-570.144.bin";
pub const FW_GSP_PATH: &str = "nvidia/gb206/gsp/gsp-570.144.bin";

pub fn init(gpu: &GpuDevice) -> Result<(), &'static str> {
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
    let chip = ChipId::from_boot0(boot0);
    serial_println!(
        "[gb206] {:04x}:{:04x} PMC_BOOT_0={:#010x} -> {} ({}) rev {}.{}",
        gpu.vendor, gpu.device, boot0, chip.codename(), chip.arch.name(),
        chip.major_rev, chip.minor_rev
    );
    if chip.arch != Architecture::Blackwell {
        serial_println!("[gb206] warn: PCI id matched GB206 but PMC_BOOT_0 disagrees; continuing read-only");
    }

    // 3) MSI / MSI-X
    let caps = msi::read_caps(gpu);
    msi::log_capabilities(&caps);

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
    let fw = probe_firmware();

    let dev = Rtx5060 {
        pci: gpu.clone(),
        bar0,
        chip,
        caps,
        boot42,
        fw,
        fsp_boot_status,
    };
    crate::drivers::gpu::nvidia::set_active_rtx5060(dev);
    serial_println!(
        "[gb206] registered {} - host-side + FSP transport; GSP-RM boot pending phases 2-3",
        super::model_name(gpu.device)
    );
    Ok(())
}

/// Probe (and if present, validate) the on-disk firmware blobs
fn probe_firmware() -> FwStatus {
    let mut fw = FwStatus {
        fmc_size: crate::kcore::firmware::load::size_of(FW_FMC_PATH),
        gsp_size: crate::kcore::firmware::load::size_of(FW_GSP_PATH),
        bl_size: crate::kcore::firmware::load::size_of(super::boot::FW_BL_PATH),
    };

    match fw.fmc_size {
        Some(sz) => {
            serial_println!("[gb206] FMC blob present: {} ({} KB)", FW_FMC_PATH, sz / 1024);
            // Parse + verify crypto sizes right away so a wrong blob is
            // caught at boot, not at COT-send time
            if let Ok(blob) = crate::kcore::firmware::load::request(FW_FMC_PATH) {
                match FmcImage::parse(blob.bytes()) {
                    Ok(img) => {
                        img.log();
                        if img.verify_gb20x().is_err() {
                            serial_println!("[gb206] warn: FMC crypto sizes wrong for GB20x COT v2; blob rejected");
                            fw.fmc_size = None;
                        }
                    }
                    Err(e) => {
                        serial_println!("[gb206] warn: FMC ELF parse failed: {:?}; blob rejected", e);
                        fw.fmc_size = None;
                    }
                }
            }
        }
        None => serial_println!(
            "[gb206] FMC blob missing: /lib/firmware/{} (copy from linux-firmware >= 570.144)",
            FW_FMC_PATH
        ),
    }

    match fw.gsp_size {
        Some(sz) => serial_println!("[gb206] GSP-RM blob present: {} ({} MB)", FW_GSP_PATH, sz / (1024 * 1024)),
        None => serial_println!(
            "[gb206] GSP-RM blob missing: /lib/firmware/{} (copy from linux-firmware >= 570.144)",
            FW_GSP_PATH
        ),
    }

    match fw.bl_size {
        Some(sz) => serial_println!("[gb206] bootloader blob present: {} ({} KB)", super::boot::FW_BL_PATH, sz / 1024),
        None => serial_println!(
            "[gb206] bootloader blob missing: /lib/firmware/{} (copy from linux-firmware >= 570.144)",
            super::boot::FW_BL_PATH
        ),
    }

    fw
}
