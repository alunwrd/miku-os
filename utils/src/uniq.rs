#![no_std]
#![no_main]

//! uniq: drops repeated adjacent lines
//!
//! Usage: uniq [-c] [-d] [file]

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
    let (mut counts, mut only_dups) = (false, false);
    let mut files: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-c" => counts = true,
            "-d" => only_dups = true,
            "-h" | "--help" => { outln("usage: uniq [-c] [-d] [file]"); miku::exit(0); }
            "-" => files.push("-"),
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("uniq: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }
    if files.is_empty() { files.push("-"); }

    let mut bad = false;
    for f in &files {
        let data = match slurp(f) {
            Ok(d) => d,
            Err(rc) => { complain("uniq", f, &reason(rc)); bad = true; continue; }
        };
        let text = String::from_utf8_lossy(&data);
        let mut prev: Option<String> = None;
        let mut run = 0u64;
        for line in text.lines() {
            match &prev {
                Some(p) if p == line => { run += 1; }
                Some(p) => { emit(p, run, counts, only_dups); prev = Some(String::from(line)); run = 1; }
                None => { prev = Some(String::from(line)); run = 1; }
            }
        }
        if let Some(p) = &prev { emit(p, run, counts, only_dups); }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn emit(line: &str, run: u64, counts: bool, only_dups: bool) {
    if only_dups && run < 2 { return; }
    if counts { outln(&format!("{:>7} {line}", run)); } else { outln(line); }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
