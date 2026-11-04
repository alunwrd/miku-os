// The heap's mapped area: big objects made of single frames
// (`dev/kernel-core.md`, "Big objects: pages, in one run or mapped")
//
// Linux's vmalloc. An object above 32 KiB does not need its frames in one run:
// nothing that allocates hands heap memory to a device (`mm::dma`), so its
// frames are taken one at a time and mapped side by side here. A 28 MiB
// firmware image needs 7168 free frames, not 28 MiB in one piece.
//
// - Where: PML4 slot 384, 512 GiB. Its table is made at boot, before the
//   first user address space copies slots 256..512 (`vmm::share_kernel_slot`),
//   so every space sees what is mapped here.
// - A guard page after every object stays unmapped: running off the end
//   faults at the instruction that did it. The page below every object is
//   never mapped either (the guard ending the span before it, free space, or
//   the slot below the area), which a kernel stack needs (`mm::kstack`).
// - Ranges are extents in two ordered maps, by address (to merge a freed
//   range with its neighbours) and by length (best fit), and so are the ranges
//   waiting for a shootdown. Every node is a small object from the heap's
//   caches, which never come from here, so the area never allocates from
//   itself; a `Vec` would, once past 32 KiB, under this lock. Lock order: this
//   area, then a cache, then frames.
// - A free unmaps at once and flushes this processor; the frames go back then.
//   The range is handed out again only after one shootdown of every processor
//   for many frees together, from kswapd with nothing held (`purge`): Linux's
//   lazy `vunmap`.

extern crate alloc;
use alloc::collections::{BTreeMap, BTreeSet};
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use x86_64::structures::paging::PageTableFlags;

use crate::kcore::irq_lock::IrqMutex;
use crate::mm::{pmm, vmm};

const PAGE: u64 = 4096;
const SLOT: usize = 384;
/// Where the area starts: PML4 slot 384
pub const BASE: u64 = 0xFFFF_0000_0000_0000 | ((SLOT as u64) << 39);
/// How long it is: one PML4 entry
pub const SIZE: u64 = 1 << 39;
/// Freed pages that wait for a shootdown before kswapd is asked for one
const LAZY_PAGES: u64 = 8192;

struct Area {
    /// Free extents, start to pages
    by_addr: BTreeMap<u64, u64>,
    /// The same extents as (pages, start), for best fit
    by_len: BTreeSet<(u64, u64)>,
    /// Freed and unmapped, but maybe still in another processor's TLB: start
    /// to pages
    dirty: BTreeMap<u64, u64>,
    dirty_pages: u64,
}

impl Area {
    const fn new() -> Self {
        Self { by_addr: BTreeMap::new(), by_len: BTreeSet::new(), dirty: BTreeMap::new(), dirty_pages: 0 }
    }

    /// A free extent back, merged with its neighbours
    fn insert(&mut self, mut start: u64, mut pages: u64) {
        if let Some((&s, &p)) = self.by_addr.range(..start).next_back() {
            if s + p * PAGE == start {
                self.by_addr.remove(&s);
                self.by_len.remove(&(p, s));
                start = s;
                pages += p;
            }
        }
        if let Some(&p) = self.by_addr.get(&(start + pages * PAGE)) {
            let s = start + pages * PAGE;
            self.by_addr.remove(&s);
            self.by_len.remove(&(p, s));
            pages += p;
        }
        self.by_addr.insert(start, pages);
        self.by_len.insert((pages, start));
    }

    /// The smallest extent of at least `pages`, cut to size
    fn take(&mut self, pages: u64) -> Option<u64> {
        let &(have, start) = self.by_len.range((pages, 0)..).next()?;
        self.by_len.remove(&(have, start));
        self.by_addr.remove(&start);
        if have > pages {
            let rest = start + pages * PAGE;
            self.by_addr.insert(rest, have - pages);
            self.by_len.insert((have - pages, rest));
        }
        Some(start)
    }
}

