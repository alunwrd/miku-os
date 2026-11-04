// Write-back for files on a disk (`dev/fs-architecture.md`, "Write-back")
//
// A write to a disk file goes into the page cache as dirty pages
// (`cache_write`) when it can, and the filesystem gets the bytes later, from a
// flush. A flush is three steps, like a fill:
//
//   1. `plan_flush`, under the VFS lock: which dirty pages, the vnode's mtime,
//      a reference on the vnode;
//   2. `write_out`, under the filesystem's slot lock (taken for each run)
//      and no VFS lock: each run of contiguous pages pinned under
//      `FILE_CACHE` with the sequence number each was last dirtied at, then
//      handed as they are to `write_pages`, then the mtime;
//   3. `finish_flush`, under the VFS lock: a page whose number has not moved
//      is clean.
//
// Who flushes: `fsync` (`flush_fd`), bdflush (`writeback_old`), a writer over
// the dirty limit (`balance`), sync and unmount (`writeback_all`). A reader
// that goes around the cache writes the vnode out first, holding the VFS lock
// (`settle`).

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use super::MikuVFS;
use crate::fs::vfs::filecache::{self, DIRTY_PAGES};
use crate::fs::vfs::fsops::{FsSetAttr, NodeId};
use crate::fs::vfs::types::*;
use crate::kcore::sched_lock::SchedMutex;

/// Dirty pages one flush plans: one trip's worth before the VFS lock is
/// taken again to mark them clean
const FLUSH_PAGES: usize = 1024;

/// Pages pinned under `FILE_CACHE` at once and written with one
/// `write_pages`: 1 MiB, one transaction for ext, and a lock held for the
/// lookups and the pins, not for any copying
const RUN_PAGES: usize = 256;

/// A vnode dirty this long is bdflush's to write out: ext's journal commits
/// every five seconds, and data older than its metadata is not worth keeping
pub const DIRTY_AGE_NS: u64 = 5_000_000_000;

/// How long a mount's free space is believed
const ROOM_FRESH_NS: u64 = 1_000_000_000;

/// Free bytes per mount, and when they were read
static ROOM: SchedMutex<BTreeMap<(u8, u8), (u64, u64)>> = SchedMutex::named("wb-room", BTreeMap::new());

/// `writethrough` on the boot line: every write goes to the filesystem
fn writethrough() -> bool {
    static MODE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);
    match MODE.load(core::sync::atomic::Ordering::Relaxed) {
        2 => {
            let on = crate::arch::x86_64::grub::cmdline_flag("writethrough");
            MODE.store(on as u8, core::sync::atomic::Ordering::Relaxed);
            on
        }
        m => m == 1,
    }
}

/// Step 1's result: one vnode's dirty pages to write out
pub(super) struct Flush {
    vid: usize,
    fs_type: FsType,
    slot: u8,
    node: NodeId,
    mtime: Timestamp,
    pages: Vec<u32>,
    /// A reference was taken for the trip without the VFS lock
    held: bool,
}

