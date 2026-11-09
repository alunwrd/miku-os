//! `<sys/resource.h>`

use crate::libc::*;

/// `struct rlimit`, likewise `libc`'s own
pub use miku_abi::posix::rlimit;

pub(crate) use miku_abi::resource::RLIMIT_NOFILE;

pub(crate) use miku_abi::resource::RLIM_INFINITY;

/// The descriptor ceiling for `RLIMIT_NOFILE`, and no limit for the rest;
/// nothing else in the table is enforced
#[no_mangle]
#[inline(never)]
pub extern "C" fn getrlimit(resource: i32, rlim: *mut rlimit) -> i32 {
    if !(0..16).contains(&resource) { return fail(EINVAL); }
    if rlim.is_null() { return fail(EFAULT); }
    let v = if resource == RLIMIT_NOFILE { OPEN_MAX } else { RLIM_INFINITY };
    unsafe { rlim.write(rlimit { rlim_cur: v, rlim_max: v }) };
    0
}

/// Succeeds when asked for what `getrlimit` reports, as Zed does at start
/// raising its soft limit to the hard one, and answers `ENOSYS` otherwise
#[no_mangle]
#[inline(never)]
pub extern "C" fn setrlimit(resource: i32, rlim: *const rlimit) -> i32 {
    if !(0..16).contains(&resource) { return fail(EINVAL); }
    if rlim.is_null() { return fail(EFAULT); }
    let want = unsafe { &*rlim };
    if want.rlim_cur > want.rlim_max { return fail(EINVAL); }
    let have = if resource == RLIMIT_NOFILE { OPEN_MAX } else { RLIM_INFINITY };
    if want.rlim_cur == have && want.rlim_max == have { 0 } else { fail(ENOSYS) }
}

/// `ENOSYS` both ways: no syscall reads or sets a task's priority
#[no_mangle]
#[inline(never)]
pub extern "C" fn getpriority(_which: i32, _who: u32) -> i32 {
    fail(ENOSYS)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn setpriority(_which: i32, _who: u32, _prio: i32) -> i32 {
    fail(ENOSYS)
}
