//! The program's arguments, off the initial stack.
//!
//! The SysV initial stack holds `argc`, then `argv` NUL-terminated, then
//! `envp`, then the auxiliary vector (`docs/MikuOS_ABI.md` §4.4). `init` is
//! handed the first two by the runtime and keeps them.

use crate::ffi::{CStr, OsString};
use crate::os::miku::ffi::OsStringExt;
use crate::sync::atomic::{AtomicIsize, AtomicPtr, Ordering};

pub use super::common::Args;

static ARGC: AtomicIsize = AtomicIsize::new(0);
static ARGV: AtomicPtr<*const u8> = AtomicPtr::new(crate::ptr::null_mut());

/// # Safety
/// `argv` must be the real initial stack's argument vector, valid for the life
/// of the program.
pub unsafe fn init(argc: isize, argv: *const *const u8) {
    ARGC.store(argc, Ordering::Relaxed);
    ARGV.store(argv as *mut *const u8, Ordering::Relaxed);
}

pub fn args() -> Args {
    let argc = ARGC.load(Ordering::Relaxed);
    let argv = ARGV.load(Ordering::Relaxed);
    let mut out = Vec::new();
    if !argv.is_null() {
        for i in 0..argc {
            // SAFETY: `argv` holds `argc` pointers to NUL-terminated strings,
            // as the kernel wrote and `init` was given
            let p = unsafe { *argv.offset(i) };
            if p.is_null() {
                break;
            }
            let bytes = unsafe { CStr::from_ptr(p.cast()) }.to_bytes().to_vec();
            out.push(OsString::from_vec(bytes));
        }
    }
    Args::new(out)
}
