//! MikuOS-specific extensions to primitives in the [`std::process`] module.
//!
//! [`std::process`]: crate::process

#![stable(feature = "rust1", since = "1.0.0")]
// Stable: these items are reached through `std::os::unix::{ffi,fs,process}`,
// which is stable on every Unix, and crates on stable Rust (`rustix`, and
// through it `polling`, `async-io`, `smol`) cannot name an unstable item.
// MikuOS-only additions carry `#[unstable]` themselves.

use crate::ffi::OsStr;
use crate::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use crate::process::{self, ExitStatus};
use crate::sys::process::ChildPipe;
use crate::sys::{AsInner, AsInnerMut, FromInner, IntoInner};
use crate::{io, sys};

/// What a child does to itself between `fork` and `execve`.
///
/// `setpgid`(101), `setgid`(53), `setuid`(52), in that order, then the
/// `pre_exec` closures. `spawn` reports a failed step as its error, with the
/// kernel's errno. Changing identity is root's; anybody else gets `EPERM`, and
/// there is no setuid bit.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait CommandExt {
    #[stable(feature = "rust1", since = "1.0.0")]
    fn uid(&mut self, id: u32) -> &mut process::Command;

    #[stable(feature = "rust1", since = "1.0.0")]
    fn gid(&mut self, id: u32) -> &mut process::Command;

    /// Runs in the child after the fork. The child is single-threaded, since
    /// `fork` copies only the caller; allocation is still best avoided
    #[stable(feature = "process_pre_exec", since = "1.34.0")]
    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut process::Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static;

    /// Become the program in this process. Returns only if that failed.
    #[stable(feature = "process_exec2", since = "1.9.0")]
    fn exec(&mut self) -> io::Error;

    #[stable(feature = "process_set_argv0", since = "1.45.0")]
    fn arg0<S>(&mut self, arg: S) -> &mut process::Command
    where
        S: AsRef<OsStr>;

    /// `setpgid(0, pgroup)` in the child; 0 makes it the leader of a new group.
    #[stable(feature = "process_set_process_group", since = "1.64.0")]
    fn process_group(&mut self, pgroup: i32) -> &mut process::Command;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl CommandExt for process::Command {
    fn uid(&mut self, id: u32) -> &mut process::Command {
        self.as_inner_mut().uid(id);
        self
    }

    fn gid(&mut self, id: u32) -> &mut process::Command {
        self.as_inner_mut().gid(id);
        self
    }

    unsafe fn pre_exec<F>(&mut self, f: F) -> &mut process::Command
    where
        F: FnMut() -> io::Result<()> + Send + Sync + 'static,
    {
        self.as_inner_mut().pre_exec(Box::new(f));
        self
    }

    fn exec(&mut self) -> io::Error {
        self.as_inner_mut().exec(sys::process::Stdio::Inherit)
    }

    fn arg0<S>(&mut self, arg: S) -> &mut process::Command
    where
        S: AsRef<OsStr>,
    {
        self.as_inner_mut().set_arg_0(arg.as_ref());
        self
    }

    fn process_group(&mut self, pgroup: i32) -> &mut process::Command {
        self.as_inner_mut().pgroup(pgroup);
        self
    }
}

/// What a status word says beyond "did it succeed".
///
/// The kernel keeps the signal beside the code, so `signal()` reads a recorded
/// fact. `128 + sig` cannot be told from `exit(148)`.
#[stable(feature = "rust1", since = "1.0.0")]
pub trait ExitStatusExt {
    /// The raw POSIX status word `wait4`(44) wrote.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn into_raw(self) -> i32;
    /// The signal that ended it, if one did.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn signal(&self) -> Option<i32>;
    /// The signal that stopped it, if it is stopped.
    #[stable(feature = "rust1", since = "1.0.0")]
    fn stopped_signal(&self) -> Option<i32>;
    /// Always false: MikuOS writes a core file for a crash
    /// (`/tmp/core.<name>.<pid>`), but the status word has no bit for it
    #[stable(feature = "rust1", since = "1.0.0")]
    fn core_dumped(&self) -> bool;
}

#[stable(feature = "rust1", since = "1.0.0")]
impl ExitStatusExt for ExitStatus {
    fn into_raw(self) -> i32 {
        self.as_inner().into_raw()
    }
    fn signal(&self) -> Option<i32> {
        self.as_inner().signal()
    }
    fn stopped_signal(&self) -> Option<i32> {
        self.as_inner().stopped_signal()
    }
    fn core_dumped(&self) -> bool {
        false
    }
}

// The child's pipes, and `Stdio` from a descriptor. A child pipe is a
// `FileDesc` on MikuOS, so these are upstream's

#[stable(feature = "process_extensions", since = "1.2.0")]
impl FromRawFd for process::Stdio {
    #[inline]
    unsafe fn from_raw_fd(fd: RawFd) -> process::Stdio {
        let fd = unsafe { sys::fd::FileDesc::from_raw_fd(fd) };
        process::Stdio::from_inner(sys::process::Stdio::Fd(fd))
    }
}

#[stable(feature = "io_safety", since = "1.63.0")]
impl From<OwnedFd> for process::Stdio {
    #[inline]
    fn from(fd: OwnedFd) -> process::Stdio {
        let fd = sys::fd::FileDesc::from_inner(fd);
        process::Stdio::from_inner(sys::process::Stdio::Fd(fd))
    }
}

macro_rules! child_pipe {
    ($t:ident) => {
        #[stable(feature = "process_extensions", since = "1.2.0")]
        impl AsRawFd for process::$t {
            #[inline]
            fn as_raw_fd(&self) -> RawFd {
                self.as_inner().as_raw_fd()
            }
        }

        #[stable(feature = "into_raw_os", since = "1.4.0")]
        impl IntoRawFd for process::$t {
            #[inline]
            fn into_raw_fd(self) -> RawFd {
                self.into_inner().into_inner().into_raw_fd()
            }
        }

        #[stable(feature = "io_safety", since = "1.63.0")]
        impl AsFd for process::$t {
            #[inline]
            fn as_fd(&self) -> BorrowedFd<'_> {
                self.as_inner().as_fd()
            }
        }

        #[stable(feature = "io_safety", since = "1.63.0")]
        impl From<process::$t> for OwnedFd {
            #[inline]
            fn from(p: process::$t) -> OwnedFd {
                p.into_inner().into_inner()
            }
        }

        #[stable(feature = "child_stream_from_fd", since = "1.74.0")]
        impl From<OwnedFd> for process::$t {
            #[inline]
            fn from(fd: OwnedFd) -> process::$t {
                process::$t::from_inner(ChildPipe::from_inner(fd))
            }
        }
    };
}

child_pipe!(ChildStdin);
child_pipe!(ChildStdout);
child_pipe!(ChildStderr);
