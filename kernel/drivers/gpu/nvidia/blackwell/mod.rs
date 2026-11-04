// NVIDIA Blackwell consumer driver: GeForce RTX 5060 Ti / 5060 (GB206)
//
// The boot model differs from turing/: no FWSEC-FRTS on a GSP falcon and no
// booter on SEC2. The FSP (Firmware Security Processor) boots at devinit; the
// host stages GSP-FMC + GSP-RM in sysmem, sends one NVDM_TYPE_COT message over
// MCTP-over-EMEM, and the FSP-verified FMC does the rest (WPR2 carve, GSP
// RISC-V start). After boot it is the CMDQ/MSGQ RPC world of Turing.
//
// Reference sources: open-gpu-kernel-modules 615.78.08 (`dev/gsp-615.md`) and,
// for the first FSP steps, nouveau r570 (fsp/gh100.c, fsp/gb202.c, gsp/gh100.c,
// gsp/gb202.c, dev_fsp_pri.h). Full plan: docs/Nvidia_gb206.md

pub mod regs;
pub mod fsp;
pub mod fmc;
// `boot.rs` is a facade re-exporting the names of the modules below.
//
// The arrow points one way; there are no cycles:
//
//   bringup  -> gspboot, hal_gb20x, memsys, display, channel, chanmem, interrupts
//   display  -> gspboot, modeset, window, chanmem
//   interrupts -> chanmem, window
//   window   -> modeset
//   channel  -> chanmem, compute_chan
//   compute_chan -> chanmem
//   gspboot, modeset, chanmem: leaves
//
// Two edges are arranged: the video memory carve constants (`VRAM_*`) live in
// `window.rs`, or `display` and `window` would need each other; the channel
// statics live in `chanmem.rs`, or `channel` and `compute_chan` would.
pub mod bar1;
pub mod boot;
mod bringup;
mod hal_gb20x;
pub mod chips;
pub(crate) mod chanmem;
mod control;
/// The display layer's update on this card: every head's change, one UPDATE
/// (`dev/kms-architecture.md`, K2)
pub mod apply;
mod channel;
mod compute_chan;
mod display;
mod engines;
mod gspboot;
mod interrupts;
mod memsys;
mod modeset;
mod window;
pub mod device;
pub mod init;

pub use device::Gb20x;

use crate::drivers::gpu::nvidia::pci::GpuDevice;

/// Known GB206 SKUs. The 8 GB and 16 GB RTX 5060 Ti share one device id
const SKUS: &[(u16, &str)] = &[
    (0x2D04, "GeForce RTX 5060 Ti"),
    (0x2D05, "GeForce RTX 5060"),
    (0x2D18, "GeForce RTX 5070 Laptop"),
    (0x2D19, "GeForce RTX 5060 Laptop"),
];

/// The marketing name for a device id, if it is one of the SKUs above.
/// PMC_BOOT_42 decides the chip (`chips.rs`)
pub fn model_name(device_id: u16) -> &'static str {
    SKUS.iter()
        .find(|(id, _)| *id == device_id)
        .map(|(_, name)| *name)
        .unwrap_or("GeForce RTX 50 series")
}

pub fn init(gpu: &GpuDevice, chip: &'static chips::Chip) -> Result<(), &'static str> {
    init::init(gpu, chip)
}
