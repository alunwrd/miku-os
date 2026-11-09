//! What `stat`, `getdents`, and `statfs` write: the kernel's own forms
//!
//! `libc_miku` translates to Linux's `stat` and `statfs`. A `dirent` is this
//! layout, so `readdir` can hand out a pointer into the batch the call filled.

/// `stat`(18), `lstat`(112), `fstat`(19)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuStat {
    pub size: u64,
    pub mode: u32,
    pub nlinks: u32,
    pub uid: u16,
    pub gid: u16,
    pub kind: u8,
    pub fs_type: u8,
    pub dev_major: u8,
    pub dev_minor: u8,
    pub atime: u64,
    pub mtime: u64,
    pub ctime: u64,
    pub inode_id: u64,
    pub blocks: u32,
    pub _reserved: [u8; 4],
}

/// How much of a name fits in a `MikuDirent`: 255 bytes and the terminator.
/// `name_len` is a `u8`
pub const DIRENT_NAME_LEN: usize = 256;

/// `getdents`(111). The inode is the number `stat` reports
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct MikuDirent {
    pub name: [u8; DIRENT_NAME_LEN],
    pub inode_id: u64,
    pub kind: u8,
    pub name_len: u8,
    pub _reserved: [u8; 6],
}

impl MikuDirent {
    pub const EMPTY: Self = Self {
        name: [0; DIRENT_NAME_LEN],
        inode_id: 0,
        kind: 0,
        name_len: 0,
        _reserved: [0; 6],
    };
}

/// `statfs`(36)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuStatfs {
    pub magic: u32,
    pub block_size: u32,
    pub total_blocks: u64,
    pub free_blocks: u64,
    pub total_inodes: u64,
    pub free_inodes: u64,
    pub max_name_len: u32,
    pub flags: u32,
}

/// `clock_gettime`(70) and the times of `utimensat`(40)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct MikuTimespec {
    pub sec: i64,
    pub nsec: i64,
}

// The `kind` byte of a stat and a dirent: `VNodeKind` in the kernel
pub const KIND_REGULAR: u8 = 0;
pub const KIND_DIRECTORY: u8 = 1;
pub const KIND_SYMLINK: u8 = 2;
pub const KIND_CHARDEV: u8 = 3;
pub const KIND_BLOCKDEV: u8 = 4;
pub const KIND_PIPE: u8 = 5;
pub const KIND_SOCKET: u8 = 6;
pub const KIND_FIFO: u8 = 7;

const _: () = assert!(core::mem::size_of::<MikuStat>() == 64);
const _: () = assert!(core::mem::size_of::<MikuDirent>() == 272);
const _: () = assert!(core::mem::size_of::<MikuStatfs>() == 48);
const _: () = assert!(core::mem::size_of::<MikuTimespec>() == 16);
