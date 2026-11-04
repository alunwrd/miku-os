// Magazines: each CPU's own stock of free objects in front of a cache
// (`dev/kernel-core.md`, "Small objects: a cache for each size")
//
// Bonwick and Adams' (2001) shape, as illumos' kmem and FreeBSD's UMA have
// it. A CPU holds two magazines per cache, `loaded` and `previous`:
// - an allocation pops from `loaded`, a free pushes onto it;
// - when `loaded` runs out or fills up, the two swap;
// - only when both are out, or both full, does the CPU go to the cache's
//   depot of full magazines, under the cache's lock.
// So the shared lock is taken once a magazine's worth of objects, not once
// an object.
//
// A magazine is a plain array the CPU owns, copied whole into and out of the
// depot. Nothing here is allocated, so the heap never allocates to free.
//
// Like `slab.rs`, this file knows no lock and no kernel: the caller holds
// the CPU's lock for `CpuMags` and the cache's for the depot.

use super::slab::{Cache, Pages};

/// Objects a magazine holds
pub const MAG: usize = 32;
/// Full magazines a cache's depot holds; past that a magazine's objects go
/// back to the slabs
pub const DEPOT: usize = 16;

#[derive(Clone, Copy)]
pub struct Mag {
    n: usize,
    objs: [*mut u8; MAG],
}

impl Mag {
    pub const EMPTY: Self = Self { n: 0, objs: [core::ptr::null_mut(); MAG] };

    pub fn len(&self) -> usize {
        self.n
    }

    /// Every object back to the slabs
    ///
    /// # Safety
    /// The caller holds the cache's lock; the objects are `cache`'s
    unsafe fn empty_into(&mut self, cache: &mut Cache, src: &mut impl Pages) {
        for &p in &self.objs[..self.n] {
            unsafe { cache.free(p, src) };
        }
        self.n = 0;
    }
}

/// One CPU's two magazines for one cache
pub struct CpuMags {
    loaded: Mag,
    previous: Mag,
}

// SAFETY: the pointers are free objects, owned by whoever holds this
unsafe impl Send for CpuMags {}

impl CpuMags {
    pub const fn new() -> Self {
        Self { loaded: Mag::EMPTY, previous: Mag::EMPTY }
    }

    /// Objects held
    pub fn held(&self) -> usize {
        self.loaded.n + self.previous.n
    }

    /// A free object, or `None` when both magazines are out
    #[inline]
    pub fn pop(&mut self) -> Option<*mut u8> {
        if self.loaded.n == 0 {
            if self.previous.n == 0 {
                return None;
            }
            core::mem::swap(&mut self.loaded, &mut self.previous);
        }
        self.loaded.n -= 1;
        Some(self.loaded.objs[self.loaded.n])
    }

    /// Whether `p` went in; not when both magazines are full
    #[inline]
    pub fn push(&mut self, p: *mut u8) -> bool {
        if self.loaded.n == MAG {
            if self.previous.n == MAG {
                return false;
            }
            core::mem::swap(&mut self.loaded, &mut self.previous);
        }
        self.loaded.objs[self.loaded.n] = p;
        self.loaded.n += 1;
        true
    }

    /// Both magazines out: a full one from the depot, or one filled from the
    /// slabs. False when there is no memory for even one object
    ///
    /// # Safety
    /// The caller holds the cache's lock
    pub unsafe fn refill(&mut self, depot: &mut Depot, cache: &mut Cache, src: &mut impl Pages) -> bool {
        if let Some(m) = depot.take() {
            self.loaded = m;
            return true;
        }
        while self.loaded.n < MAG {
            let p = unsafe { cache.alloc(src) };
            if p.is_null() {
                break;
            }
            self.loaded.objs[self.loaded.n] = p;
            self.loaded.n += 1;
        }
        self.loaded.n > 0
    }

    /// Both magazines full: `previous` to the depot, or its objects to the
    /// slabs when the depot is full. There is room for a push after
    ///
    /// # Safety
    /// The caller holds the cache's lock
    pub unsafe fn spill(&mut self, depot: &mut Depot, cache: &mut Cache, src: &mut impl Pages) {
        if !depot.put(self.previous) {
            unsafe { self.previous.empty_into(cache, src) };
        }
        self.previous.n = 0;
    }

    /// Every object back to the slabs
    ///
    /// # Safety
    /// The caller holds the cache's lock
    pub unsafe fn drain(&mut self, cache: &mut Cache, src: &mut impl Pages) {
        unsafe {
            self.loaded.empty_into(cache, src);
            self.previous.empty_into(cache, src);
        }
    }
}

/// A cache's full magazines, shared by its CPUs
pub struct Depot {
    mags: [Mag; DEPOT],
    n: usize,
    /// The fewest full magazines held since the last `reap`: that many sat
    /// unused for the whole interval
    low: usize,
}

// SAFETY: as `CpuMags`
unsafe impl Send for Depot {}

impl Depot {
    pub const fn new() -> Self {
        Self { mags: [Mag::EMPTY; DEPOT], n: 0, low: 0 }
    }

    /// Objects held
    pub fn held(&self) -> usize {
        self.mags[..self.n].iter().map(|m| m.n).sum()
    }

    fn take(&mut self) -> Option<Mag> {
        if self.n == 0 {
            return None;
        }
        self.n -= 1;
        self.low = self.low.min(self.n);
        Some(self.mags[self.n])
    }

    fn put(&mut self, m: Mag) -> bool {
        if self.n == DEPOT {
            return false;
        }
        self.mags[self.n] = m;
        self.n += 1;
        true
    }

    /// Bonwick's working set: the magazines that were never needed since the
    /// last reap go back to the slabs. Objects returned
    ///
    /// # Safety
    /// The caller holds the cache's lock
    pub unsafe fn reap(&mut self, cache: &mut Cache, src: &mut impl Pages) -> usize {
        let mut objects = 0;
        for _ in 0..self.low.min(self.n) {
            self.n -= 1;
            objects += self.mags[self.n].n;
            unsafe { self.mags[self.n].empty_into(cache, src) };
        }
        self.low = self.n;
        objects
    }

    /// Every magazine back to the slabs. Objects returned
    ///
    /// # Safety
    /// The caller holds the cache's lock
    pub unsafe fn drain(&mut self, cache: &mut Cache, src: &mut impl Pages) -> usize {
        let objects = self.held();
        while self.n > 0 {
            self.n -= 1;
            unsafe { self.mags[self.n].empty_into(cache, src) };
        }
        self.low = 0;
        objects
    }
}
