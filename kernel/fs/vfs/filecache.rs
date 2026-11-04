// The page cache for files on a disk (`dev/fs-architecture.md`, "The page cache
// for files on a disk")
//
// One entry per vnode that has used the cache: the pages read of that file, the
// generation that says whether a read made without the VFS lock may still go
// in, and what the filesystem said the file was when they were read. The pages
// are frames in a `PageCache` of their own (tmpfs's index and frames, a limit
// of their own). Clean pages can go at any time; eviction takes the coldest
// files by last read.
//
// Both are maps by vnode id. An entry goes when its vnode does (`forget`); a
// fill in flight holds a reference on its vnode, so no id is freed and handed
// to another file under one.
//
// This file is the table. What reads and writes through it, and the three steps
// of a miss, is `core/cached.rs`.
//
// It has a lock of its own (`FILE_CACHE`), inside the VFS lock: a read whose
// pages are all here is answered with no VFS lock (`cached::read_hit`,
// `dev/vfs-locking.md`, "4b"). Nothing takes the VFS lock while holding this
// one.
//
// A page can be dirty: written here and not yet on the disk
// (`core/writeback.rs`, `dev/fs-architecture.md`, "Write-back"). A dirty
// vnode's pages are never evicted or dropped; they go only with the file
// (`discard`).

extern crate alloc;

use alloc::collections::BTreeMap;

use crate::fs::vfs::address_space::AddressSpace;
use crate::fs::vfs::pages::PageCache;
use crate::fs::vfs::types::*;
use crate::kcore::sched_lock::{SchedRwLock, SchedRwReadGuard, SchedRwWriteGuard};
use core::sync::atomic::{AtomicU64, Ordering};

/// Reads answered from the cache with no VFS lock (`cached::read_hit`), of
/// all the `hits`: `/proc/stat`'s `filecache:` line
pub static UNLOCKED_HITS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Vnodes the cache has forgotten (`forget`), ever. A reader without the VFS
/// lock that reads the same count before it looks at its descriptor and
/// again under the cache's lock knows no vnode went in between: the id its
/// descriptor named is still that file, and the entry is its
/// (`cached::read_hit`)
pub static FORGOTTEN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A file read again within this many reads of other files keeps its place
/// in the eviction order (`pin`)
const TOUCH_SLACK: u64 = 32;

/// Dirty pages, all files together: what a writer reads to know whether to
/// write some out before it returns (`core::writeback`)
pub static DIRTY_PAGES: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The dirty pages one vnode has
pub struct Dirty {
    /// Page number, and the sequence number of the write that last dirtied it
    pub pages: BTreeMap<u32, u64>,
    /// When the vnode went dirty, for bdflush's age
    pub since_ns: u64,
}

/// The cache and its lock: read-write, so the hits of many readers (`pin`) go
/// in beside each other (`dev/vfs-locking.md`)
pub static FILE_CACHE: SchedRwLock<FileCache> = SchedRwLock::named("filecache", FileCache::new());

/// The cache, held alone. For a statement or a short block: release it before
/// anything that frees a vnode (`forget` takes it again), and never hold it
/// across a copy to or from a program
#[track_caller]
pub fn lock() -> SchedRwWriteGuard<'static, FileCache> {
    FILE_CACHE.lock()
}

/// The cache, held beside other readers: `pin`
#[track_caller]
pub fn read() -> SchedRwReadGuard<'static, FileCache> {
    FILE_CACHE.read()
}

/// Pages read past a sequential read's end
pub const READAHEAD_PAGES: usize = 32;

/// The most one fill reads: it holds frames outside the lock that nobody else
/// can use until it is back
pub const MAX_FILL_PAGES: usize = 64;

/// The share of the machine the cache may grow to: half
const SHARE_DIVISOR: usize = 2;

/// Pages the cache may hold
fn cache_limit() -> usize {
    let (_, total) = crate::mm::pmm::stats();
    (total / SHARE_DIVISOR).max(256)
}

/// Dirty pages past which a writer writes some out before it returns: an
/// eighth of what the cache may hold
pub fn dirty_limit() -> usize {
    (cache_limit() / 8).max(64)
}

