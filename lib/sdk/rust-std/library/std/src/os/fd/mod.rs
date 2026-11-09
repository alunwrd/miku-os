//! Owned and borrowed Unix-like file descriptors.
//!
//! This module is supported on Unix platforms and WASI, which both use a
//! similar file descriptor system for referencing OS resources.

#![stable(feature = "os_fd", since = "1.66.0")]
#![deny(unsafe_op_in_unsafe_fn)]

// `RawFd`, `AsRawFd`, etc.
mod raw;

// `OwnedFd`, `AsFd`, etc.
mod owned;

// Implementations for `AsRawFd` etc. for network types.
//
// On MikuOS a socket descriptor answers `poll` and `fcntl`(113), with
// `O_NONBLOCK` and `FD_CLOEXEC` on the socket's slot. `dup`(28) answers with an
// alias descriptor, so `OwnedFd::try_clone` works
#[cfg(not(target_os = "trusty"))]
mod net;

// Implementation of stdio file descriptor constants.
mod stdio;

#[cfg(test)]
mod tests;

// Export the types and traits for the public API.
#[stable(feature = "os_fd", since = "1.66.0")]
pub use owned::*;
#[stable(feature = "os_fd", since = "1.66.0")]
pub use raw::*;
#[unstable(feature = "stdio_fd_consts", issue = "150836")]
pub use stdio::*;
