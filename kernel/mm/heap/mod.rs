// The kernel's heap (`dev/kernel-core.md`, "The kernel heap")
//
// Linux's shape: a cache for each size of small object (`class`, `slab`), each
// CPU's magazines in front of them (`magazine`), and pages for anything bigger
// (`pages`). The layout an object was allocated with picks its cache again when
// it is freed, so a free never looks anything up.
//
// This file is the kernel's part: the `GlobalAlloc`, the locks, the per-CPU
// array, what memory pressure asks of the heap, and `slabdebug`. `class.rs`,
// `slab.rs` and `magazine.rs` know no lock and no kernel, and
// `kernel/mm/heap/hosttest` checks them on the build host.
//
// Drivers have device memory of their own (`mm::dma`), so an object above 32
// KiB is single frames in the mapped area (`mm::vmap`).

pub mod class;
pub mod kcache;
pub mod magazine;
mod pages;
pub mod slab;

use core::alloc::{GlobalAlloc, Layout};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::arch::x86_64::percpu::{self, MAX_CPUS};
use crate::kcore::irq_lock::IrqMutex;
use class::{class_of, CLASSES, NCLASSES};
use magazine::{CpuMags, Depot};
use pages::Frames;
use slab::{Cache, PAGE};

/// A class's cache and its depot of full magazines, under one lock
struct Shared {
    cache: Cache,
    depot: Depot,
}

const NAMES: [&str; NCLASSES] = [
    "kmalloc-8", "kmalloc-16", "kmalloc-32", "kmalloc-48", "kmalloc-64", "kmalloc-96",
    "kmalloc-128", "kmalloc-192", "kmalloc-256", "kmalloc-384", "kmalloc-512", "kmalloc-768",
    "kmalloc-1k", "kmalloc-1.5k", "kmalloc-2k", "kmalloc-3k", "kmalloc-6k",
];

const fn shared_all() -> [IrqMutex<Shared>; NCLASSES] {
    let mut out: [MaybeUninit<IrqMutex<Shared>>; NCLASSES] = [const { MaybeUninit::uninit() }; NCLASSES];
    let mut i = 0;
    while i < NCLASSES {
        out[i] = MaybeUninit::new(IrqMutex::new(Shared {
            cache: Cache::new(NAMES[i], CLASSES[i].0, CLASSES[i].1),
            depot: Depot::new(),
        }));
        i += 1;
    }
    // SAFETY: every element was written above
    unsafe { core::mem::transmute(out) }
}

static SHARED: [IrqMutex<Shared>; NCLASSES] = shared_all();

/// Each CPU's magazines, one pair per class. Lock order: a CPU's magazines,
/// then a class's `Shared`, then frames
static CPUS: [IrqMutex<[CpuMags; NCLASSES]>; MAX_CPUS] =
    [const { IrqMutex::new([const { CpuMags::new() }; NCLASSES]) }; MAX_CPUS];

/// Whether allocations go through the magazines: from `init`, once every
/// CPU that runs has its per-CPU base (`gdt::init` sets the boot CPU's
/// before the heap's step), and not under `slabdebug`. Before that, straight
/// to the caches
static MAGAZINES: AtomicBool = AtomicBool::new(false);

/// Bytes handed out by the page path in one run
static PAGE_BYTES: AtomicUsize = AtomicUsize::new(0);

/// Past this an object is single frames in the mapped area. Order 3,
/// Linux's `PAGE_ALLOC_COSTLY_ORDER`: a longer run is what fragmentation
/// takes first
const RUN_MAX: usize = 32 << 10;

/// What fills the bytes between the size asked for and the class's size,
/// under `slabdebug`: a write past an object's end lands there
const REDZONE: u8 = 0xbb;

fn pages_for(size: usize) -> usize {
    size.div_ceil(PAGE).max(1)
}

