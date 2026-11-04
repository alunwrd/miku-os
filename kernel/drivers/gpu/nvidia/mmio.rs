// MMIO primitives for NVIDIA GPUs
//
// A BAR physical address is translated through the higher-half direct mapping
// (HHDM), which covers the full physical range. A BAR outside the HHDM would
// need an explicit vmm::map_range with PCD/PWT flags

use core::ptr;

use crate::arch::x86_64::grub;

#[derive(Clone, Copy)]
pub struct MmioRegion {
    virt_base: u64,
    size: u64,
}

impl MmioRegion {
    pub fn new(phys: u64, size: u64) -> Self {
        // Remap the BAR range as strict UC. The HHDM 1-GiB pages default to WB,
        // so MMIO writes (e.g. pmc::mask_all_interrupts) would sink into L1
        // instead of reaching silicon. A zero-sized BAR means unassigned
        if phys != 0 && size != 0 {
            crate::mm::vmm::map_mmio_uc(phys, size);
        }
        Self { virt_base: grub::phys_to_virt(phys), size }
    }

    pub fn size(&self) -> u64 { self.size }
    pub fn virt_base(&self) -> u64 { self.virt_base }

    #[inline(always)]
    fn ptr32(&self, off: u32) -> *mut u32 {
        debug_assert!((off as u64) + 4 <= self.size, "mmio r/w beyond BAR size");
        (self.virt_base + off as u64) as *mut u32
    }

    #[inline(always)]
    pub fn read32(&self, off: u32) -> u32 {
        unsafe { ptr::read_volatile(self.ptr32(off)) }
    }

    #[inline(always)]
    pub fn write32(&self, off: u32, val: u32) {
        unsafe { ptr::write_volatile(self.ptr32(off), val); }
    }

    /// True if a read came back as a PRI error, not register data. The widest
    /// of the three tests in `pri`; that module says when a narrower one is
    /// meant
    #[inline(always)]
    pub fn is_pri_error(v: u32) -> bool {
        crate::drivers::gpu::nvidia::pri::is_error(v)
    }

    /// PTIMER as one 64-bit count of nanoseconds. TIME_1 is read on both
    /// sides of TIME_0, so a carry between the two halves is seen and the
    /// read taken again
    pub fn read_ptimer_ns(&self) -> u64 {
        use crate::drivers::gpu::nvidia::regs::{PTIMER_TIME_0, PTIMER_TIME_1};
        loop {
            let hi1 = self.read32(PTIMER_TIME_1);
            let lo = self.read32(PTIMER_TIME_0);
            let hi2 = self.read32(PTIMER_TIME_1);
            if hi1 == hi2 {
                return ((hi1 as u64) << 32) | (lo as u64);
            }
        }
    }

    /// Read-modify-write: (current & !mask)/(new & mask)
    pub fn modify32(&self, off: u32, mask: u32, new: u32) {
        let cur = self.read32(off);
        self.write32(off, (cur & !mask) | (new & mask));
    }

    /// Poll a register until (read & mask) == expected, or 'max_spins'
    /// iterations have elapsed. Returns true on match, false on timeout
    pub fn wait32(&self, off: u32, mask: u32, expected: u32, max_spins: u32) -> bool {
        for _ in 0..max_spins {
            if self.read32(off) & mask == expected {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }
}
