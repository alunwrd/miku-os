// The *at family: a path from a directory descriptor
//
// A path is two words, a pointer and a length, and the high half of the length
// is the directory a relative path starts from: 0 for the working directory,
// `fd + 1` for a descriptor (docs/MikuOS_ABI.md, 3.2e). Each call here is the
// plain call with a base.
//
// POSIX's two rules, kept before the kernel: an absolute path ignores `dirfd`,
// and an empty path is `ENOENT` unless `AT_EMPTY_PATH` asks for the descriptor,
// which the kernel spells as a zero length with a base, not necessarily a
// directory.

use crate::libc::*;
use crate::sys::{sc2, sc3, sc4};
use crate::sys::{
    SYS_CHDIR, SYS_CHMOD, SYS_CHOWN, SYS_LINK, SYS_LSTAT, SYS_MKDIR, SYS_OPEN, SYS_READLINK,
    SYS_RENAME, SYS_RMDIR, SYS_STAT, SYS_SYMLINK, SYS_UNLINK,
};

use miku_abi::fcntl::AT_REMOVEDIR;
use miku_abi::fcntl::AT_SYMLINK_FOLLOW;
use miku_abi::fcntl::AT_EMPTY_PATH;

/// A path's two words: its address, and its length with the base above bit 31.
/// `Err` is the errno for a path that cannot be sent
pub(crate) fn word(dirfd: i32, path: *const u8) -> Result<(u64, u64), i32> {
    if path.is_null() { return Err(EFAULT); }
    let len = strlen(path) as u64;
    if len == 0 { return Err(ENOENT); }
    let absolute = unsafe { *path } == b'/';
    let base = if absolute || dirfd == AT_FDCWD {
        0
    } else if dirfd < 0 {
        return Err(EBADF);
    } else {
        dirfd as u64 + 1
    };
    Ok((path as u64, len | base << 32))
}

/// A kernel answer as C's: -1 and errno, or the value
fn ret(r: i64) -> i32 {
    if r < 0 { fail(-r as i32) } else { r as i32 }
}

/// `.` from `fd`: the directory itself, for calls acting on a descriptor
/// through the directory it names
fn dot(fd: i32) -> Result<(u64, u64), i32> {
    word(fd, b".\0".as_ptr())
}

