#![no_std]
#![no_main]
// `...` parameters and `VaList` for lib/runtime/libmiku/libc/printf.rs
#![feature(c_variadic)]
#![allow(dead_code, unused)]

// libc_miku.so: POSIX/C compatibility layer

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(mem);
shim!(heap);
shim!(string);
shim!(ctype);
shim!(convert);
shim!(num);
shim!(io);
shim!(file);
shim!(proc);
shim!(env);
shim!(signal);
shim!(time);
shim!(math);
shim!(random);
shim!(errno);
shim!(auxv);
pub mod libc;
pub mod auxv_h;
pub mod ctype_h;
pub mod dirent_h;
pub mod dlfcn_h;
pub mod errno_h;
pub mod fcntl_h;
pub mod inet_h;
pub mod inotify_h;
pub mod locale_h;
pub mod mman_h;
pub mod poll_h;
pub mod resource_h;
pub mod select_h;
pub mod signal_h;
pub mod socket_h;
pub mod stat_h;
pub mod statfs_h;
pub mod stdio_h;
pub mod stdlib_h;
pub mod string_h;
pub mod uio_h;
pub mod unistd_h;
pub mod wctype_h;
// C math library, using musl's algorithms
pub mod cmath;
// C-facing areas (dev/c-architecture.md)
pub mod printf;
pub mod strtod;
pub mod ctime;
pub mod pthread;
pub mod netdb;
pub mod termios;
pub mod scanf;
pub mod pty;
pub mod accounts;
pub mod sysinfo;
pub mod at;
pub mod setjmp;

static PANIC_MSG: &[u8] = b"libc_miku: panic\n";

#[no_mangle]
#[link_section = ".text._libmiku_start"]
pub extern "C" fn _libmiku_start() -> ! { loop {} }

#[panic_handler]
fn rust_panic(_: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys::sc3(sys::SYS_WRITE, 2, PANIC_MSG.as_ptr() as u64, PANIC_MSG.len() as u64);
        sys::sc1(sys::SYS_EXIT, 127);
    }
    loop {}
}
