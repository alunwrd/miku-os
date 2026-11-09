//! `<locale.h>`

use crate::libc::*;

/// One locale, "C": asking for it, "", "POSIX", or nothing answers "C";
/// anything else is refused
#[no_mangle]
#[inline(never)]
pub extern "C" fn setlocale(_cat: i32, name: *const u8) -> *const u8 {
    const C: &[u8] = b"C\0";
    if name.is_null() {
        return C.as_ptr();
    }
    let n = string::miku_strlen(name);
    let s = unsafe { core::slice::from_raw_parts(name, n) };
    if s.is_empty() || s == b"C" || s == b"POSIX" || s == b"C.UTF-8" {
        C.as_ptr()
    } else {
        core::ptr::null()
    }
}
