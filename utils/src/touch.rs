#![no_std]
#![no_main]

//! touch: creates a file or sets its times to now
//!
//! Usage: touch file...

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
    let mut files: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => { outln("usage: touch file..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("touch: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }
    if files.is_empty() { errln("touch: nothing to touch"); miku::exit(2); }

    let mut bad = false;
    for f in files {
        if stat(f).is_ok() {
            if let Err(rc) = stamp_now(f) {
                complain("touch", f, &reason(rc));
                bad = true;
            }
            continue;
        }
        // Created, not truncated, in case it appeared since the stat
        match open(f, O_WRITE | O_CREATE, 0o644) {
            Ok(fd) => miku::close(fd),
            Err(rc) => { complain("touch", f, &reason(rc)); bad = true; }
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// A null `times` sets both to now
fn stamp_now(path: &str) -> Result<(), i64> {
    let c = cpath(path).ok_or(-36i64)?;
    let rc = unsafe { miku::miku_utimensat(c.as_ptr(), -1, core::ptr::null(), 0) };
    if rc < 0 { Err(rc) } else { Ok(()) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
