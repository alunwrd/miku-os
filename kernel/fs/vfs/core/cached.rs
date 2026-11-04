// Reading and writing through the page cache for files on a disk
//
// `filecache.rs` is the table; this is what the VFS does with it. A read
// finding every page it covers copies them under the VFS lock, asking the
// filesystem nothing. Otherwise it is three steps (`dev/fs-architecture.md`,
// "The page cache for files on a disk"):
//
//   1. `cache_plan`, under the lock: which pages to read, a reference on the
//      vnode so it outlives the lock, and the generation it was planned under;
//   2. `fill`, without it: the pages read through the filesystem, under the
//      filesystem's own lock, into frames nobody else can see yet;
//   3. `cache_insert`, under the lock again: in, if the vnode is still the
//      same node on the same mount and nothing has changed the file since.
//
// The callers loop on 1-3 until the read hits (`core::read_fd`,
// `core::read_at_vnode`). A write is dirty pages first (`core/writeback.rs`);
// one that goes to the filesystem directly brings the cache's copy up to date
// (`cache_written`).
//
// Before any of that, a descriptor read tries `read_hit`: the same hit with no
// VFS lock, under the cache's own (`dev/vfs-locking.md`, "4b").

extern crate alloc;

use alloc::vec::Vec;

use super::MikuVFS;
use crate::fs::vfs::address_space::AddressSpace;
use crate::fs::vfs::filecache::{self, MAX_FILL_PAGES, READAHEAD_PAGES};
use crate::fs::vfs::fsops::NodeId;
use crate::fs::vfs::types::*;

/// What a read outside the lock is to fetch, and what it was planned against
pub struct FillPlan {
    pub vid: usize,
    pub fs_type: FsType,
    pub slot: u8,
    pub node: NodeId,
    /// The first page, and how many after it
    pub first: usize,
    pub count: usize,
    pub gen: u32,
}

/// What a fill came back with. The frames are the caller's until
/// `cache_insert` takes or frees them
pub struct Filled {
    pub frames: Vec<u64>,
    pub size: u64,
    pub mtime: u64,
    pub ok: bool,
}

/// One step of a read through the cache: done, or a fill to make first
pub enum ReadStep {
    Done(VfsResult<usize>),
    Fill(FillPlan),
}

#[inline]
pub(super) fn hhdm_ptr(phys: u64) -> *mut u8 {
    (phys + crate::arch::x86_64::grub::hhdm()) as *mut u8
}

/// Pages a fill asks the filesystem for in one `read_at`
///
/// One page at a time is eight times slower than no cache (101 against 832 MB/s
/// from QEMU's NVMe): between two data blocks ext reads its own metadata, the
/// block cache's readahead stops seeing a stream, and every page becomes a 4
/// KiB command. A larger read keeps the stream whole; the bytes are then dealt
/// out to the pages' frames
const FILL_SLICE_PAGES: usize = 16;

/// Step 2: read `plan`'s pages through its filesystem, with no VFS lock held.
/// Stops at the end of the file as the filesystem has it now
pub fn fill(plan: &FillPlan) -> Filled {
    let mut frames = Vec::with_capacity(plan.count);
    let mut slice: Vec<u8> = Vec::new();
    let r = crate::fs::registry::with_mounted(plan.fs_type, plan.slot, |fs| -> VfsResult<(u64, u64)> {
        let meta = fs.getattr(plan.node)?;
        let start = (plan.first * PAGE_SIZE) as u64;
        let end = (((plan.first + plan.count) * PAGE_SIZE) as u64).min(meta.size);
        let mut at = start;
        while at < end {
            let len = ((end - at) as usize).min(FILL_SLICE_PAGES * PAGE_SIZE);
            if slice.len() < len {
                slice.resize(len, 0);
            }
            let mut got = 0;
            while got < len {
                match fs.read_at(plan.node, at + got as u64, &mut slice[got..len]) {
                    Ok(0) => break,
                    Ok(n) => got += n,
                    Err(e) => return Err(e),
                }
            }
            // Whole pages only, but the file's last: a page shorter than the
            // size the filesystem gave a moment ago is a file changing
            // underneath, not one to keep
            let whole = if at + got as u64 >= meta.size { got.div_ceil(PAGE_SIZE) } else { got / PAGE_SIZE };
            for p in 0..whole {
                let Some(phys) = crate::mm::pmm::alloc_frame() else { break };
                let page = unsafe { core::slice::from_raw_parts_mut(hhdm_ptr(phys), PAGE_SIZE) };
                let from = p * PAGE_SIZE;
                let n = (got - from).min(PAGE_SIZE);
                page[..n].copy_from_slice(&slice[from..from + n]);
                page[n..].fill(0);
                frames.push(phys);
            }
            if got < len || frames.len() < ((at - start) as usize + len).div_ceil(PAGE_SIZE) {
                break;
            }
            at += len as u64;
        }
        fs.touch_atime(plan.node);
        Ok((meta.size, meta.mtime))
    });
    match r {
        Some(Ok((size, mtime))) => Filled { frames, size, mtime, ok: true },
        _ => Filled { frames, size: 0, mtime: 0, ok: false },
    }
}

