//! `<sys/utsname.h>`

#![allow(unused_imports)]

use super::prelude::*;

c_structs! {
    /// Linux's, six 65-byte fields (`lib/runtime/libmiku/libc/sysinfo.rs`)
    pub struct utsname {
        pub sysname: [c_char; 65],
        pub nodename: [c_char; 65],
        pub release: [c_char; 65],
        pub version: [c_char; 65],
        pub machine: [c_char; 65],
        pub domainname: [c_char; 65],
    }
}
