#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// parse_miku.so: json, csv, ini, getopt, args

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(heap);
shim!(mem);
shim!(num);
shim!(string);
shim!(utf8);
pub mod json;
pub mod csv;
pub mod ini;
pub mod args;

static PANIC_MSG: &[u8] = b"parse_miku: panic\n";

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
