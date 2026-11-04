// File truncation and page eviction

use super::MikuVFS;
use crate::fs::vfs::address_space::AddressSpace;
use crate::fs::vfs::fsops::{FileSystem, NodeId};
use crate::fs::vfs::types::*;

/// An `O_TRUNC` of a big file on ext, done in the VFS and not yet on the disk
/// (`dev/vfs-locking.md`, "Two more long holds"). `with_vfs` takes ext's lock
/// before releasing its own and finishes it with that alone: a flush, a write
/// through, a fill or a `getattr` of that disk comes after
pub struct LateTruncate {
    slot: u8,
    node: NodeId,
}

impl MikuVFS {
    /// A pending `LateTruncate` done now, under the VFS lock: before anything
    /// else in the same call reaches a disk, which would otherwise land
    /// before the truncate and be cut by it
    pub(super) fn finish_late_truncate(&self) {
        if let Some(t) = self.late_truncate.take() {
            let mut state = crate::fs::ext::mount::STATE.lock();
            t.finish(&mut state);
        }
    }
}

impl LateTruncate {
    /// With ext's lock already held (`with_vfs`)
    pub(super) fn finish(self, state: &mut crate::fs::ext::mount::ExtFsState) {
        let slot = self.slot as usize;
        if slot >= crate::fs::ext::mount::MAX_MOUNTS || !state.ready[slot] {
            return;
        }
        if let Err(e) = FileSystem::truncate(&mut state.slots[slot], self.node, 0) {
            crate::kerror!("[vfs] O_TRUNC of inode {} on ext slot {} failed after open: {:?}", self.node.as_ino(), slot, e);
        }
    }
}

impl MikuVFS {
    /// Every page of a tmpfs file back to the cache, the index with them. A
    /// page still mapped stays its mapper's until they unmap it (`vfs::pages`)
    pub fn free_file_pages(&mut self, id: usize) {
        // What the page cache holds of it, when it is a file on a disk, dirty
        // pages too: the file is going, or being emptied
        crate::fs::vfs::filecache::lock().discard(id);
        self.detach_pages(id);
    }

    /// Truncate to nothing: `O_TRUNC` on open. The pages are dropped only once
    /// the disk agrees
    pub(super) fn truncate_file(&mut self, id: usize) -> Result<(), VfsError> {
        // A big file on ext: the disk's half after the VFS lock, with ext's
        // lock taken before it is released (`LateTruncate`)
        self.finish_late_truncate();
        if self.nodes[id].is_regular()
            && self.nodes[id].size >= super::unlinked::DRAIN_MIN
            && self.nodes[id].fs_type.is_ext_family()
        {
            if let Some((_, slot, node)) = self.backing_of(id) {
                self.late_truncate.set(Some(LateTruncate { slot, node }));
                self.free_file_pages(id);
                crate::fs::vfs::filecache::lock().resize(id, 0);
                self.nodes[id].size = 0;
                self.nodes[id].addr_space = AddressSpace::new();
                let now = self.now();
                self.nodes[id].touch_mtime(now);
                crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_MODIFY);
                return Ok(());
            }
        }
        match self.with_backing(id, |fs, node| fs.truncate(node, 0)) {
            Some(Ok(())) => {}
            Some(Err(e)) => return Err(e),
            // No filesystem behind it: tmpfs, where emptying the file is the
            // page work below
            None => {}
        }
        self.free_file_pages(id);
        crate::fs::vfs::filecache::lock().resize(id, 0);
        self.nodes[id].size = 0;
        self.nodes[id].addr_space = AddressSpace::new();
        let now = self.now();
        self.nodes[id].touch_mtime(now);
        crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_MODIFY);
        Ok(())
    }

    /// Set a file's length, and tell whoever watches it
    pub fn truncate_to(&mut self, id: usize, new_size: u64) -> Result<(), VfsError> {
        self.set_length(id, new_size)?;
        crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_MODIFY);
        Ok(())
    }

    fn set_length(&mut self, id: usize, new_size: u64) -> Result<(), VfsError> {
        let old_size = self.nodes[id].size;

        // An ext-backed file goes to the driver for every length: growing sets
        // i_size on disk and leaves the gap sparse, as a program preallocating
        // by seeking expects. Only the tmpfs path below settles a grow by
        // moving a number in memory. The cached pages of a disk file go at any
        // length: a page straddling the new end would keep the bytes past it.
        // Dirty ones past the new end are not written; the rest are, before the
        // cut
        crate::fs::vfs::filecache::lock().undirty_from(id, new_size);
        self.settle(id);
        crate::fs::vfs::filecache::lock().drop_file(id);
        match self.with_backing(id, |fs, node| fs.truncate(node, new_size)) {
            Some(Ok(())) => {
                self.nodes[id].size = new_size;
                crate::fs::vfs::filecache::lock().resize(id, new_size);
                let ts = self.now();
                self.nodes[id].touch_mtime(ts);
                self.nodes[id].touch_ctime(ts);
                return Ok(());
            }
            Some(Err(e)) => {
                crate::serial_println!("[vfs] truncate of vnode {} failed: {:?}", id, e);
                return Err(e);
            }
            None => {}
        }

        if new_size >= old_size {
            self.nodes[id].size = new_size;
            return Ok(());
        }

        let keep_pages = if new_size == 0 {
            0
        } else {
            ((new_size as usize - 1) / PAGE_SIZE) + 1
        };

        let mut space = self.nodes[id].addr_space;
        for pid in space.truncate(&mut self.page_cache, keep_pages) {
            self.page_cache.free_page(pid);
        }
        self.nodes[id].addr_space = space;

        if new_size > 0 {
            let last_page = (new_size as usize - 1) / PAGE_SIZE;
            let zero_from = new_size as usize % PAGE_SIZE;
            if zero_from > 0 {
                if let Some(pid) = self.nodes[id].addr_space.get_page(&mut self.page_cache, last_page) {
                    if let Some(data) = self.page_cache.get_page_data_mut(pid) {
                        data[zero_from..].fill(0);
                    }
                }
            }
        }

        self.nodes[id].size = new_size;
        let ts = self.now();
        self.nodes[id].touch_mtime(ts);
        self.nodes[id].touch_ctime(ts);
        Ok(())
    }
}
