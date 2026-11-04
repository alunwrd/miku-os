//! A pipe is a descriptor, so `Pipe` is a `FileDesc`.
//!
//! `pipe`(34) returns two descriptors as two `u64`s, not the two `i32`s of
//! every Unix, handled here.

use crate::io;
use crate::os::fd::{FromRawFd, RawFd};
use crate::sys::fd::FileDesc;
use crate::sys::pal::{abi, cvt};

pub type Pipe = FileDesc;

#[inline]
pub fn pipe() -> io::Result<(Pipe, Pipe)> {
    let mut fds = [0i64; 2];
    // Close-on-exec from the kernel, as every descriptor `std` makes: a
    // `Command` on another thread would otherwise hand both ends to its child.
    // The child that should have one gets it by `dup2`, which clears the flag
    // on the copy
    cvt(abi::miku_pipe2(fds.as_mut_ptr(), abi::OPEN_CLOEXEC))?;
    // SAFETY: both were just handed over by the kernel and nobody else owns
    // them. The read end is first
    unsafe {
        Ok((
            FileDesc::from_raw_fd(fds[0] as RawFd),
            FileDesc::from_raw_fd(fds[1] as RawFd),
        ))
    }
}
