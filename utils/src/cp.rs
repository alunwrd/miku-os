#![no_std]
#![no_main]

//! cp: copies a file or tree
//!
//! Usage: cp [-r] [-v] source... destination
//!
//! The destination is a directory if it is one, otherwise a name. The mode is
//! preserved, timestamps are not.

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
    let mut recurse = false;
    let mut verbose = false;
    let mut rest: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-r" | "-R" => recurse = true,
            "-v" => verbose = true,
            "-rv" | "-vr" => { recurse = true; verbose = true; }
            "-h" | "--help" => { outln("usage: cp [-r] [-v] source... destination"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("cp: unknown option {other}"));
                miku::exit(2);
            }
            other => rest.push(other),
        }
    }
    if rest.len() < 2 {
        errln("usage: cp [-r] [-v] source... destination");
        miku::exit(2);
    }

    let dest = rest.pop().unwrap();
    let dest_is_dir = stat(dest).map(|s| is_dir(&s)).unwrap_or(false);
    if rest.len() > 1 && !dest_is_dir {
        errln(&format!("cp: {dest}: not a directory, and there is more than one source"));
        miku::exit(2);
    }

    let mut bad = false;
    for src in rest {
        let target = if dest_is_dir { join(dest, basename(src)) } else { String::from(dest) };
        if let Err((path, rc)) = copy(src, &target, recurse, verbose) {
            complain("cp", &path, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// The failure and the path it happened on
type Failed = (String, i64);

fn on(path: &str) -> impl Fn(i64) -> Failed + '_ {
    move |rc| (String::from(path), rc)
}

fn copy(src: &str, dst: &str, recurse: bool, verbose: bool) -> Result<(), Failed> {
    let st = stat(src).map_err(on(src))?;
    if is_dir(&st) {
        if !recurse { return Err(on(src)(-21)); }
        let c = cpath(dst).ok_or(-36i64).map_err(on(dst))?;
        let rc = unsafe { miku::miku_mkdir(c.as_ptr(), st.mode) };
        if rc < 0 && -rc != 17 { return Err(on(dst)(rc)); }
        for (name, _) in read_dir(src).map_err(on(src))? {
            if name == "." || name == ".." { continue; }
            copy(&join(src, &name), &join(dst, &name), recurse, verbose)?;
        }
        return Ok(());
    }

    // Chunked, costing one buffer. The first chunk is read before opening the
    // destination, since opening truncates it
    let input = open(src, O_READ, 0).map_err(on(src))?;
    let mut buf = [0u8; 4096];
    let first = unsafe { miku::miku_read(input as u64, buf.as_mut_ptr(), buf.len()) };
    if first < 0 {
        miku::close(input);
        return Err(on(src)(first));
    }
    let output = match open(dst, O_WRITE | O_CREATE | O_TRUNC, st.mode) {
        Ok(fd) => fd,
        Err(e) => { miku::close(input); return Err(on(dst)(e)); }
    };
    let mut n = first;
    let r = loop {
        if n == 0 { break Ok(()); }
        if let Err(e) = write_all(output, &buf[..n as usize]) { break Err(on(dst)(e)); }
        n = unsafe { miku::miku_read(input as u64, buf.as_mut_ptr(), buf.len()) };
        if n < 0 { break Err(on(src)(n)); }
    };
    miku::close(input);
    miku::close(output);
    r?;
    if verbose { outln(&format!("{src} -> {dst}")); }
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
