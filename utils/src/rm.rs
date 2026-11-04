#![no_std]
#![no_main]

//! rm: removes files, and directories with `-r`
//!
//! Usage: rm [-r] [-f] [-v] path...

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
    let mut force = false;
    let mut verbose = false;
    let mut paths: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-r" | "-R" => recurse = true,
            "-f" => force = true,
            "-v" => verbose = true,
            "-rf" | "-fr" => { recurse = true; force = true; }
            "-h" | "--help" => { outln("usage: rm [-r] [-f] [-v] path..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("rm: unknown option {other}"));
                miku::exit(2);
            }
            other => paths.push(other),
        }
    }
    if paths.is_empty() {
        if force { miku::exit(0); }   // `rm -f` with nothing to do did nothing
        errln("rm: nothing to remove");
        miku::exit(2);
    }

    let mut bad = false;
    for p in paths {
        if let Err(rc) = remove(p, recurse, verbose) {
            // `-f` silences only a missing file
            if force && -rc == 2 { continue; }
            complain("rm", p, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn remove(path: &str, recurse: bool, verbose: bool) -> Result<(), i64> {
    let st = stat(path)?;
    if is_dir(&st) {
        if !recurse {
            return Err(-21);          // EISDIR
        }
        // Depth first
        for (name, _) in read_dir(path)? {
            if name == "." || name == ".." { continue; }
            let child = join(path, &name);
            remove(&child, recurse, verbose)?;
        }
        let c = cpath(path).ok_or(-36i64)?;
        let rc = unsafe { miku::miku_rmdir(c.as_ptr()) };
        if rc < 0 { return Err(rc); }
        if verbose { outln(&format!("removed directory {path}")); }
        return Ok(());
    }

    let c = cpath(path).ok_or(-36i64)?;
    let rc = unsafe { miku::miku_unlink(c.as_ptr()) };
    if rc < 0 { return Err(rc); }
    if verbose { outln(&format!("removed {path}")); }
    Ok(())
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
