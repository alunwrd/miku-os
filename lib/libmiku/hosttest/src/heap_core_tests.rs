// `heap_core` over a source of runs of pages, with slabs of 2 MiB when a run is
// long enough and large blocks from 256 KiB returned as their own runs. Each
// test builds its own heap, separate from the process heap of `heap_tests`

use crate::heap_core::{Heap, Large, Source};

const SLAB: usize = 2 << 20;
const LARGE: usize = 256 << 10;
const PAGE: usize = 4096;

/// Memory in runs no longer than `longest`, as frames get after uptime; what it
/// gave, `(start, length)`, by start
struct Runs {
    base: usize,
    size: usize,
    next: usize,
    longest: usize,
    out: Vec<(usize, usize)>,
}

impl Runs {
    fn new(size: usize, longest: usize) -> Runs {
        let layout = std::alloc::Layout::from_size_align(size, PAGE).unwrap();
        // SAFETY: a non-zero size; leaked with the test
        let base = unsafe { std::alloc::alloc_zeroed(layout) } as usize;
        assert!(base != 0);
        Runs { base, size, next: 0, longest, out: Vec::new() }
    }

    fn give(&mut self, l: Large) {
        let i = self.out.iter().position(|&(a, n)| a == l.ptr as usize && n == l.bytes)
            .unwrap_or_else(|| panic!("{:#x}+{} was not handed out", l.ptr as usize, l.bytes));
        self.out.remove(i);
    }
}

impl Source for Runs {
    unsafe fn grab(&mut self, bytes: usize) -> *mut u8 {
        assert!(bytes % PAGE == 0, "asked for {bytes}, not a whole number of pages");
        if bytes > self.longest || self.next + bytes > self.size {
            return std::ptr::null_mut();
        }
        let at = self.base + self.next;
        self.next += bytes;
        self.out.push((at, bytes));
        at as *mut u8
    }
}

fn whole(h: &mut Heap<Runs>) -> (usize, usize) {
    let out = h.source().out.clone();
    h.check(&out).unwrap_or_else(|e| panic!("the heap is broken: {e}"))
}

#[test]
fn a_slab_is_smaller_when_no_run_is_as_long() {
    // No 2 MiB run left: each request still gets memory
    let mut h = Heap::new(Runs::new(64 << 20, 64 << 10), SLAB, LARGE);
    let ptrs: Vec<*mut u8> = (0..2000).map(|i| unsafe { h.alloc(16 + i % 3000) }).collect();
    assert!(ptrs.iter().all(|p| !p.is_null()), "an allocation was refused");
    assert!(h.source().out.iter().all(|&(_, n)| n <= 64 << 10), "a slab longer than any run");
    whole(&mut h);
    for p in ptrs {
        assert!(unsafe { h.free(p) }.is_none());
    }
    let (free, slabs) = whole(&mut h);
    assert_eq!(free, slabs, "{free} free blocks in {slabs} slabs");
    assert_eq!(h.used(), 0);
}

#[test]
fn a_firmware_image_is_its_own_block_and_leaves_no_hole() {
    // A 28 MiB `Vec` with small allocations before and after, then dropped
    let mut h = Heap::new(Runs::new(128 << 20, usize::MAX), SLAB, LARGE);
    let before: Vec<*mut u8> = (0..500).map(|_| unsafe { h.alloc(200) }).collect();
    let held = h.held();
    let image = unsafe { h.alloc(28 << 20) };
    assert!(!image.is_null());
    unsafe { std::ptr::write_bytes(image, 0x5a, 28 << 20) };
    let after: Vec<*mut u8> = (0..500).map(|_| unsafe { h.alloc(200) }).collect();
    whole(&mut h);

    let large = unsafe { h.free(image) }.expect("the image came from a slab");
    assert!(large.bytes >= 28 << 20 && large.bytes % PAGE == 0, "{} bytes back", large.bytes);
    h.source().give(large);
    // Everything else the heap holds is the slab the small ones are in
    assert_eq!(h.held(), held, "the image left {} bytes behind", h.held() - held);
    whole(&mut h);
    for p in before.into_iter().chain(after) {
        assert!(unsafe { h.free(p) }.is_none());
    }
    assert_eq!(h.used(), 0);
    let (free, slabs) = whole(&mut h);
    assert_eq!(free, slabs);
}

#[test]
fn page_aligned_blocks_for_devices() {
    // A driver's `#[repr(align(4096))]` rings and bounce buffers
    let mut h = Heap::new(Runs::new(64 << 20, usize::MAX), SLAB, LARGE);
    let mut ptrs = Vec::new();
    for n in [64, 4096, 8192, 65536, 300 << 10] {
        let p = unsafe { h.memalign(PAGE, n) };
        assert!(!p.is_null() && p as usize % PAGE == 0, "memalign(4096, {n}) gave {p:?}");
        assert!(unsafe { h.usable(p) } >= n);
        unsafe { std::ptr::write_bytes(p, 0xc3, n) };
        ptrs.push(p);
    }
    whole(&mut h);
    for p in ptrs {
        if let Some(l) = unsafe { h.free(p) } {
            h.source().give(l);
        }
    }
    assert_eq!(h.used(), 0);
    whole(&mut h);
}
