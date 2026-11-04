#![no_std]
#![no_main]

//! head: first lines of a file
//!
//! Usage: head [-n count] [file...]

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
    let mut count = 10u64;
    let mut files: Vec<&str> = Vec::new();
    let mut want_count = false;

    for a in args.iter().skip(1) {
        if want_count {
            match parse_u64(a) {
                Some(n) => count = n,
                None => { errln(&format!("head: {a}: not a number")); miku::exit(2); }
            }
            want_count = false;
            continue;
        }
        match *a {
            "-n" => want_count = true,
            "-h" | "--help" => { outln("usage: head [-n count] [file...]"); miku::exit(0); }
            "-" => files.push("-"),
            // `-5` is older than `-n 5` and still what fingers type
            other if other.starts_with('-') && other.len() > 1 => {
                match parse_u64(&other[1..]) {
                    Some(n) => count = n,
                    None => { errln(&format!("head: unknown option {other}")); miku::exit(2); }
                }
            }
            other => files.push(other),
        }
    }
    if files.is_empty() { files.push("-"); }
    let many = files.len() > 1;

    let mut bad = false;
    for (i, f) in files.iter().enumerate() {
        let data = match slurp(f) {
            Ok(d) => d,
            Err(rc) => { complain("head", f, &reason(rc)); bad = true; continue; }
        };
        if many {
            if i > 0 { outln(""); }
            outln(&format!("==> {f} <=="));
        }
        let mut seen = 0u64;
        for line in data.split_inclusive(|b| *b == b'\n') {
            if seen >= count { break; }
            let _ = write_all(1, line);
            seen += 1;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
