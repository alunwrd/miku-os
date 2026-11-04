// btrfs as an implementation of `FileSystem`
//
// Read-only. Every mutating method returns `ReadOnly`, not `NotSupported`:
// btrfs supports the operation, and this driver does not write.
//
// A btrfs node is named by `(tree, objectid)`: the subvolume it lives in and
// its number inside that subvolume. `NodeId` has two words for this, but a
// `VNode` has one (`ext2_ino`), so the tree word arrives as zero. Tree 0 does
// not exist, and it means the default subvolume, the only one this driver
// reads: following a name into another subvolume is a declared limit.
// `tree_or_default` is the one place that decides it.

use crate::fs::vfs::fsops::{FileSystem, FsSetAttr, NodeId, NodeMeta, RawDirent};
use crate::fs::vfs::types::*;
use btrfs::fs_tree::*;
use btrfs::Volume;

use crate::fs::btrfs::mount::BtrfsVolume;

/// The `FT_*` a directory entry carries, as the VFS names the same thing
fn kind_of_ft(ft: u8) -> VNodeKind {
    match ft {
        FT_DIR => VNodeKind::Directory,
        FT_SYMLINK => VNodeKind::Symlink,
        FT_CHRDEV => VNodeKind::CharDevice,
        FT_BLKDEV => VNodeKind::BlockDevice,
        FT_FIFO => VNodeKind::Fifo,
        FT_SOCK => VNodeKind::Socket,
        _ => VNodeKind::Regular,
    }
}

fn kind_of_mode(mode: u32) -> VNodeKind {
    match mode & 0xF000 {
        0x4000 => VNodeKind::Directory,
        0xA000 => VNodeKind::Symlink,
        0x2000 => VNodeKind::CharDevice,
        0x6000 => VNodeKind::BlockDevice,
        0x1000 => VNodeKind::Fifo,
        0xC000 => VNodeKind::Socket,
        _ => VNodeKind::Regular,
    }
}

fn meta_of(id: NodeId, ino: &btrfs::Inode) -> NodeMeta {
    NodeMeta {
        id,
        kind: kind_of_mode(ino.mode),
        mode: ino.permissions(),
        uid: ino.uid,
        gid: ino.gid,
        size: ino.size,
        blocks: ino.nbytes / 512,
        // btrfs keeps the link count as 32 bits and the VFS as 16. Saturating:
        // a file with 70000 links truncated to 4464 could reach zero while
        // still named
        nlinks: ino.nlink.min(u16::MAX as u32) as u16,
        atime: ino.atime.0,
        mtime: ino.mtime.0,
        ctime: ino.ctime.0,
        rdev: (((ino.rdev >> 20) & 0xFFF) as u32, (ino.rdev & 0xFFFFF) as u32),
    }
}

impl FileSystem for BtrfsVolume {
    // identity

    fn fs_type(&self) -> FsType {
        FsType::Btrfs
    }

    /// Always. This driver does not call the transaction code in `lib/btrfs`
    fn read_only(&self) -> bool {
        true
    }

    fn root(&self) -> NodeId {
        // Zero in both words: the default subvolume's top directory, resolved
        // by the methods below. Finding it reads the disk, and this takes
        // `&self`
        NodeId::NONE
    }

    fn statfs(&mut self) -> VfsResult<StatFs> {
        let g = self.geometry;
        Ok(StatFs {
            fs_type: FsType::Btrfs,
            block_size: g.sectorsize,
            total_blocks: g.total_bytes / g.sectorsize as u64,
            // Free space is in the space-info and block-group accounting, which
            // a reader does not maintain. Zero, not a guess for `df`
            free_blocks: 0,
            total_inodes: 0,
            free_inodes: 0,
            max_name_len: NAME_LEN_MAX as u32,
            flags: 1,
        })
    }

    fn sync(&mut self) -> VfsResult<()> {
        // Nothing is ever staged
        Ok(())
    }

    // namespace

    fn lookup(&mut self, dir: NodeId, name: &str) -> VfsResult<NodeMeta> {
        let tree = self.tree_or_default(dir.tree)?;
        let dir_node = if dir.node == 0 { self.fs_root()?.root_dirid } else { dir.node };
        let entry = Volume::lookup(self, tree, dir_node, name.as_bytes())?;

        // A name may point at another subvolume, not at an inode here.
        // Following it means reading that subvolume's root and continuing in
        // its tree, which this driver does not do; refused by name
        if !entry.points_at_inode() {
            return Err(VfsError::NotSupported);
        }
        let id = NodeId { tree: dir.tree, node: entry.location.objectid };
        let ino = Volume::inode(self, tree, id.node)?;
        Ok(meta_of(id, &ino))
    }

