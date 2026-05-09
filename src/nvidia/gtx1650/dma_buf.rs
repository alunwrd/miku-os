// Phys-contiguous DMA buffer holder for the GTX 1650 driver
//
// The Falcon DMA engine needs source/destination memory that is
//   1. physically contiguous (it does not gather scattered pages),
//   2. visible to the GPU's PCIe master (true for any sysmem the kernel allocator hands us),
//   3. 256-byte aligned (covered by 4K page alignment)
//
// MikuOS already exposes 'pmm::alloc_frames(N)' which returns the phys
// base of N physically contiguous 4 KiB pages. This module wraps that
// allocator with a thin RAII handle plus a CPU-side slice view through
// the kernel's HHDM mapping
//
// On x86 + COHERENT_SYSMEM aperture, the GPU snoops CPU caches on read,
// so writes the CPU has retired (followed by an 'sfence'/SeqCst fence)
// are guaranteed to be visible to a DMA read kicked afterwards

#![allow(dead_code)]

use crate::grub;
use crate::pmm;

const PAGE_SIZE: usize = 4096;

/// A physically-contiguous DMA buffer
/// Owns its frames; freeing on Drop returns them to the kernel allocator
pub struct DmaBuffer {
    phys:  u64,
    pages: usize,
}

#[derive(Debug, Copy, Clone)]
pub enum DmaBufError {
    /// 'pages' was zero
    EmptyAllocation,
    /// 'pmm::alloc_frames' could not satisfy the request
    AllocFailed { pages: usize },
}

impl DmaBuffer {
    /// Allocate 'pages' physically-contiguous 4 KiB pages
    pub fn alloc(pages: usize) -> Result<Self, DmaBufError> {
        if pages == 0 { return Err(DmaBufError::EmptyAllocation); }
        match pmm::alloc_frames(pages) {
            Some(phys) => Ok(Self { phys, pages }),
            None       => Err(DmaBufError::AllocFailed { pages }),
        }
    }

    #[inline]
    pub fn phys(&self) -> u64 { self.phys }

    #[inline]
    pub fn size(&self) -> usize { self.pages * PAGE_SIZE }

    #[inline]
    pub fn pages(&self) -> usize { self.pages }

    /// CPU virtual address of the buffer base, via HHDM
    #[inline]
    pub fn virt_base(&self) -> *mut u8 {
        (self.phys + grub::hhdm()) as *mut u8
    }

    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.virt_base(), self.size()) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.virt_base(), self.size()) }
    }

    pub fn zero(&mut self) {
        for b in self.as_mut_slice() { *b = 0; }
    }

    /// Fill the buffer with a 32-bit pattern (little-endian)
    pub fn fill_u32(&mut self, pattern: u32) {
        let s = self.as_mut_slice();
        let mut i = 0;
        while i + 4 <= s.len() {
            s[i..i+4].copy_from_slice(&pattern.to_le_bytes());
            i += 4;
        }
    }

    /// Issue a write barrier so anything written through 'as_mut_slice'
    /// is visible to a subsequent DMA read kicked by the GPU
    pub fn write_barrier() {
        core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        pmm::free_frames(self.phys, self.pages);
    }
}
