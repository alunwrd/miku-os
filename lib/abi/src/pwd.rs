//! `<pwd.h>`

#![allow(unused_imports)]

use super::prelude::*;

c_structs! {
    /// `passwd`. Accounts are `/etc/passwd`; `libc_miku.so` has the reentrant
    /// lookups (`getpwuid_r`, `getpwnam_r`), not the static-buffer ones
    pub struct passwd {
        pub pw_name: *mut c_char,
        pub pw_passwd: *mut c_char,
        pub pw_uid: uid_t,
        pub pw_gid: gid_t,
        pub pw_gecos: *mut c_char,
        pub pw_dir: *mut c_char,
        pub pw_shell: *mut c_char,
    }
}
