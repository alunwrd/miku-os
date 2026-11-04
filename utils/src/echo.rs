#![no_std]
#![no_main]

//! echo: prints its arguments
//!
//! Usage: echo [-n] [word...]

extern crate alloc;

pub use miku;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut start = 1usize;
    let mut newline = true;
    if args.len() > 1 && args[1] == "-n" { newline = false; start = 2; }

    for (i, a) in args.iter().enumerate().skip(start) {
        if i > start { out(" "); }
        out(a);
    }
    if newline { outln(""); }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
