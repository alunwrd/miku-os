use crate::fs::vfs::address_space::AddressSpace;
use crate::fs::vfs::children::Children;
use crate::fs::vfs::types::*;

#[derive(Debug, Clone, Copy, Default)]
pub struct VNodeFlags {
    pub dirty: bool,
    pub immutable: bool,
    pub append_only: bool,
    pub no_atime: bool,
    pub encrypted: bool,
    pub compressed: bool,
    pub versioned: bool,
    pub locked: bool,
}

impl VNodeFlags {
    pub const fn empty() -> Self {
        Self {
            dirty: false,
            immutable: false,
            append_only: false,
            no_atime: false,
            encrypted: false,
            compressed: false,
            versioned: false,
            locked: false,
        }
    }
}

/// Another name of a hard-linked file, besides its first (`parent`, `name`)
pub struct Alias {
    pub dir: InodeId,
    pub name: alloc::boxed::Box<str>,
}

pub struct VNode {
    pub id: InodeId,
    /// Its first name: the directory and the name in it. A file with more
    /// than one name cached has the others in `aliases`
    pub parent: InodeId,
    pub name: NameBuf,
    /// Its other cached names (`dev/vfs-dentries-inodes.md`, "Stage 5"). A
    /// directory never has any
    pub aliases: alloc::vec::Vec<Alias>,

    pub kind: VNodeKind,
    pub fs_type: FsType,
    pub active: bool,

    pub mode: FileMode,
    pub uid: u16,
    pub gid: u16,

    pub size: u64,
    pub nlinks: u16,
    /// Descriptors, working directories, mappings and plans holding it. Atomic:
    /// `open` takes one under the VFS lock held to read (`dev/vfs-locking.md`,
    /// "4c"); read it with `refs`
    pub refcount: core::sync::atomic::AtomicU32,

    pub atime: Timestamp,
    pub mtime: Timestamp,
    pub ctime: Timestamp,
    pub btime: Timestamp,
    /// The VFS operation (`MikuVFS::op`, a count of `with_vfs` calls) that last
    /// handed this vnode to anybody: a lookup that found it cached, or the fill
    /// that made it. Not `atime`, which is the file's and is shown by `stat`:
    /// this is the cache's recency, and `reclaim` spares what the last
    /// operations touched. A count, since a tight loop touches thousands of
    /// vnodes inside one tick
    pub used: u64,
    /// Walked through by a reader under the VFS lock held to read, which may
    /// not move it in the LRU or stamp `used`: reclaim's sweep reads and clears
    /// it, and a vnode found set goes back to the head (`dev/vfs-locking.md`,
    /// "4c")
    pub referenced: core::sync::atomic::AtomicBool,
    /// Its place in the cache's LRU (`vfs::lru`)
    pub lru_prev: InodeId,
    pub lru_next: InodeId,
    pub on_lru: bool,

    pub children: Children,
    pub addr_space: AddressSpace,
    pub symlink_target: SymlinkTarget,

    pub dev_major: u8,
    pub dev_minor: u8,
    pub mount_id: u8,

    pub flags: VNodeFlags,

    pub ext2_ino: u32,
    /// Which mounted ext filesystem this vnode's inode number belongs to.
    ///
    /// Up to four can be mounted at once, and an inode number means nothing
    /// without its filesystem: resolved against another's superblock and inode
    /// table it silently names a different file.
    ///
    /// `INVALID_U8` means not yet attributed and falls back to the active slot,
    /// correct when only one filesystem is mounted.
    pub ext2_slot: u8,
    pub children_loaded: bool,
    /// Size, blocks and times are the filesystem's: loaded with the vnode or
    /// read back since, and nothing has asked the filesystem to change the
    /// inode after. Any call into the filesystem about this node clears it
    /// (`MikuVFS::with_backing`), and `stat` reads the inode again only then,
    /// so it does not wait for ext's lock behind a directory listing or a
    /// journal commit (`dev/kernel-core.md`, "Second round")
    pub attrs_fresh: core::sync::atomic::AtomicBool,

    /// Read-only backing in kernel image memory (embedded syslibs). Reads
    /// serve straight from the slice, bypassing the page cache; writes are
    /// rejected.
    pub static_data: Option<&'static [u8]>,

}

