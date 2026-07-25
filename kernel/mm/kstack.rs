// Kernel stacks with a guard page.
//
// Thread kernel stacks used to be 'vec![0u8; 512 KiB]' on the kernel heap.
// Nothing separated one stack from the next allocation, and since the whole
// kernel image is mapped by a single 1 GiB huge page there was no fault to
// be had either - a thread that ran off the bottom of its stack simply kept
// writing into whatever the allocator had handed out below it. That is the
// same failure mode that made a 0.5 MiB VFS temporary silently zero the APIC
// state on the boot stack, except corrupting the heap is even harder to
// trace back to its cause.
//
// A 'KernelStack' instead takes 'pages + 1' contiguous frames from the PMM
// and unmaps the lowest one from the HHDM. Overflow now faults on the guard
// page, at the instruction that did it, with the address in CR2

use crate::arch::x86_64::grub;
use crate::mm::{pmm, vmm};

const PAGE_SIZE: usize = 4096;

pub struct KernelStack {
    /// Physical base of the whole allocation - guard frame first
    base_phys: u64,
    /// Total frames held, guard included
    frames: usize,
    /// HHDM address of the first usable stack byte (guard page skipped)
    stack_virt: u64,
    stack_bytes: usize,
    guarded: bool,
}

impl KernelStack {
    /// Allocate a zeroed stack of at least 'size' bytes, fenced below by an
    /// unmapped guard page. Returns None when physical memory is exhausted
    pub fn new(size: usize) -> Option<Self> {
        let stack_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        let frames = stack_pages + 1; // +1 guard
        let base_phys = pmm::alloc_frames(frames)?;

        let guard_phys = base_phys;
        let stack_phys = base_phys + PAGE_SIZE as u64;
        let stack_virt = grub::phys_to_virt(stack_phys);
        let stack_bytes = stack_pages * PAGE_SIZE;

        unsafe {
            core::ptr::write_bytes(stack_virt as *mut u8, 0, stack_bytes);
        }

        // Losing the guard is not fatal - the stack is still perfectly
        // usable, we just fall back to the old silent-corruption behaviour -
        // so a failure here degrades instead of refusing to spawn
        let guarded = vmm::hhdm_unmap_guard_frame(guard_phys);
        if !guarded {
            crate::serial_println!(
                "[kstack] warn: could not unmap guard frame {:#x}; stack is unguarded",
                guard_phys
            );
        }

        Some(Self { base_phys, frames, stack_virt, stack_bytes, guarded })
    }

    #[inline]
    pub fn as_ptr(&self) -> *const u8 {
        self.stack_virt as *const u8
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.stack_bytes
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.stack_bytes == 0
    }

    /// Lowest address a healthy stack may reach. One page below this is the
    /// guard; the page-fault handler uses it to name an overflow instead of
    /// reporting an anonymous bad access
    #[inline]
    pub fn guard_virt(&self) -> u64 {
        grub::phys_to_virt(self.base_phys)
    }
}

impl Drop for KernelStack {
    fn drop(&mut self) {
        // Put the HHDM mapping back before the frame returns to the pool,
        // otherwise the next owner of that frame faults the first time it is
        // touched through the direct map
        if self.guarded {
            vmm::hhdm_remap_guard_frame(self.base_phys);
        }
        pmm::free_frames(self.base_phys, self.frames);
    }
}

unsafe impl Send for KernelStack {}
unsafe impl Sync for KernelStack {}

/// If 'addr' lands on the guard page of a live kernel stack, name its owner.
/// Only called from the page-fault handler, so walking the pid table is fine
pub fn guard_fault_owner(addr: u64) -> Option<(u64, &'static str)> {
    let page = addr & !(PAGE_SIZE as u64 - 1);
    for pid in 0..crate::sched::MAX_PROCS as u64 {
        let ptr = unsafe { crate::sched::proc_index_raw(pid) };
        if ptr.is_null() { continue; }
        let p = unsafe { &*ptr };
        if p.stack.guard_virt() == page {
            return Some((p.pid, p.name));
        }
    }
    None
}
