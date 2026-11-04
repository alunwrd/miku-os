// A filesystem with no disk
//
// Ext and btrfs are disk formats; tmpfs, devfs and procfs are the vnode table
// under a name. `MemFs` implements `fsops::FileSystem` in full and keeps the
// filesystem in a `BTreeMap` on the heap.
//
// A second filesystem for tests. The VFS decides which filesystem answers for a
// vnode at every lookup, directory fill and write, and those decisions need a
// second mount to test; mounting one of these costs one allocation, so the VFS
// selftest has one on any machine, including a headless QEMU with a single
// disk.
//
// Not a tmpfs: /tmp is the vnode table itself, and a second in-memory store
// behind the VFS would be two answers to one question.
//
// The mount table is at the bottom of this file. A mount is `MemFs::new()`, so
// the table is three functions and a static.

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::fs::vfs::fsops::{FileSystem, FsSetAttr, NodeId, NodeMeta, RawDirent};
use crate::fs::vfs::types::*;
use crate::kcore::sched_lock::SchedMutex;

/// The root directory's number.
///
/// One; the counter never hands out zero. `NodeId::is_none()` reads zero as "no
/// node", and the VFS treats a vnode whose `ext2_ino` is zero as having no
/// filesystem behind it (`vfs/core/backing.rs`).
const ROOT_NODE: u64 = 1;

/// File data budget, so that `NoSpace` is an answer here and the VFS's
/// out-of-space paths can be exercised. A megabyte is far more than the
/// selftest writes.
const MAX_BYTES: u64 = 1024 * 1024;

/// Node budget, for the same reason.
///
/// 262144: `vfs::selftest::beyond_the_table` makes two hundred thousand files,
/// and the vnode cache has to fill before this filesystem does
const MAX_NODES: usize = 262144;

/// A node's id on this filesystem.
///
/// `tree` is zero and `node` carries the number, as on ext. The pair exists for
/// btrfs, where an object is named by its tree and its number; there are no
/// subvolumes here. A vnode stores its id as one `ext2_ino`, so a `tree` would
/// not survive the trip back.
const fn id_of(node: u64) -> NodeId {
    NodeId { tree: 0, node }
}

/// What a node holds, as against what is known about it.
///
/// Metadata is the same shape for every kind; only the contents differ. A
/// device node has no contents, so `Special` carries nothing: its `rdev` is
/// metadata
enum Body {
    File(Vec<u8>),
    /// Name to node number. A `BTreeMap`, as `readdir` hands out names in a
    /// stable order; a listing reshuffled between two calls appears to lose
    /// entries
    Dir(BTreeMap<String, u64>),
    Symlink(String),
    /// A character or block device, a fifo or a socket: a name and an owner,
    /// nothing to read
    Special,
}

struct MemNode {
    kind: VNodeKind,
    /// Permission bits only, like `NodeMeta::mode`. The type is in `kind`
    mode: u16,
    uid: u32,
    gid: u32,
    nlinks: u16,
    atime: u64,
    mtime: u64,
    ctime: u64,
    rdev: (u32, u32),
    body: Body,
}

impl MemNode {
    /// Bytes of this node's contents. Zero for directories, which are a map
    /// with no image on any medium
    fn data_len(&self) -> u64 {
        match &self.body {
            Body::File(data) => data.len() as u64,
            Body::Symlink(target) => target.len() as u64,
            Body::Dir(_) | Body::Special => 0,
        }
    }
}

/// The wall clock, the one ext stamps an inode with, so an mtime means the same
/// on either filesystem
fn now() -> u64 {
    crate::kcore::clock::now_unix()
}

/// A `String` with a fallible allocation.
///
/// For every name and symlink target. `String::from` on a name from a path
/// aborts when the heap is full; a create should answer that it cannot, as
/// `read_path` does with `try_reserve_exact`
fn try_string(s: &str) -> VfsResult<String> {
    let mut out = String::new();
    out.try_reserve_exact(s.len()).map_err(|_| VfsError::OutOfMemory)?;
    out.push_str(s);
    Ok(out)
}