impl VNode {
    pub const fn empty() -> Self {
        Self {
            id: INVALID_ID,
            parent: INVALID_ID,
            name: NameBuf::empty(),
            aliases: alloc::vec::Vec::new(),
            kind: VNodeKind::Regular,
            fs_type: FsType::TmpFS,
            mode: FileMode(0o644),
            uid: 0,
            gid: 0,
            size: 0,
            nlinks: 0,
            refcount: core::sync::atomic::AtomicU32::new(0),
            atime: 0,
            mtime: 0,
            ctime: 0,
            btime: 0,
            used: 0,
            lru_prev: INVALID_ID,
            lru_next: INVALID_ID,
            on_lru: false,
            children: Children::new(),
            addr_space: AddressSpace::new(),
            symlink_target: SymlinkTarget::none(),
            dev_major: 0,
            dev_minor: 0,
            mount_id: INVALID_U8,
            flags: VNodeFlags::empty(),
            active: false,
            ext2_ino: 0,
            ext2_slot: INVALID_U8,
            children_loaded: false,
            attrs_fresh: core::sync::atomic::AtomicBool::new(false),
            referenced: core::sync::atomic::AtomicBool::new(false),
            static_data: None,
        }
    }

    pub fn init(
        &mut self,
        id: InodeId,
        parent: InodeId,
        name: &str,
        kind: VNodeKind,
        fs_type: FsType,
        mode: FileMode,
        uid: u16,
        gid: u16,
        now: Timestamp,
    ) {
        *self = Self::empty();
        self.id = id;
        self.parent = parent;
        self.name = NameBuf::from_str(name);
        self.kind = kind;
        self.fs_type = fs_type;
        self.mode = mode;
        self.uid = uid;
        self.gid = gid;
        self.nlinks = if kind == VNodeKind::Directory { 2 } else { 1 };
        self.atime = now;
        self.mtime = now;
        self.ctime = now;
        self.btime = now;
        self.active = true;
    }

    pub fn reset(&mut self) {
        *self = Self::empty();
    }

    #[inline]
    pub fn is_dir(&self) -> bool {
        self.kind == VNodeKind::Directory
    }
    #[inline]
    pub fn is_regular(&self) -> bool {
        self.kind == VNodeKind::Regular
    }
    #[inline]
    pub fn is_symlink(&self) -> bool {
        self.kind == VNodeKind::Symlink
    }
    #[inline]
    pub fn is_pipe(&self) -> bool {
        self.kind == VNodeKind::Pipe || self.kind == VNodeKind::Fifo
    }

    #[inline]
    pub fn is_device(&self) -> bool {
        matches!(self.kind, VNodeKind::CharDevice | VNodeKind::BlockDevice)
    }

    #[inline]
    pub fn is_mountpoint(&self) -> bool {
        self.mount_id != INVALID_U8
    }
    #[inline]
    pub fn name_eq(&self, name: &str) -> bool {
        self.name.eq_str(name)
    }

    pub fn get_name(&self) -> &str {
        self.name.as_str()
    }

    #[inline]
    /// A node on a mounted filesystem, not in this table.
    ///
    /// `ext2_ino` holds this node's id on its filesystem, whichever one
    /// `fs_type` names
    pub fn is_ext_backed(&self) -> bool {
        self.fs_type.has_mounted_fs() && self.ext2_ino != 0
    }

    /// Which mount its inode number belongs to: the filesystem's type and
    /// its slot in that type's table, which both number from zero
    #[inline]
    pub fn mount_key(&self) -> u16 {
        ((self.fs_type as u16) << 8) | self.ext2_slot as u16
    }

    /// `st_ino` and `d_ino`. On a disk, the filesystem's inode number with
    /// the mount above it: the same for every name of the file and after any
    /// eviction, and unique across mounts, since nothing in userspace reads a
    /// per-mount `st_dev`. Elsewhere the vnode's number, which never changes
    /// because those vnodes are never evicted
    pub fn ino(&self) -> u64 {
        if self.is_ext_backed() {
            ((1 + self.mount_key() as u64) << 32) | self.ext2_ino as u64
        } else {
            self.id as u64
        }
    }

    pub fn stat(&self) -> VNodeStat {
        VNodeStat {
            ino: self.ino(),
            kind: self.kind,
            mode: self.mode,
            size: self.size,
            // In 512-byte blocks, which is what `st_blocks` counts. A mounted
            // filesystem's count is already that (`refresh_attrs_from_fs`); a
            // tmpfs page is eight of them
            blocks: if self.static_data.is_some() {
                self.size.div_ceil(BLOCK_SIZE as u64) as u32
            } else if self.is_ext_backed() {
                self.addr_space.nr_pages
            } else {
                self.addr_space.nr_pages * (PAGE_SIZE / BLOCK_SIZE) as u32
            },
            nlinks: self.nlinks,
            uid: self.uid,
            gid: self.gid,
            fs_type: self.fs_type,
            dev_major: self.dev_major,
            dev_minor: self.dev_minor,
            atime: self.atime,
            mtime: self.mtime,
            ctime: self.ctime,
            btime: self.btime,
        }
    }

