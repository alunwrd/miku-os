//! `<errno.h>`

use crate::libc::*;

pub use miku_abi::errno::EPERM;

pub use miku_abi::errno::ENOENT;

pub use miku_abi::errno::ESRCH;

pub use miku_abi::errno::EINTR;

pub use miku_abi::errno::EIO;

pub use miku_abi::errno::ENXIO;

pub use miku_abi::errno::E2BIG;

pub use miku_abi::errno::ENOEXEC;

pub use miku_abi::errno::EBADF;

pub use miku_abi::errno::ECHILD;

pub use miku_abi::errno::EAGAIN;

pub use miku_abi::errno::ENOMEM;

pub use miku_abi::errno::EACCES;

pub use miku_abi::errno::EFAULT;

pub use miku_abi::errno::EBUSY;

pub use miku_abi::errno::EEXIST;

pub use miku_abi::errno::ENOTDIR;

pub use miku_abi::errno::EISDIR;

pub use miku_abi::errno::EINVAL;

pub use miku_abi::errno::EMFILE;

pub use miku_abi::errno::EFBIG;

pub use miku_abi::errno::ENOSPC;

pub use miku_abi::errno::EPIPE;

pub use miku_abi::errno::EDOM;

pub use miku_abi::errno::ERANGE;

pub use miku_abi::errno::ENOSYS;

/// This thread's `errno`, the TCB word `errno::errno_cell` names. An `int *`,
/// as C has it. See `errno.rs`
#[no_mangle]
#[inline(never)]
pub extern "C" fn __errno_location() -> *mut i32 {
    errno::miku_errno_location()
}

/// The same cell under the name the `errno` crate links on a system not in its
/// `cfg_attr` list: `__errno_location` for Linux, `__error` for BSDs, else bare
/// `errno_location`, as `rustix` reaches it here
#[no_mangle]
#[inline(never)]
pub extern "C" fn errno_location() -> *mut i32 {
    errno::miku_errno_location()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strerror(errnum: i64) -> *const u8 {
    errno::miku_strerror(errnum)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn perror(s: *const u8) {
    errno::miku_perror(s);
}
