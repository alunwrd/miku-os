// What a filesystem has to answer
//
// The model is `kernel/io/block/driver.rs`: object-safe, `&mut self`, a small
// required set, and optional methods defaulting to `NotSupported`.
//
// The list is what ext is asked, not what Linux's VFS has.

use crate::fs::vfs::types::*;

/// Where a vnode's contents live on its filesystem.
///
/// Two words. ext needs an inode number, so `tree` is zero for it and
/// `NodeId::ino` is its constructor. btrfs needs a pair: the tree an object
/// lives in (a subvolume root) and its objectid inside that tree.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct NodeId {
    pub tree: u64,
    pub node: u64,
}

impl NodeId {
    /// No node. Inode 0 does not exist in any ext filesystem, so zero is the
    /// absent value
    pub const NONE: Self = Self { tree: 0, node: 0 };

    pub const fn ino(n: u32) -> Self {
        Self { tree: 0, node: n as u64 }
    }

    pub const fn is_none(self) -> bool {
        self.node == 0
    }

    /// The inode number, for a filesystem that has them. Saturating: a silent
    /// wrap would name a different file
    pub const fn as_ino(self) -> u32 {
        if self.node > u32::MAX as u64 { 0 } else { self.node as u32 }
    }
}

/// Everything the VFS caches about a node.
///
/// `uid`/`gid` are the full 32 bits the disk can hold; folding them to what
/// this kernel's credentials can represent is the VFS's decision
/// (`cred::fold_id`), since a uid 65536 from a foreign disk folds to root.
#[derive(Clone, Copy, Debug)]
pub struct NodeMeta {
    pub id: NodeId,
    pub kind: VNodeKind,
    /// Permission bits only. The type is in `kind`
    pub mode: u16,
    pub uid: u32,
    pub gid: u32,
    pub size: u64,
    pub blocks: u64,
    pub nlinks: u16,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    /// Device number for a character or block device, `(0, 0)` otherwise
    pub rdev: (u32, u32),
}

impl NodeMeta {
    pub const fn empty() -> Self {
        Self {
            id: NodeId::NONE,
            kind: VNodeKind::Regular,
            mode: 0,
            uid: 0,
            gid: 0,
            size: 0,
            blocks: 0,
            nlinks: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            rdev: (0, 0),
        }
    }
}

/// One directory entry, borrowed from the filesystem's own buffer, up to 255
/// bytes. A name too long for the caller's table is the caller's to handle,
/// visibly.
pub struct RawDirent<'a> {
    pub name: &'a [u8],
    pub id: NodeId,
    pub kind: VNodeKind,
}

/// What `setattr` is being asked to change. `None` means leave it.
#[derive(Clone, Copy, Default)]
pub struct FsSetAttr {
    pub mode: Option<u16>,
    pub uid: Option<u32>,
    pub gid: Option<u32>,
    pub size: Option<u64>,
    pub atime: Option<u64>,
    pub mtime: Option<u64>,
}

/// A mounted filesystem.
///
/// `&mut self` throughout, like `ExtHost` one layer down: an implementation
/// that caches blocks, counts I/O or holds a transaction open needs no interior
/// mutability.
pub trait FileSystem {
    // identity

    fn fs_type(&self) -> FsType;

    /// True when this mount refuses writes. Every mutating method below agrees,
    /// returning `ReadOnly`, not `NotSupported`
    fn read_only(&self) -> bool;

    fn root(&self) -> NodeId;

    fn statfs(&mut self) -> VfsResult<StatFs>;

    /// Everything staged in memory onto the medium.
    fn sync(&mut self) -> VfsResult<()>;

    // namespace

    fn lookup(&mut self, dir: NodeId, name: &str) -> VfsResult<NodeMeta>;

    /// Hand every entry of `dir` to `f`, stopping early when it returns false.
    ///
    /// A callback, not `&mut [DirEntry]`: a fixed array puts a large buffer on
    /// the kernel stack and stops at its last entry. An implementation must not
    /// stop early by itself; running out of room is the caller's decision
    fn readdir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(RawDirent<'_>) -> bool,
    ) -> VfsResult<()>;