/// A whole filesystem, in a map
pub struct MemFs {
    nodes: BTreeMap<u64, MemNode>,
    /// The next number to hand out. It only rises: a reused number would make a
    /// stale `NodeId` in a vnode point at a different file
    next_node: u64,
}

impl MemFs {
    /// An empty filesystem: a root directory and nothing in it.
    ///
    /// Not a `const fn`: the root's map is an allocation, so the mount table
    /// below holds `Option`s
    pub fn new() -> Self {
        let ts = now();
        let mut nodes = BTreeMap::new();
        nodes.insert(
            ROOT_NODE,
            MemNode {
                kind: VNodeKind::Directory,
                mode: 0o755,
                uid: 0,
                gid: 0,
                // Itself and its own `.`, as on other filesystems
                nlinks: 2,
                atime: ts,
                mtime: ts,
                ctime: ts,
                rdev: (0, 0),
                body: Body::Dir(BTreeMap::new()),
            },
        );
        MemFs { nodes, next_node: ROOT_NODE + 1 }
    }

    fn node(&self, id: NodeId) -> VfsResult<&MemNode> {
        self.nodes.get(&id.node).ok_or(VfsError::NotFound)
    }

    fn node_mut(&mut self, id: NodeId) -> VfsResult<&mut MemNode> {
        self.nodes.get_mut(&id.node).ok_or(VfsError::NotFound)
    }

    fn entries(&self, id: NodeId) -> VfsResult<&BTreeMap<String, u64>> {
        match &self.node(id)?.body {
            Body::Dir(map) => Ok(map),
            _ => Err(VfsError::NotDirectory),
        }
    }

    fn entries_mut(&mut self, id: NodeId) -> VfsResult<&mut BTreeMap<String, u64>> {
        match &mut self.node_mut(id)?.body {
            Body::Dir(map) => Ok(map),
            _ => Err(VfsError::NotDirectory),
        }
    }

    fn meta_of(&self, node: u64, n: &MemNode) -> NodeMeta {
        let size = n.data_len();
        NodeMeta {
            id: id_of(node),
            kind: n.kind,
            mode: n.mode,
            uid: n.uid,
            gid: n.gid,
            size,
            // In 512-byte units, rounded up, as `stat` defines `blocks`
            blocks: size.div_ceil(BLOCK_SIZE as u64),
            nlinks: n.nlinks,
            atime: n.atime,
            mtime: n.mtime,
            ctime: n.ctime,
            rdev: n.rdev,
        }
    }

    /// Everything this filesystem holds.
    ///
    /// Summed on demand. A counter would need updating on write, truncate,
    /// create, unlink, rename over a victim and setattr's size; the map is
    /// small
    fn used(&self) -> u64 {
        self.nodes.values().map(|n| n.data_len()).sum()
    }

    /// The number for a new node. Free numbers are not recycled, so running out
    /// is an error
    fn alloc_node(&mut self) -> VfsResult<u64> {
        if self.nodes.len() >= MAX_NODES {
            return Err(VfsError::NoSpace);
        }
        let id = self.next_node;
        self.next_node += 1;
        Ok(id)
    }

    /// Drop one name's worth of a node's link count, and the node itself when
    /// the last name goes.
    ///
    /// A directory is dropped outright: its count includes its `.`, so counting
    /// down would leave every removed directory at one. Called only on an empty
    /// directory, so no child's `..` points here
    fn drop_link(&mut self, node: u64) {
        let Some(n) = self.nodes.get_mut(&node) else {
            return;
        };
        if n.kind == VNodeKind::Directory {
            self.nodes.remove(&node);
            return;
        }
        n.nlinks = n.nlinks.saturating_sub(1);
        if n.nlinks == 0 {
            self.nodes.remove(&node);
        }
    }

    /// Room for `growth` more bytes of file data, or `NoSpace`
    fn room_for(&self, growth: u64) -> VfsResult<()> {
        if self.used().saturating_add(growth) > MAX_BYTES {
            return Err(VfsError::NoSpace);
        }
        Ok(())
    }