/// Pages one read without the VFS lock pins at most: 128 KiB. A longer read
/// takes the rest the locked way
const PIN_PAGES: usize = 32;

/// A descriptor read answered from the cache with no VFS lock: the bytes, or
/// `None` for the locked way (`core::read_fd`).
///
/// The descriptor is copied out, with no lock either (`fd::peek`); under the
/// cache's lock, a count of the vnodes freed (`filecache::FORGOTTEN`) unmoved
/// since before the copy says its vnode is still the file it named, so the
/// entry is that file's. When the count has moved the descriptor is looked at
/// again: while it names the vnode it holds a reference on it. The frames are
/// pinned there and copied from with no lock held, since a copy to a program
/// can fault. The offset moves by a swap from where this read started
/// (`fd::advance`): if another thread's read on the same descriptor went first,
/// this one starts over under the lock, so reads on one descriptor stay one
/// after another
pub fn read_hit(fd: usize, buf: &mut [u8]) -> Option<usize> {
    use crate::fs::vfs::fd::{self, Route};
    use core::sync::atomic::Ordering;
    if buf.is_empty() {
        return None;
    }
    let forgotten = filecache::FORGOTTEN.load(Ordering::Acquire);
    let (mut f, mut pos) = fd::peek_pos(fd)?;
    if f.route == Route::Unknown {
        super::route(fd)?;
        (f, pos) = fd::peek_pos(fd)?;
    }
    if f.route != (Route::Vfs { cached: true }) || !f.readable() {
        return None;
    }
    let vid = f.vnode_id as usize;
    let mut frames = [0u64; PIN_PAGES];
    let (n, got, touch) = {
        let fc = filecache::read();
        // A vnode went since the descriptor was copied: asked again whether it
        // still names this one, which then holds a reference
        if filecache::FORGOTTEN.load(Ordering::Relaxed) != forgotten
            && fd::peek(fd).map(|g| g.vnode_id) != Some(f.vnode_id)
        {
            return None;
        }
        fc.pin(vid, f.offset, buf.len(), &mut frames)?
    };
    if touch {
        filecache::lock().touch(vid);
    }
    let mut done = 0;
    for (i, &phys) in frames[..got].iter().enumerate() {
        let from = if i == 0 { (f.offset % PAGE_SIZE as u64) as usize } else { 0 };
        let chunk = (PAGE_SIZE - from).min(n - done);
        let src = unsafe { core::slice::from_raw_parts(hhdm_ptr(phys).add(from), chunk) };
        buf[done..done + chunk].copy_from_slice(src);
        done += chunk;
    }
    for &phys in &frames[..got] {
        crate::mm::pmm::free_frame_cow(phys);
    }
    if !fd::advance(fd, f.vnode_id, pos, f.offset + n as u64) {
        return None;
    }
    filecache::UNLOCKED_HITS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    Some(n)
}

impl MikuVFS {
    /// Whether `vid`'s data goes through the cache: a regular file on a
    /// filesystem with a disk under it. memfs is memory already
    pub(super) fn cacheable(&self, vid: usize) -> bool {
        // `nopagecache` on the boot line: every read of a disk file goes to the
        // filesystem under the VFS lock
        static OFF: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(2);
        let off = match OFF.load(core::sync::atomic::Ordering::Relaxed) {
            2 => {
                let v = crate::arch::x86_64::grub::cmdline_flag("nopagecache") as u8;
                OFF.store(v, core::sync::atomic::Ordering::Relaxed);
                v
            }
            v => v,
        };
        off == 0
            && self.valid_vnode(vid)
            && self.nodes[vid].is_regular()
            && self.nodes[vid].ext2_ino != 0
            && matches!(self.nodes[vid].fs_type, FsType::Ext2 | FsType::Ext3 | FsType::Ext4 | FsType::Btrfs)
    }

