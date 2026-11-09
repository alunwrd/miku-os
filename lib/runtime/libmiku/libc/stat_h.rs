//! `<sys/stat.h>`

use crate::libc::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn mkdir(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_mkdir(path, mode);
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// Linux's `struct stat`, 144 bytes, as C and the `libc` crate read.
///
/// MikuOS's own is 64 bytes (`file::MikuStat`, what `stat`(18) writes), which
/// `miku_stat` and `stat_path` use. The C names answer in this one, via
/// `to_c_stat`: crates such as `rustix`, `tempfile`, and `cap-primitives` read
/// `st_dev`, `st_rdev`, and `st_*_nsec` by name
pub use miku_abi::stat::stat;

pub(crate) const _: () = assert!(core::mem::size_of::<stat>() == 144);

/// MikuOS's stat to C's.
///
/// - `st_mode` gets the file type from `kind` (the VFS's `VNodeKind`); the
///   kernel's mode is
///   only the permission bits
/// - `st_dev` is the filesystem (the kernel's `fs_type`); with `st_ino`, unique
///   across every
///   mounted filesystem, it names one file
/// - `st_rdev` is the device a node is, `/dev/*`, in Linux's encoding
/// - `st_blksize` is 4096, the page and VFS block; zero would be a divisor in C
///   code sizing
///   its writes by it (SQLite's `fcntlSizeHint`)
/// - times are whole seconds; the nanoseconds are zero
pub(crate) fn to_c_stat(k: &file::MikuStat, out: *mut stat) {
    let ifmt: u32 = match k.kind {
        0 => 0o100000, // regular
        1 => 0o040000, // directory
        2 => 0o120000, // symlink
        3 => 0o020000, // character device
        4 => 0o060000, // block device
        5 | 7 => 0o010000, // pipe, FIFO
        6 => 0o140000, // socket
        _ => 0,
    };
    let rdev = ((k.dev_major as u64 & 0xfff) << 8) | (k.dev_minor as u64 & 0xff);
    unsafe {
        out.write(stat {
            st_dev: k.fs_type as u64,
            st_ino: k.inode_id,
            st_nlink: k.nlinks as u64,
            st_mode: (k.mode & 0o7777) | ifmt,
            st_uid: k.uid as u32,
            st_gid: k.gid as u32,
            __pad0: 0,
            st_rdev: rdev,
            st_size: k.size as i64,
            st_blksize: 4096,
            st_blocks: k.blocks as i64,
            st_atime: k.atime as i64,
            st_atime_nsec: 0,
            st_mtime: k.mtime as i64,
            st_mtime_nsec: 0,
            st_ctime: k.ctime as i64,
            st_ctime_nsec: 0,
            __unused: [0; 3],
        });
    }
}

/// One of the kernel's stat calls, answered in C's layout
pub(crate) fn c_stat_with(st: *mut stat, call: impl FnOnce(*mut file::MikuStat) -> i64) -> i32 {
    if st.is_null() { return fail(EFAULT); }
    let mut k: file::MikuStat = unsafe { core::mem::zeroed() };
    let r = call(&mut k);
    if r < 0 { return fail(-r as i32); }
    to_c_stat(&k, st);
    0
}

/// MikuOS's stat by path in `MikuStat`, called by the `miku` crate. C's `stat`
/// is below
#[no_mangle]
#[inline(never)]
pub extern "C" fn stat_path(path: *const u8, st: *mut file::MikuStat) -> i32 {
    let r = file::miku_stat(path, st);
    if r < 0 { fail(-r as i32) } else { with_blksize(st) }
}

/// `st_blksize`, in the four bytes the kernel's `stat` leaves zero; see the
/// field in the `libc` module. Every stat here goes through this
pub(crate) fn with_blksize(st: *mut file::MikuStat) -> i32 {
    if !st.is_null() {
        unsafe { (*st)._reserved = 4096u32.to_le_bytes() };
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstat(fd: i32, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_fstat(fd as i64, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chmod(path: *const u8, mode: u32) -> i32 {
    let r = file::miku_chmod(path, mode);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn chown(path: *const u8, uid: u32, gid: u32) -> i32 {
    let r = file::miku_chown(path, uid, gid);
    if r < 0 { fail(-r as i32) } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn lstat(path: *const u8, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_lstat(path, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fsync(fd: i32) -> i32 {
    let r = file::miku_fsync(fd as i64);
    if r < 0 { set_errno(-r as i32); return -1; }
    0
}

/// C's `stat` in C's layout; `stat_path` is MikuOS's own
#[no_mangle]
#[inline(never)]
pub extern "C" fn stat(path: *const u8, st: *mut stat) -> i32 {
    c_stat_with(st, |k| file::miku_stat(path, k) as i64)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn umask(mask: u32) -> u32 {
    unsafe { sc1(crate::sys::SYS_UMASK, mask as u64) as u32 }
}

/// `ENOSYS`: a descriptor knows no name here, no `/proc/self/fd` exists, and
/// the kernel's `chmod`(26) and `chown`(27) take a path. SQLite, the caller,
/// ignores the answer
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchmod(fd: i32, mode: u32) -> i32 {
    ret(crate::file::miku_fchmod(fd, mode)) as i32
}

/// -1 (`(uid_t)-1`) leaves one unchanged, per POSIX; the kernel's word for that
/// is 0xFFFF
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchown(fd: i32, uid: u32, gid: u32) -> i32 {
    let keep = |v: u32| if v == u32::MAX { 0xFFFF } else { v };
    ret(crate::file::miku_fchown(fd, keep(uid), keep(gid))) as i32
}

/// The microsecond form: two `struct timeval` {sec, usec}, or null for now
#[no_mangle]
#[inline(never)]
pub extern "C" fn utimes(path: *const u8, times: *const u8) -> i32 {
    use miku_abi::fcntl::AT_FDCWD;
    if times.is_null() {
        return utimensat(AT_FDCWD, path, core::ptr::null(), 0);
    }
    let tv = times as *const [i64; 2];
    let ts = unsafe {
        [
            timespec { tv_sec: (*tv)[0], tv_nsec: (*tv)[1] * 1000 },
            timespec { tv_sec: (*tv.add(1))[0], tv_nsec: (*tv.add(1))[1] * 1000 },
        ]
    };
    utimensat(AT_FDCWD, path, ts.as_ptr(), 0)
}