impl MikuVFS {
    /// `data` at `offset` into the cache as dirty pages: how many bytes went,
    /// or `None` for the write to go through to the filesystem. It goes when
    /// it starts at or before the end, and each page it touches is cached,
    /// wholly written, or at or past the end. The caller moves the size, the
    /// mtime and the offset
    pub(super) fn cache_write(&mut self, vid: usize, offset: u64, data: &[u8]) -> Option<usize> {
        if data.is_empty() || writethrough() || !self.cacheable(vid) {
            return None;
        }
        let size = self.nodes[vid].size;
        if offset > size || !self.room_for_dirty(vid, data.len()) {
            return None;
        }
        let page = PAGE_SIZE as u64;
        let end = offset + data.len() as u64;
        let first = (offset / page) as usize;
        let last = ((end - 1) / page) as usize;

        let mut fc = filecache::lock();
        let mut fresh = 0;
        for p in first..=last {
            if fc.frame(vid, p).is_some() {
                continue;
            }
            let start = p as u64 * page;
            let whole = offset <= start && end >= start + page;
            if !whole && start < size {
                return None;
            }
            fresh += 1;
        }
        if fresh > 0 && !fc.make_room(fresh, vid) {
            return None;
        }
        // Page by page, so a page the cache refuses ends the write there:
        // every page before it has its bytes, and none after it is in
        let mut done = 0usize;
        for p in first..=last {
            let at = offset + done as u64;
            let in_page = (at % page) as usize;
            let chunk = (PAGE_SIZE - in_page).min(data.len() - done);
            let phys = match fc.frame(vid, p) {
                Some(phys) => phys,
                None => {
                    let Some(phys) = crate::mm::pmm::alloc_frame() else { break };
                    // Zeros only where this write leaves the page alone
                    let base = super::cached::hhdm_ptr(phys);
                    unsafe {
                        core::ptr::write_bytes(base, 0, in_page);
                        core::ptr::write_bytes(base.add(in_page + chunk), 0, PAGE_SIZE - in_page - chunk);
                    }
                    if fc.insert(vid, p, phys).is_err() {
                        crate::mm::pmm::free_frame(phys);
                        break;
                    }
                    phys
                }
            };
            let dst = unsafe { core::slice::from_raw_parts_mut(super::cached::hhdm_ptr(phys).add(in_page), chunk) };
            dst.copy_from_slice(&data[done..done + chunk]);
            fc.mark_dirty(vid, p);
            done += chunk;
        }
        if done == 0 {
            return None;
        }
        // From the VFS's size, which is exact: the entry's may be from before
        // a truncate
        let e = &mut fc.entries[vid];
        e.size = size.max(offset + done as u64);
        // Right by construction, whatever the filesystem's clock says
        e.stamp = None;
        Some(done)
    }

    /// Whether the mount `vid` is on has room for every page dirty now and
    /// `len` more, twice over: the blocks are only allocated at the flush, and
    /// a write the disk cannot take should hear `ENOSPC` itself. Twice, for
    /// the free count being up to a second old and for ext's own blocks
    fn room_for_dirty(&self, vid: usize, len: usize) -> bool {
        let Some((fs_type, slot, _)) = self.backing_of(vid) else { return false };
        let key = (fs_type as u8, slot);
        let now = crate::kcore::time::now_ns();
        let known = ROOM.lock().get(&key).copied();
        let free = match known {
            Some((free, at)) if now.saturating_sub(at) < ROOM_FRESH_NS => free,
            _ => {
                // Not `with_backing`, which takes the vnode's attributes for
                // changed
                let free = match crate::fs::registry::with_mounted(fs_type, slot, |fs| fs.statfs()) {
                    Some(Ok(st)) if st.flags & 1 == 0 => st.free_blocks.saturating_mul(st.block_size as u64),
                    _ => 0,
                };
                ROOM.lock().insert(key, (free, now));
                free
            }
        };
        let dirty = DIRTY_PAGES.load(core::sync::atomic::Ordering::Relaxed) * PAGE_SIZE;
        free / 2 > (dirty + len + PAGE_SIZE) as u64
    }

    /// Step 1: up to `FLUSH_PAGES` of `vid`'s pages dirtied before `before`
    fn plan_flush(&mut self, vid: usize, before: u64, hold: bool) -> Option<Flush> {
        let pages = filecache::lock().dirty_pages(vid, before, FLUSH_PAGES);
        if pages.is_empty() {
            return None;
        }
        let Some((fs_type, slot, node)) = self.backing_of(vid) else {
            // Nowhere to write them any more: a mount that went
            crate::kwarn!("[writeback] vnode {} has no filesystem: {} dirty page(s) dropped", vid, pages.len());
            filecache::lock().undirty_from(vid, 0);
            return None;
        };
        if hold {
            self.nodes[vid].inc_ref();
        }
        Some(Flush { vid, fs_type, slot, node, mtime: self.nodes[vid].mtime, pages, held: hold })
    }