/// What `slabdebug` found, said once the lock is down: a report allocates
enum Found {
    Nothing,
    WrittenAfterFree(&'static str, usize),
    PastTheEnd(&'static str, usize, usize),
    FreedTwice(&'static str, usize),
}

/// What `slabdebug` has reported since boot
static REPORTS: AtomicUsize = AtomicUsize::new(0);

/// What `slabdebug` has reported since boot, for its selftest
pub fn reports() -> usize {
    REPORTS.load(Ordering::Relaxed)
}

/// Whether `slabdebug` is on
pub fn debugging() -> bool {
    !MAGAZINES.load(Ordering::Relaxed) && SHARED[0].lock().cache.poison
}

fn report(f: Found) {
    if !matches!(f, Found::Nothing) {
        REPORTS.fetch_add(1, Ordering::Relaxed);
    }
    match f {
        Found::Nothing => {}
        Found::WrittenAfterFree(name, at) =>
            crate::kerror!("[heap] {}: the object at {:#x} was written after it was freed", name, at),
        Found::PastTheEnd(name, at, size) =>
            crate::kerror!("[heap] {}: the object at {:#x} was written past its {} bytes", name, at, size),
        Found::FreedTwice(name, at) =>
            crate::kerror!("[heap] {}: the object at {:#x} was freed twice; the second free is ignored", name, at),
    }
}

fn small_alloc_once(c: usize, size: usize) -> *mut u8 {
    if MAGAZINES.load(Ordering::Relaxed) {
        let mut mine = CPUS[percpu::current_index()].lock();
        let mags = &mut mine[c];
        if let Some(p) = mags.pop() {
            return p;
        }
        let mut sh = SHARED[c].lock();
        let Shared { cache, depot } = &mut *sh;
        if unsafe { mags.refill(depot, cache, &mut Frames) } {
            return mags.pop().unwrap_or(core::ptr::null_mut());
        }
        return core::ptr::null_mut();
    }
    let mut sh = SHARED[c].lock();
    let p = unsafe { sh.cache.alloc(&mut Frames) };
    if !sh.cache.poison {
        return p;
    }
    // `slabdebug`: what was found poisoned wrongly, and a red zone after the
    // bytes asked for
    let breach = core::mem::take(&mut sh.cache.breach);
    let name = sh.cache.name;
    if !p.is_null() {
        unsafe { core::ptr::write_bytes(p.add(size), REDZONE, sh.cache.size - size) };
    }
    drop(sh);
    if breach != 0 {
        report(Found::WrittenAfterFree(name, breach));
    }
    p
}

fn small_alloc(c: usize, size: usize) -> *mut u8 {
    let p = small_alloc_once(c, size);
    if !p.is_null() {
        return p;
    }
    // No frames: what can be had without waiting, and once more
    crate::mm::shrink::direct();
    small_alloc_once(c, size)
}

fn small_free(c: usize, p: *mut u8, size: usize) {
    if MAGAZINES.load(Ordering::Relaxed) {
        let mut mine = CPUS[percpu::current_index()].lock();
        let mags = &mut mine[c];
        if mags.push(p) {
            return;
        }
        let mut sh = SHARED[c].lock();
        let Shared { cache, depot } = &mut *sh;
        unsafe { mags.spill(depot, cache, &mut Frames) };
        mags.push(p);
        return;
    }
    let mut sh = SHARED[c].lock();
    if !sh.cache.poison {
        unsafe { sh.cache.free(p, &mut Frames) };
        return;
    }
    let name = sh.cache.name;
    let tail = unsafe { core::slice::from_raw_parts(p.add(size), sh.cache.size - size) };
    let mut found = if tail.iter().all(|&b| b == REDZONE) { Found::Nothing } else { Found::PastTheEnd(name, p as usize, size) };
    if unsafe { sh.cache.is_free(p) } {
        found = Found::FreedTwice(name, p as usize);
    } else {
        unsafe { sh.cache.free(p, &mut Frames) };
    }
    drop(sh);
    report(found);
}

fn big_alloc_once(size: usize, align: usize) -> *mut u8 {
    let count = pages_for(size);
    if count * PAGE > RUN_MAX && align <= PAGE && crate::mm::vmap::ready() {
        return crate::mm::vmap::alloc(count);
    }
    let p = pages::run(count, (align / PAGE).max(1));
    if !p.is_null() {
        PAGE_BYTES.fetch_add(count * PAGE, Ordering::Relaxed);
    }
    p
}

fn big_alloc(size: usize, align: usize) -> *mut u8 {
    let p = big_alloc_once(size, align);
    if !p.is_null() {
        return p;
    }
    crate::mm::shrink::direct();
    big_alloc_once(size, align)
}

fn big_free(p: *mut u8, size: usize) {
    let count = pages_for(size);
    if crate::mm::vmap::contains(p as u64) {
        crate::mm::vmap::free(p, count);
        return;
    }
    pages::give_run(p, count);
    PAGE_BYTES.fetch_sub(count * PAGE, Ordering::Relaxed);
}

pub struct KernelHeap;

unsafe impl GlobalAlloc for KernelHeap {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        match class_of(layout.size(), layout.align()) {
            Some(c) => small_alloc(c, layout.size()),
            None => big_alloc(layout.size(), layout.align()),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        match class_of(layout.size(), layout.align()) {
            Some(c) => small_free(c, ptr, layout.size()),
            None => big_free(ptr, layout.size()),
        }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // In place when the new size lands where the old one is: the same
        // class, or the same number of pages (the same path for them, run or
        // mapped). The free that comes later gets the new size and finds the
        // same place. Not under `slabdebug`, whose red zone starts at the size
        let old = class_of(layout.size(), layout.align());
        let new = class_of(new_size, layout.align());
        let same = match (old, new) {
            (Some(a), Some(b)) => a == b && MAGAZINES.load(Ordering::Relaxed),
            (None, None) => pages_for(layout.size()) == pages_for(new_size),
            _ => false,
        };
        if same {
            return ptr;
        }
        let Ok(new_layout) = Layout::from_size_align(new_size, layout.align()) else {
            return core::ptr::null_mut();
        };
        let new_ptr = unsafe { self.alloc(new_layout) };
        if !new_ptr.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(ptr, new_ptr, layout.size().min(new_size));
                self.dealloc(ptr, layout);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: KernelHeap = KernelHeap;

/// The kernel heap's boot step, after the boot CPU's per-CPU base: from here
/// allocations go through the magazines, and every cache with no slab yet gets
/// a secret of its own for its free-list links. `slabdebug` on the boot line
/// leaves the magazines off and poisons and red-zones every cache with no
/// object out yet
pub fn init() {
    let debug = crate::arch::x86_64::grub::cmdline_flag("slabdebug");
    let mut early = 0;
    for s in SHARED.iter() {
        let mut sh = s.lock();
        if sh.cache.slabs() == 0 {
            sh.cache.set_secret(crate::kcore::random::random_u64() as usize);
            sh.cache.poison = debug;
        } else {
            early += 1;
        }
    }
    MAGAZINES.store(!debug, Ordering::Release);
    crate::serial_println!(
        "[heap] {} caches by size, {}; {} KiB of the boot arena used, {} caches made before this",
        NCLASSES,
        if debug { "slabdebug: no magazines, freed objects poisoned, red zones checked" } else { "magazines on" },
        pages::arena_used() / 1024, early
    );
}

/// Free objects each class has in magazines: the CPUs' and the depot's.
/// Taken one lock at a time, so a moment's picture
fn in_magazines() -> [usize; NCLASSES] {
    let mut held = [0usize; NCLASSES];
    for cpu in 0..percpu::cpu_count().clamp(1, MAX_CPUS) {
        let mine = CPUS[cpu].lock();
        for (c, m) in mine.iter().enumerate() {
            held[c] += m.held();
        }
    }
    for (c, s) in SHARED.iter().enumerate() {
        held[c] += s.lock().depot.held();
    }
    held
}

/// Bytes handed out: objects with whoever asked for them, not those waiting in
/// a magazine, and the page path's, run or mapped
pub fn used() -> usize {
    let held = in_magazines();
    let objects: usize = SHARED.iter().enumerate().map(|(c, s)| {
        let sh = s.lock();
        sh.cache.out.saturating_sub(held[c]) * sh.cache.size
    }).sum();
    objects + PAGE_BYTES.load(Ordering::Relaxed) + crate::mm::vmap::mapped_pages() * PAGE
}

/// Bytes the heap holds: its slabs, and the page path's
pub fn total() -> usize {
    slab_bytes() + PAGE_BYTES.load(Ordering::Relaxed) + crate::mm::vmap::mapped_pages() * PAGE
}

/// Bytes in slabs, the typed caches' too
pub fn slab_bytes() -> usize {
    let by_size: usize = SHARED.iter().map(|s| {
        let sh = s.lock();
        sh.cache.slabs() * sh.cache.pages * PAGE
    }).sum();
    let mut typed = 0;
    kcache::each(|t| {
        let c = t.cache.lock();
        typed += c.slabs() * c.pages * PAGE;
    });
    by_size + typed
}

/// Bytes the heap holds and nobody has: free objects in its slabs
pub fn free() -> usize {
    total().saturating_sub(used())
}

/// One cache, as `/proc/slabinfo` shows it
pub struct CacheInfo {
    pub name: &'static str,
    pub objsize: usize,
    /// Objects with whoever asked for them
    pub active: usize,
    /// Objects the slabs have room for
    pub objects: usize,
    pub per_slab: usize,
    pub pages_per_slab: usize,
    /// Slabs with something out
    pub active_slabs: usize,
    pub slabs: usize,
}

fn info_of(c: &Cache, active: usize) -> CacheInfo {
    CacheInfo {
        name: c.name,
        objsize: c.size,
        active,
        objects: c.slabs() * c.per_slab,
        per_slab: c.per_slab,
        pages_per_slab: c.pages,
        active_slabs: c.slabs() - c.empty_slabs(),
        slabs: c.slabs(),
    }
}

/// Every cache by size, smallest first, then the typed caches
pub fn each_cache(mut f: impl FnMut(CacheInfo)) {
    let held = in_magazines();
    for (c, s) in SHARED.iter().enumerate() {
        let sh = s.lock();
        let info = info_of(&sh.cache, sh.cache.out.saturating_sub(held[c]));
        drop(sh);
        f(info);
    }
    let mut typed: [Option<CacheInfo>; 16] = [const { None }; 16];
    let mut n = 0;
    kcache::each(|t| {
        let c = t.cache.lock();
        if n < typed.len() {
            typed[n] = Some(info_of(&c, c.out));
            n += 1;
        }
    });
    for info in typed.into_iter().flatten() {
        f(info);
    }
}

/// Bonwick's working set, from kswapd every five seconds: the magazines a
/// depot did not need through the whole interval go back to the slabs, and
/// the slabs they empty go back to the frame allocator
pub fn reap() {
    for s in SHARED.iter() {
        let mut sh = s.lock();
        let Shared { cache, depot } = &mut *sh;
        unsafe { depot.reap(cache, &mut Frames) };
    }
}

/// The heap as a shrinker (`mm::shrink`): every magazine back to the slabs,
/// then every empty slab back to the frame allocator. From an allocation
/// (`direct`) only what no other CPU holds the lock of
pub static SHRINKER: crate::mm::shrink::Shrinker = crate::mm::shrink::Shrinker {
    name: "kernel heap",
    count: || {
        let mut frames: usize = SHARED.iter().map(|s| s.try_lock().map_or(0, |sh| {
            sh.cache.empty_slabs() * sh.cache.pages + sh.depot.held() * sh.cache.size / PAGE
        })).sum();
        kcache::each(|t| {
            if let Some(c) = t.cache.try_lock() {
                frames += c.empty_slabs() * c.pages;
            }
        });
        frames
    },
    scan: |_, direct| {
        for cpu in 0..percpu::cpu_count().clamp(1, MAX_CPUS) {
            let mine = if direct { CPUS[cpu].try_lock() } else { Some(CPUS[cpu].lock()) };
            let Some(mut mine) = mine else { continue };
            for (c, mags) in mine.iter_mut().enumerate() {
                let sh = if direct { SHARED[c].try_lock() } else { Some(SHARED[c].lock()) };
                if let Some(mut sh) = sh {
                    unsafe { mags.drain(&mut sh.cache, &mut Frames) };
                }
            }
        }
        let mut frames = 0;
        for s in SHARED.iter() {
            let sh = if direct { s.try_lock() } else { Some(s.lock()) };
            let Some(mut sh) = sh else { continue };
            let Shared { cache, depot } = &mut *sh;
            unsafe {
                depot.drain(cache, &mut Frames);
                frames += cache.shrink(&mut Frames);
            }
        }
        kcache::each(|t| {
            let c = if direct { t.cache.try_lock() } else { Some(t.cache.lock()) };
            if let Some(mut c) = c {
                frames += unsafe { c.shrink(&mut Frames) };
            }
        });
        frames
    },
};
