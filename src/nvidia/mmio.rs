/////////////////////////////////////////////////////////////////////////////
//               MMIO primitives for NVIDIA GPUs                           //
//                                                                         //
// A BAR physical address is translated through the higher-half direct     //
// mapping (HHDM) which in MikuOS covers the full physical range. If a     // 
// BAR ever lands outside of HHDM (some real hardware), an explicit        //
// vmm::map_range with PCD/PWT flags will be needed. That can be added     //
// later without changing the MmioRegion API.                              //
/////////////////////////////////////////////////////////////////////////////

use core::ptr;

use crate::grub;

#[derive(Clone, Copy)]
pub struct MmioRegion {
    virt_base: u64,
    size: u64,
}

impl MmioRegion {
    pub fn new(phys: u64, size: u64) -> Self {
        // Remap the BAR range as strict UC. On real hardware the HHDM 1-GiB
        // pages default to WB, so MMIO writes (e.g. pmc::mask_all_interrupts)
        // would sink into L1 instead of reaching silicon. Skip page 0 guard
        // implicitly via size check - a zero-sized BAR means unassigned
        if phys != 0 && size != 0 {
            crate::vmm::map_mmio_uc(phys, size);
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

    #[inline(always)]
    pub fn read16(&self, off: u32) -> u16 {
        unsafe { ptr::read_volatile((self.virt_base + off as u64) as *mut u16) }
    }

    #[inline(always)]
    pub fn read8(&self, off: u32) -> u8 {
        unsafe { ptr::read_volatile((self.virt_base + off as u64) as *mut u8) }
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
