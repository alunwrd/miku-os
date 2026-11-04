#![no_std]
#![no_main]

//! tee: copies input to files and output
//!
//! Usage: tee [-a] file...
//!
//! Continues when one file fails.

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
    let mut append = false;
    let mut names: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-a" => append = true,
            "-h" | "--help" => { outln("usage: tee [-a] file..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("tee: unknown option {other}"));
                miku::exit(2);
            }
            other => names.push(other),
        }
    }

    let mut bad = false;
    let mut fds: Vec<i64> = Vec::new();
    for n in &names {
        let flags = if append { O_WRITE | O_CREATE | O_APPEND } else { O_WRITE | O_CREATE | O_TRUNC };
        match open(n, flags, 0o644) {
            Ok(fd) => fds.push(fd),
            Err(rc) => { complain("tee", n, &reason(rc)); bad = true; }
        }
    }

    let mut buf = [0u8; 4096];
    loop {
        let n = miku::read(0, &mut buf);
        if n < 0 { complain("tee", "-", &reason(n)); bad = true; break; }
        if n == 0 { break; }
        let chunk = &buf[..n as usize];
        let _ = write_all(1, chunk);
        for fd in &fds {
            if write_all(*fd, chunk).is_err() { bad = true; }
        }
    }
    for fd in fds { miku::close(fd); }
    miku::exit(if bad { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
