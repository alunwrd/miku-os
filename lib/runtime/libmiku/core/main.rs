#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// core_miku.so: syscalls, errno, sync, memory, heap, process, threads,
// strings, stdio

pub use miku_abi::sys;
pub mod errno;
pub mod sync;
pub mod panic;
pub mod mem;
pub mod heap_core;
pub mod heap;
pub mod io;
pub mod proc;
pub mod thread;
pub mod string;
pub mod ctype;
pub mod convert;
pub mod num;
pub mod stdio;
pub mod time;
pub mod auxv;

#[no_mangle]
#[link_section = ".text._libmiku_start"]
pub extern "C" fn _libmiku_start() -> ! { loop {} }

#[panic_handler]
fn rust_panic(_: &core::panic::PanicInfo) -> ! {
    io::miku_write(2, b"core_miku: panic\n".as_ptr(), 17);
    proc::miku_exit(127);
}