/// Free frames below which kswapd takes pages back from the cache: a
/// sixteenth of the machine (`pmm::running_low`)
pub fn low_memory() -> bool {
    crate::mm::pmm::running_low()
}

pub struct Entry {
    pub pages: AddressSpace,
    /// Moved by every change to the file's data the VFS makes or is told of.
    /// A fill that started under one generation and finds another inserts
    /// nothing
    pub gen: u32,
    /// The page after the last one a read ended in, for telling a
    /// sequential reader from a random one. Atomic: a hit moves it under the
    /// cache's lock held to read
    pub next_page: AtomicU64,
    /// Size and mtime as the filesystem gave them when the pages were read;
    /// `None` once the VFS itself has written the file, when the pages are
    /// right by construction and the filesystem's numbers are whatever its
    /// own clock made them
    pub stamp: Option<(u64, u64)>,
    /// The file's size as the cache knows it, what a read without the VFS
    /// lock ends at: from the fill that read the pages, the open that
    /// checked them, and every write through the VFS. Whatever changes the
    /// size otherwise drops the pages with it
    pub size: u64,
    /// A write-out of its dirty pages failed: what the next `fsync` returns
    pub wb_error: Option<VfsError>,
    /// Its place in the order files were read in; 0 when it holds no pages
    seq: u64,
}

impl Entry {
    const fn empty() -> Self {
        Self { pages: AddressSpace::new(), gen: 0, next_page: AtomicU64::new(0), stamp: None, size: 0, wb_error: None, seq: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.pages.root == INVALID_PAGE
    }
}

/// What an id with no entry reads as: a file the cache has never seen, at
/// generation 0, which is where a new entry starts
static NO_ENTRY: Entry = Entry::empty();

/// The entries by vnode id. Reading an id with none gives `NO_ENTRY`;
/// writing one makes it
pub struct Entries(BTreeMap<u32, Entry>);

impl core::ops::Index<usize> for Entries {
    type Output = Entry;
    fn index(&self, vid: usize) -> &Entry {
        self.0.get(&(vid as u32)).unwrap_or(&NO_ENTRY)
    }
}

impl core::ops::IndexMut<usize> for Entries {
    fn index_mut(&mut self, vid: usize) -> &mut Entry {
        self.0.entry(vid as u32).or_insert(Entry::empty())
    }
}

pub struct FileCache {
    pub entries: Entries,
    pub pages: PageCache,
    /// Vnodes that hold pages, by when they were last read: coldest first
    order: BTreeMap<u64, u32>,
    next_seq: u64,
    /// Vnodes with pages written and not on the disk yet
    dirty: BTreeMap<u32, Dirty>,
    /// The sequence the next dirtied page gets
    next_write: u64,
    pub hits: AtomicU64,
    pub misses: AtomicU64,
    pub fills: u64,
    pub evicted: u64,
    /// Pages written into the cache, and pages written out of it
    pub dirtied: u64,
    pub cleaned: u64,
    /// Files' pages taken off them to be freed after the VFS lock (`reap`)
    detached: alloc::vec::Vec<crate::fs::vfs::address_space::Detached>,
}

/// How many `FileCache::detached` holds, read without the lock: every
/// `with_vfs` asks
static DETACHED: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// The pages files let go of under the VFS lock, freed a part at a time with
/// the VFS lock released and `FILE_CACHE` taken for each part: after each
/// `with_vfs`, so the call that dropped them pays before it returns
pub fn reap() {
    while DETACHED.load(Ordering::Acquire) > 0 {
        let mut fc = lock();
        let FileCache { detached, pages, .. } = &mut *fc;
        let Some(d) = detached.last_mut() else { return };
        if d.free_part(pages, crate::fs::vfs::address_space::FREE_PART) {
            detached.pop();
            DETACHED.fetch_sub(1, Ordering::AcqRel);
        }
    }
}

impl FileCache {
    pub const fn new() -> Self {
        Self {
            entries: Entries(BTreeMap::new()),
            pages: PageCache::with_limit(cache_limit),
            order: BTreeMap::new(),
            next_seq: 1,
            dirty: BTreeMap::new(),
            next_write: 1,
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            fills: 0,
            evicted: 0,
            dirtied: 0,
            cleaned: 0,
            detached: alloc::vec::Vec::new(),
        }
    }

