//! `<sys/stat.h>`: file modes and Linux's 144-byte `stat`

#![allow(unused_imports)]

use super::prelude::*;

// File modes, `seek`, `access`, and the rest of the POSIX constants
/// The file-type bits of a mode word, Linux's values
///
/// `stat` reports the kind in a byte of its own (`VNodeKind`) and `mode` holds
/// permissions only, so the kernel produces none of these. Unix-shaped crates
/// compose and match on them.
pub const S_IFMT: mode_t = 0o170000;
pub const S_IFIFO: mode_t = 0o010000;
pub const S_IFCHR: mode_t = 0o020000;
pub const S_IFDIR: mode_t = 0o040000;
pub const S_IFBLK: mode_t = 0o060000;
pub const S_IFREG: mode_t = 0o100000;
pub const S_IFLNK: mode_t = 0o120000;
pub const S_IFSOCK: mode_t = 0o140000;
pub const S_IRWXU: mode_t = 0o700;
pub const S_IRUSR: mode_t = 0o400;
pub const S_IWUSR: mode_t = 0o200;
pub const S_IXUSR: mode_t = 0o100;
pub const S_IRWXG: mode_t = 0o070;
pub const S_IRGRP: mode_t = 0o040;
pub const S_IWGRP: mode_t = 0o020;
pub const S_IXGRP: mode_t = 0o010;
pub const S_IRWXO: mode_t = 0o007;
pub const S_IROTH: mode_t = 0o004;
pub const S_IWOTH: mode_t = 0o002;
pub const S_IXOTH: mode_t = 0o001;
/// `utimensat`'s two special nanosecond values.
pub const UTIME_NOW: c_long = (1 << 30) - 1;
pub const UTIME_OMIT: c_long = (1 << 30) - 2;
// Below: for Rust only, not in the C headers
pub type off_t = i64;
pub type dev_t = u64;
pub type ino_t = u64;
pub type mode_t = u32;
pub type nlink_t = u64;
pub type blksize_t = i64;
pub type blkcnt_t = i64;
/// Device numbers, packed as on Linux
///
/// `const fn` here, not symbols in `libc_miku.so`. `stat` carries a major and a
/// minor byte separately and never a packed `dev_t`, so only a caller wanting
/// Unix's shape composes one.
pub const fn major(dev: dev_t) -> c_uint {
    (((dev >> 32) & 0xffff_f000) | ((dev >> 8) & 0x0000_0fff)) as c_uint
}
pub const fn minor(dev: dev_t) -> c_uint {
    (((dev >> 12) & 0xffff_ff00) | (dev & 0x0000_00ff)) as c_uint
}
pub const fn makedev(major: c_uint, minor: c_uint) -> dev_t {
    let (major, minor) = (major as dev_t, minor as dev_t);
    ((major & 0x0000_0fff) << 8)
        | ((major & 0xffff_f000) << 32)
        | ((minor & 0x0000_00ff) << 0)
        | ((minor & 0xffff_ff00) << 12)
}
c_structs! {
    /// `stat`, in Linux's 144-byte layout. The kernel's is 64 bytes, and
    /// `libc_miku.so`'s `stat`, `fstat` and `lstat` translate it (`to_c_stat`
    /// in `lib/runtime/libmiku/libc/`): the file type goes into `st_mode`, `st_dev`
    /// is the filesystem, `st_rdev` the device of a `/dev` node, `st_blksize`
    /// 4096, and the nanosecond fields zero, since the kernel keeps whole
    /// seconds.
    pub struct stat {
        pub st_dev: dev_t,
        pub st_ino: ino_t,
        pub st_nlink: nlink_t,
        pub st_mode: mode_t,
        pub st_uid: uid_t,
        pub st_gid: gid_t,
        pub __pad0: c_int,
        pub st_rdev: dev_t,
        pub st_size: off_t,
        pub st_blksize: blksize_t,
        pub st_blocks: blkcnt_t,
        pub st_atime: time_t,
        pub st_atime_nsec: i64,
        pub st_mtime: time_t,
        pub st_mtime_nsec: i64,
        pub st_ctime: time_t,
        pub st_ctime_nsec: i64,
        pub __unused: [i64; 3],
    }
}
