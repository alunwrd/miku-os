//! `<poll.h>`

use crate::libc::*;

pub use miku_abi::poll::POLLIN;

pub use miku_abi::poll::POLLPRI;

pub use miku_abi::poll::POLLOUT;

pub use miku_abi::poll::POLLERR;

pub use miku_abi::poll::POLLHUP;

pub use miku_abi::poll::POLLNVAL;

/// `struct pollfd`: the kernel's `MikuPollFd`, eight bytes, under the name
/// crates read
pub type pollfd = io::MikuPollFd;

#[no_mangle]
#[inline(never)]
pub extern "C" fn poll(fds: *mut pollfd, nfds: u64, timeout: i32) -> i32 {
    // The kernel caps a call at `MAX_POLL_FDS` (256) and answers `-EINVAL` past
    // it, which is passed through
    let r = io::miku_poll(fds, nfds, timeout as i64);
    if r < 0 { set_errno(-r as i32); return -1; }
    r as i32
}