    /// The frame behind page `page` of `vid`'s file, if it is in
    pub fn frame(&mut self, vid: usize, page: usize) -> Option<u64> {
        let e = self.entries.0.get(&(vid as u32))?;
        if e.is_empty() {
            return None;
        }
        let pid = e.pages.get_page(&mut self.pages, page)?;
        self.pages.phys(pid)
    }

    /// The frames `[offset, offset + len)` of `vid`'s file covers, ended at the
    /// file's end and at as many pages as `frames` holds, each pinned for the
    /// caller (`pmm::try_ref`) to copy from with no lock held and then let go
    /// (`pmm::free_frame_cow`). The bytes and the frames, or `None` when a page
    /// is not in and the read must go the locked way. A hit is counted as
    /// `cache_copy` counts one
    ///
    /// Under the cache's lock held to read: with the third number, whether the
    /// file is due a `touch`, which takes it held alone
    pub fn pin(&self, vid: usize, offset: u64, len: usize, frames: &mut [u64]) -> Option<(usize, usize, bool)> {
        let key = vid as u32;
        let (size, space, seq) = {
            let e = self.entries.0.get(&key)?;
            if e.is_empty() || len == 0 {
                return None;
            }
            (e.size, e.pages, e.seq)
        };
        if offset >= size {
            return Some((0, 0, false));
        }
        let page = PAGE_SIZE as u64;
        let room = frames.len() * PAGE_SIZE - (offset % page) as usize;
        let n = len.min((size - offset) as usize).min(room);
        let first = (offset / page) as usize;
        let last = ((offset + n as u64 - 1) / page) as usize;
        let mut got = 0;
        for p in first..=last {
            let pinned = space
                .get_page_ro(&self.pages, p)
                .and_then(|pid| self.pages.phys(pid))
                .filter(|&phys| crate::mm::pmm::try_ref(phys));
            let Some(phys) = pinned else {
                for &f in &frames[..got] {
                    crate::mm::pmm::free_frame_cow(f);
                }
                self.misses.fetch_add(1, Ordering::Relaxed);
                return None;
            };
            frames[got] = phys;
            got += 1;
        }
        self.hits.fetch_add(1, Ordering::Relaxed);
        if let Some(e) = self.entries.0.get(&key) {
            e.next_page.store(last as u64 + 1, Ordering::Relaxed);
        }
        // Among the last few files read: left where it is. Moving it is two
        // changes to a tree on every hit, taken with the lock held alone, for
        // an order eviction needs only roughly
        let touch = seq == 0 || seq + TOUCH_SLACK < self.next_seq;
        Some((n, got, touch))
    }

    /// `vid` was just read: last to go
    pub fn touch(&mut self, vid: usize) {
        let Some(e) = self.entries.0.get_mut(&(vid as u32)) else { return };
        if e.is_empty() {
            return;
        }
        if e.seq != 0 {
            self.order.remove(&e.seq);
        }
        e.seq = self.next_seq;
        self.next_seq += 1;
        self.order.insert(e.seq, vid as u32);
    }

    /// Out of the order, holding no pages any more
    fn unorder(&mut self, vid: usize) {
        if let Some(e) = self.entries.0.get_mut(&(vid as u32)) {
            if e.seq != 0 {
                self.order.remove(&e.seq);
                e.seq = 0;
            }
        }
    }

    /// `vid`'s vnode is gone: its pages and its entry with it, dirty or not
    pub fn forget(&mut self, vid: usize) {
        FORGOTTEN.fetch_add(1, core::sync::atomic::Ordering::Release);
        self.discard(vid);
        self.entries.0.remove(&(vid as u32));
    }

    /// Whether `vid` has pages not on the disk yet
    pub fn is_dirty(&self, vid: usize) -> bool {
        self.dirty.contains_key(&(vid as u32))
    }

