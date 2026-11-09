//! `<dlfcn.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// `dlsym`'s "search everything already loaded" handle
///
/// `ld-miku` resolves every `DT_NEEDED` at load and there is no `dlopen`.
/// `dlsym` always answers null, the same as for a symbol it cannot find, which
/// crates probing for an optional libc function handle.
pub const RTLD_DEFAULT: *mut c_void = core::ptr::null_mut();
c_structs! {
    pub struct Dl_info {
        pub dli_fname: *const c_char,
        pub dli_fbase: *mut c_void,
        pub dli_sname: *const c_char,
        pub dli_saddr: *mut c_void,
    }
}
