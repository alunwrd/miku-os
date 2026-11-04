// Phys-contiguous DMA buffer holder
//
// Every busmastering device wants the same thing: the NVIDIA Falcon engines,
// the xHCI controller (`drivers/input/usb/xhci.rs`), the compositor's cursor
// plane and its shared memory pools.
//
// A DMA engine needs source/destination memory that is
//   1. physically contiguous (it does not gather scattered pages),
//   2. visible to the device's PCIe master (true for any sysmem the kernel
//      allocator hands us),
//   3. 256-byte aligned (covered by 4K page alignment)
//
// 'pmm::alloc_frames(N)' returns the phys base of N physically contiguous 4 KiB
// pages. This module wraps that allocator with a thin RAII handle plus a
// CPU-side slice view through the kernel's HHDM mapping
//
// On x86 + COHERENT_SYSMEM aperture, the GPU snoops CPU caches on read, so
// writes the CPU has retired (followed by an 'sfence'/SeqCst fence) are visible
// to a DMA read kicked afterwards

#![allow(dead_code)]

use crate::arch::x86_64::grub;
use crate::mm::pmm;

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
        Self::alloc_aligned(pages, 1, u64::MAX)
    }

    /// 'pages' contiguous pages that all lie below 'limit': for a device
    /// that addresses 32 bits
    pub fn alloc_below(pages: usize, limit: u64) -> Result<Self, DmaBufError> {
        Self::alloc_aligned(pages, 1, limit)
    }

    /// 'pages' contiguous pages starting on a multiple of 'align' pages (a
    /// power of two), all below 'limit' (`u64::MAX` for anywhere)
    pub fn alloc_aligned(pages: usize, align: usize, limit: u64) -> Result<Self, DmaBufError> {
        if pages == 0 { return Err(DmaBufError::EmptyAllocation); }
        match pmm::alloc_frames_aligned(pages, align, limit) {
            Some(phys) => Ok(Self { phys, pages }),
            None       => Err(DmaBufError::AllocFailed { pages }),
        }
    }

    /// The buffer for good, as bytes, with its physical address: for one
    /// that is handed over for the rest of the boot (the console's shadow)
    pub fn leak(self) -> (&'static mut [u8], u64) {
        let (virt, size, phys) = (self.virt_base(), self.size(), self.phys);
        core::mem::forget(self);
        // SAFETY: the frames are this buffer's, and nobody frees them now
        (unsafe { core::slice::from_raw_parts_mut(virt, size) }, phys)
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
        // SAFETY: the whole buffer is this one's frames
        unsafe { core::ptr::write_bytes(self.virt_base(), 0, self.size()) };
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

    /// Pushes the whole buffer out of the CPU caches into DRAM, for a buffer a
    /// device reads without snooping (an aperture that says non-coherent, with
    /// No Snoop enabled on its link): the HHDM maps it write-back, so what the
    /// CPU wrote can sit in a cache line the device never looks at. OpenRM
    /// allocates such buffers uncached (`NV_MEMORY_UNCACHED`); a buffer written
    /// once and then handed over gets the same guarantee from this
    pub fn flush_to_memory(&self) {
        flush_lines(self.virt_base(), self.size());
    }
}

/// Pushes the bytes covering [ptr, ptr+len) out of the cache hierarchy into
/// DRAM, then fences so nothing that follows is ordered before the flush
pub fn flush_lines(ptr: *const u8, len: usize) {
    const LINE: usize = 64;
    let start = ptr as usize & !(LINE - 1);
    let end = ptr as usize + len;
    let mut p = start;
    while p < end {
        unsafe { core::arch::x86_64::_mm_clflush(p as *const u8) };
        p += LINE;
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::SeqCst);
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        pmm::free_frames(self.phys, self.pages);
    }
}

/// Below this a device that addresses 32 bits can reach
pub const DMA32: u64 = 1 << 32;

/// A `T` laid out for a device: a driver's rings, command lists and bounce
/// buffers.
///
/// The heap is neither contiguous nor below 4 GiB (`dev/kernel-core.md`,
/// "Devices get device memory"), so device memory gets zeroed frames of its
/// own, page aligned (or `T`'s alignment if more), and below a limit for a
/// device that needs one. `phys_of` answers for a pointer inside it and for
/// nothing else.
///
/// Made in place, never on the stack: NVMe's queue memory is 76 KiB
pub struct DmaBox<T: Zeroed> {
    buf: DmaBuffer,
    _t: core::marker::PhantomData<T>,
}

/// All-zero bytes are a valid `T`: plain bytes, integers, arrays of them.
///
/// # Safety
/// Only for a type whose every field is valid as zeroes
pub unsafe trait Zeroed {}

unsafe impl Zeroed for u8 {}
unsafe impl<T: Zeroed, const N: usize> Zeroed for [T; N] {}

impl<T: Zeroed> DmaBox<T> {
    /// A zeroed `T` anywhere in memory, for a device that addresses 64 bits
    pub fn new() -> Result<Self, DmaBufError> {
        Self::new_below(u64::MAX)
    }

    /// A zeroed `T` below `limit` (`DMA32` for a 32-bit device)
    pub fn new_below(limit: u64) -> Result<Self, DmaBufError> {
        let size = core::mem::size_of::<T>().max(1);
        let pages = size.div_ceil(PAGE_SIZE);
        let align = (core::mem::align_of::<T>() / PAGE_SIZE).max(1);
        let mut buf = DmaBuffer::alloc_aligned(pages, align, limit)?;
        buf.zero();
        Ok(Self { buf, _t: core::marker::PhantomData })
    }

    /// The physical address of the whole `T`
    #[inline]
    pub fn phys(&self) -> u64 {
        self.buf.phys()
    }

    /// The physical address of a pointer into this `T` (a field, an array
    /// element). Panics for one outside: a device handed an address that is not
    /// its memory writes into somebody else's
    #[inline]
    pub fn phys_of<U>(&self, ptr: *const U) -> u64 {
        let base = self.buf.virt_base() as usize;
        let at = ptr as usize;
        assert!(
            at >= base && at < base + core::mem::size_of::<T>().max(1),
            "DmaBox::phys_of: {:#x} is not inside this buffer at {:#x}", at, base
        );
        self.buf.phys() + (at - base) as u64
    }
}

impl<T: Zeroed> core::ops::Deref for DmaBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        // SAFETY: the frames hold a zeroed `T`, which `Zeroed` says is valid,
        // aligned by `new_below`, and owned by this box
        unsafe { &*(self.buf.virt_base() as *const T) }
    }
}

impl<T: Zeroed> core::ops::DerefMut for DmaBox<T> {
    fn deref_mut(&mut self) -> &mut T {
        // SAFETY: as `deref`, and `&mut self` is the only way in
        unsafe { &mut *(self.buf.virt_base() as *mut T) }
    }
}