    /// The file behind an id, for the three calls specific to regular files.
    /// The two refusals differ: asking a directory to hold bytes and asking a
    /// symlink to are different errors
    fn file_mut(&mut self, id: NodeId) -> VfsResult<&mut Vec<u8>> {
        let n = self.node_mut(id)?;
        match &mut n.body {
            Body::File(data) => Ok(data),
            Body::Dir(_) => Err(VfsError::IsDirectory),
            _ => Err(VfsError::InvalidArgument),
        }
    }

    /// Grow a file to `size`, zero-filling what was never written.
    ///
    /// A write past the end of a file leaves a hole, and a hole reads as
    /// zeroes; allocator contents would expose another program's memory
    fn grow_file(&mut self, id: NodeId, size: usize) -> VfsResult<()> {
        let have = self.file_mut(id)?.len();
        if size <= have {
            return Ok(());
        }
        self.room_for((size - have) as u64)?;
        let data = self.file_mut(id)?;
        data.try_reserve(size - have).map_err(|_| VfsError::OutOfMemory)?;
        data.resize(size, 0);
        Ok(())
    }
}

impl FileSystem for MemFs {
    // identity

    fn fs_type(&self) -> FsType {
        FsType::MemFs
    }

    fn read_only(&self) -> bool {
        false
    }

    fn root(&self) -> NodeId {
        id_of(ROOT_NODE)
    }

    fn statfs(&mut self) -> VfsResult<StatFs> {
        let used = self.used();
        Ok(StatFs {
            fs_type: FsType::MemFs,
            block_size: BLOCK_SIZE as u32,
            total_blocks: MAX_BYTES / BLOCK_SIZE as u64,
            free_blocks: (MAX_BYTES - used) / BLOCK_SIZE as u64,
            total_inodes: MAX_NODES as u64,
            free_inodes: (MAX_NODES - self.nodes.len()) as u64,
            max_name_len: NAME_LEN as u32,
            flags: 0,
        })
    }

    fn sync(&mut self) -> VfsResult<()> {
        // Memory is the medium, so the promise holds by construction
        Ok(())
    }

    // namespace

    fn lookup(&mut self, dir: NodeId, name: &str) -> VfsResult<NodeMeta> {
        let node = *self.entries(dir)?.get(name).ok_or(VfsError::NotFound)?;
        let n = self.nodes.get(&node).ok_or(VfsError::Corrupted)?;
        Ok(self.meta_of(node, n))
    }

