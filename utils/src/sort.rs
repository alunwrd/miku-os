#![no_std]
#![no_main]

//! sort: sorts lines
//!
//! Usage: sort [-r] [-u] [-n] [file...]
//!
//! Byte order, or numeric with `-n`. No locale.

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
    let (mut reverse, mut unique, mut numeric) = (false, false, false);
    let mut files: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-r" => reverse = true,
            "-u" => unique = true,
            "-n" => numeric = true,
            "-h" | "--help" => { outln("usage: sort [-r] [-u] [-n] [file...]"); miku::exit(0); }
            "-" => files.push("-"),
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("sort: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }
    if files.is_empty() { files.push("-"); }

    let mut lines: Vec<String> = Vec::new();
    let mut bad = false;
    for f in &files {
        match slurp(f) {
            Ok(d) => {
                let text = String::from_utf8_lossy(&d);
                for l in text.lines() { lines.push(String::from(l)); }
            }
            Err(rc) => { complain("sort", f, &reason(rc)); bad = true; }
        }
    }

    if numeric {
        lines.sort_by_key(|l| leading_number(l));
    } else {
        lines.sort();
    }
    if reverse { lines.reverse(); }

    let mut last: Option<&String> = None;
    for l in &lines {
        if unique {
            if last == Some(l) { continue; }
            last = Some(l);
        }
        outln(l);
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// Leading number, or zero
fn leading_number(s: &str) -> u64 {
    let digits: String = s.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    parse_u64(&digits).unwrap_or(0)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
