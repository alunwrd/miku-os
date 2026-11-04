#![no_std]
#![no_main]

//! ln: creates a hard or symbolic link
//!
//! Usage: ln [-s] target linkname

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut symbolic = false;
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-s" => symbolic = true,
            "-h" | "--help" => { outln("usage: ln [-s] target linkname"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("ln: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.len() != 2 {
        errln("usage: ln [-s] target linkname");
        miku::exit(2);
    }

    let target = rest[0];
    let link = if stat(rest[1]).map(|s| is_dir(&s)).unwrap_or(false) {
        join(rest[1], basename(target))
    } else {
        String::from(rest[1])
    };

    let t = match cpath(target) { Some(c) => c, None => { complain("ln", target, "name too long"); miku::exit(1); } };
    let l = match cpath(&link) { Some(c) => c, None => { complain("ln", &link, "name too long"); miku::exit(1); } };

    let rc = if symbolic {
        unsafe { miku::miku_symlink(t.as_ptr(), l.as_ptr()) }
    } else {
        unsafe { miku::miku_link(t.as_ptr(), l.as_ptr()) }
    };
    if rc < 0 {
        complain("ln", &link, &reason(rc));
        miku::exit(1);
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