    fn readdir(
        &mut self,
        dir: NodeId,
        f: &mut dyn FnMut(RawDirent<'_>) -> bool,
    ) -> VfsResult<()> {
        // Shared borrows throughout, so a name can be handed out borrowed from
        // its map (`RawDirent`), not copied through something narrower
        let this: &Self = self;
        let entries = this.entries(dir)?;

        for (name, node) in entries.iter() {
            let kind = match this.nodes.get(node) {
                Some(n) => n.kind,
                // A name whose node is gone: an unlink took the node and left
                // the name, a bug in this file. Reported and skipped
                None => {
                    crate::serial_println!(
                        "[memfs] entry {:?} names node {} which does not exist",
                        name,
                        node
                    );
                    continue;
                }
            };
            if !f(RawDirent { name: name.as_bytes(), id: id_of(*node), kind }) {
                // The caller's decision, the only one that ends this walk
                // early. The trait's callback shape exists to prevent a stop at
                // some fixed entry count
                return Ok(());
            }
        }

        // `.` and `..` are not stored. The VFS synthesises both in `readdir`,
        // and its directory fill skips them when a filesystem does emit them,
        // as ext does
        Ok(())
    }

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
    ) -> VfsResult<NodeMeta> {
        if name.is_empty() {
            return Err(VfsError::InvalidArgument);
        }
        if name.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        if self.entries(dir)?.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        // One creator, with `kind` deciding what gets made, as ext's `create`
        // does: the VFS does not carry four near-identical branches
        let body = match kind {
            VNodeKind::Directory => Body::Dir(BTreeMap::new()),
            VNodeKind::Symlink => {
                let target = target.ok_or(VfsError::InvalidArgument)?;
                if target.is_empty() {
                    return Err(VfsError::InvalidArgument);
                }
                if target.len() > SYMLINK_MAX {
                    return Err(VfsError::NameTooLong);
                }
                self.room_for(target.len() as u64)?;
                Body::Symlink(try_string(target)?)
            }
            VNodeKind::Regular => Body::File(Vec::new()),
            VNodeKind::CharDevice
            | VNodeKind::BlockDevice
            | VNodeKind::Fifo
            | VNodeKind::Pipe
            | VNodeKind::Socket => Body::Special,
        };

        // The name is allocated before anything is inserted: a node left in the
        // map because its name would not fit could never be reached
        let owned = try_string(name)?;
        let node = self.alloc_node()?;
        let ts = now();

        self.nodes.insert(
            node,
            MemNode {
                kind,
                mode: mode & 0o7777,
                uid,
                gid,
                nlinks: if kind == VNodeKind::Directory { 2 } else { 1 },
                atime: ts,
                mtime: ts,
                ctime: ts,
                // Only the two device kinds have one, per the trait. Elsewhere
                // a device number would be something `stat` prints and nothing
                // can open
                rdev: match kind {
                    VNodeKind::CharDevice | VNodeKind::BlockDevice => rdev,
                    _ => (0, 0),
                },
                body,
            },
        );

        let parent = self.node_mut(dir)?;
        if let Body::Dir(map) = &mut parent.body {
            map.insert(owned, node);
        }
        parent.mtime = ts;
        parent.ctime = ts;
        if kind == VNodeKind::Directory {
            // The new directory's `..` is a link to this one
            parent.nlinks = parent.nlinks.saturating_add(1);
        }

        let n = self.nodes.get(&node).ok_or(VfsError::Corrupted)?;
        Ok(self.meta_of(node, n))
    }

    fn link(&mut self, dir: NodeId, name: &str, target: NodeId) -> VfsResult<()> {
        if name.is_empty() {
            return Err(VfsError::InvalidArgument);
        }
        if name.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        // A second name for a directory makes a path walk a cycle that `..`
        // cannot leave. Directories cannot be hard-linked
        if self.node(target)?.kind == VNodeKind::Directory {
            return Err(VfsError::PermissionDenied);
        }
        if self.entries(dir)?.contains_key(name) {
            return Err(VfsError::AlreadyExists);
        }

        let owned = try_string(name)?;
        let ts = now();
        let n = self.node_mut(target)?;
        if n.nlinks == u16::MAX {
            return Err(VfsError::TooManyLinks);
        }
        n.nlinks += 1;
        n.ctime = ts;

        let parent = self.node_mut(dir)?;
        if let Body::Dir(map) = &mut parent.body {
            map.insert(owned, target.node);
        }
        parent.mtime = ts;
        Ok(())
    }

    fn unlink(&mut self, dir: NodeId, name: &str, is_dir: bool) -> VfsResult<()> {
        let node = *self.entries(dir)?.get(name).ok_or(VfsError::NotFound)?;
        let kind = self.nodes.get(&node).ok_or(VfsError::Corrupted)?.kind;

        // `is_dir` is the caller's word for which removal this is, and it is
        // checked: a directory removed as a file leaves its contents in the map
        // with no name above them, an unreported leak
        if is_dir != (kind == VNodeKind::Directory) {
            return Err(if kind == VNodeKind::Directory {
                VfsError::IsDirectory
            } else {
                VfsError::NotDirectory
            });
        }
        if is_dir {
            match &self.nodes[&node].body {
                Body::Dir(map) if !map.is_empty() => return Err(VfsError::NotEmpty),
                _ => {}
            }
        }

        let ts = now();
        let parent = self.node_mut(dir)?;
        if let Body::Dir(map) = &mut parent.body {
            map.remove(name);
        }
        parent.mtime = ts;
        parent.ctime = ts;
        if is_dir {
            parent.nlinks = parent.nlinks.saturating_sub(1);
        }

        self.drop_link(node);
        Ok(())
    }

