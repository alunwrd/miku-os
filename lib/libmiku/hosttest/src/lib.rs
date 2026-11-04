// libmiku's files on the build host
//
// Compiled as they are by `#[path]`, as `lib/mikulibs` compiles them.
// MikuOS-only parts stand in here:
//
//   proc     `miku_mmap` / `miku_munmap` over one big reservation, lowest free range first,
//            since MikuOS places mappings bottom up, so the memory after a slab is often another mapping
//   thread   the futex calls `sync`'s mutex makes, as a yield
//   mem      `miku_memcpy` / `miku_memset`, libmiku's own, by `#[path]`
//
// `proc::mapped` gives a test the mapped ranges.

#![allow(dead_code)]

#[path = "../../sync.rs"]
pub mod sync;
#[path = "../../mem.rs"]
pub mod mem;
#[path = "../../ctype.rs"]
pub mod ctype;
#[path = "../../string.rs"]
pub mod string;
#[path = "../../heap_core.rs"]
pub mod heap_core;
#[path = "../../heap.rs"]
pub mod heap;

pub mod thread {
    pub extern "C" fn miku_futex_wait(_addr: *const u32, _expected: u32, _timeout_ns: u64) -> i64 {
        std::thread::yield_now();
        0
    }

    pub extern "C" fn miku_futex_wake(_addr: *const u32, _count: u32) -> i64 {
        0
    }
}

pub mod proc {
    use std::sync::Mutex;

    const PAGE: usize = 4096;
    /// The reservation: address space, committed as touched
    const SPACE: usize = 1 << 30;

    struct Space {
        base: usize,
        /// `(start, length)` of what is mapped, by start
        maps: Vec<(usize, usize)>,
    }

    static SPACE_MAP: Mutex<Option<Space>> = Mutex::new(None);

    fn with<R>(f: impl FnOnce(&mut Space) -> R) -> R {
        let mut g = SPACE_MAP.lock().unwrap_or_else(|e| e.into_inner());
        let s = g.get_or_insert_with(|| {
            let layout = std::alloc::Layout::from_size_align(SPACE, PAGE).unwrap();
            // SAFETY: a non-zero size; kept for the life of the process
            let base = unsafe { std::alloc::alloc(layout) } as usize;
            assert!(base != 0, "no address space for the stand-in mmap");
            Space { base, maps: Vec::new() }
        });
        f(s)
    }

    /// The lowest free range that fits, zeroed, as MikuOS's `mmap` gives it
    pub extern "C" fn miku_mmap(_addr: u64, len: usize, _prot: u64) -> *mut u8 {
        let len = len.div_ceil(PAGE) * PAGE;
        with(|s| {
            let mut at = s.base;
            let mut i = 0;
            while i < s.maps.len() && s.maps[i].0 < at + len {
                at = at.max(s.maps[i].0 + s.maps[i].1);
                i += 1;
            }
            if at + len > s.base + SPACE {
                return std::ptr::null_mut();
            }
            s.maps.insert(i, (at, len));
            // SAFETY: inside the reservation and mapped to nobody else
            unsafe { std::ptr::write_bytes(at as *mut u8, 0, len) };
            at as *mut u8
        })
    }

    pub extern "C" fn miku_munmap(addr: *mut u8, len: usize) -> i64 {
        let len = len.div_ceil(PAGE) * PAGE;
        with(|s| match s.maps.iter().position(|&(a, l)| a == addr as usize && l == len) {
            Some(i) => {
                // Unmapped memory is scribbled, so a stale pointer reads
                // nonsense.
                // SAFETY: the range was mapped, and is nobody's now
                unsafe { std::ptr::write_bytes(addr, 0xA5, len) };
                s.maps.remove(i);
                0
            }
            None => -22,
        })
    }

    /// What is mapped now, `(start, length)` by start
    pub fn mapped() -> Vec<(usize, usize)> {
        with(|s| s.maps.clone())
    }
}

#[cfg(test)]
mod heap_tests;
#[cfg(test)]
mod heap_core_tests;
#[cfg(test)]
mod mem_tests;
