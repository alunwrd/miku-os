// A cache of one type, by name: Linux's `kmem_cache_create`
// (`dev/kernel-core.md`, "Small objects: a cache for each size")
//
// The same slabs as the caches by size (`slab::Cache`), sized for `T`, with a
// lock of its own and no magazines: vnodes are made and freed at the rate of
// lookups. The name shows in `/proc/slabinfo`, and the heap's shrinker gives
// its empty slabs back with the rest.
//
// It hands out pointers. Whoever holds them owns the object (the VFS's table,
// `fs::vfs::table`) and gives it back with `free`.

use core::marker::PhantomData;
use core::ptr::NonNull;
use core::sync::atomic::{AtomicBool, Ordering};

use super::pages::Frames;
use super::slab::{Cache, HDR, PAGE};
use crate::kcore::irq_lock::IrqMutex;

/// The untyped half, which the heap lists and shrinks
pub struct Typed {
    pub(super) cache: IrqMutex<Cache>,
    listed: AtomicBool,
}

/// Typed caches made so far, for `/proc/slabinfo` and the shrinker
const MAX_TYPED: usize = 16;
static TYPED: IrqMutex<[Option<&'static Typed>; MAX_TYPED]> = IrqMutex::new([None; MAX_TYPED]);

/// Every typed cache that has handed out an object
pub(super) fn each(mut f: impl FnMut(&'static Typed)) {
    let list = *TYPED.lock();
    for t in list.iter().flatten() {
        f(t);
    }
}

pub struct KCache<T> {
    typed: Typed,
    _t: PhantomData<T>,
}

// SAFETY: the cache is behind its lock; the objects go to whoever allocates
unsafe impl<T: Send> Sync for KCache<T> {}

/// What one `T` takes in a slab: its size, rounded so that every object in a
/// slab aligned to its own size is aligned for `T`
const fn object_size<T>() -> usize {
    let size = core::mem::size_of::<T>();
    let align = core::mem::align_of::<T>();
    let at_least = if size < 16 { 16 } else { size };
    (at_least + align - 1) / align * align
}

/// The smallest slab, of 1 to 8 pages, that leaves the header room and
/// wastes under an eighth
const fn slab_pages(size: usize) -> usize {
    let mut pages = 1;
    while pages < 8 {
        let bytes = pages * PAGE;
        let n = (bytes - HDR) / size;
        if n > 0 && (bytes - n * size) * 8 < bytes {
            break;
        }
        pages *= 2;
    }
    pages
}

impl<T> KCache<T> {
    pub const fn new(name: &'static str) -> Self {
        let size = object_size::<T>();
        assert!(size <= 4 * PAGE, "a typed cache is for small objects");
        Self {
            typed: Typed { cache: IrqMutex::new(Cache::new(name, size, slab_pages(size))), listed: AtomicBool::new(false) },
            _t: PhantomData,
        }
    }

    /// `value` in an object of its own, or `None` when there is no memory
    /// even after what can be given back without waiting
    pub fn alloc(&'static self, value: T) -> Option<NonNull<T>> {
        if !self.typed.listed.swap(true, Ordering::AcqRel) {
            let mut list = TYPED.lock();
            if let Some(slot) = list.iter_mut().find(|s| s.is_none()) {
                *slot = Some(&self.typed);
            }
        }
        let mut p = unsafe { self.typed.cache.lock().alloc(&mut Frames) };
        if p.is_null() {
            crate::mm::shrink::direct();
            p = unsafe { self.typed.cache.lock().alloc(&mut Frames) };
        }
        let p = NonNull::new(p as *mut T)?;
        // SAFETY: a fresh object of `T`'s size, aligned for it (`object_size`)
        unsafe { p.as_ptr().write(value) };
        Some(p)
    }

    /// An object back, dropped first
    ///
    /// # Safety
    /// `p` came from this cache's `alloc` and nothing uses it after
    pub unsafe fn free(&'static self, p: NonNull<T>) {
        unsafe {
            core::ptr::drop_in_place(p.as_ptr());
            self.typed.cache.lock().free(p.as_ptr() as *mut u8, &mut Frames);
        }
    }
}
