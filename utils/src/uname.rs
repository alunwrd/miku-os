#![no_std]
#![no_main]

//! uname: prints the system name from `/proc/version`
//!
//! Usage: uname [-a]

extern crate alloc;

pub use miku;

use alloc::string::String;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let all = args.iter().skip(1).any(|a| *a == "-a");
    for a in args.iter().skip(1) {
        if *a == "-h" || *a == "--help" { outln("usage: uname [-a]"); miku::exit(0); }
    }

    let text = match slurp("/proc/version") {
        Ok(d) => String::from_utf8_lossy(&d).into_owned(),
        Err(rc) => { complain("uname", "/proc/version", &reason(rc)); miku::exit(1); }
    };
    let first = text.lines().next().unwrap_or("MikuOS");
    if all {
        outln(first);
    } else {
        // The name alone
        outln(first.split_whitespace().next().unwrap_or("MikuOS"));
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