    /// Page `page` of `vid` was just written here
    pub fn mark_dirty(&mut self, vid: usize, page: usize) {
        let seq = self.next_write;
        self.next_write += 1;
        let d = self.dirty.entry(vid as u32).or_insert_with(|| Dirty {
            pages: BTreeMap::new(),
            since_ns: crate::kcore::time::now_ns(),
        });
        if d.pages.insert(page as u32, seq).is_none() {
            DIRTY_PAGES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
        self.dirtied += 1;
    }

    /// The sequence the next write will get: a flush that writes out what was
    /// dirty before this stops at it, however fast a writer keeps adding
    pub fn write_mark(&self) -> u64 {
        self.next_write
    }

    /// Up to `max` of `vid`'s dirty pages dirtied before `before`, in order
    pub fn dirty_pages(&self, vid: usize, before: u64, max: usize) -> alloc::vec::Vec<u32> {
        match self.dirty.get(&(vid as u32)) {
            Some(d) => d.pages.iter().filter(|(_, &s)| s < before).map(|(&p, _)| p).take(max).collect(),
            None => alloc::vec::Vec::new(),
        }
    }

    /// The sequence page `page` of `vid` was last dirtied at, if it is dirty
    pub fn dirty_seq(&self, vid: usize, page: u32) -> Option<u64> {
        self.dirty.get(&(vid as u32))?.pages.get(&page).copied()
    }

    /// These pages of `vid` reached the disk as they were at the sequence
    /// given: clean, unless written again since
    pub fn clean(&mut self, vid: usize, written: &[(u32, u64)]) {
        let key = vid as u32;
        let Some(d) = self.dirty.get_mut(&key) else { return };
        for &(page, seq) in written {
            if d.pages.get(&page) == Some(&seq) {
                d.pages.remove(&page);
                DIRTY_PAGES.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
                self.cleaned += 1;
            }
        }
        if d.pages.is_empty() {
            self.dirty.remove(&key);
        }
    }

    /// Vnodes dirty since before `ns`, oldest first, or every dirty one for 0
    pub fn dirty_since(&self, ns: u64) -> alloc::vec::Vec<u32> {
        let mut v: alloc::vec::Vec<(u64, u32)> =
            self.dirty.iter().filter(|(_, d)| ns == 0 || d.since_ns < ns).map(|(&vid, d)| (d.since_ns, vid)).collect();
        v.sort_unstable();
        v.into_iter().map(|(_, vid)| vid).collect()
    }

    /// `vid`'s dirty pages at or past byte `size` are not to be written: a
    /// truncate is cutting them off
    pub fn undirty_from(&mut self, vid: usize, size: u64) {
        let key = vid as u32;
        let Some(d) = self.dirty.get_mut(&key) else { return };
        let first = size.div_ceil(PAGE_SIZE as u64) as u32;
        let gone = d.pages.split_off(&first).len();
        DIRTY_PAGES.fetch_sub(gone, core::sync::atomic::Ordering::Relaxed);
        if d.pages.is_empty() {
            self.dirty.remove(&key);
        }
    }

    /// `vid`'s file is `size` long now: a truncate. A write-out cuts the last
    /// page here
    pub fn resize(&mut self, vid: usize, size: u64) {
        if let Some(e) = self.entries.0.get_mut(&(vid as u32)) {
            e.size = size;
        }
    }

    /// Every page of `vid` out, the dirty ones too: the file itself is going,
    /// or being emptied
    pub fn discard(&mut self, vid: usize) {
        if let Some(d) = self.dirty.remove(&(vid as u32)) {
            DIRTY_PAGES.fetch_sub(d.pages.len(), core::sync::atomic::Ordering::Relaxed);
        }
        self.drop_file(vid);
    }

    /// Entries held, for the selftest and `info`
    pub fn files(&self) -> usize {
        self.entries.0.len()
    }

    /// Put `phys` in as page `page` of `vid`'s file. The cache owns the frame
    /// if this says `Ok`; on `Err` the caller still does. A page already
    /// there is kept and the new frame refused, as the same bytes
    pub fn insert(&mut self, vid: usize, page: usize, phys: u64) -> VfsResult<()> {
        let mut space = self.entries[vid].pages;
        if space.get_page(&mut self.pages, page).is_some() {
            return Err(VfsError::AlreadyExists);
        }
        let pid = self.pages.adopt(phys)?;
        if let Err(e) = space.set_page(&mut self.pages, page, pid) {
            // The page goes back without its frame: that is still the caller's
            self.forget_page(pid);
            self.entries[vid].pages = space;
            return Err(e);
        }
        self.entries[vid].pages = space;
        self.touch(vid);
        Ok(())
    }

    /// A page id dropped without freeing its frame: the caller hands the frame
    /// back to the allocator itself
    fn forget_page(&mut self, pid: PageId) {
        if let Some(phys) = self.pages.phys(pid) {
            // One more holder for `free_page` to drop, so the frame survives it
            crate::mm::pmm::ref_inc(phys);
            self.pages.free_page(pid);
        }
    }

    /// Every page of `vid` out, and its generation moved: whatever was
    /// being read for it is not to go in. A dirty vnode keeps its pages,
    /// which are newer than the disk: they go with the file (`discard`)
    pub fn drop_file(&mut self, vid: usize) {
        if !self.is_dirty(vid) {
            self.free_pages_later(vid);
        }
        let e = &mut self.entries[vid];
        e.gen = e.gen.wrapping_add(1);
        e.next_page.store(0, Ordering::Relaxed);
        e.stamp = None;
    }

    /// `free_pages` for a file being dropped (deleted, truncated, found
    /// changed): a big one's pages go after the VFS lock (`reap`). Eviction
    /// frees at once, since it counts what it freed
    fn free_pages_later(&mut self, vid: usize) {
        let space = self.entries[vid].pages;
        if space.root == INVALID_PAGE || space.nr_pages as usize <= crate::fs::vfs::address_space::FREE_PART {
            return self.free_pages(vid);
        }
        self.entries[vid].pages = AddressSpace::new();
        self.detached.push(crate::fs::vfs::address_space::Detached::new(space, &self.pages));
        DETACHED.fetch_add(1, Ordering::AcqRel);
        self.unorder(vid);
    }

    fn free_pages(&mut self, vid: usize) {
        let mut space = self.entries[vid].pages;
        if space.root == INVALID_PAGE {
            return;
        }
        for pid in space.truncate(&mut self.pages, 0) {
            self.pages.free_page(pid);
        }
        self.entries[vid].pages = space;
        self.unorder(vid);
    }

    /// Pages held, index pages included
    pub fn used_pages(&self) -> usize {
        self.pages.used_pages()
    }

    pub fn capacity(&self) -> usize {
        self.pages.total_capacity()
    }

    /// Drop the coldest files until `pages` have gone or nothing is left,
    /// sparing `keep`. How many went
    pub fn shrink(&mut self, pages: usize, keep: usize) -> usize {
        let target = self.used_pages().saturating_sub(pages);
        self.evict_while(keep, |fc| fc.used_pages() > target)
    }

    /// Drop the coldest files until `want` pages are free under the cap or
    /// nothing is left, sparing `keep`. How many pages went
    pub fn evict(&mut self, want: usize, keep: usize) -> usize {
        self.evict_while(keep, |fc| fc.pages.free_pages() < want)
    }

    fn evict_while(&mut self, keep: usize, more: impl Fn(&Self) -> bool) -> usize {
        let before = self.used_pages();
        let mut spared = alloc::vec::Vec::new();
        while more(self) {
            let Some((_, v)) = self.order.pop_first() else { break };
            let v = v as usize;
            if let Some(e) = self.entries.0.get_mut(&(v as u32)) {
                e.seq = 0;
            }
            // Dirty pages are the only copy until they are written out
            if v == keep || self.is_dirty(v) {
                spared.push(v);
                continue;
            }
            // Its pages go, its generation stays: the file did not change, so
            // a read of it that is under way may still put them back
            self.free_pages(v);
            self.evicted += 1;
        }
        for v in spared {
            self.touch(v);
        }
        before.saturating_sub(self.used_pages())
    }

    /// Room for `pages` more, making it from the coldest files but `keep`.
    /// Whether there is
    pub fn make_room(&mut self, pages: usize, keep: usize) -> bool {
        // Index pages come with data pages; a few more than asked covers them
        let want = pages + 4;
        if self.pages.free_pages() >= want {
            return true;
        }
        // A sixteenth of the cap at a time, so a cache that is full is not
        // evicting on every fill
        let batch = want.max(self.capacity() / 16);
        self.evict(batch, keep);
        self.pages.free_pages() >= want
    }
}
