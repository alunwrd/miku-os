#![no_std]
#![no_main]

//! clear: blanks the terminal through a kernel request, atomically
//!
//! Usage: clear

extern crate alloc;

pub use miku;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(_argc: i32, _argv: *const *const u8) -> ! {
    match miku::tty::clear() {
        Ok(()) => miku::exit(0),
        Err(rc) => { complain("clear", "this terminal", &reason(rc)); miku::exit(1); }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
