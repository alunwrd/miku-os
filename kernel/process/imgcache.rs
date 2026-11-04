// Physical pages of a program image, shared between the processes running it.
//
// A read-only segment is not per-process: its contents do not depend on where
// it was loaded (a relocation never targets a segment without PF_W, which
// PT_GNU_RELRO arranges), and it is never written afterwards. So the frames are
// the same frames, mapped again at the address this process chose.
//
// What is cached is the loader's runs rather than its pages: the loader
// allocates physically contiguous runs to map each with one `map_range`, and
// caching at that granularity keeps that on the second load. A second exec of a
// 4 MiB binary costs a handful of page-table walks and no copies.
//
// Writable segments are never cached. They are relocated, and RELRO turns part
// of them read-only afterwards, so a shared frame there would give one
// process's relocations to every other process running the same binary.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::mm::pmm;

/// How many distinct images may hold frames at once. Past this the cache stops
/// taking new ones rather than evicting: eviction would have to wait for every
/// process mapping the image
const MAX_IMAGES: usize = 24;
/// The ceiling on what they may hold between them: 64 MiB. Cached frames are
/// not reclaimable while anything maps them, so this is memory out of the
/// allocator for as long as the system runs
const MAX_CACHED_PAGES: usize = 16 * 1024;

/// One physically contiguous run of an image, at a page offset from the
/// start of its segment. The offset is what makes the entry reusable: the
/// virtual address differs per process (ASLR), the offset does not
#[derive(Clone, Copy)]
pub struct Run {
    pub page_idx: usize,
    pub phys:     u64,
    pub count:    usize,
}

/// Everything cached for one segment of one image
struct Entry {
    /// Path the image was loaded from, and the segment's own unbiased p_vaddr:
    /// together they identify a segment wherever this process put it
    path:   String,
    vaddr:  u64,
    /// The edge pages the loader decided to share with a neighbouring
    /// segment rather than allocate. Recorded so a later load can check
    /// that it made the same decision before reusing the runs below
    first:  usize,
    last:   usize,
    runs:   Vec<Run>,
    /// How many loads have taken these frames. Reported, not enforced: the
    /// frames' own refcounts keep them alive
    users:  u32,
}

struct Cache {
    entries: Vec<Entry>,
    pages:   usize,
    /// Pages that did not have to be allocated or copied because they were
    /// already here
    saved:   usize,
}

static CACHE: Mutex<Cache> = Mutex::new(Cache {
    entries: Vec::new(),
    pages: 0,
    saved: 0,
});

/// Takes a cached copy of a segment, if one matches.
///
/// `first`/`last` must be what this load computed: they encode which edge pages
/// a neighbouring segment already mapped, and a different decision means a
/// different page layout. A mismatch means this load is different, so it is
/// built fresh.
///
/// Every returned frame is already reference-counted for the caller, who
/// releases the references by mapping the frames (address-space teardown then
/// does it) or by `release` on failure.
pub fn take(path: &str, vaddr: u64, first: usize, last: usize) -> Option<Vec<Run>> {
    let mut c = CACHE.lock();
    let e = c.entries.iter_mut()
        .find(|e| e.vaddr == vaddr && e.path == path && e.first == first && e.last == last)?;

    for r in &e.runs {
        for i in 0..r.count as u64 {
            let f = r.phys + i * 4096;
            pmm::ref_inc(f);
            // Re-pinned: when the last process using an image exits, the
            // refcount path treats the frame as unique and clears the pin. The
            // cache still owns it, so a cached image frame must not be swapped
            crate::mm::swap_map::set_pinned(f, true);
        }
    }
    e.users += 1;
    let runs = e.runs.clone();
    c.saved += runs.iter().map(|r| r.count).sum::<usize>();
    Some(runs)
}

/// Keeps a segment's frames for the next load.
///
/// Called after the frames are filled and mapped, so the cache's reference is
/// on top of the caller's, which keeps them alive after the process that first
/// loaded the image exits.
///
/// Declines when the cache is full: the next load then builds it again.
pub fn store(path: &str, vaddr: u64, first: usize, last: usize, runs: &[Run]) {
    if runs.is_empty() {
        return;
    }
    let pages: usize = runs.iter().map(|r| r.count).sum();

    let mut c = CACHE.lock();
    if c.entries.len() >= MAX_IMAGES || c.pages + pages > MAX_CACHED_PAGES {
        return;
    }
    if c.entries.iter().any(|e| e.vaddr == vaddr && e.path == path) {
        return;
    }

    for r in runs {
        for i in 0..r.count as u64 {
            let f = r.phys + i * 4096;
            pmm::ref_inc(f);
            // A shared image page is never swapped out: the frame is mapped in
            // every process running this binary, and eviction would take it
            // from all of them through one process's page table
            crate::mm::swap_map::set_pinned(f, true);
        }
    }
    c.pages += pages;
    c.entries.push(Entry {
        path: String::from(path),
        vaddr,
        first,
        last,
        runs: runs.to_vec(),
        users: 1,
    });
}

/// Gives back references taken by `take` that never became mappings, when the
/// load failed after this segment. Mapped frames are released by address-space
/// teardown
pub fn release(runs: &[Run]) {
    for r in runs {
        for i in 0..r.count as u64 {
            pmm::ref_dec(r.phys + i * 4096);
        }
    }
}

/// Drops every entry for a path, releasing the cache's own references.
///
/// The frames survive as long as a process maps them; the next load reads the
/// file again. For when a binary is replaced. Nothing calls it yet
pub fn forget(path: &str) {
    let mut c = CACHE.lock();
    let mut freed = 0usize;
    c.entries.retain(|e| {
        if e.path != path {
            return true;
        }
        for r in &e.runs {
            for i in 0..r.count as u64 {
                let f = r.phys + i * 4096;
                if pmm::ref_dec(f) == 0 {
                    pmm::free_frame(f);
                }
            }
            freed += r.count;
        }
        false
    });
    c.pages = c.pages.saturating_sub(freed);
}

/// (images, pages held, pages a load did not have to build)
pub fn stats() -> (usize, usize, usize) {
    let c = CACHE.lock();
    (c.entries.len(), c.pages, c.saved)
}

/// One line per cached image, for `elf cache`
pub fn report() {
    let c = CACHE.lock();
    if c.entries.is_empty() {
        crate::println!("  image cache empty");
        return;
    }
    for e in &c.entries {
        let pages: usize = e.runs.iter().map(|r| r.count).sum();
        crate::println!(
            "  {:<24} vaddr={:#010x} {:>4} pages in {:>2} runs  {} loads",
            e.path, e.vaddr, pages, e.runs.len(), e.users,
        );
    }
    crate::println!(
        "  {} images, {} pages held, {} pages saved",
        c.entries.len(), c.pages, c.saved,
    );
}
