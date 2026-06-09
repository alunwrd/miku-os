// Generic host-side bring-up for any NVIDIA GPU
//
// gtx1650 is the one chip with an embedded firmware bundle, so it runs the
// full GSP-RM offload pipeline. Every other NVIDIA card (other Turing SKUs,
// Ampere, Ada, ...) has no firmware shipped in this tree, but the host-side
// register map is generic across the whole GSP era. This module brings such
// a card up as far as is possible without firmware and registers it so the
// shell can inspect it:
//
//   1) validate + map BAR0
//   2) enable memory decode + bus mastering, mask legacy INTx
//   3) read PMC_BOOT_0 / PMC_BOOT_42, decode the chip, resolve its profile
//   4) walk PCI MSI / MSI-X capabilities
//   5) read-only PMC_ENABLE snapshot + PTIMER liveness
//   6) on Turing, mask top-level interrupts and read the on-die thermal
//      sensor (the interrupt tree and PTHERM layout moved on Ampere, so we
//      do NOT poke those offsets on later families; reads of an unmodelled
//      register could be meaningless and a *write* to the wrong offset is
//      not worth the risk for a card we are only probing)
//   7) probe every Falcon engine in the profile for liveness (HWCFG reads
//      only - safe on any architecture)
//
// Nothing here writes to an engine or to PMC_ENABLE: this is a pure
// recognition + diagnostics path. The full firmware pipeline stays behind
// the per-chip bundle gate in 'ChipProfile::has_firmware'

use alloc::vec::Vec;
use spin::Mutex;

use crate::nvidia::chip::{Architecture, ChipId, PMC_BOOT_0};
use crate::nvidia::gtx1650::falcon::Engine;
use crate::nvidia::gtx1650::regs::{PMC_BOOT_42, PTIMER_TIME_0};
use crate::nvidia::gtx1650::{pmc, therm};
use crate::nvidia::mmio::MmioRegion;
use crate::nvidia::msi::{self, Capabilities};
use crate::nvidia::pci::{self, GpuDevice};
use crate::nvidia::profile::ChipProfile;
use crate::serial_println;

/// A non-GTX1650 NVIDIA GPU brought up to host-side level. Pinned in the
/// registry so the shell can inspect it later
pub struct GenericGpu {
    pub pci: GpuDevice,
    pub bar0: MmioRegion,
    pub chip: ChipId,
    pub profile: ChipProfile,
    pub caps: Capabilities,
    pub boot42: u32,
}

/// Every generic NVIDIA GPU we have brought up. The GTX 1650 has its own
/// dedicated slot (`ACTIVE_GTX1650`); this holds the rest
static GENERIC_GPUS: Mutex<Vec<GenericGpu>> = Mutex::new(Vec::new());

/// Run a closure over the list of generic GPUs. Returns the closure result
pub fn with_generic_gpus<R>(f: impl FnOnce(&[GenericGpu]) -> R) -> R {
    let guard = GENERIC_GPUS.lock();
    f(guard.as_slice())
}

/// Number of generic GPUs currently registered
pub fn count() -> usize {
    GENERIC_GPUS.lock().len()
}

