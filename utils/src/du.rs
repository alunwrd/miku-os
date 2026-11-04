#![no_std]
#![no_main]

//! du: size of a tree
//!
//! Usage: du [-s] [-b] [path...]
//!
//! Kilobytes by default, bytes with `-b`, summed from file sizes, not blocks.

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
    let mut summary = false;
    let mut bytes = false;
    let mut paths: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-s" => summary = true,
            "-b" => bytes = true,
            "-h" | "--help" => { outln("usage: du [-s] [-b] [path...]"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("du: unknown option {other}"));
                miku::exit(2);
            }
            other => paths.push(other),
        }
    }
    if paths.is_empty() { paths.push("."); }

    let mut bad = false;
    for p in paths {
        match walk(p, summary, bytes, true) {
            Ok(total) => {
                if summary { outln(&format!("{}\t{}", scale(total, bytes), p)); }
            }
            Err(rc) => { complain("du", p, &reason(rc)); bad = true; }
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn walk(path: &str, summary: bool, bytes: bool, top: bool) -> Result<u64, i64> {
    let st = stat(path)?;
    if !is_dir(&st) {
        if !summary && top { outln(&format!("{}\t{}", scale(st.size, bytes), path)); }
        return Ok(st.size);
    }
    let mut total = 0u64;
    for (name, _) in read_dir(path)? {
        if name == "." || name == ".." { continue; }
        let child = join(path, &name);
        // An unreadable child prints an error and counts as zero
        match walk(&child, summary, bytes, false) {
            Ok(n) => total += n,
            Err(rc) => complain("du", &child, &reason(rc)),
        }
    }
    if !summary { outln(&format!("{}\t{}", scale(total, bytes), path)); }
    Ok(total)
}

fn scale(n: u64, bytes: bool) -> u64 {
    if bytes { n } else { n.div_ceil(1024) }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
