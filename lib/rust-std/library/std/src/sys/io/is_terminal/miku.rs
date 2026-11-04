//! `IsTerminal` on MikuOS.
//!
//! `libc_miku`'s `isatty` is `ioctl(TCGETS)`, which the kernel answers for a
//! tty and a pty master and refuses with `ENOTTY` for everything else. This is
//! the adapter.

use crate::os::fd::{AsFd, AsRawFd};
use crate::sys::pal::abi;

pub fn is_terminal(fd: &impl AsFd) -> bool {
    abi::isatty(fd.as_fd().as_raw_fd() as i32) != 0
}
