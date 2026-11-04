#![no_std]
#![no_main]

//! chown: changes a file's owner
//!
//! Usage: chown [-R] uid[:gid] file...
//!
//! Numeric ids only. Requires `CAP_CHOWN`, checked by the kernel.

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
    let mut recurse = false;
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-R" | "-r" => recurse = true,
            "-h" | "--help" => { outln("usage: chown [-R] uid[:gid] file..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 && rest.is_empty() => {
                errln(&format!("chown: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.len() < 2 {
        errln("usage: chown [-R] uid[:gid] file...");
        miku::exit(2);
    }

    let (uid, gid) = match parse_owner(rest[0]) {
        Some(p) => p,
        None => { errln(&format!("chown: {}: want a uid, or uid:gid, as numbers", rest[0])); miku::exit(2); }
    };

    let mut bad = false;
    for f in &rest[1..] {
        if let Err(rc) = apply(f, uid, gid, recurse) {
            complain("chown", f, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// `1000` or `1000:1000`. A missing gid is `u32::MAX`: unchanged
fn parse_owner(s: &str) -> Option<(u32, u32)> {
    match s.split_once(':') {
        Some((u, g)) => Some((parse_u64(u)? as u32, parse_u64(g)? as u32)),
        None => Some((parse_u64(s)? as u32, u32::MAX)),
    }
}

fn apply(path: &str, uid: u32, gid: u32, recurse: bool) -> Result<(), i64> {
    let c = cpath(path).ok_or(-36i64)?;
    let rc = unsafe { miku::miku_chown(c.as_ptr(), uid, gid) };
    if rc < 0 { return Err(rc); }
    if !recurse { return Ok(()); }
    let st = stat(path)?;
    if !is_dir(&st) { return Ok(()); }
    for (name, _) in read_dir(path)? {
        if name == "." || name == ".." { continue; }
        apply(&join(path, &name), uid, gid, true)?;
    }
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
