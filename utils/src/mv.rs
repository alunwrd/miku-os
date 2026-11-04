#![no_std]
#![no_main]

//! mv: renames or moves into a directory
//!
//! Usage: mv [-v] source... destination
//!
//! One `rename`, with no copy fallback, so moves across filesystems fail.

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
    let mut verbose = false;
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-v" => verbose = true,
            "-h" | "--help" => { outln("usage: mv [-v] source... destination"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("mv: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.len() < 2 {
        errln("usage: mv [-v] source... destination");
        miku::exit(2);
    }

    let dest = rest.pop().unwrap();
    let dest_is_dir = stat(dest).map(|s| is_dir(&s)).unwrap_or(false);
    if rest.len() > 1 && !dest_is_dir {
        errln(&format!("mv: {dest}: not a directory, and there is more than one source"));
        miku::exit(2);
    }

    let mut bad = false;
    for src in rest {
        let target = if dest_is_dir { join(dest, basename(src)) } else { String::from(dest) };
        let a = match cpath(src) { Some(c) => c, None => { complain("mv", src, "name too long"); bad = true; continue; } };
        let b = match cpath(&target) { Some(c) => c, None => { complain("mv", &target, "name too long"); bad = true; continue; } };
        let rc = unsafe { miku::miku_rename(a.as_ptr(), b.as_ptr()) };
        if rc < 0 {
            complain("mv", src, &reason(rc));
            bad = true;
        } else if verbose {
            outln(&format!("{src} -> {target}"));
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
