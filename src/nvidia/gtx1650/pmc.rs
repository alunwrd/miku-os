// Host-side (PMC / PBUS / PFIFO / PTIMER) helpers for the GTX 1650 driver.
//
// These never touch engines guarded by GSP. They only:
//   mask every top-level interrupt source so stray IRQs cannot fire
//   before the driver is able to service them, read PTIMER scaling and PMC_ENABLE for diagnostics, expose PMC_BOOT_2 straps for the debug command

use crate::nvidia::mmio::MmioRegion;

use super::regs::{
    PBUS_INTR_0, PBUS_INTR_EN_0, PFIFO_INTR_0, PFIFO_INTR_EN_0,
    PMC_BOOT_2, PMC_ENABLE, PMC_ENABLE_CE0, PMC_ENABLE_GR,
    PMC_INTR_0, PMC_INTR_EN_0, PTIMER_DENOMINATOR, PTIMER_NUMERATOR,
};

#[derive(Copy, Clone, Debug, Default)]
pub struct PtimerFreq {
    pub numerator: u32,
    pub denominator: u32,
}

impl PtimerFreq {
    /// Input-clock frequency in Hz, derived from numerator/denominator.
    /// PTIMER scales its input clock so one tick = (denom/numer) ns
    /// Returns 0 if the registers are zero (unreliable/not programmed)
    pub fn input_clock_hz(&self) -> u64 {
        if self.numerator == 0 || self.denominator == 0 {
            return 0;
        }
        // ticks-per-second = numerator * 1e9 / denominator when tick = denom/numer ns.
        (self.numerator as u64).saturating_mul(1_000_000_000) / self.denominator as u64
    }
}

/// Mask every top-level interrupt source the driver can see and ack any
/// pending bits. Safe to call during init before MSI is programmed
pub fn mask_all_interrupts(bar0: &MmioRegion) {
    bar0.write32(PMC_INTR_EN_0, 0);
    bar0.write32(PBUS_INTR_EN_0, 0);
    bar0.write32(PFIFO_INTR_EN_0, 0);
    // Ack anything already latched. Most Turing interrupt-pending registers
    // are write-1-to-clear; writing the whole word we just read is the
    // standard nouveau pattern
    let p = bar0.read32(PMC_INTR_0);
    if p != 0 { bar0.write32(PMC_INTR_0, p); }
    let b = bar0.read32(PBUS_INTR_0);
    if b != 0 { bar0.write32(PBUS_INTR_0, b); }
    let f = bar0.read32(PFIFO_INTR_0);
    if f != 0 { bar0.write32(PFIFO_INTR_0, f); }
}

pub fn read_enable(bar0: &MmioRegion) -> u32 {
    bar0.read32(PMC_ENABLE)
}

/// Read-modify-write PMC_ENABLE: OR 'bits' into the current value, flush
/// the write back via a read, then return the post-write register value
///
/// PMC_ENABLE gates whole engine clocks. Setting a bit is the closest
/// thing to "powering on" an engine block from the host side; clearing
/// one mid-flight will hang any engine still mid-transaction. We never
/// clear bits here - callers can only set
pub fn pmc_enable_set(bar0: &MmioRegion, bits: u32) -> u32 {
    let cur = bar0.read32(PMC_ENABLE);
    let new = cur | bits;
    if new != cur {
        bar0.write32(PMC_ENABLE, new);
        // Read-back forces the write to drain on the PRI bus before
        // anyone tries to talk to the engine through its own window
        let _ = bar0.read32(PMC_ENABLE);
    }
    bar0.read32(PMC_ENABLE)
}

/// Ungate the engines we want before touching their Falcon windows
///
/// What this enables:
///   PMC_ENABLE_GR  (bit 12): brings the PGRAPH cluster online -
///   FECS / GPCCS register windows stop returning the
///   0xBADF_xxxx PRI sentinel and start exposing real HWCFG.
///   PMC_ENABLE_CE0 (bit 14): copy engine 0, used by ACR for
///   sysmem<->VRAM staging once we have a DMA buffer allocator
///
/// What this does NOT enable:
///   SEC2/GSP/NVDEC - on Turing those live in
///   NV_PMC_DEVICE_ENABLE_0 (BAR0+0x88c), which has a chip-specific
///   bit layout we do not yet model. SEC2 and GSP come up alive
///   after POST on the GTX 1650, so we do not need them ungated by
///   hand for first contact.
///   PMC_ENABLE_PWR (PMU): we do not run a PMU image.
///   PMC_ENABLE_DISP: the display block is already owned by GRUB's
///   framebuffer; touching its enable would be visually disruptive
///
/// Returns (before, after) so the caller can log the transition
pub fn ungate_default_engines(bar0: &MmioRegion) -> (u32, u32) {
    let before = bar0.read32(PMC_ENABLE);
    let after  = pmc_enable_set(bar0, PMC_ENABLE_GR | PMC_ENABLE_CE0);
    (before, after)
}

pub fn read_straps(bar0: &MmioRegion) -> u32 {
    bar0.read32(PMC_BOOT_2)
}

pub fn read_ptimer_freq(bar0: &MmioRegion) -> PtimerFreq {
    PtimerFreq {
        numerator:   bar0.read32(PTIMER_NUMERATOR),
        denominator: bar0.read32(PTIMER_DENOMINATOR),
    }
}
