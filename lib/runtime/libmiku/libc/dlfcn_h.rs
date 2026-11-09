//! `<dlfcn.h>`

use crate::libc::*;

/// Always null: `ld-miku` resolves everything before `main` and keeps no symbol
/// table to ask. Callers such as `std`'s weak symbols and `rustix`'s probes
/// have a fallback
#[no_mangle]
#[inline(never)]
pub extern "C" fn dlsym(_handle: *mut u8, _symbol: *const u8) -> *mut u8 {
    core::ptr::null_mut()
}

// dlfcn.h: no run-time loading, as C in a MikuOS program is linked in. Each
// call fails like a missing library, with a message saying why

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlopen(_path: *const u8, _flags: i32) -> *mut u8 {
    core::ptr::null_mut()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlerror() -> *const u8 {
    b"dynamic loading is not supported on MikuOS\0".as_ptr()
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn dlclose(_handle: *mut u8) -> i32 {
    -1
}
