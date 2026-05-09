// GTX 1650 device state

use crate::nvidia::chip::ChipId;
use crate::nvidia::fb::FbLocation;
use crate::nvidia::mmio::MmioRegion;
use crate::nvidia::msi::Capabilities;
use crate::nvidia::pci::GpuDevice;

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
    /// If the boot framebuffer from firmware falls inside one of our BARs,
    /// this records which BAR and at what offset. A 'Some' value means the
    /// display we are already painting on is physically routed through this
    /// card
    pub boot_fb: Option<FbLocation>,
}

impl Gtx1650 {
    pub fn read32(&self, off: u32) -> u32 { self.bar0.read32(off) }
    pub fn write32(&self, off: u32, v: u32) { self.bar0.write32(off, v); }

    /// Atomic 64-bit read of PTIMER. TIME_1 is sampled twice to detect a
    /// wraparound between reading TIME_0 and the high word
    pub fn read_ptimer_ns(&self) -> u64 {
        use super::regs::{PTIMER_TIME_0, PTIMER_TIME_1};
        loop {
            let hi1 = self.read32(PTIMER_TIME_1);
            let lo  = self.read32(PTIMER_TIME_0);
            let hi2 = self.read32(PTIMER_TIME_1);
            if hi1 == hi2 {
                return ((hi1 as u64) << 32) | (lo as u64);
            }
        }
    }
}
