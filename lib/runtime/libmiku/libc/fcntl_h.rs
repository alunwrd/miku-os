//! `<fcntl.h>`

use crate::libc::*;

pub use miku_abi::fcntl::F_DUPFD;

pub use miku_abi::fcntl::F_GETFD;

pub use miku_abi::fcntl::F_SETFD;

pub use miku_abi::fcntl::F_GETFL;

pub use miku_abi::fcntl::F_SETFL;

pub use miku_abi::fcntl::FD_CLOEXEC;

pub use miku_abi::fcntl::O_NONBLOCK;

pub use miku_abi::fcntl::O_CLOEXEC;

/// Variadic in C and three-argument here, as every caller passes. A socket
/// answers too, though above `SOCK_FD_BASE` in its own table: the kernel routes
/// it to the socket layer, which keeps `O_NONBLOCK` and `FD_CLOEXEC` on the
/// slot and makes `F_DUPFD` an alias
#[no_mangle]
#[inline(never)]
pub extern "C" fn fcntl(fd: i32, cmd: i32, arg: i64) -> i32 {
    // `F_GETPATH` is this library's: `realpath` (119) on the descriptor itself
    use miku_abi::fcntl::F_GETPATH;
    const PATH_MAX: usize = miku_abi::limits::PATH_MAX as usize;
    if cmd == F_GETPATH {
        let r = file::miku_fd_path(fd, arg as *mut u8, PATH_MAX);
        return if r < 0 { fail(-r as i32) } else { 0 };
    }
    let r = file::miku_fcntl(fd as i64, cmd as u64, arg);
    if r < 0 { set_errno(-r as i32); return -1; }
    r as i32
}

/// BSD's whole-file lock as a POSIX record lock over the whole file, the
/// kernel's only lock (`fcntl`(113) `F_SETLK`, `fs::vfs::lock`). Differences:
/// it belongs to the process, so any `close` of the file drops it; and a shared
/// lock needs a read descriptor, an exclusive one a write descriptor (`EBADF`
/// otherwise)
#[no_mangle]
#[inline(never)]
pub extern "C" fn flock(fd: i32, op: i32) -> i32 {
    use miku_abi::fcntl::LOCK_SH;
    use miku_abi::fcntl::LOCK_EX;
    use miku_abi::fcntl::LOCK_NB;
    use miku_abi::fcntl::LOCK_UN;
    use miku_abi::fcntl::F_SETLK;
    use miku_abi::fcntl::F_SETLKW;
    let l_type: i16 = match op & !LOCK_NB {
        LOCK_SH => 0,
        LOCK_EX => 1,
        LOCK_UN => 2,
        _ => return fail(EINVAL),
    };
    // `struct flock`, the 32 bytes `record_lock` reads: type, whence
    // (SEEK_SET), start 0, length 0 to the end
    let mut fl = [0u8; 32];
    fl[0..2].copy_from_slice(&l_type.to_le_bytes());
    let cmd = if op & LOCK_NB != 0 || l_type == 2 { F_SETLK } else { F_SETLKW };
    let r = fcntl(fd, cmd, fl.as_mut_ptr() as i64);
    // A conflict is `EWOULDBLOCK` for `flock`, where `fcntl` may say `EACCES`
    if r < 0 && unsafe { *__errno_location() } == EACCES { return fail(EAGAIN); }
    r
}

// fcntl.h: advice and allocation

/// Advisory and ignored, so "done" conforms. Returns the error, not `-1`, as
/// POSIX specifies for this call
#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_fadvise(fd: i32, offset: i64, len: i64, advice: i32) -> i32 {
    if offset < 0 || len < 0 || !(0..=5).contains(&advice) { return EINVAL; }
    // A descriptor that is not open is still an error
    let mut st = unsafe { core::mem::zeroed::<file::MikuStat>() };
    if file::miku_fstat(fd as i64, &mut st) < 0 { return EBADF; }
    0
}

/// Through `fallocate`(37). Returns the error without setting errno, per POSIX
#[no_mangle]
#[inline(never)]
pub extern "C" fn posix_fallocate(fd: i32, offset: i64, len: i64) -> i32 {
    if offset < 0 || len <= 0 { return EINVAL; }
    let r = file::miku_fallocate(fd as i64, offset as u64, len as u64);
    if r < 0 { -r as i32 } else { 0 }
}