macro_rules! at {
    ($dirfd:expr, $path:expr) => {
        match word($dirfd, $path) {
            Ok(w) => w,
            Err(e) => return fail(e),
        }
    };
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn openat(dirfd: i32, path: *const u8, flags: i32, mode: u32) -> i32 {
    let (p, w) = at!(dirfd, path);
    ret(unsafe { sc4(SYS_OPEN, p, w, flags as u32 as u64, mode as u64) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn fstatat(dirfd: i32, path: *const u8, st: *mut stat, flags: i32) -> i32 {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 { return fail(EINVAL); }
    if flags & AT_EMPTY_PATH != 0 && !path.is_null() && unsafe { *path } == 0 {
        return crate::libc::fstat(dirfd, st);
    }
    let (p, w) = at!(dirfd, path);
    let nr = if flags & AT_SYMLINK_NOFOLLOW != 0 { SYS_LSTAT } else { SYS_STAT };
    c_stat_with(st, |k| unsafe { sc3(nr, p, w, k as u64) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn mkdirat(dirfd: i32, path: *const u8, mode: u32) -> i32 {
    let (p, w) = at!(dirfd, path);
    ret(unsafe { sc3(SYS_MKDIR, p, w, mode as u64) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn unlinkat(dirfd: i32, path: *const u8, flags: i32) -> i32 {
    if flags & !AT_REMOVEDIR != 0 { return fail(EINVAL); }
    let (p, w) = at!(dirfd, path);
    let nr = if flags & AT_REMOVEDIR != 0 { SYS_RMDIR } else { SYS_UNLINK };
    ret(unsafe { sc2(nr, p, w) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn renameat(olddirfd: i32, old: *const u8, newdirfd: i32, new: *const u8) -> i32 {
    let (op, ow) = at!(olddirfd, old);
    let (np, nw) = at!(newdirfd, new);
    ret(unsafe { sc4(SYS_RENAME, op, ow, np, nw) })
}

/// The kernel's `link` follows a symbolic link, which is `AT_SYMLINK_FOLLOW`. A
/// hard link to the link itself cannot be made, so that request is refused
#[no_mangle]
#[inline(never)]
pub extern "C" fn linkat(olddirfd: i32, old: *const u8, newdirfd: i32, new: *const u8, flags: i32) -> i32 {
    if flags & !(AT_SYMLINK_FOLLOW | AT_EMPTY_PATH) != 0 { return fail(EINVAL); }
    let (op, ow) = at!(olddirfd, old);
    if flags & AT_SYMLINK_FOLLOW == 0 {
        let mut st: stat = unsafe { core::mem::zeroed() };
        if c_stat_with(&mut st, |k| unsafe { sc3(SYS_LSTAT, op, ow, k as u64) }) == 0
            && st.st_mode & 0o170000 == 0o120000
        {
            return fail(EOPNOTSUPP);
        }
    }
    let (np, nw) = at!(newdirfd, new);
    ret(unsafe { sc4(SYS_LINK, op, ow, np, nw) })
}

/// The target is stored as given; only the link's path has a base
#[no_mangle]
#[inline(never)]
pub extern "C" fn symlinkat(target: *const u8, newdirfd: i32, linkpath: *const u8) -> i32 {
    if target.is_null() { return fail(EFAULT); }
    let tlen = strlen(target) as u64;
    let (lp, lw) = at!(newdirfd, linkpath);
    ret(unsafe { sc4(SYS_SYMLINK, target as u64, tlen, lp, lw) })
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn readlinkat(dirfd: i32, path: *const u8, buf: *mut u8, bufsiz: usize) -> isize {
    if buf.is_null() { return fail(EFAULT) as isize; }
    let (p, w) = match word(dirfd, path) {
        Ok(w) => w,
        Err(e) => return fail(e) as isize,
    };
    let r = unsafe { sc4(SYS_READLINK, p, w, buf as u64, bufsiz as u64) };
    if r < 0 { fail(-r as i32) as isize } else { r as isize }
}

/// `AT_SYMLINK_NOFOLLOW` is `EOPNOTSUPP`, as on Linux: `chmod`(26) cannot reach
/// a link's mode
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchmodat(dirfd: i32, path: *const u8, mode: u32, flags: i32) -> i32 {
    if flags & !AT_SYMLINK_NOFOLLOW != 0 { return fail(EINVAL); }
    if flags & AT_SYMLINK_NOFOLLOW != 0 { return fail(EOPNOTSUPP); }
    let (p, w) = at!(dirfd, path);
    ret(unsafe { sc3(SYS_CHMOD, p, w, mode as u64) })
}

/// Owners are 16 bits in the kernel; -1 leaves one unchanged, which `chown`(27)
/// spells 0xFFFF. `AT_SYMLINK_NOFOLLOW` is refused as in `fchmodat`, since
/// `chown`(27) follows
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchownat(dirfd: i32, path: *const u8, uid: u32, gid: u32, flags: i32) -> i32 {
    if flags & !(AT_SYMLINK_NOFOLLOW | AT_EMPTY_PATH) != 0 { return fail(EINVAL); }
    if flags & AT_EMPTY_PATH != 0 && !path.is_null() && unsafe { *path } == 0 {
        return crate::libc::fchown(dirfd, uid, gid);
    }
    if flags & AT_SYMLINK_NOFOLLOW != 0 { return fail(EOPNOTSUPP); }
    let narrow = |x: u32| -> Option<u64> {
        if x == u32::MAX { Some(0xFFFF) } else if x < 0xFFFF { Some(x as u64) } else { None }
    };
    let (Some(u), Some(g)) = (narrow(uid), narrow(gid)) else { return fail(EINVAL) };
    let (p, w) = at!(dirfd, path);
    ret(unsafe { sc4(SYS_CHOWN, p, w, u, g) })
}

/// `chdir(".")` from `fd`
#[no_mangle]
#[inline(never)]
pub extern "C" fn fchdir(fd: i32) -> i32 {
    if fd < 0 { return fail(EBADF); }
    let (p, w) = match dot(fd) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };
    ret(unsafe { sc2(SYS_CHDIR, p, w) })
}

/// `statfs`(36)'s 48 bytes for `path` from `dirfd`, read by `statfs`,
/// `fstatfs`, `statvfs`, and `fstatvfs`
pub(crate) fn statfs_raw(dirfd: i32, path: *const u8, raw: &mut MikuStatfs) -> Result<(), i32> {
    let (p, w) = word(dirfd, path)?;
    let r = unsafe { sc3(crate::sys::SYS_STATFS, p, w, raw as *mut MikuStatfs as u64) };
    if r < 0 { Err(-r as i32) } else { Ok(()) }
}

/// The filesystem of a descriptor's file: the empty path with the descriptor as
/// its base, naming its vnode of any kind (`AT_EMPTY_PATH`, in the path word)
pub(crate) fn fstatfs_raw(fd: i32, raw: &mut MikuStatfs) -> Result<(), i32> {
    if fd < 0 { return Err(EBADF); }
    let w = (fd as u64 + 1) << 32;
    let r = unsafe { sc3(crate::sys::SYS_STATFS, b"\0".as_ptr() as u64, w, raw as *mut MikuStatfs as u64) };
    if r < 0 { Err(-r as i32) } else { Ok(()) }
}

// The *at family; the rest is lib/runtime/libmiku/libc/at.rs

#[no_mangle]
#[inline(never)]
pub extern "C" fn faccessat(dirfd: i32, path: *const u8, mode: i32, flags: i32) -> i32 {
    if flags & !(AT_EACCESS | AT_SYMLINK_NOFOLLOW) != 0 { return fail(EINVAL); }
    if mode & !(R_OK | W_OK | X_OK) != 0 { return fail(EINVAL); }
    let (p, w) = match crate::at::word(dirfd, path) {
        Ok(w) => w,
        Err(e) => return fail(e),
    };

    let mut st = unsafe { core::mem::zeroed::<file::MikuStat>() };
    let nr = if flags & AT_SYMLINK_NOFOLLOW != 0 { crate::sys::SYS_LSTAT } else { crate::sys::SYS_STAT };
    let r = unsafe { crate::sys::sc3(nr, p, w, &mut st as *mut _ as u64) };
    if r < 0 { return fail(-r as i32); }
    if mode == 0 { return 0; }

    // The real ids unless asked for effective ones, which matters to a program
    // started through mikudo
    let (uid, gid) = if flags & AT_EACCESS != 0 {
        (proc::miku_geteuid(), proc::miku_getegid())
    } else {
        (proc::miku_getuid(), proc::miku_getgid())
    };
    let perm = st.mode & 0o777;
    if uid == 0 {
        // Root reads and writes anything, and executes anything something can
        let any_x = perm & 0o111 != 0 || st.kind == file::KIND_DIRECTORY;
        return if mode & X_OK != 0 && !any_x { fail(EACCES) } else { 0 };
    }
    let bits = if uid == st.uid as u64 {
        (perm >> 6) & 7
    } else if gid == st.gid as u64 {
        (perm >> 3) & 7
    } else {
        perm & 7
    };
    if (mode as u32) & !bits != 0 { fail(EACCES) } else { 0 }
}

/// `ENOSYS`: no `mknod`. Pipes come from `pipe`(34), device nodes from the
/// kernel
#[no_mangle]
#[inline(never)]
pub extern "C" fn mknodat(_dirfd: i32, _path: *const u8, _mode: u32, _dev: u64) -> i32 {
    fail(ENOSYS)
}

/// A file's times via `utimensat`(40), passing `times` through as two
/// `timespec`s or null
#[no_mangle]
#[inline(never)]
pub extern "C" fn utimensat(dirfd: i32, path: *const u8, times: *const timespec, flags: i32) -> i32 {
    let (p, w) = match crate::at::word(dirfd, path) {
        Ok(pw) => pw,
        Err(e) => return fail(e),
    };
    let r = unsafe {
        crate::sys::sc4(crate::sys::SYS_UTIMENSAT, p, w, times as u64, flags as u32 as u64)
    };
    if r < 0 { fail(-r as i32) } else { 0 }
}

/// The same on a descriptor: an empty path with the descriptor as its base
#[no_mangle]
#[inline(never)]
pub extern "C" fn futimens(fd: i32, times: *const timespec) -> i32 {
    if fd < 0 { return fail(EBADF); }
    let w = (fd as u64 + 1) << 32;
    let r = unsafe { crate::sys::sc4(crate::sys::SYS_UTIMENSAT, 0, w, times as u64, 0) };
    if r < 0 { fail(-r as i32) } else { 0 }
}