    /// The bytes at `offset` from the cache alone: `Some(n)` when every page
    /// the read covers is in (n is short only at the end of the file),
    /// `None` when one is not
    pub(super) fn cache_copy(&mut self, vid: usize, offset: u64, buf: &mut [u8]) -> Option<usize> {
        let size = self.nodes[vid].size;
        if offset >= size || buf.is_empty() {
            return Some(0);
        }
        let n = buf.len().min((size - offset) as usize);
        let first = (offset / PAGE_SIZE as u64) as usize;
        let last = ((offset + n as u64 - 1) / PAGE_SIZE as u64) as usize;
        let mut fc = filecache::lock();
        for page in first..=last {
            if fc.frame(vid, page).is_none() {
                fc.misses.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                return None;
            }
        }
        let mut done = 0;
        while done < n {
            let at = offset + done as u64;
            let page = (at / PAGE_SIZE as u64) as usize;
            let in_page = (at % PAGE_SIZE as u64) as usize;
            let chunk = (PAGE_SIZE - in_page).min(n - done);
            let phys = fc.frame(vid, page)?;
            let src = unsafe { core::slice::from_raw_parts(hhdm_ptr(phys).add(in_page), chunk) };
            buf[done..done + chunk].copy_from_slice(src);
            done += chunk;
        }
        fc.hits.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        fc.touch(vid);
        fc.entries[vid].next_page.store(last as u64 + 1, core::sync::atomic::Ordering::Relaxed);
        Some(n)
    }

    /// Step 1: the fill a miss at `offset` needs, with a reference on the
    /// vnode for it to hold across the unlocked read. `None` when the vnode
    /// has no filesystem behind it after all
    pub(super) fn cache_plan(&mut self, vid: usize, offset: u64, len: usize) -> Option<FillPlan> {
        let (fs_type, slot, node) = self.backing_of(vid)?;
        let first_req = (offset / PAGE_SIZE as u64) as usize;
        let last_req = ((offset + len.max(1) as u64 - 1) / PAGE_SIZE as u64) as usize;
        let mut first = first_req;
        let (gen, next_page) = {
            let mut fc = filecache::lock();
            while first < last_req && fc.frame(vid, first).is_some() {
                first += 1;
            }
            let e = &fc.entries[vid];
            (e.gen, e.next_page.load(core::sync::atomic::Ordering::Relaxed))
        };
        // Where the last read ended, or the start of the file: a stream
        let sequential = first_req as u64 == next_page || offset == 0;
        let mut end = last_req + 1;
        if sequential {
            end += READAHEAD_PAGES;
        }
        // Not past the end of the file as the vnode has it: the fill stops at
        // the filesystem's own end anyway, and frames read for nothing are
        // frames lost to others
        let file_pages = AddressSpace::pages_for_size(self.nodes[vid].size).max(last_req + 1);
        let end = end.min(first + MAX_FILL_PAGES).min(file_pages).max(first + 1);
        self.nodes[vid].inc_ref();
        Some(FillPlan { vid, fs_type, slot, node, first, count: end - first, gen })
    }

