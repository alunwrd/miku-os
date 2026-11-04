// NVIDIA Turing driver: GeForce GTX 1650 / 1660 (TU116, TU117)
//
// Variants:
//   Desktop GDDR5:        TU117 (primary)
//   Desktop GDDR6:        TU117 (late revisions) and TU116 (refresh, dev 0x2188)
//   GTX 1650 SUPER:       TU116
//   GTX 1660 / 1660 Ti / 1660 SUPER: TU116
//   Mobile/Max-Q:         TU117M / TU116M
//
// TU117 and TU116 share the host-side register layout (PMC, PBUS, PTIMER,
// PFIFO, Falcon/GSP). They differ in the PMC_BOOT_0 implementation field (0x7
// vs 0x8) and the PCI device-ID range (0x1Fxx vs 0x21xx). Each silicon has its
// SKU/device-id table in tu117.rs / tu116.rs.
//
// Turing is the first NVIDIA generation with a GSP (GPU System Processor) on an
// embedded RISC-V core. NVIDIA's open kernel module offloads most setup to a
// signed GSP firmware; without it a driver is limited to host-side registers

pub mod tu117;
pub mod tu116;
pub mod tu116_fw;
pub mod quirks;
pub mod gsp;
pub mod gsprm;
pub mod sec2;
pub mod fwsec;
pub mod nvdec;
pub mod regs;
pub mod device;
pub mod init;
pub mod therm;

pub use device::Gtx1650;

use crate::drivers::gpu::nvidia::pci::GpuDevice;

/// The /lib/firmware directory of this architecture's blobs, the prefix of
/// every `fwload::request` here
pub const FW_DIR: &str = "nvidia/tu116";

/// True if 'device_id' is one of the supported GTX 1650 / 1660 SKUs, either
/// TU117 or TU116 silicon
pub fn matches(device_id: u16) -> bool {
    tu117::DEVICE_IDS.iter().any(|&id| id == device_id)
        || tu116::matches(device_id)
}

/// Resolve a printable model name across both silicon variants. TU116 first:
/// its device-id range is disjoint from TU117, and the TU117 fallback returns a
/// generic unknown-SKU string
pub fn model_name(device_id: u16) -> &'static str {
    if tu116::matches(device_id) {
        tu116::model_name(device_id)
    } else {
        tu117::model_name(device_id)
    }
}

pub fn init(gpu: &GpuDevice) -> Result<(), &'static str> {
    init::init(gpu)
}
