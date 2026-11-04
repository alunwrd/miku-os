#![no_std]
#![no_main]

//! grep: lines containing a substring
//!
//! Usage: grep [-i] [-n] [-v] [-c] pattern [file...]
//!
//! Plain substrings, not regular expressions.

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
    let (mut icase, mut numbers, mut invert, mut only_count) = (false, false, false, false);
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-i" => icase = true,
            "-n" => numbers = true,
            "-v" => invert = true,
            "-c" => only_count = true,
            "-in" | "-ni" => { icase = true; numbers = true; }
            "-h" | "--help" => { outln("usage: grep [-i] [-n] [-v] [-c] pattern [file...]"); miku::exit(0); }
            "-" => rest.push("-"),
            other if other.starts_with('-') && other.len() > 1 && rest.is_empty() => {
                errln(&format!("grep: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.is_empty() {
        errln("usage: grep [-i] [-n] [-v] [-c] pattern [file...]");
        miku::exit(2);
    }

    let pattern = if icase { lower(rest[0]) } else { String::from(rest[0]) };
    let files: Vec<&str> = if rest.len() > 1 { rest[1..].to_vec() } else { alloc::vec!["-"] };
    let many = files.len() > 1;

    let mut found_any = false;
    let mut bad = false;

    for f in &files {
        let data = match slurp(f) {
            Ok(d) => d,
            Err(rc) => { complain("grep", f, &reason(rc)); bad = true; continue; }
        };
        let mut hits = 0u64;
        for (i, raw) in data.split(|b| *b == b'\n').enumerate() {
            // Lossy, so one bad byte does not reject a file
            let line = String::from_utf8_lossy(raw);
            if raw.is_empty() && i > 0 { continue; }
            let hay = if icase { lower(&line) } else { String::from(line.as_ref()) };
            if hay.contains(pattern.as_str()) == invert { continue; }
            hits += 1;
            found_any = true;
            if only_count { continue; }
            let mut s = String::new();
            if many { s.push_str(f); s.push(':'); }
            if numbers { s.push_str(&format!("{}:", i + 1)); }
            s.push_str(&line);
            outln(&s);
        }
        if only_count {
            if many { outln(&format!("{f}:{hits}")); } else { outln(&format!("{hits}")); }
        }
    }

    // 0 found, 1 not found, 2 error
    if bad { miku::exit(2); }
    miku::exit(if found_any { 0 } else { 1 });
}

fn lower(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() { out.push(c.to_ascii_lowercase()); }
    out
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