/// Bring up a single NVIDIA GPU at host-side level and register it. Returns
/// the resolved chip codename on success. Errors are returned (not logged
/// fatally) so the caller can keep enumerating other devices
pub fn bringup(gpu: &GpuDevice) -> Result<&'static str, &'static str> {
    // 1) BAR0 must be a real memory window
    let bar0 = &gpu.bars[0];
    if bar0.is_io || bar0.phys == 0 || bar0.size == 0 {
        return Err("bar0 is not memory or not assigned by firmware");
    }

    // 2) Memory decode + bus master on, legacy INTx masked
    pci::enable_memory_and_bus_master(gpu);
    pci::disable_intx(gpu);

    // 3) Map BAR0 and identify the chip
    let bar0_region = MmioRegion::new(bar0.phys, bar0.size);
    let boot0 = bar0_region.read32(PMC_BOOT_0);
    let boot42 = bar0_region.read32(PMC_BOOT_42);
    let chip = ChipId::from_boot0(boot0);
    let profile = ChipProfile::resolve(&chip);

    serial_println!(
        "[nvidia/generic] {:04x}:{:04x} PMC_BOOT_0={:#010x} -> {} ({}, impl={:#x} rev={}.{} step={})",
        gpu.vendor, gpu.device, boot0, chip.codename(), profile.arch.name(),
        chip.implementation, chip.major_rev, chip.minor_rev, chip.stepping
    );
    serial_println!("[nvidia/generic] PMC_BOOT_42={:#010x} model: {}", boot42, profile.model_hint);

    if !profile.arch.is_known() {
        serial_println!(
            "[nvidia/generic] warn: unrecognized architecture (arch byte {:#x}); probing read-only with Turing register map",
            boot0 >> 24
        );
    }

    // 4) MSI / MSI-X discovery (PCI config reads only)
    let caps = msi::read_caps(gpu);
    msi::log_capabilities(&caps);

    // 5) Read-only PMC_ENABLE snapshot + PTIMER liveness (stable offsets
    //    across the GSP era; both are pure reads)
    let enable = pmc::read_enable(&bar0_region);
    serial_println!("[nvidia/generic] PMC_ENABLE = {:#010x}", enable);

    let t0 = bar0_region.read32(PTIMER_TIME_0);
    for _ in 0..5000 { core::hint::spin_loop(); }
    let t1 = bar0_region.read32(PTIMER_TIME_0);
    if t1 != t0 {
        serial_println!("[nvidia/generic] PTIMER alive ({} -> {})", t0, t1);
    } else {
        serial_println!("[nvidia/generic] warn: PTIMER did not advance");
    }

    // 6) Turing-only writes: mask the top-level interrupt tree and read the
    //    on-die thermal sensor. The interrupt tree and PTHERM layout changed
    //    on Ampere, so we skip both there rather than poke unmodelled offsets
    if matches!(chip.arch, Architecture::Turing) {
        pmc::mask_all_interrupts(&bar0_region);
        serial_println!("[nvidia/generic] top-level interrupts masked");

        let temp = therm::read(&bar0_region);
        if temp.valid {
            serial_println!(
                "[nvidia/generic] GPU temp: {} C{} (TEMP_SENSOR={:#010x})",
                temp.celsius, if temp.shadowed { " (stale)" } else { "" }, temp.raw
            );
        } else {
            serial_println!("[nvidia/generic] GPU temp: sensor not valid (TEMP_SENSOR={:#010x})", temp.raw);
        }
    } else {
        serial_println!(
            "[nvidia/generic] {}: interrupt-mask + thermal skipped (offsets differ from Turing; not modelled)",
            profile.arch.name()
        );
    }

    // 7) Falcon engine liveness across the profile's bases (HWCFG reads)
    probe_falcons(&bar0_region, &profile);

    // Register the brought-up card
    let codename = chip.codename();
    GENERIC_GPUS.lock().push(GenericGpu {
        pci: gpu.clone(),
        bar0: bar0_region,
        chip,
        profile,
        caps,
        boot42,
    });

    serial_println!(
        "[nvidia/generic] registered {} ({}) - host-side only{}",
        codename, profile.arch.name(),
        if profile.has_firmware { "" } else { " (no embedded firmware; GSP-RM pipeline unavailable)" }
    );
    Ok(codename)
}

/// Probe every Falcon engine in the profile and log its liveness. Pure
/// HWCFG reads, safe regardless of whether the base is correct for this
/// exact chip (a wrong base reads the PRI sentinel and is reported gated)
fn probe_falcons(bar0: &MmioRegion, profile: &ChipProfile) {
    let engines = [
        ("sec2",   profile.engines.sec2),
        ("gsp",    profile.engines.gsp),
        ("nvdec0", profile.engines.nvdec0),
        ("fecs",   profile.engines.fecs),
        ("gpccs0", profile.engines.gpccs0),
        ("gpccs1", profile.engines.gpccs1),
    ];
    for (name, base) in engines {
        let e = Engine::new(bar0, base, name);
        if e.is_alive() {
            serial_println!(
                "[nvidia/generic] falcon {:<6} @ BAR0+{:#x}: imem={} dmem={} halted={}",
                name, base, e.imem_size(), e.dmem_size(), e.is_halted()
            );
        } else {
            serial_println!(
                "[nvidia/generic] falcon {:<6} @ BAR0+{:#x}: {:?} (gated or absent on this chip)",
                name, base, e.liveness()
            );
        }
    }
}
