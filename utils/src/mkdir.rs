#![no_std]
#![no_main]

//! mkdir: creates directories
//!
//! Usage: mkdir [-p] [-m mode] dir...

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
    let mut parents = false;
    let mut mode: u32 = 0o755;
    let mut dirs: Vec<&str> = Vec::new();
    let mut want_mode = false;

    for a in args.iter().skip(1) {
        if want_mode {
            match parse_octal(a) {
                Some(m) => mode = m,
                None => { errln(&format!("mkdir: {a}: not an octal mode")); miku::exit(2); }
            }
            want_mode = false;
            continue;
        }
        match *a {
            "-p" => parents = true,
            "-m" => want_mode = true,
            "-h" | "--help" => { outln("usage: mkdir [-p] [-m mode] dir..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("mkdir: unknown option {other}"));
                miku::exit(2);
            }
            other => dirs.push(other),
        }
    }
    if dirs.is_empty() {
        errln("mkdir: nothing to make");
        miku::exit(2);
    }

    let mut bad = false;
    for d in dirs {
        let r = if parents { make_parents(d, mode) } else { make(d, mode) };
        if let Err(rc) = r {
            // With `-p`, an existing directory is not a failure
            if parents && -rc == 17 { continue; }
            complain("mkdir", d, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn make(path: &str, mode: u32) -> Result<(), i64> {
    let c = match cpath(path) { Some(c) => c, None => return Err(-36) };
    let rc = unsafe { miku::miku_mkdir(c.as_ptr(), mode) };
    if rc < 0 { Err(rc) } else { Ok(()) }
}

/// Every component in turn, ignoring the ones that are already there
fn make_parents(path: &str, mode: u32) -> Result<(), i64> {
    let mut so_far = String::new();
    if path.starts_with('/') { so_far.push('/'); }
    for part in path.split('/') {
        if part.is_empty() { continue; }
        if !so_far.is_empty() && !so_far.ends_with('/') { so_far.push('/'); }
        so_far.push_str(part);
        match make(&so_far, mode) {
            Ok(()) => {}
            // Already existing is normal for each component; an existing file
            // fails at the next step
            Err(rc) if -rc == 17 => {}
            Err(rc) => return Err(rc),
        }
    }
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