    /// Step 3: what reached the disk is clean. A failed write-out counts its
    /// pages as written too, as on Linux: the cache still reads them, and the
    /// next `fsync` on the file returns the error
    fn finish_flush(&mut self, f: &Flush, written: &[(u32, u64)], err: Option<VfsError>) {
        {
            let mut fc = filecache::lock();
            fc.clean(f.vid, written);
            if let Some(e) = err {
                let lost: Vec<(u32, u64)> =
                    f.pages.iter().filter_map(|&p| fc.dirty_seq(f.vid, p).map(|s| (p, s))).collect();
                fc.clean(f.vid, &lost);
                fc.entries[f.vid].wb_error = Some(e);
                crate::kwarn!("[writeback] vnode {}: write-out failed ({:?}), {} page(s) not on the disk", f.vid, e, lost.len());
            }
        }
        if f.held {
            self.drop_ref(f.vid);
        }
    }

    /// `vid`'s dirty pages on the disk now, holding the VFS lock: for a reader
    /// about to go around the cache, and a truncate
    pub(super) fn settle(&mut self, vid: usize) {
        self.finish_late_truncate();
        let before = {
            let fc = filecache::lock();
            if !fc.is_dirty(vid) {
                return;
            }
            fc.write_mark()
        };
        while let Some(f) = self.plan_flush(vid, before, false) {
            let (written, err) = write_out(&f);
            self.finish_flush(&f, &written, err);
            if err.is_some() {
                break;
            }
        }
    }

    /// Whether `vid` has pages not on the disk: its size and mtime are the
    /// VFS's then, and the filesystem's are older
    pub(super) fn has_dirty_pages(&self, vid: usize) -> bool {
        filecache::lock().is_dirty(vid)
    }
}

/// Step 2. The pages written, each with the sequence it was copied at, and
/// the error that stopped it if one did
fn write_out(f: &Flush) -> (Vec<(u32, u64)>, Option<VfsError>) {
    let mut written = Vec::with_capacity(f.pages.len());
    let mut frames: Vec<u64> = Vec::with_capacity(RUN_PAGES);
    let mut i = 0;
    while i < f.pages.len() {
        let mut j = i + 1;
        while j < f.pages.len() && j - i < RUN_PAGES && f.pages[j] == f.pages[j - 1] + 1 {
            j += 1;
        }
        let run = &f.pages[i..j];
        i = j;
        // The filesystem's lock a run at a time: whatever else wants that disk
        // (a create under the VFS lock) waits for one run, not the whole flush
        let r = crate::fs::registry::with_mounted(f.fs_type, f.slot, |fs| {
            write_run(fs, f, run, &mut frames, &mut written)
        });
        match r {
            Some(Ok(())) => {}
            Some(Err(e)) => return (written, Some(e)),
            None => return (written, Some(VfsError::NotMounted)),
        }
    }
    // The time the writes were made, not the time they reached the disk
    if !written.is_empty() {
        let set = crate::fs::registry::with_mounted(f.fs_type, f.slot, |fs| {
            let _ = fs.setattr(f.node, &FsSetAttr { mtime: Some(f.mtime), ..FsSetAttr::default() });
        });
        if set.is_none() {
            return (written, Some(VfsError::NotMounted));
        }
    }
    (written, None)
}

