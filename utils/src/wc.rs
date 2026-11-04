#![no_std]
#![no_main]

//! wc: counts lines, words, and bytes
//!
//! Usage: wc [-l] [-w] [-c] [file...]

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
    let (mut want_l, mut want_w, mut want_c) = (false, false, false);
    let mut files: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-l" => want_l = true,
            "-w" => want_w = true,
            "-c" => want_c = true,
            "-h" | "--help" => { outln("usage: wc [-l] [-w] [-c] [file...]"); miku::exit(0); }
            "-" => files.push("-"),
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("wc: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }
    if !want_l && !want_w && !want_c { want_l = true; want_w = true; want_c = true; }
    if files.is_empty() { files.push("-"); }

    let (mut tl, mut tw, mut tc) = (0u64, 0u64, 0u64);
    let many = files.len() > 1;
    let mut bad = false;

    for f in &files {
        let data = match slurp(f) {
            Ok(d) => d,
            Err(rc) => { complain("wc", f, &reason(rc)); bad = true; continue; }
        };
        let (l, w, c) = count(&data);
        tl += l; tw += w; tc += c;
        outln(&line(l, w, c, want_l, want_w, want_c, f));
    }
    if many { outln(&line(tl, tw, tc, want_l, want_w, want_c, "total")); }
    miku::exit(if bad { 1 } else { 0 });
}

/// Lines are newlines, words are runs of non-space. A last line with no newline
/// is not counted, as in other `wc`s
fn count(data: &[u8]) -> (u64, u64, u64) {
    let mut lines = 0u64;
    let mut words = 0u64;
    let mut in_word = false;
    for &b in data {
        if b == b'\n' { lines += 1; }
        let space = b == b' ' || b == b'\t' || b == b'\n' || b == b'\r';
        if space { in_word = false; }
        else if !in_word { in_word = true; words += 1; }
    }
    (lines, words, data.len() as u64)
}

fn line(l: u64, w: u64, c: u64, want_l: bool, want_w: bool, want_c: bool, name: &str) -> String {
    let mut s = String::new();
    if want_l { s.push_str(&pad_left(&format!("{l}"), 7)); }
    if want_w { s.push_str(&pad_left(&format!("{w}"), 8)); }
    if want_c { s.push_str(&pad_left(&format!("{c}"), 9)); }
    if name != "-" { s.push(' '); s.push_str(name); }
    s
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
