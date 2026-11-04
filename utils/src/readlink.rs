#![no_std]
#![no_main]

//! readlink: prints a symbolic link's target
//!
//! Usage: readlink link...

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
    let mut links: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => { outln("usage: readlink link..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("readlink: unknown option {other}"));
                miku::exit(2);
            }
            other => links.push(other),
        }
    }
    if links.is_empty() { errln("usage: readlink link..."); miku::exit(2); }

    let mut bad = false;
    for l in links {
        let c = match cpath(l) { Some(c) => c, None => { complain("readlink", l, "name too long"); bad = true; continue; } };
        let mut buf = [0u8; 256];
        let n = unsafe { miku::miku_readlink(c.as_ptr(), buf.as_mut_ptr(), buf.len()) };
        if n < 0 {
            complain("readlink", l, &reason(n));
            bad = true;
            continue;
        }
        match core::str::from_utf8(&buf[..n as usize]) {
            Ok(s) => outln(s),
            Err(_) => { complain("readlink", l, "the target is not text"); bad = true; }
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
