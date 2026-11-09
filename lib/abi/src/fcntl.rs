//! `<fcntl.h>`: open flags, `fcntl` commands, record locks, the `*at` family

#![allow(unused_imports)]

use super::prelude::*;

// `fcntl`, and the two flags changeable on an open descriptor. The access mode
// is not
pub const F_DUPFD: c_int = 0;
pub const F_GETFD: c_int = 1;
pub const F_SETFD: c_int = 2;
pub const F_GETFL: c_int = 3;
pub const F_SETFL: c_int = 4;
pub const FD_CLOEXEC: c_int = 1;
// `open` flags, as `kernel/fs/vfs/types.rs` defines them, not the Linux numbers
pub const O_RDONLY: c_int = 0x0001;
pub const O_WRONLY: c_int = 0x0002;
pub const O_RDWR: c_int = 0x0003;
pub const O_APPEND: c_int = 0x0004;
pub const O_CREAT: c_int = 0x0008;
pub const O_EXCL: c_int = 0x0010;
pub const O_TRUNC: c_int = 0x0020;
pub const O_DIRECTORY: c_int = 0x0040;
pub const O_NOFOLLOW: c_int = 0x0080;
pub const O_SYNC: c_int = 0x0200;
pub const O_NONBLOCK: c_int = 0x0400;
pub const O_CLOEXEC: c_int = 0x0800;
/// The access mode bits of an open flag word
///
/// `O_RDONLY` and `O_WRONLY` are separate bits (1 and 2) and `O_RDWR` is both,
/// unlike Linux's 0/1/2. The mask is 3 in both.
pub const O_ACCMODE: c_int = 0x0003;
/// Open flags `open`(11) does not implement
///
/// Named by crates that build a flag word, and zero so they collide with no
/// real bit. Opening a terminal here does not make it the controlling one
/// (`setsid`/`ioctl(TIOCSCTTY)` does), so `O_NOCTTY` holds anyway.
pub const O_NOCTTY: c_int = 0;
pub const O_ASYNC: c_int = 0;
pub const O_DSYNC: c_int = 0;
/// `fcntl` again: the CLOEXEC duplicate and the record locks, both the kernel's
/// (`fcntl`(113), `fs::vfs::lock`)
pub const F_DUPFD_CLOEXEC: c_int = 1030;
pub const F_GETLK: c_int = 5;
pub const F_SETLK: c_int = 6;
pub const F_SETLKW: c_int = 7;
/// The path of an open descriptor, into a `PATH_MAX` buffer: macOS's and
/// NetBSD's command, with macOS's number. `libc_miku.so` answers it with
/// `realpath` (119) on the descriptor. The kernel keeps no path with a
/// descriptor, so a file renamed since it was opened answers with its new name
pub const F_GETPATH: c_int = 50;
pub const F_RDLCK: c_short = 0;
pub const F_WRLCK: c_short = 1;
pub const F_UNLCK: c_short = 2;
/// `flock` operations. `flock` in `lib/runtime/libmiku/libc/` maps them onto a
/// whole-file record lock
pub const LOCK_SH: c_int = 1;
pub const LOCK_EX: c_int = 2;
pub const LOCK_NB: c_int = 4;
pub const LOCK_UN: c_int = 8;
/// The `*at` family's directory sentinel and flags
///
/// There are no `openat`-style syscall numbers: a path's length word carries
/// the base directory in its high half (`docs/MikuOS_ABI.md`, 3.2e), and
/// `lib/runtime/libmiku/libc/at.rs` is the `*at` family over that. `AT_FDCWD` is a base of
/// nothing, the working directory; an absolute path ignores `dirfd`, as POSIX
/// says.
pub const AT_FDCWD: c_int = -100;
pub const AT_SYMLINK_NOFOLLOW: c_int = 0x100;
pub const AT_REMOVEDIR: c_int = 0x200;
pub const AT_SYMLINK_FOLLOW: c_int = 0x400;
pub const AT_EACCESS: c_int = 0x200;
/// `O_DIRECT` is zero: every read goes through the page cache
pub const O_DIRECT: c_int = 0;
/// `O_NONBLOCK`'s old name
pub const O_NDELAY: c_int = O_NONBLOCK;
/// The descriptor itself, not a path from it: `fstatat` and `fchownat` take it.
/// It is a length of zero with a base in the path word
pub const AT_EMPTY_PATH: c_int = 0x1000;
// Below: for Rust only, not in the C headers
/// `posix_fadvise` hints, and the `fallocate` modes
///
/// The hints are advisory, so answering "done" conforms. MikuOS implements none
/// of the `FALLOC_FL_*` modes; `fallocate` with any of them set answers
/// `EOPNOTSUPP`.
pub const POSIX_FADV_NORMAL: c_int = 0;
pub const POSIX_FADV_RANDOM: c_int = 1;
pub const POSIX_FADV_SEQUENTIAL: c_int = 2;
pub const POSIX_FADV_WILLNEED: c_int = 3;
pub const POSIX_FADV_DONTNEED: c_int = 4;
pub const POSIX_FADV_NOREUSE: c_int = 5;
pub const FALLOC_FL_KEEP_SIZE: c_int = 0x01;
pub const FALLOC_FL_PUNCH_HOLE: c_int = 0x02;
pub const FALLOC_FL_NO_HIDE_STALE: c_int = 0x04;
pub const FALLOC_FL_COLLAPSE_RANGE: c_int = 0x08;
pub const FALLOC_FL_ZERO_RANGE: c_int = 0x10;
pub const FALLOC_FL_INSERT_RANGE: c_int = 0x20;
pub const FALLOC_FL_UNSHARE_RANGE: c_int = 0x40;
c_structs! {
    /// `struct flock`, for `fcntl`'s record locks: `F_GETLK`, `F_SETLK` and
    /// `F_SETLKW`, POSIX's byte-range advisory locks, owned by the process
    /// and dropped when it closes any descriptor on the file
    /// (`kernel/fs/vfs/lock.rs`). `F_SETLK` against another process's lock
    /// is `EAGAIN`.
    pub struct flock {
        pub l_type: c_short,
        pub l_whence: c_short,
        pub l_start: off_t,
        pub l_len: off_t,
        pub l_pid: pid_t,
    }
}