    /// Step 3: what `fill` read goes in, if the vnode is still the node it was
    /// planned for and its generation has not moved, and the plan's reference
    /// is given back. Frames that do not go in go back to the allocator
    pub(super) fn cache_insert(&mut self, plan: &FillPlan, filled: Filled) {
        let same = self.valid_vnode(plan.vid)
            && self.backing_of(plan.vid) == Some((plan.fs_type, plan.slot, plan.node));
        let mut taken = 0;
        {
            let mut fc = filecache::lock();
            if same && fc.entries[plan.vid].gen == plan.gen && filled.ok {
                // The fill read the file, which may have moved its atime
                self.nodes[plan.vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
                // A dirty file's end is the VFS's: the disk's is older
                if !fc.is_dirty(plan.vid) {
                    self.nodes[plan.vid].size = filled.size;
                    let e = &mut fc.entries[plan.vid];
                    e.stamp = Some((filled.size, filled.mtime));
                    e.size = filled.size;
                }
                if fc.make_room(filled.frames.len(), plan.vid) {
                    for (i, &phys) in filled.frames.iter().enumerate() {
                        if fc.insert(plan.vid, plan.first + i, phys).is_err() {
                            crate::mm::pmm::free_frame(phys);
                        }
                        taken += 1;
                    }
                }
                fc.fills += 1;
            }
        }
        for &phys in &filled.frames[taken..] {
            crate::mm::pmm::free_frame(phys);
        }
        // Last: it may be the last reference to a file unlinked meanwhile,
        // and freeing that takes its entry with it
        self.drop_ref(plan.vid);
    }

    /// A descriptor read, as far as the lock alone can take it
    pub(super) fn read_step(&mut self, fd: usize, buf: &mut [u8]) -> ReadStep {
        let (vid, offset, readable) = match self.fds().get(fd) {
            Ok(f) => (f.vnode_id as usize, f.offset, f.flags.readable()),
            Err(e) => return ReadStep::Done(Err(e)),
        };
        if !readable || buf.is_empty() || !self.cacheable(vid) {
            return ReadStep::Done(self.read(fd, buf));
        }
        if let Some(n) = self.cache_copy(vid, offset, buf) {
            if let Ok(mut f) = self.fds().get_mut(fd) {
                f.offset += n as u64;
            }
            return ReadStep::Done(Ok(n));
        }
        match self.cache_plan(vid, offset, buf.len()) {
            Some(plan) => ReadStep::Fill(plan),
            None => ReadStep::Done(self.read(fd, buf)),
        }
    }

    /// `read_at_vnode`, as far as the lock alone can take it
    pub(super) fn read_at_step(&mut self, vid: usize, offset: u64, buf: &mut [u8]) -> ReadStep {
        if buf.is_empty() || !self.cacheable(vid) {
            return ReadStep::Done(self.read_at_vnode(vid, offset, buf));
        }
        if let Some(n) = self.cache_copy(vid, offset, buf) {
            return ReadStep::Done(Ok(n));
        }
        match self.cache_plan(vid, offset, buf.len()) {
            Some(plan) => ReadStep::Fill(plan),
            None => ReadStep::Done(self.read_at_vnode(vid, offset, buf)),
        }
    }

    /// The filesystem took `data` at `offset` (or refused it, `ok` false):
    /// the cached pages it covers made to match, and any fill under way told
    /// not to go in
    pub(super) fn cache_written(&mut self, vid: usize, offset: u64, data: &[u8], ok: bool) {
        let mut fc = filecache::lock();
        if !self.nodes.contains(vid) || fc.entries[vid].is_empty() {
            if self.nodes.contains(vid) {
                let e = &mut fc.entries[vid];
                e.gen = e.gen.wrapping_add(1);
            }
            return;
        }
        if !ok {
            fc.drop_file(vid);
            return;
        }
        let e = &mut fc.entries[vid];
        e.gen = e.gen.wrapping_add(1);
        // Right by construction now, whatever the filesystem's clock says
        e.stamp = None;
        // What a read without the VFS lock ends at: past the old end if the
        // write went past it
        e.size = e.size.max(offset + data.len() as u64);
        let mut done = 0;
        while done < data.len() {
            let at = offset + done as u64;
            let page = (at / PAGE_SIZE as u64) as usize;
            let in_page = (at % PAGE_SIZE as u64) as usize;
            let chunk = (PAGE_SIZE - in_page).min(data.len() - done);
            if let Some(phys) = fc.frame(vid, page) {
                let dst = unsafe { core::slice::from_raw_parts_mut(hhdm_ptr(phys).add(in_page), chunk) };
                dst.copy_from_slice(&data[done..done + chunk]);
            }
            done += chunk;
        }
    }

    /// At `open`: the filesystem's size and mtime against what they were when
    /// the cached pages were read. A difference is a write that went around the
    /// VFS, and the pages go.
    ///
    /// The vnode's size is the filesystem's after this either way, cached pages
    /// or none: a read that hits decides the end of the file from it. Not every
    /// open: what `stat` loaded is the filesystem's until something asks the
    /// filesystem about the file again, or changes it around the VFS
    /// (`VNode::attrs_fresh`).
    ///
    /// `cache_validate` for an open under the VFS lock held to read: done if
    /// the vnode's attributes are the filesystem's already, and `false` when
    /// they would have to be asked for (the open goes the locked way)
    pub(super) fn cache_validate_fresh(&self, vid: usize) -> bool {
        if !self.cacheable(vid) {
            return true;
        }
        if !self.nodes[vid].attrs_fresh.load(core::sync::atomic::Ordering::Relaxed) {
            return false;
        }
        let now = (self.nodes[vid].size, self.nodes[vid].mtime);
        // Looked at held to read first: an open of a file whose pages match it,
        // or that has none, changes nothing
        {
            let fc = filecache::read();
            let e = &fc.entries[vid];
            if e.is_empty() || (e.stamp == Some(now) && e.size == now.0) {
                return true;
            }
        }
        let mut fc = filecache::lock();
        if fc.entries[vid].is_empty() {
            return true;
        }
        match fc.entries[vid].stamp {
            Some(was) if was != now => fc.drop_file(vid),
            _ => {
                let e = &mut fc.entries[vid];
                e.stamp = Some(now);
                e.size = now.0;
            }
        }
        true
    }

    pub(super) fn cache_validate(&mut self, vid: usize) {
        if !self.cacheable(vid) {
            return;
        }
        self.refresh_attrs_from_fs(vid);
        if !self.nodes[vid].attrs_fresh.load(core::sync::atomic::Ordering::Relaxed) {
            filecache::lock().drop_file(vid);
            return;
        }
        let now = (self.nodes[vid].size, self.nodes[vid].mtime);
        let mut fc = filecache::lock();
        if fc.entries[vid].is_empty() {
            return;
        }
        match fc.entries[vid].stamp {
            Some(was) if was != now => fc.drop_file(vid),
            _ => {
                let e = &mut fc.entries[vid];
                e.stamp = Some(now);
                e.size = now.0;
            }
        }
    }
}