/// One run of contiguous pages, under the filesystem's lock: what they are
/// now, pinned, and handed to `write_pages`. The size is read and the pages
/// written under one hold of that lock, so a truncate's own lands before or
/// after the run, never inside it
fn write_run(
    fs: &mut dyn crate::fs::vfs::fsops::FileSystem,
    f: &Flush,
    run: &[u32],
    frames: &mut Vec<u64>,
    written: &mut Vec<(u32, u64)>,
) -> VfsResult<()> {
    let start = run[0] as u64 * PAGE_SIZE as u64;
    // As the pages are now, each still the cache's frame: one a truncate or a
    // drop took is not this file's any more. Pinned, so a frame the cache
    // releases meanwhile can still be read; a write landing meanwhile moves its
    // page's number
    frames.clear();
    let (len, seqs) = {
        let mut fc = filecache::lock();
        let size = fc.entries[f.vid].size;
        let mut seqs = Vec::with_capacity(run.len());
        if start >= size {
            // Past the end: cut off, nothing to write
            seqs.extend(run.iter().filter_map(|&p| fc.dirty_seq(f.vid, p).map(|s| (p, s))));
            written.extend(seqs);
            return Ok(());
        }
        for &p in run {
            let (Some(seq), Some(phys)) = (fc.dirty_seq(f.vid, p), fc.frame(f.vid, p as usize)) else {
                break;
            };
            if !crate::mm::pmm::try_ref(phys) {
                break;
            }
            frames.push(phys);
            seqs.push((p, seq));
        }
        let len = ((seqs.len() * PAGE_SIZE) as u64).min(size - start) as usize;
        (len, seqs)
    };
    if seqs.is_empty() {
        return Ok(());
    }
    let pages: Vec<&[u8]> = frames
        .iter()
        .enumerate()
        .map(|(k, &phys)| {
            let n = (len - (k * PAGE_SIZE).min(len)).min(PAGE_SIZE);
            unsafe { core::slice::from_raw_parts(super::cached::hhdm_ptr(phys), n) }
        })
        .filter(|p| !p.is_empty())
        .collect();
    let r = fs.write_pages(f.node, start, &pages);
    drop(pages);
    for &phys in frames.iter() {
        crate::mm::pmm::free_frame_cow(phys);
    }
    r?;
    written.extend(seqs);
    Ok(())
}

/// `vid`'s pages dirty now, on the disk: the flush's steps 1-3 until none is
/// left, with the VFS lock released for each write. It stops at what was dirty
/// when it began, however fast a writer adds pages
pub fn flush_vnode(vid: usize) -> VfsResult<()> {
    let before = filecache::lock().write_mark();
    loop {
        let Some(f) = super::with_vfs(|vfs| vfs.plan_flush(vid, before, true)) else { return Ok(()) };
        let (written, err) = write_out(&f);
        super::with_vfs(|vfs| vfs.finish_flush(&f, &written, err));
        if let Some(e) = err {
            return Err(e);
        }
    }
}

/// `fsync`'s first half: the descriptor's file written out, and a write-out
/// that failed since the last `fsync` reported once
pub fn flush_fd(fd: usize) -> VfsResult<()> {
    let vid = crate::fs::vfs::fd::peek(fd).ok_or(VfsError::BadFd)?.vnode_id as usize;
    flush_vnode(vid)?;
    let mut fc = filecache::lock();
    // Looked at before it is taken: indexing for a write makes an entry, and
    // a tmpfs file has none
    if fc.entries[vid].wb_error.is_none() {
        return Ok(());
    }
    fc.entries[vid].wb_error.take().map_or(Ok(()), Err)
}

/// Every dirty vnode written out: sync, unmount, power-off
pub fn writeback_all() {
    let vids = filecache::lock().dirty_since(0);
    for vid in vids {
        let _ = flush_vnode(vid as usize);
    }
}

/// bdflush: the vnodes dirty for `DIRTY_AGE_NS` written out
pub fn writeback_old() {
    if DIRTY_PAGES.load(core::sync::atomic::Ordering::Relaxed) == 0 {
        return;
    }
    let cutoff = crate::kcore::time::now_ns().saturating_sub(DIRTY_AGE_NS);
    let vids = filecache::lock().dirty_since(cutoff.max(1));
    for vid in vids {
        let _ = flush_vnode(vid as usize);
    }
}

/// After a write: over the dirty limit, the oldest vnodes written out until
/// half of it is left. The writer pays, as in Linux's `balance_dirty_pages`, so
/// a program writing faster than the disk cannot fill memory
pub fn balance() {
    let limit = filecache::dirty_limit();
    if DIRTY_PAGES.load(core::sync::atomic::Ordering::Relaxed) <= limit {
        return;
    }
    let vids = filecache::lock().dirty_since(0);
    for vid in vids {
        if DIRTY_PAGES.load(core::sync::atomic::Ordering::Relaxed) <= limit / 2 {
            break;
        }
        let _ = flush_vnode(vid as usize);
    }
}
