//! miku: the system interface crate
//!
//! What a no_std program includes. The `extern` blocks are generated from the
//! libraries' own signatures (`lib/buildgen/src/decls.rs`), the types are
//! `lib/abi`, and the modules here are safe forms of them. The build script makes
//! the link stubs that `ld-miku` resolves.

#![no_std]
#![allow(dead_code)]

extern crate alloc;

pub mod cred;
pub mod heap;
pub mod power;
pub mod proc;
pub mod session;
pub mod time;
pub mod tty;

mod io;
mod process;
mod start;
mod util;

pub use io::*;
pub use process::*;
pub use util::*;

pub use miku_abi::kstat::*;
pub use miku_abi::libtypes::*;
pub use miku_abi::poll::{POLLERR, POLLHUP, POLLIN, POLLNVAL, POLLOUT};
pub use miku_abi::posix::pollfd as MikuPollFd;

pub const CLOCK_REALTIME: u64 = miku_abi::time::CLOCK_REALTIME as u64;
pub const CLOCK_MONOTONIC: u64 = miku_abi::time::CLOCK_MONOTONIC as u64;

/// What the generated declarations name
mod decls {
    use miku_abi::ctypes::*;
    use miku_abi::dirent::*;
    use miku_abi::fcntl::*;
    use miku_abi::kstat::*;
    use miku_abi::libtypes::*;
    use miku_abi::netdb::*;
    use miku_abi::posix::*;
    use miku_abi::pthread::*;
    use miku_abi::pwd::*;
    use miku_abi::sched::*;
    use miku_abi::signal::*;
    use miku_abi::signal::sigaction as c_sigaction;
    use miku_abi::socket::*;
    use miku_abi::stat::*;
    use miku_abi::statvfs::*;
    use miku_abi::termios::*;
    use miku_abi::time::tm as Tm;
    use miku_abi::posix::timeval as Timeval;
    use miku_abi::posix::pollfd as MikuPollFd;
    use miku_abi::utsname::*;

    include!(concat!(env!("OUT_DIR"), "/decls.rs"));
}

pub use decls::*;

/// Variadic, in `global_asm!`: `lib/runtime/libmiku/text/fmt.rs`
extern "C" {
    pub fn miku_printf(fmt: *const u8, ...) -> i32;
    pub fn miku_snprintf(buf: *mut u8, max: usize, fmt: *const u8, ...) -> i32;
    pub fn miku_dprintf(fd: u64, fmt: *const u8, ...) -> i32;
    pub fn miku_fprintf(fd: u64, fmt: *const u8, ...) -> i32;
}

// Enables `Vec`, `String`, and `Box` over the process heap in core_miku.so
#[global_allocator]
static ALLOC: heap::MikuAlloc = heap::MikuAlloc;

#[macro_export]
macro_rules! cstr {
    ($s:expr) => {
        concat!($s, "\0").as_ptr()
    };
}
