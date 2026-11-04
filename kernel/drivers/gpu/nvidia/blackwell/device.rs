// GB206 device handle pinned in the driver registry after bring-up

use crate::drivers::gpu::nvidia::chip::ChipId;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::bus::msi::Capabilities;
use crate::drivers::gpu::nvidia::pci::GpuDevice;

/// On-disk firmware status probed at init: the four files `Chip` names,
/// under /lib/firmware
#[derive(Clone, Copy, Debug, Default)]
pub struct FwStatus {
    /// GSP-FMC, the chain-of-trust image
    pub fmc_size: Option<u64>,
    /// GSP-RM itself
    pub gsp_size: Option<u64>,
    /// The RM RISC-V monitor
    pub bl_size: Option<u64>,
    /// The ucodes GSP-RM loads from the host (615 on)
    pub ucodes_size: Option<u64>,
}

/// The card as bring-up found it. `caps` is the MSI/MSI-X capability walk,
/// discovered and printed at init; `intr::enable` walks it again at the end of
/// a boot, since the card may have been reset meanwhile
#[allow(dead_code)]
#[derive(Clone)]
pub struct Gb20x {
    pub pci: GpuDevice,
    pub bar0: MmioRegion,
    pub chip: ChipId,
    /// Which GB20x, by PMC_BOOT_42, and where its firmware is
    pub gb: &'static super::chips::Chip,
    pub caps: Capabilities,
    pub boot42: u32,
    pub fw: FwStatus,
    /// FSP secure-boot scratch as read at init time
    pub fsp_boot_status: u32,
}
