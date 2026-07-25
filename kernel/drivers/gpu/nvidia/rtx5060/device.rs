// GB206 device handle pinned in the driver registry after bring-up

use crate::drivers::gpu::nvidia::chip::ChipId;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::gpu::nvidia::msi::Capabilities;
use crate::drivers::gpu::nvidia::pci::GpuDevice;

/// On-disk firmware status probed at init (files under /lib/firmware)
#[derive(Clone, Copy, Debug, Default)]
pub struct FwStatus {
    /// nvidia/gb206/gsp/fmc-570.144.bin present (GSP-FMC chain-of-trust image)
    pub fmc_size: Option<u64>,
    /// nvidia/gb206/gsp/gsp-570.144.bin present (GSP-RM itself)
    pub gsp_size: Option<u64>,
    /// nvidia/gb206/gsp/bootloader-570.144.bin present (RM RISC-V monitor)
    pub bl_size: Option<u64>,
}

pub struct Rtx5060 {
    pub pci: GpuDevice,
    pub bar0: MmioRegion,
    pub chip: ChipId,
    pub caps: Capabilities,
    pub boot42: u32,
    pub fw: FwStatus,
    /// FSP secure-boot scratch as read at init time
    pub fsp_boot_status: u32,
}