    fn rename(
        &mut self,
        old_dir: NodeId,
        old_name: &str,
        new_dir: NodeId,
        new_name: &str,
    ) -> VfsResult<()> {
        if new_name.is_empty() {
            return Err(VfsError::InvalidArgument);
        }
        if new_name.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        let node = *self.entries(old_dir)?.get(old_name).ok_or(VfsError::NotFound)?;
        let moving_dir = self.nodes[&node].kind == VNodeKind::Directory;

        // A name landing on an existing one replaces it, and POSIX has two
        // rules about the victim, both about kind. The VFS checks them too;
        // checking here keeps this filesystem correct for any caller
        let victim = self.entries(new_dir)?.get(new_name).copied();
        if let Some(v) = victim {
            if v == node {
                return Ok(());
            }
            let v_is_dir = self.nodes.get(&v).ok_or(VfsError::Corrupted)?.kind
                == VNodeKind::Directory;
            if v_is_dir != moving_dir {
                return Err(if v_is_dir {
                    VfsError::IsDirectory
                } else {
                    VfsError::NotDirectory
                });
            }
            if v_is_dir {
                match &self.nodes[&v].body {
                    Body::Dir(map) if !map.is_empty() => return Err(VfsError::NotEmpty),
                    _ => {}
                }
            }
        }

        let owned = try_string(new_name)?;
        let ts = now();

        self.entries_mut(old_dir)?.remove(old_name);
        if let Some(v) = victim {
            self.entries_mut(new_dir)?.remove(new_name);
            if moving_dir {
                // The victim directory's `..` went with it
                let p = self.node_mut(new_dir)?;
                p.nlinks = p.nlinks.saturating_sub(1);
            }
            self.drop_link(v);
        }
        self.entries_mut(new_dir)?.insert(owned, node);

        // There is no `..` entry to rewrite. A move between directories still
        // has to fix the two parents' link counts, which ext needs a second
        // call for
        if moving_dir && old_dir != new_dir {
            let old = self.node_mut(old_dir)?;
            old.nlinks = old.nlinks.saturating_sub(1);
            let new = self.node_mut(new_dir)?;
            new.nlinks = new.nlinks.saturating_add(1);
        }

        self.node_mut(old_dir)?.mtime = ts;
        self.node_mut(new_dir)?.mtime = ts;
        self.node_mut(id_of(node))?.ctime = ts;
        Ok(())
    }

    // data

    fn read_at(&mut self, id: NodeId, offset: u64, buf: &mut [u8]) -> VfsResult<usize> {
        let n = self.node(id)?;
        let data = match &n.body {
            Body::File(data) => data,
            Body::Dir(_) => return Err(VfsError::IsDirectory),
            _ => return Err(VfsError::InvalidArgument),
        };
        if offset >= data.len() as u64 {
            return Ok(0);
        }
        let start = offset as usize;
        let count = buf.len().min(data.len() - start);
        buf[..count].copy_from_slice(&data[start..start + count]);
        Ok(count)
    }

    fn write_at(&mut self, id: NodeId, offset: u64, data: &[u8]) -> VfsResult<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(VfsError::InvalidArgument)?;
        if end > MAX_BYTES {
            return Err(VfsError::NoSpace);
        }
        self.grow_file(id, end as usize)?;

        let start = offset as usize;
        let file = self.file_mut(id)?;
        file[start..end as usize].copy_from_slice(data);

