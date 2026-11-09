//! `<poll.h>`

#![allow(unused_imports)]

use super::prelude::*;

pub const POLLIN: c_short = 0x001;
pub const POLLPRI: c_short = 0x002;
pub const POLLOUT: c_short = 0x004;
pub const POLLERR: c_short = 0x008;
pub const POLLHUP: c_short = 0x010;
pub const POLLNVAL: c_short = 0x020;
/// `poll` again: the `*NORM`/`*BAND` events
///
/// `poll`(76) reports `POLLIN`, `POLLOUT`, `POLLERR`, `POLLHUP` and `POLLNVAL`.
/// No descriptor has out-of-band or priority data. The normal-data pair alias
/// the plain ones, as on Linux; the band pair are zero, since a caller could
/// wait forever on a bit the kernel never sets.
pub const POLLRDNORM: c_short = POLLIN;
pub const POLLWRNORM: c_short = POLLOUT;
pub const POLLRDBAND: c_short = 0;
pub const POLLWRBAND: c_short = 0;
// Below: for Rust only, not in the C headers
pub type nfds_t = u64;
