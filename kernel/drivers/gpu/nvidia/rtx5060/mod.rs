// NVIDIA GeForce RTX 5060 Ti / 5060 driver (GB206, Blackwell consumer)
//
// Boot model differs fundamentally from the Turing driver next door
// (gtx1650/): there is no FWSEC-FRTS on a GSP falcon and no booter on SEC2.
// The FSP (Firmware Security Processor) boots at devinit; the host stages
// GSP-FMC + GSP-RM in sysmem, sends one NVDM_TYPE_COT message over
// MCTP-over-EMEM and the FSP-verified FMC does the rest (WPR2 carve, GSP
// RISC-V start). Post-boot it is the same CMDQ/MSGQ RPC world as Turing,
// so gtx1650's msgq/rpc layers are reusable in later phases
//
// Reference sources: nouveau r570 (fsp/gh100.c, fsp/gb202.c, gsp/gh100.c,
// gsp/gb202.c, dev_fsp_pri.h) and open-gpu-kernel-modules 570.144.
// Full plan: docs/Nvidia_gb206.md

pub mod regs;
pub mod fsp;
pub mod fmc;
pub mod boot;
pub mod device;
pub mod init;

pub use device::Rtx5060;

use crate::drivers::gpu::nvidia::pci::GpuDevice;

/// Known GB206 SKUs. The 8 GB and 16 GB RTX 5060 Ti share one device id
const SKUS: &[(u16, &str)] = &[
    (0x2D04, "GeForce RTX 5060 Ti"),
    (0x2D05, "GeForce RTX 5060"),
    (0x2D18, "GeForce RTX 5070 Laptop"),
    (0x2D19, "GeForce RTX 5060 Laptop"),
];

/// True for the GB206 device-id block (0x2Dxx). SKUs outside the table
/// still bring up with a generic GB206 label
pub fn matches(device_id: u16) -> bool {
    (device_id & 0xFF00) == 0x2D00
}

pub fn model_name(device_id: u16) -> &'static str {
    SKUS.iter()
        .find(|(id, _)| *id == device_id)
        .map(|(_, name)| *name)
        .unwrap_or("GeForce RTX 50 series (GB206)")
}

pub fn init(gpu: &GpuDevice) -> Result<(), &'static str> {
    init::init(gpu)
}