    pub fn touch_atime(&mut self, now: Timestamp) {
        if !self.flags.no_atime {
            self.atime = now;
        }
    }

    pub fn touch_mtime(&mut self, now: Timestamp) {
        self.mtime = now;
        self.ctime = now;
        self.flags.dirty = true;
    }

    pub fn touch_ctime(&mut self, now: Timestamp) {
        self.ctime = now;
    }

    /// Holders now
    #[inline]
    pub fn refs(&self) -> u32 {
        self.refcount.load(core::sync::atomic::Ordering::Relaxed)
    }

    /// One more holder: `&self`, for an `open` under the VFS lock held to read.
    /// A pipe end and a pty master are never opened that way, so their counts
    /// below move one writer at a time
    pub fn inc_ref(&self) {
        self.refcount.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        // A pipe end's descriptors are its readers or its writers
        if self.active && self.kind == VNodeKind::Pipe {
            crate::fs::vfs::pipe::ref_changed(self.id as usize, true);
        }
        if let Some(slot) = self.pty_master_slot() {
            crate::io::tty::master_ref(slot);
        }
    }

    /// The pty slot this node is the master of, if it is one
    #[inline]
    fn pty_master_slot(&self) -> Option<usize> {
        if !self.active || self.kind != VNodeKind::CharDevice {
            return None;
        }
        match crate::fs::devfs::dev_type_from_node(self.dev_major, self.dev_minor) {
            Some(crate::fs::devfs::DevType::PtyMaster { slot }) => Some(slot as usize),
            _ => None,
        }
    }

    /// One holder fewer, from `&self`, when that is all there is to do: a file
    /// or directory with a name, or with other holders. False, with nothing
    /// changed, for the last holder of one that has to go then (no name, or
    /// unlinked), for `MikuVFS::drop_ref` under the lock held alone. Only for
    /// kinds whose `dec_ref` does nothing else (`close_as`)
    pub fn dec_ref_quietly(&self) -> bool {
        debug_assert!(self.kind == VNodeKind::Regular || self.kind == VNodeKind::Directory);
        let keeps = self.active && self.nlinks > 0;
        self.refcount
            .fetch_update(core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed, |r| {
                (r >= 2 || (r == 1 && keeps)).then(|| r - 1)
            })
            .is_ok()
    }

    pub fn dec_ref(&mut self) {
        if self.refs() > 0 {
            if let Some(slot) = self.pty_master_slot() {
                crate::io::tty::master_unref(slot);
            }
            if self.active && self.kind == VNodeKind::Pipe {
                crate::fs::vfs::pipe::ref_changed(self.id as usize, false);
            }
        }
        let _ = self.refcount.fetch_update(core::sync::atomic::Ordering::Relaxed, core::sync::atomic::Ordering::Relaxed, |r| r.checked_sub(1));
        // Every path that drops a descriptor passes through here (`close`, the
        // close-on-exec sweep, both exit paths, a descriptor in flight over a
        // dying socket), so an inotify instance ends here with its last
        // reference
        if self.refs() == 0 && self.is_notify_instance() {
            crate::fs::vfs::notify::destroy(self.id as usize);
            self.active = false;
        }
    }

    /// The anonymous vnode behind an inotify descriptor
    #[inline]
    pub fn is_notify_instance(&self) -> bool {
        self.active
            && self.kind == VNodeKind::CharDevice
            && (self.dev_major, self.dev_minor) == crate::fs::vfs::notify::DEV
    }

    pub fn is_referenced(&self) -> bool {
        self.refs() > 0
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}

// What a vnode costs, checked at compile time.
//
// About 680 bytes, from the `vnode` typed cache (`vfs::table`), so six to a
// page; the cache holds that times the vnodes cached.
//
// The ceiling is a budget. Before raising it, decide whether the field belongs
// in a vnode at all.
const _: () = assert!(
    core::mem::size_of::<VNode>() <= 704,
    "a vnode has grown past its budget; see the note above this assertion"
);
