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

pub struct VNode {
    pub id: InodeId,
    pub parent: InodeId,
    pub name: NameBuf,

    pub kind: VNodeKind,
    pub fs_type: FsType,
    pub active: bool,

    pub mode: FileMode,
    pub uid: u16,
    pub gid: u16,

    pub size: u64,
    pub nlinks: u16,
    pub refcount: u16,

    pub atime: Timestamp,
    pub mtime: Timestamp,
    pub ctime: Timestamp,
    pub btime: Timestamp,

    pub children: Children,
    pub addr_space: AddressSpace,
    pub symlink_target: NameBuf,

    pub dev_major: u8,
    pub dev_minor: u8,
    pub mount_id: u8,

    pub flags: VNodeFlags,

    pub ext2_ino: u32,
    pub children_loaded: bool,

    /// Read-only backing in kernel image memory (embedded syslibs). Files
    /// with this set never touch the page cache: reads serve straight from
    /// the slice, writes are rejected. Any size, zero page-cache cost
    pub static_data: Option<&'static [u8]>,

    /// Pipes only: how far the reader has consumed. A pipe is a stream, so
    /// both ends share one cursor where an ordinary file gives every
    /// descriptor its own offset
    pub pipe_read_pos: u64,

}

impl VNode {
    pub const fn empty() -> Self {
        Self {
            id: INVALID_ID,
            parent: INVALID_ID,
            name: NameBuf::empty(),
            kind: VNodeKind::Regular,
            fs_type: FsType::TmpFS,
            mode: FileMode(0o644),
            uid: 0,
            gid: 0,
            size: 0,
            nlinks: 0,
            refcount: 0,
            atime: 0,
            mtime: 0,
            ctime: 0,
            btime: 0,
            children: Children::new(),
            addr_space: AddressSpace::new(),
            symlink_target: NameBuf::empty(),
            dev_major: 0,
            dev_minor: 0,
            mount_id: INVALID_U8,
            flags: VNodeFlags::empty(),
            active: false,
            ext2_ino: 0,
            children_loaded: false,
            static_data: None,
            pipe_read_pos: 0,
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
    pub fn is_ext_backed(&self) -> bool {
        self.fs_type.is_ext_family() && self.ext2_ino != 0
    }

    #[inline]
    pub fn is_ext2_backed(&self) -> bool {
        self.is_ext_backed()
    }

    pub fn stat(&self) -> VNodeStat {
        VNodeStat {
            id: self.id,
            kind: self.kind,
            mode: self.mode,
            size: self.size,
            blocks: if self.static_data.is_some() {
                self.size.div_ceil(BLOCK_SIZE as u64) as u32
            } else {
                self.addr_space.nr_pages
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

    pub fn inc_ref(&mut self) {
        self.refcount = self.refcount.saturating_add(1);
    }

    pub fn dec_ref(&mut self) {
        self.refcount = self.refcount.saturating_sub(1);
    }

    pub fn is_referenced(&self) -> bool {
        self.refcount > 0
    }

    pub fn child_count(&self) -> usize {
        self.children.len()
    }
}
