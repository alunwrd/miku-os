//! The global allocator: `libmiku`'s heap.
//!
//! `core_miku.so` owns the heap every MikuOS program uses. With a second one
//! inside `std`, one would free the other's pointers once a `Vec` crossed the
//! boundary.

use crate::alloc::{GlobalAlloc, Layout, System};
use crate::sys::pal::abi;

/// What `miku_malloc` guarantees. Above it the request goes through
/// `miku_memalign`, which records the way back to the real block in a header in
/// front of the pointer it returns
const MALLOC_ALIGN: usize = 16;

#[stable(feature = "alloc_system_type", since = "1.28.0")]
unsafe impl GlobalAlloc for System {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            abi::miku_malloc(layout.size())
        } else {
            abi::miku_memalign(layout.align(), layout.size())
        }
    }

    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            return abi::miku_calloc(1, layout.size());
        }
        // `memalign` does not zero and there is no `calloc` that aligns, so the
        // zeroing is done here
        let p = abi::miku_memalign(layout.align(), layout.size());
        if !p.is_null() {
            unsafe { crate::ptr::write_bytes(p, 0, layout.size()) };
        }
        p
    }

    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        abi::miku_free(ptr)
    }

    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            return abi::miku_realloc(ptr, new_size);
        }
        // `miku_realloc` on an over-aligned block returns an ordinarily aligned
        // one, as C's `realloc` over `aligned_alloc` does, so for an
        // over-aligned layout the move is done here
        let fresh = abi::miku_memalign(layout.align(), new_size);
        if fresh.is_null() {
            return fresh;
        }
        let copy = if layout.size() < new_size { layout.size() } else { new_size };
        unsafe {
            crate::ptr::copy_nonoverlapping(ptr, fresh, copy);
            abi::miku_free(ptr);
        }
        fresh
    }
}