    fn readdir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(RawDirent<'_>) -> bool,
    ) -> VfsResult<()> {
        let tree = self.tree_or_default(dir.tree)?;
        let dir_node = if dir.node == 0 { self.fs_root()?.root_dirid } else { dir.node };
        Volume::readdir(self, tree, dir_node, &mut |e| {
            Ok(f(RawDirent {
                name: &e.name,
                id: NodeId { tree: dir.tree, node: e.location.objectid },
                kind: kind_of_ft(e.ftype),
            }))
        })?;
        Ok(())
    }

    fn create(
        &mut self,
        _dir: NodeId,
        _name: &str,
        _kind: VNodeKind,
        _mode: u16,
        _uid: u32,
        _gid: u32,
        _rdev: (u32, u32),
        _target: Option<&str>,
    ) -> VfsResult<NodeMeta> {
        Err(VfsError::ReadOnly)
    }

    fn link(&mut self, _dir: NodeId, _name: &str, _target: NodeId) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }

    fn unlink(&mut self, _dir: NodeId, _name: &str, _is_dir: bool) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }

    fn rename(
        &mut self,
        _old_dir: NodeId,
        _old: &str,
        _new_dir: NodeId,
        _new: &str,
    ) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }

    // data

    fn read_at(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let tree = self.tree_or_default(id.tree)?;
        Ok(Volume::read_file(self, tree, id.node, offset, buf)?)
    }

    fn write_at(&mut self, _id: NodeId, _offset: u64, _data: &[u8]) -> VfsResult<usize> {
        Err(VfsError::ReadOnly)
    }

    fn truncate(&mut self, _id: NodeId, _size: u64) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }

    fn readlink(&mut self, id: NodeId, buf: &mut [u8]) -> VfsResult<usize> {
        // A symlink's target is its file contents, as for a slow symlink on ext
        let tree = self.tree_or_default(id.tree)?;
        Ok(Volume::read_file(self, tree, id.node, 0, buf)?)
    }

    // attributes

    fn getattr(&mut self, id: NodeId) -> VfsResult<NodeMeta> {
        let tree = self.tree_or_default(id.tree)?;
        let node = if id.node == 0 { self.fs_root()?.root_dirid } else { id.node };
        let ino = Volume::inode(self, tree, node)?;
        Ok(meta_of(NodeId { tree: id.tree, node }, &ino))
    }

    fn setattr(&mut self, _id: NodeId, _attr: &FsSetAttr) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }

    fn touch_atime(&mut self, _id: NodeId) {
        // A read-only mount does not record reads, and `relatime` on it is not
        // a failure
    }

    // optional

    fn fsync(&mut self, _id: NodeId) -> VfsResult<()> {
        Ok(())
    }

    fn getxattr(&mut self, id: NodeId, name: &str, buf: &mut [u8]) -> VfsResult<usize> {
        let mut found: Option<usize> = None;
        let tree = self.tree_or_default(id.tree)?;
        Volume::xattrs(self, tree, id.node, &mut |e| {
            if e.name == name.as_bytes() {
                let n = e.data.len().min(buf.len());
                buf[..n].copy_from_slice(&e.data[..n]);
                found = Some(e.data.len());
                return Ok(false);
            }
            Ok(true)
        })?;
        match found {
            Some(len) if len > buf.len() => Err(VfsError::NoSpace),
            Some(len) => Ok(len),
            None => Err(VfsError::NotFound),
        }
    }

    fn setxattr(&mut self, _id: NodeId, _name: &str, _value: &[u8]) -> VfsResult<()> {
        Err(VfsError::ReadOnly)
    }
}

/// The one mapping from `BtrfsError` to `VfsError`.
///
/// `Corrupt` and `ChecksumMismatch` stay distinct upwards: the first says a
/// structure described nothing, the second that the bytes were not the bytes
/// written
impl From<btrfs::BtrfsError> for VfsError {
    fn from(e: btrfs::BtrfsError) -> Self {
        use btrfs::BtrfsError as B;
        match e {
            B::Io => VfsError::IoError,
            B::Corrupt => VfsError::Corrupted,
            B::ChecksumMismatch => VfsError::Corrupted,
            B::Unsupported => VfsError::NotSupported,
            B::NotFound => VfsError::NotFound,
            B::ReadOnly => VfsError::ReadOnly,
            B::InvalidArg => VfsError::InvalidArgument,
            B::OutOfMemory => VfsError::OutOfMemory,
        }
    }
}
