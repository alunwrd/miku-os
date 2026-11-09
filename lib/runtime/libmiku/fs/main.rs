#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// fs_miku.so: files, buffered io, directories, paths

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(errno);
shim!(heap);
shim!(io);
shim!(mem);
shim!(string);
pub mod file;
pub mod bufio;
pub mod dir;
pub mod path;

static PANIC_MSG: &[u8] = b"fs_miku: panic\n";

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
