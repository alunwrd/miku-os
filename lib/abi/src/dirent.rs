//! `<dirent.h>`

#![allow(unused_imports)]

use super::prelude::*;

c_structs! {
    /// A directory entry, 272 bytes: what `getdents`(111) writes
    ///
    /// `readdir` in `lib/runtime/libmiku/libc/` returns a pointer into the batch the
    /// syscall filled, so the C view and the kernel's share one layout.
    ///
    /// The name is 256 bytes; the inode is 64 bits, the same as `stat` reports
    /// (`kernel/syscall/abi.rs`).
    pub struct dirent {
        pub d_name: [c_char; 256],
        pub d_ino: u64,
        pub d_type: u8,
        pub d_namlen: u8,
        pub d_reserved: [u8; 6],
    }
}
