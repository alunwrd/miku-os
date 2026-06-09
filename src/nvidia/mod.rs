////////////////////////////////////////////////////////////////////////////////////
//                      NVIDIA GPU drivers for MikuOS.                            //
//                                                                                //
// Layout:                                                                        //
//   nvidia/mod.rs     root module, probe entry, driver registry                  //
//   nvidia/pci.rs     PCI scan for class 0x03 + vendor 0x10DE, BAR sizing        //
//   nvidia/mmio.rs    MMIO primitives (volatile r/w over HHDM)                   //
//   nvidia/chip.rs    chip identification from PMC_BOOT_0                        //
//   nvidia/msi.rs     PCI MSI / MSI-X capability walker                          //
//   nvidia/vbios.rs   VBIOS image extraction from the PCI expansion ROM          //
//   nvidia/profile.rs per-chip profile: engine bases + firmware capability       //
//   nvidia/generic.rs host-side bring-up for any NVIDIA GPU (all families)       //
//   nvidia/gtx1650/   GTX 1650 / 1660 (TU117 + TU116, Turing) full driver        //
//                                                                                //
// Turing is the first NVIDIA family with a GSP (GPU System Processor)            //
// co-processor. Without a signed GSP firmware blob, most engines beyond          //
// the host registers are off-limits. Dispatch in init():                         //
//   - GTX 1650 / 1660 (TU116/TU117): the one chip with an embedded firmware      //
//     bundle, so it runs the full GSP-RM offload pipeline (gtx1650::init).       //
//   - every other NVIDIA card (Turing/Ampere/Ada/...): generic::bringup does     //
//     host-side recognition (BAR map, chip-ID, MSI, VBIOS, PMC, thermal,         //
//     Falcon liveness) and registers it; the firmware pipeline is gated on a     //
//     per-chip bundle that only TU116 ships today.                               //
////////////////////////////////////////////////////////////////////////////////////

pub mod pci;
pub mod mmio;
pub mod chip;
pub mod msi;
pub mod vbios;
pub mod fb;
pub mod profile;
pub mod gtx1650;
pub mod generic;

use spin::Mutex;

use crate::serial_println;

pub const VENDOR_NVIDIA: u16 = 0x10DE;

/// Global handle for the first GTX 1650 we bring up,none until init() runs
static ACTIVE_GTX1650: Mutex<Option<gtx1650::Gtx1650>> = Mutex::new(None);

pub fn set_active_gtx1650(dev: gtx1650::Gtx1650) {
    *ACTIVE_GTX1650.lock() = Some(dev);
}

/// Run a closure with the active GTX 1650 device, if any. Returns None if
/// the driver did not bring up a card
pub fn with_gtx1650<R>(f: impl FnOnce(&gtx1650::Gtx1650) -> R) -> Option<R> {
    let guard = ACTIVE_GTX1650.lock();
    guard.as_ref().map(f)
}

pub fn init() -> Result<(), &'static str> {
    let gpus = pci::scan_nvidia();
    if gpus.is_empty() {
        serial_println!("[nvidia] no NVIDIA GPU found");
        return Ok(());
    }

    for gpu in gpus.iter() {
        serial_println!(
            "[nvidia] found {:04x}:{:04x} at {:02x}:{:02x}.{:x} rev={:#x}",
            gpu.vendor, gpu.device, gpu.bus, gpu.dev, gpu.func, gpu.revision
        );
        if gtx1650::matches(gpu.device) {
            // GTX 1650 / 1660 (TU116/TU117): the one chip with an embedded
            // firmware bundle, so it runs the full GSP-RM pipeline
            if let Err(e) = gtx1650::init(gpu) {
                serial_println!("[nvidia] gtx1650 init failed: {}", e);
            }
        } else {
            // Any other NVIDIA GPU: generic host-side bring-up + register it.
            // Recognizes the chip, probes engines, but has no firmware so it
            // stops short of the GSP-RM offload pipeline
            match generic::bringup(gpu) {
                Ok(codename) => serial_println!(
                    "[nvidia] {:04x} brought up as {} (host-side only)", gpu.device, codename
                ),
                Err(e) => serial_println!(
                    "[nvidia] device {:04x} host-side bring-up failed: {}", gpu.device, e
                ),
            }
        }
    }
    Ok(())
}
