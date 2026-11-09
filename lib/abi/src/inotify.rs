//! `<sys/inotify.h>`

#![allow(unused_imports)]

use super::prelude::*;

// inotify, Linux's values and semantics (`inotify_init1`(116),
// `inotify_add_watch`(117), `inotify_rm_watch`(118)). `IN_ACCESS` and `IN_OPEN`
// are accepted in a mask and never raised
pub const IN_ACCESS: u32 = 0x0000_0001;
pub const IN_MODIFY: u32 = 0x0000_0002;
pub const IN_ATTRIB: u32 = 0x0000_0004;
pub const IN_CLOSE_WRITE: u32 = 0x0000_0008;
pub const IN_CLOSE_NOWRITE: u32 = 0x0000_0010;
pub const IN_CLOSE: u32 = IN_CLOSE_WRITE | IN_CLOSE_NOWRITE;
pub const IN_OPEN: u32 = 0x0000_0020;
pub const IN_MOVED_FROM: u32 = 0x0000_0040;
pub const IN_MOVED_TO: u32 = 0x0000_0080;
pub const IN_MOVE: u32 = IN_MOVED_FROM | IN_MOVED_TO;
pub const IN_CREATE: u32 = 0x0000_0100;
pub const IN_DELETE: u32 = 0x0000_0200;
pub const IN_DELETE_SELF: u32 = 0x0000_0400;
pub const IN_MOVE_SELF: u32 = 0x0000_0800;
pub const IN_UNMOUNT: u32 = 0x0000_2000;
pub const IN_Q_OVERFLOW: u32 = 0x0000_4000;
pub const IN_IGNORED: u32 = 0x0000_8000;
pub const IN_ONLYDIR: u32 = 0x0100_0000;
pub const IN_DONT_FOLLOW: u32 = 0x0200_0000;
pub const IN_EXCL_UNLINK: u32 = 0x0400_0000;
pub const IN_MASK_CREATE: u32 = 0x1000_0000;
pub const IN_MASK_ADD: u32 = 0x2000_0000;
pub const IN_ISDIR: u32 = 0x4000_0000;
pub const IN_ONESHOT: u32 = 0x8000_0000;
pub const IN_ALL_EVENTS: u32 = 0x0000_0fff;
pub const IN_CLOEXEC: c_int = O_CLOEXEC;
pub const IN_NONBLOCK: c_int = O_NONBLOCK;
c_structs! {
    /// One record a `read` on an inotify descriptor returns, followed by `len`
    /// bytes of NUL-padded name. Linux's shape.
    pub struct inotify_event {
        pub wd: c_int,
        pub mask: u32,
        pub cookie: u32,
        pub len: u32,
    }
}
