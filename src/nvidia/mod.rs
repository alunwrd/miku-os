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
//   nvidia/gtx1650/   GTX 1650 / 1660 (TU117 + TU116, Turing) driver             //
//                                                                                //
// Turing is the first NVIDIA family with a GSP (GPU System Processor)            //
// co-processor. Without a signed GSP firmware blob, most engines beyond          //
// the host registers are off-limits. The current driver only covers the          //
// host-side probe: BAR mapping, chip-ID read, MSI discovery, VBIOS extract.      //
////////////////////////////////////////////////////////////////////////////////////

pub mod pci;
pub mod mmio;
pub mod chip;
pub mod msi;
pub mod vbios;
pub mod fb;
pub mod gtx1650;

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
            if let Err(e) = gtx1650::init(gpu) {
                serial_println!("[nvidia] gtx1650 init failed: {}", e);
            }
        } else {
            serial_println!("[nvidia] device {:04x} not supported yet", gpu.device);
        }
    }
    Ok(())
}
