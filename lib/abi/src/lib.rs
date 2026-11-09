//! The MikuOS ABI: what the kernel, the libraries, and programs must agree on
//!
//! Three contracts, one file per concern:
//!
//! - C's view, one file per POSIX header. The builder writes
//!   `bits/<file>.h` from the constants above a file's marker line
//!   (`builder/src/stdport/cheaders.rs`), and `libc` re-exports all of it.
//! - The kernel's own shapes, in `k*` and the system-call files: `sys`, `kstat`,
//!   `ktermios`, `ksignal`, `kproc`, `cred`, `futex`, `elf`, `auxv`, `vvar`, `tcb`.
//!   Where the kernel's form differs from Linux's, `libc_miku` translates.
//! - The types `miku_*` functions take and return by value, in `libtypes`.
//!
//! Constants have C types, because `libc`'s public API fixes them. A site wanting
//! another type casts.

#![no_std]
#![allow(non_camel_case_types, non_snake_case)]

#[macro_use]
mod macros;

pub mod ctypes;
pub mod posix;
pub mod types;

pub mod dirent;
pub mod dlfcn;
pub mod errno;
pub mod fcntl;
pub mod inet;
pub mod inotify;
pub mod ioctl;
pub mod limits;
pub mod locale;
pub mod mman;
pub mod netdb;
pub mod poll;
pub mod pthread;
pub mod pwd;
pub mod random;
pub mod resource;
pub mod sched;
pub mod select;
pub mod signal;
pub mod socket;
pub mod stat;
pub mod statvfs;
pub mod stdlib;
pub mod termios;
pub mod time;
pub mod ucontext;
pub mod unistd;
pub mod utsname;
pub mod wait;

pub mod auxv;
pub mod cred;
pub mod elf;
pub mod futex;
pub mod kproc;
pub mod libtypes;
pub mod ksignal;
pub mod kstat;
pub mod ktermios;
pub mod sys;
pub mod tcb;
pub mod vvar;

/// Every name in C's view, for the files here to refer to each other
#[allow(unused_imports)]
mod prelude {
    pub use core::prelude::v1::*;
    pub use super::ctypes::*;
    pub use super::dirent::*;
    pub use super::dlfcn::*;
    pub use super::fcntl::*;
    pub use super::inet::*;
    pub use super::inotify::*;
    pub use super::ioctl::*;
    pub use super::limits::*;
    pub use super::locale::*;
    pub use super::mman::*;
    pub use super::netdb::*;
    pub use super::poll::*;
    pub use super::posix::*;
    pub use super::pthread::*;
    pub use super::pwd::*;
    pub use super::random::*;
    pub use super::resource::*;
    pub use super::sched::*;
    pub use super::select::*;
    pub use super::signal::*;
    pub use super::socket::*;
    pub use super::stat::*;
    pub use super::statvfs::*;
    pub use super::stdlib::*;
    pub use super::termios::*;
    pub use super::time::*;
    pub use super::types::*;
    pub use super::ucontext::*;
    pub use super::unistd::*;
    pub use super::utsname::*;
    pub use super::wait::*;
}
