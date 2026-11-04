// Where the heap's memory comes from (`dev/kernel-core.md`, "Small objects: a
// cache for each size")
//
// Frames from anywhere in memory, reached through the direct map. Before the
// frame allocator is laid out (`pmm::layout`), for the console's grid and
// history, the display probe and the memory map, from an 8 MiB arena in `.bss`,
// and what is carved there stays.

use core::sync::atomic::{AtomicUsize, Ordering};

use super::slab::{Pages, PAGE};
use crate::mm::pmm;

/// Where slabs come from before the frame allocator. A megabyte or two of it
/// is used
const ARENA: usize = 8 << 20;

#[repr(C, align(4096))]
struct Arena([u8; ARENA]);

static mut ARENA_MEM: Arena = Arena([0; ARENA]);
/// How much of the arena has been handed out
static ARENA_USED: AtomicUsize = AtomicUsize::new(0);

fn arena_base() -> usize {
    core::ptr::addr_of!(ARENA_MEM) as usize
}

/// `count` pages in one run, starting on a multiple of `align` pages, or
/// null
pub fn run(count: usize, align: usize) -> *mut u8 {
    if pmm::ready() {
        let phys = if count == 1 && align == 1 {
            pmm::alloc_frame()
        } else {
            pmm::alloc_frames_aligned(count, align, u64::MAX)
        };
        return match phys {
            Some(p) => crate::arch::x86_64::grub::phys_to_virt(p) as *mut u8,
            None => core::ptr::null_mut(),
        };
    }
    // One CPU this early, but a bump that cannot hand one byte out twice
    let base = arena_base();
    let mut at = 0;
    let got = ARENA_USED.fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
        let align_bytes = align * PAGE;
        at = ((base + used + align_bytes - 1) & !(align_bytes - 1)) - base;
        (at + count * PAGE <= ARENA).then_some(at + count * PAGE)
    });
    match got {
        Ok(_) => (base + at) as *mut u8,
        Err(_) => core::ptr::null_mut(),
    }
}

/// Pages `run` handed out, back. A run cut from the arena stays where it
/// is: the arena is part of the kernel image
pub fn give_run(ptr: *mut u8, count: usize) {
    let at = ptr as usize;
    if (arena_base()..arena_base() + ARENA).contains(&at) {
        return;
    }
    let phys = crate::arch::x86_64::grub::any_virt_to_phys(at as u64);
    pmm::free_frames(phys, count);
}

/// Bytes of the arena handed out
pub fn arena_used() -> usize {
    ARENA_USED.load(Ordering::Relaxed)
}

/// Slabs, aligned to their own size
pub struct Frames;

impl Pages for Frames {
    unsafe fn grab(&mut self, count: usize) -> *mut u8 {
        run(count, count)
    }

    unsafe fn give(&mut self, ptr: *mut u8, count: usize) {
        give_run(ptr, count)
    }
}
