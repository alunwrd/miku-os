// Global allocator over `miku_malloc` in core_miku.so

use core::alloc::{GlobalAlloc, Layout};

/// `miku_malloc`'s alignment. Larger requests go through `miku_memalign`, which
/// keeps the real block's address in a header before the returned pointer
const MALLOC_ALIGN: usize = 16;

pub struct MikuAlloc;

unsafe impl GlobalAlloc for MikuAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = if layout.align() <= MALLOC_ALIGN {
            crate::miku_malloc(layout.size())
        } else {
            crate::miku_memalign(layout.align(), layout.size())
        };
        if p.is_null() { oom(layout.size()); }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        crate::miku_free(ptr);
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        if layout.align() <= MALLOC_ALIGN {
            let p = crate::miku_calloc(1, layout.size());
            if p.is_null() { oom(layout.size()); }
            return p;
        }
        let p = self.alloc(layout);
        if !p.is_null() {
            crate::miku_bzero(p, layout.size());
        }
        p
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // `miku_realloc` handles only plain blocks, so an over-aligned one
        // moves by hand
        if layout.align() <= MALLOC_ALIGN {
            let p = crate::miku_realloc(ptr, new_size);
            if p.is_null() && new_size != 0 { oom(new_size); }
            return p;
        }
        let new_layout = match Layout::from_size_align(new_size, layout.align()) {
            Ok(l) => l,
            Err(_) => return core::ptr::null_mut(),
        };
        let p = self.alloc(new_layout);
        if !p.is_null() {
            let keep = if layout.size() < new_size { layout.size() } else { new_size };
            crate::miku_memcpy(p, ptr, keep);
            crate::miku_free(ptr);
        }
        p
    }
}

/// Reports exhaustion first, since Rust's panic message would only name the
/// program
unsafe fn oom(size: usize) {
    let mut buf = [0u8; 48];
    let msg = b"miku: out of memory, wanted ";
    buf[..msg.len()].copy_from_slice(msg);
    let mut n = msg.len();
    n += write_usize(&mut buf[n..], size);
    if n < buf.len() { buf[n] = b'\n'; n += 1; }
    crate::miku_write(2, buf.as_ptr(), n);
}

fn write_usize(out: &mut [u8], mut v: usize) -> usize {
    let mut digits = [0u8; 20];
    let mut d = 0;
    loop {
        digits[d] = b'0' + (v % 10) as u8;
        d += 1;
        v /= 10;
        if v == 0 { break; }
    }
    let n = if d < out.len() { d } else { out.len() };
    for i in 0..n {
        out[i] = digits[d - 1 - i];
    }
    n
}
