// The kernel heap's layers that need no kernel, on the build host
//
// The files are compiled as they are, by `#[path]`. Under them, pages come
// from the host's allocator, aligned as `Pages::grab` promises, and every
// page handed out is accounted for: a test ends with nothing outstanding, or
// with exactly what the cache is allowed to keep.

#[path = "../../class.rs"]
pub mod class;
#[path = "../../slab.rs"]
pub mod slab;
#[path = "../../magazine.rs"]
pub mod magazine;

use std::collections::{BTreeMap, HashMap};

/// Pages from the host, each run remembered until it comes back
#[derive(Default)]
pub struct HostPages {
    out: HashMap<usize, usize>,
    pub grabbed: usize,
    pub given: usize,
}

impl HostPages {
    /// Pages handed out and not back
    pub fn outstanding(&self) -> usize {
        self.out.values().sum()
    }
}

impl slab::Pages for HostPages {
    unsafe fn grab(&mut self, count: usize) -> *mut u8 {
        let bytes = count * slab::PAGE;
        let layout = std::alloc::Layout::from_size_align(bytes, bytes).unwrap();
        let p = unsafe { std::alloc::alloc(layout) };
        assert!(!p.is_null());
        self.out.insert(p as usize, count);
        self.grabbed += count;
        p
    }

    unsafe fn give(&mut self, ptr: *mut u8, count: usize) {
        let had = self.out.remove(&(ptr as usize));
        assert_eq!(had, Some(count), "pages at {ptr:p} given back that were not handed out as {count}");
        let bytes = count * slab::PAGE;
        unsafe { std::alloc::dealloc(ptr, std::alloc::Layout::from_size_align(bytes, bytes).unwrap()) };
        self.given += count;
    }
}

/// A cache for each class, as the kernel has them
pub fn caches() -> Vec<slab::Cache> {
    class::CLASSES.iter().map(|&(size, pages)| slab::Cache::new("test", size, pages)).collect()
}

/// xorshift: the same sequence on every run
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// Live objects by address, for overlap checks, and in a list for picking
/// one at random in constant time
#[derive(Default)]
pub struct Live {
    by_addr: BTreeMap<usize, (usize, u8, usize)>,
    list: Vec<usize>,
}

impl Live {
    /// Takes `p` for `len` bytes, filled with `fill`; panics if it overlaps
    /// anything live
    pub fn add(&mut self, p: *mut u8, len: usize, fill: u8) {
        let at = p as usize;
        if let Some((&a, &(l, _, _))) = self.by_addr.range(..=at).next_back() {
            assert!(a + l <= at, "{at:#x} handed out inside the live object at {a:#x}");
        }
        if let Some((&a, _)) = self.by_addr.range(at..).next() {
            assert!(at + len <= a, "{at:#x}+{len} runs into the live object at {a:#x}");
        }
        unsafe { std::ptr::write_bytes(p, fill, len) };
        self.by_addr.insert(at, (len, fill, self.list.len()));
        self.list.push(at);
    }

    /// Gives `p` up, after checking nothing wrote over it
    pub fn take(&mut self, p: *mut u8) {
        let (len, fill, k) = self.by_addr.remove(&(p as usize)).expect("not live");
        self.list.swap_remove(k);
        if let Some(&moved) = self.list.get(k) {
            self.by_addr.get_mut(&moved).unwrap().2 = k;
        }
        let bytes = unsafe { std::slice::from_raw_parts(p, len) };
        assert!(bytes.iter().all(|&b| b == fill), "the object at {p:p} was written over while live");
    }

    pub fn len(&self) -> usize {
        self.by_addr.len()
    }

    pub fn any(&self, k: usize) -> Option<*mut u8> {
        if self.list.is_empty() {
            return None;
        }
        Some(self.list[k % self.list.len()] as *mut u8)
    }
}

#[cfg(test)]
mod tests;
