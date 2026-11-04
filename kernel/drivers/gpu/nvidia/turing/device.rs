// GTX 1650 device state

use crate::drivers::gpu::nvidia::chip::ChipId;
use crate::drivers::gpu::nvidia::fb::FbLocation;
use crate::drivers::gpu::nvidia::mmio::MmioRegion;
use crate::drivers::bus::msi::Capabilities;
use crate::drivers::gpu::nvidia::pci::GpuDevice;

pub struct Gtx1650 {
    pub pci: GpuDevice,
    pub bar0: MmioRegion,      // MMIO registers
    pub bar1_phys: u64,         // framebuffer aperture (not yet mapped)
    pub bar1_size: u64,
    pub bar3_phys: u64,         // USER / IFB aperture
    pub bar3_size: u64,
    pub chip: ChipId,
    pub model_name: &'static str,
    pub caps: Capabilities,
    pub boot42: u32,            // extended chip ID from PMC_BOOT_42
    /// If the firmware's boot framebuffer falls inside one of our BARs, which
    /// BAR and at what offset: the display being painted on is routed through
    /// this card
    pub boot_fb: Option<FbLocation>,
}

impl Gtx1650 {
    /// Turing bring-up reaches BAR0 through the engine modules and PTIMER
    /// through `MmioRegion`, so nothing reads through here yet
    #[allow(dead_code)]
    pub fn read32(&self, off: u32) -> u32 { self.bar0.read32(off) }
    /// The write half of the pair, unused for the same reason
    #[allow(dead_code)]
    pub fn write32(&self, off: u32, v: u32) { self.bar0.write32(off, v); }

    /// Atomic 64-bit read of PTIMER (`MmioRegion::read_ptimer_ns`)
    pub fn read_ptimer_ns(&self) -> u64 {
        self.bar0.read_ptimer_ns()
    }
}
