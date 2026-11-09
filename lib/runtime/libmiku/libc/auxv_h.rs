//! `<sys/auxv.h>`

use crate::libc::*;

/// Value of `key` in the program's auxiliary vector, or 0: glibc's `getauxval`,
/// from what ld-miku gave libmiku (`lib/runtime/libmiku/core/auxv.rs`) before any
/// constructor
#[no_mangle]
#[inline(never)]
pub extern "C" fn getauxval(key: u64) -> u64 {
    crate::auxv::miku_getauxval(key)
}