static AREA: IrqMutex<Area> = IrqMutex::new(Area::new());
static READY: AtomicBool = AtomicBool::new(false);
/// Freed ranges are waiting: kswapd should `purge`
static PURGE_WANTED: AtomicBool = AtomicBool::new(false);
/// Pages mapped for objects now
static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// After the frame allocator, before the first user address space
pub fn init() {
    if !vmm::share_kernel_slot(SLOT) {
        crate::kerror!("[vmap] no frame for the area's table: big objects stay in one run");
        return;
    }
    AREA.lock().insert(BASE, SIZE / PAGE);
    READY.store(true, Ordering::Release);
    crate::serial_println!("[vmap] {} GiB at {:#x} for objects above 32 KiB", SIZE >> 30, BASE);
}

pub fn ready() -> bool {
    READY.load(Ordering::Acquire)
}

/// Whether `addr` is in the area
#[inline]
pub fn contains(addr: u64) -> bool {
    (BASE..BASE + SIZE).contains(&addr)
}

/// Pages mapped for objects now
pub fn mapped_pages() -> usize {
    MAPPED.load(Ordering::Relaxed)
}

/// `pages` of single frames mapped side by side, with an unmapped guard page
/// after them; null when there are not that many frames
pub fn alloc(pages: usize) -> *mut u8 {
    let span = pages as u64 + 1;
    let start = {
        let mut a = AREA.lock();
        a.take(span)
    };
    let start = match start {
        Some(s) => s,
        // 512 GiB of ranges taken or waiting: give the waiting ones back
        // now, and try again
        None => {
            purge();
            match AREA.lock().take(span) {
                Some(s) => s,
                None => return core::ptr::null_mut(),
            }
        }
    };
    let flags = PageTableFlags::WRITABLE | PageTableFlags::NO_EXECUTE;
    for i in 0..pages as u64 {
        let ok = match pmm::alloc_frame() {
            Some(f) => vmm::map_kernel_page(start + i * PAGE, f, flags) || {
                pmm::free_frame(f);
                false
            },
            None => false,
        };
        if !ok {
            // What was mapped goes back; the range was never seen elsewhere
            unmap(start, i);
            AREA.lock().insert(start, span);
            return core::ptr::null_mut();
        }
    }
    MAPPED.fetch_add(pages, Ordering::Relaxed);
    start as *mut u8
}

/// `pages` from `start`: unmapped, flushed here, and their frames freed
fn unmap(start: u64, pages: u64) {
    for i in 0..pages {
        if let Some(f) = vmm::unmap_kernel_page(start + i * PAGE) {
            pmm::free_frame(f);
        }
    }
}

/// An object `alloc` made, back. Its range waits for `purge`
pub fn free(ptr: *mut u8, pages: usize) {
    let start = ptr as u64;
    unmap(start, pages as u64);
    MAPPED.fetch_sub(pages, Ordering::Relaxed);
    let mut a = AREA.lock();
    a.dirty.insert(start, pages as u64 + 1);
    a.dirty_pages += pages as u64 + 1;
    if a.dirty_pages >= LAZY_PAGES {
        PURGE_WANTED.store(true, Ordering::Relaxed);
    }
}

/// The physical address of a byte in the area, for a check: memory here is
/// never one physical run, and never device memory
pub fn phys_of(addr: u64) -> Option<u64> {
    vmm::kernel_page_phys(addr)
}

/// Every processor's TLB flushed once, then every waiting range free again.
/// With nothing held that another processor could spin on with interrupts
/// off: from kswapd, or from an `alloc` that found the area full
pub fn purge() {
    let waiting = {
        let mut a = AREA.lock();
        a.dirty_pages = 0;
        core::mem::take(&mut a.dirty)
    };
    PURGE_WANTED.store(false, Ordering::Relaxed);
    if waiting.is_empty() {
        return;
    }
    crate::mm::tlb::shootdown();
    let mut a = AREA.lock();
    for (s, p) in waiting {
        a.insert(s, p);
    }
}

/// From kswapd: a purge when enough frees have piled up
pub fn purge_if_wanted() {
    if PURGE_WANTED.load(Ordering::Relaxed) {
        purge();
    }
}
