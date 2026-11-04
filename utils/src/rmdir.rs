#![no_std]
#![no_main]

//! rmdir: removes empty directories
//!
//! Usage: rmdir dir...

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut dirs: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => { outln("usage: rmdir dir..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("rmdir: unknown option {other}"));
                miku::exit(2);
            }
            other => dirs.push(other),
        }
    }
    if dirs.is_empty() { errln("rmdir: nothing to remove"); miku::exit(2); }

    let mut bad = false;
    for d in dirs {
        let c = match cpath(d) { Some(c) => c, None => { complain("rmdir", d, "name too long"); bad = true; continue; } };
        let rc = unsafe { miku::miku_rmdir(c.as_ptr()) };
        if rc < 0 { complain("rmdir", d, &reason(rc)); bad = true; }
    }
    miku::exit(if bad { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