        let ts = now();
        let n = self.node_mut(id)?;
        n.mtime = ts;
        n.ctime = ts;
        Ok(data.len())
    }

    fn truncate(&mut self, id: NodeId, size: u64) -> VfsResult<()> {
        if size > MAX_BYTES {
            return Err(VfsError::NoSpace);
        }
        let have = self.file_mut(id)?.len() as u64;
        if size > have {
            self.grow_file(id, size as usize)?;
        } else {
            self.file_mut(id)?.truncate(size as usize);
        }
        let ts = now();
        let n = self.node_mut(id)?;
        n.mtime = ts;
        n.ctime = ts;
        Ok(())
    }

    fn readlink(&mut self, id: NodeId, buf: &mut [u8]) -> VfsResult<usize> {
        let n = self.node(id)?;
        let Body::Symlink(target) = &n.body else {
            return Err(VfsError::InvalidArgument);
        };
        let count = target.len().min(buf.len());
        buf[..count].copy_from_slice(&target.as_bytes()[..count]);
        Ok(count)
    }

    // attributes

    fn getattr(&mut self, id: NodeId) -> VfsResult<NodeMeta> {
        let n = self.node(id)?;
        Ok(self.meta_of(id.node, n))
    }

    fn setattr(&mut self, id: NodeId, attr: &FsSetAttr) -> VfsResult<()> {
        // The size first, since it can fail: mode and owner applied and then a
        // refused truncate would mean an error for a partly applied request
        if let Some(size) = attr.size {
            FileSystem::truncate(self, id, size)?;
        }
        let ts = now();
        let n = self.node_mut(id)?;
        if let Some(mode) = attr.mode {
            n.mode = mode & 0o7777;
        }
        // Each half separately, so that a chgrp leaves the owner alone. The VFS
        // sends both every time; reading the absent one as zero would hand the
        // file to root on every group change
        if let Some(uid) = attr.uid {
            n.uid = uid;
        }
        if let Some(gid) = attr.gid {
            n.gid = gid;
        }
        if let Some(t) = attr.atime {
            n.atime = t;
        }
        if let Some(t) = attr.mtime {
            n.mtime = t;
        }
        n.ctime = ts;
        Ok(())
    }

    fn touch_atime(&mut self, id: NodeId) {
        let ts = now();
        if let Ok(n) = self.node_mut(id) {
            n.atime = ts;
        }
    }
}

// The mounts

/// Two: one for the selftest, and a second so "which of the two answered" can
/// be asked of one filesystem type as well as across two
pub const MAX_MOUNTS: usize = 2;

pub struct MemFsState {
    /// `Option`, as in btrfs's table: a `MemFs` owns its root's map, so it
    /// cannot be const-constructed, and a placeholder that answered lookups
    /// would be a false mount
    slots: [Option<MemFs>; MAX_MOUNTS],
}

impl MemFsState {
    const fn new() -> Self {
        Self { slots: [const { None }; MAX_MOUNTS] }
    }
}

/// A `SchedMutex` like every other mount table here. Nothing here waits on a
/// device, but it is taken on the same paths as the ext and btrfs tables, where
/// a spin lock would hold a CPU while the others yield
static STATE: SchedMutex<MemFsState> = SchedMutex::named("memfs", MemFsState::new());

/// Make an empty filesystem and mount it. `None` when every slot is taken
pub fn mount_new() -> Option<u8> {
    let mut state = STATE.lock();
    let slot = (0..MAX_MOUNTS).find(|&s| state.slots[s].is_none())?;
    state.slots[slot] = Some(MemFs::new());
    crate::serial_println!("[memfs] slot {} mounted", slot);
    Some(slot as u8)
}

/// Drop a mount and everything in it. There is nothing to flush
pub fn unmount_slot(slot: u8) {
    let mut state = STATE.lock();
    let idx = slot as usize;
    if idx >= MAX_MOUNTS {
        return;
    }
    state.slots[idx] = None;
    crate::serial_println!("[memfs] slot {} unmounted", slot);
}

/// Run `f` against the filesystem in `slot`. `None` when the slot is empty,
/// which `fs::registry::with_mounted` turns into `NotMounted`
pub fn with_memfs_for<F, R>(slot: u8, f: F) -> Option<R>
where
    F: FnOnce(&mut MemFs) -> R,
{
    let mut state = STATE.lock();
    let idx = slot as usize;
    if idx >= MAX_MOUNTS {
        return None;
    }
    state.slots[idx].as_mut().map(f)
}