    /// Make a node. One creator, not four.
    ///
    /// `kind` decides what gets made, so the VFS has no separate branches for
    /// create, mkdir, symlink and mknod. `rdev` is read only for the two device
    /// kinds and `target` only for a symlink; passing either for anything else
    /// is a caller bug.
    #[allow(clippy::too_many_arguments)]
    fn create(
        &mut self,
        dir: NodeId,
        name: &str,
        kind: VNodeKind,
        mode: u16,
        uid: u32,
        gid: u32,
        rdev: (u32, u32),
        target: Option<&str>,
    ) -> VfsResult<NodeMeta>;

    fn link(&mut self, dir: NodeId, name: &str, target: NodeId) -> VfsResult<()>;

    /// Remove a name. `is_dir` picks between the two removals: the caller
    /// knows, and a guess from the inode can unlink a directory as a file
    fn unlink(&mut self, dir: NodeId, name: &str, is_dir: bool) -> VfsResult<()>;

    fn rename(
        &mut self,
        old_dir: NodeId,
        old_name: &str,
        new_dir: NodeId,
        new_name: &str,
    ) -> VfsResult<()>;

    // data

    fn read_at(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> VfsResult<usize>;

    fn write_at(&mut self, id: NodeId, offset: u64, data: &[u8]) -> VfsResult<usize>;

    /// `pages` back to back from `offset`, all of them or an error: the page
    /// cache's write-out, handing its frames as they are. A filesystem copying
    /// blocks into its own cache takes them one by one; this default gathers
    /// them for `write_at`
    fn write_pages(&mut self, id: NodeId, offset: u64, pages: &[&[u8]]) -> VfsResult<()> {
        let mut buf = alloc::vec::Vec::with_capacity(pages.iter().map(|p| p.len()).sum());
        for p in pages {
            buf.extend_from_slice(p);
        }
        let mut done = 0;
        while done < buf.len() {
            match self.write_at(id, offset + done as u64, &buf[done..])? {
                0 => return Err(VfsError::NoSpace),
                n => done += n,
            }
        }
        Ok(())
    }

    fn truncate(&mut self, id: NodeId, size: u64) -> VfsResult<()>;

    /// The target of a symlink, as bytes. Returns how many were written; a
    /// target longer than `buf` is truncated, and the count then equals the
    /// buffer length
    fn readlink(&mut self, id: NodeId, buf: &mut [u8]) -> VfsResult<usize>;

    // attributes

    fn getattr(&mut self, id: NodeId) -> VfsResult<NodeMeta>;

    fn setattr(&mut self, id: NodeId, attr: &FsSetAttr) -> VfsResult<()>;

    /// Note that this node was read. Best effort: a read does not fail over an
    /// access time
    fn touch_atime(&mut self, _id: NodeId) {}

    // optional, like discard

    fn fsync(&mut self, _id: NodeId) -> VfsResult<()> {
        self.sync()
    }

    fn fallocate(&mut self, _id: NodeId, _offset: u64, _len: u64) -> VfsResult<()> {
        Err(VfsError::NotSupported)
    }

    fn punch_hole(&mut self, _id: NodeId, _offset: u64, _len: u64) -> VfsResult<()> {
        Err(VfsError::NotSupported)
    }

    fn getxattr(&mut self, _id: NodeId, _name: &str, _buf: &mut [u8]) -> VfsResult<usize> {
        Err(VfsError::NotSupported)
    }

    fn setxattr(&mut self, _id: NodeId, _name: &str, _value: &[u8]) -> VfsResult<()> {
        Err(VfsError::NotSupported)
    }

    /// Hand free space back to the device. Returns bytes trimmed
    fn trim(&mut self) -> VfsResult<u64> {
        Err(VfsError::NotSupported)
    }
}
