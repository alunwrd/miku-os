// Userspace ABI: byte layouts of structs returned to user-mode programs. Stable
// contracts: do not reorder or resize fields without coordinating libmiku and
// userspace

use crate::fs::vfs::types::VNodeStat;

// stat(2)/fstat(2), 64 bytes:
//   0  u64  size
//   8  u32  mode
//  12  u32  nlinks
//  16  u16  uid
//  18  u16  gid
//  20  u8   kind
//  21  u8   fs_type
//  22  u8   dev_major
//  23  u8   dev_minor
//  24  u64  atime
//  32  u64  mtime
//  40  u64  ctime
//  48  u64  inode_id
//  56  u32  blocks
//  60  [4]  reserved
pub const STAT_SIZE: u64 = 64;

// getdents(2) entry, 272 bytes:
//    0  [256] name (null-terminated, 255 max usable)
//  256  u64   inode_id
//  264  u8    kind
//  265  u8    name_len
//  266  [6]   reserved
//
// The inode is 64 bits, the same number `stat`(18) reports, and what
// `std::os::unix::fs::DirEntryExt::ino` returns
pub const UDIRENT_SIZE: u64 = 272;

/// Where the inode sits in an entry, and how wide it is. Named because the
/// offsets after the name all move together when the name field or the inode
/// changes width
pub const UDIRENT_INO_OFF: usize = UDIRENT_NAME_LEN;
pub const UDIRENT_KIND_OFF: usize = UDIRENT_INO_OFF + 8;
pub const UDIRENT_NAMELEN_OFF: usize = UDIRENT_KIND_OFF + 1;

/// How much of a name fits in a `getdents` entry.
///
/// A truncated name is a name that does not exist, so a program listing a
/// directory and opening what it found would get `ENOENT` for it, as
/// `std::fs::read_dir` feeding `File::open` does.
///
/// 256 so the terminator has somewhere to go; `name_len` is a `u8` and
/// describes the 255 usable bytes
pub const UDIRENT_NAME_LEN: usize = 256;

// statfs(2), 48 bytes:
//   0  u32  fs_type
//   4  u32  block_size
//   8  u64  total_blocks
//  16  u64  free_blocks
//  24  u64  total_inodes
//  32  u64  free_inodes
//  40  u32  max_name_len
//  44  u32  flags
pub const STATFS_SIZE: u64 = 48;

pub fn write_stat_to_user(ptr: u64, st: &VNodeStat) {
    unsafe {
        let p = ptr as *mut u8;
        (p as *mut u64).write_unaligned(st.size);
        (p.add(8)  as *mut u32).write_unaligned(st.mode.0 as u32);
        (p.add(12) as *mut u32).write_unaligned(st.nlinks as u32);
        (p.add(16) as *mut u16).write_unaligned(st.uid);
        (p.add(18) as *mut u16).write_unaligned(st.gid);
        *p.add(20) = st.kind as u8;
        *p.add(21) = st.fs_type as u8;
        *p.add(22) = st.dev_major;
        *p.add(23) = st.dev_minor;
        (p.add(24) as *mut u64).write_unaligned(st.atime);
        (p.add(32) as *mut u64).write_unaligned(st.mtime);
        (p.add(40) as *mut u64).write_unaligned(st.ctime);
        (p.add(48) as *mut u64).write_unaligned(st.ino);
        (p.add(56) as *mut u32).write_unaligned(st.blocks);
    }
}
