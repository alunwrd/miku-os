#![no_std]
#![no_main]

//! sleep: waits, interruptible by Ctrl-C
//!
//! Usage: sleep seconds

extern crate alloc;

pub use miku;

use alloc::format;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    if args.len() < 2 || args[1] == "-h" || args[1] == "--help" {
        outln("usage: sleep seconds");
        miku::exit(if args.len() < 2 { 2 } else { 0 });
    }
    let secs = match parse_u64(args[1]) {
        Some(s) => s,
        None => { errln(&format!("sleep: {}: not a number of seconds", args[1])); miku::exit(2); }
    };
    unsafe { miku::miku_sleep_ms(secs.saturating_mul(1000)) };
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
