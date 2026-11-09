//! `<sys/inotify.h>`

use crate::libc::*;

//
// Linux's calls with Linux's meaning (116-118). `inotify-sys` declares these
// three, so the `inotify` crate and `notify` backend link here. `IN_NONBLOCK`
// and `IN_CLOEXEC` are `O_NONBLOCK` and `O_CLOEXEC`, as on Linux

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_init1(flags: i32) -> i32 {
    ret(unsafe { crate::sys::sc1(crate::sys::SYS_INOTIFY_INIT1, flags as u32 as u64) }) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_init() -> i32 {
    inotify_init1(0)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_add_watch(fd: i32, path: *const u8, mask: u32) -> i32 {
    if path.is_null() { return fail(EFAULT); }
    let len = strlen(path);
    ret(unsafe {
        crate::sys::sc4(crate::sys::SYS_INOTIFY_ADD_WATCH, fd as u64, path as u64, len as u64, mask as u64)
    }) as i32
}


#[no_mangle]
#[inline(never)]
pub extern "C" fn inotify_rm_watch(fd: i32, wd: i32) -> i32 {
    ret(unsafe { crate::sys::sc2(crate::sys::SYS_INOTIFY_RM_WATCH, fd as u64, wd as u32 as u64) }) as i32
}
