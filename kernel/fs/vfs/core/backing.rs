// Which filesystem is behind a vnode, and the one way to reach it
//
// `None` here does not mean "no filesystem and therefore an error": it means
// tmpfs, where the vnode table is the storage and there is nothing to forward
// to. So this returns an `Option`, not a `VfsResult`, and every caller keeps
// its in-memory branch.

use super::MikuVFS;
use crate::fs::vfs::fsops::{FileSystem, NodeId};
use crate::fs::vfs::types::*;

impl MikuVFS {
    /// The id this vnode has on its filesystem. `NONE` for a vnode that has
    /// no filesystem behind it
    pub(super) fn node_id_of(&self, vid: usize) -> NodeId {
        if !self.nodes.contains(vid) || !self.nodes[vid].fs_type.has_mounted_fs() {
            return NodeId::NONE;
        }
        NodeId::ino(self.nodes[vid].ext2_ino)
    }

    /// Whether this directory's children have all been read in.
    ///
    /// For the selftest: the flag separates "the tree has the whole directory"
    /// from "the tree will ask the disk again"
    pub fn children_are_loaded(&self, vid: usize) -> bool {
        self.nodes.contains(vid) && self.nodes[vid].children_loaded
    }

    /// Whether a filesystem stands behind this vnode at all: the question every
    /// routing decision in the VFS asks
    pub(super) fn has_backing(&self, vid: usize) -> bool {
        self.backing_of(vid).is_some()
    }

    /// Which mount, and which node on it.
    pub(super) fn backing_of(&self, vid: usize) -> Option<(FsType, u8, NodeId)> {
        if !self.nodes.contains(vid) {
            return None;
        }
        let node = &self.nodes[vid];
        if !node.fs_type.has_mounted_fs() || node.ext2_ino == 0 {
            return None;
        }
        Some((node.fs_type, node.ext2_slot, NodeId::ino(node.ext2_ino)))
    }

    /// Run `f` against the filesystem behind `vid`.
    ///
    /// A closure, not `fn backing(&mut self) -> Option<&mut dyn FileSystem>`: a
    /// mounted filesystem lives in a table behind its own lock, not inside
    /// `MikuVFS`, so a borrow of it cannot outlive the guard. That lock is held
    /// across block I/O.
    ///
    /// `None` means the vnode has no filesystem behind it (tmpfs, devfs,
    /// procfs), and each caller has an in-memory branch for it. `Some(Err(..))`
    /// is a filesystem that was asked and refused.
    ///
    /// `&self`: the filesystem being mutated is not part of `MikuVFS`, and
    /// `&mut` would stop a caller reading a vnode field during the call.
    pub(super) fn with_backing<R>(
        &self,
        vid: usize,
        f: impl FnOnce(&mut dyn FileSystem, NodeId) -> VfsResult<R>,
    ) -> Option<VfsResult<R>> {
        let (fs_type, slot, id) = self.backing_of(vid)?;
        self.finish_late_truncate();
        // Whatever this asks may change the inode: `stat` reads it again
        self.nodes[vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
        // Two different Nones. This vnode has a filesystem, so a slot that is
        // not ready answers "not mounted". Falling through to the in-memory
        // path would let a write to a file on a vanished disk land in the page
        // cache and report success
        match crate::fs::registry::with_mounted(fs_type, slot, |fs| f(fs, id)) {
            Some(r) => Some(r),
            None => Some(Err(VfsError::NotMounted)),
        }
    }

    /// `with_backing`, for an operation that names a second node on the same
    /// mount: a rename across directories, or a hard link.
    ///
    /// Both nodes must be on the same filesystem; crossing mounts is a copy,
    /// not a rename, and refusing it here stops a cross-device link
    /// half-succeeding.
    pub(super) fn with_backing_pair<R>(
        &self,
        vid: usize,
        other: usize,
        f: impl FnOnce(&mut dyn FileSystem, NodeId, NodeId) -> VfsResult<R>,
    ) -> Option<VfsResult<R>> {
        let (fs_type, slot, id) = self.backing_of(vid)?;
        let (other_type, other_slot, other_id) = self.backing_of(other)?;
        self.finish_late_truncate();
        self.nodes[vid].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
        self.nodes[other].attrs_fresh.store(false, core::sync::atomic::Ordering::Relaxed);
        if fs_type != other_type || slot != other_slot {
            return Some(Err(VfsError::CrossDevice));
        }
        match crate::fs::registry::with_mounted(fs_type, slot, |fs| f(fs, id, other_id)) {
            Some(r) => Some(r),
            None => Some(Err(VfsError::NotMounted)),
        }
    }
}
