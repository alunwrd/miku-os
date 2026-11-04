#![no_std]
#![no_main]

//! which: locates a command
//!
//! Usage: which name...
//!
//! Looks in `/bin` only, since there is no PATH.

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
    let mut names: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => { outln("usage: which name..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("which: unknown option {other}"));
                miku::exit(2);
            }
            other => names.push(other),
        }
    }
    if names.is_empty() { errln("usage: which name..."); miku::exit(2); }

    let mut missing = false;
    for n in names {
        let path = if n.contains('/') { alloc::string::String::from(n) } else { join("/bin", n) };
        match stat(&path) {
            Ok(st) if !is_dir(&st) => outln(&path),
            _ => { errln(&format!("which: {n}: not in /bin")); missing = true; }
        }
    }
    miku::exit(if missing { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
