#![no_std]
#![no_main]

//! tree: prints a directory tree
//!
//! Usage: tree [-d] [-a] [-L depth] [path]

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
    let mut dirs_only = false;
    let mut all = false;
    let mut max_depth = 16usize;
    let mut root = ".";
    let mut want_depth = false;

    for a in args.iter().skip(1) {
        if want_depth {
            match parse_u64(a) {
                Some(n) => max_depth = n as usize,
                None => { errln(&format!("tree: {a}: not a depth")); miku::exit(2); }
            }
            want_depth = false;
            continue;
        }
        match *a {
            "-d" => dirs_only = true,
            "-a" => all = true,
            "-L" => want_depth = true,
            "-h" | "--help" => { outln("usage: tree [-d] [-a] [-L depth] [path]"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("tree: unknown option {other}"));
                miku::exit(2);
            }
            other => root = other,
        }
    }

    outln(root);
    let mut counts = (0u64, 0u64);
    walk(root, &String::new(), 1, max_depth, dirs_only, all, &mut counts);
    outln(&format!(""));
    outln(&format!("{} directories, {} files", counts.0, counts.1));
    miku::exit(0);
}

fn walk(path: &str, prefix: &str, depth: usize, max: usize,
        dirs_only: bool, all: bool, counts: &mut (u64, u64)) {
    if depth > max { return; }
    let entries = match read_dir(path) {
        Ok(e) => e,
        Err(rc) => { outln(&format!("{prefix}    [{}]", reason(rc))); return; }
    };
    let shown: Vec<(String, u8)> = entries.into_iter()
        .filter(|(n, _)| n != "." && n != "..")
        .filter(|(n, _)| all || !n.starts_with('.'))
        .filter(|(_, k)| !dirs_only || *k == KIND_DIR)
        .collect();

    for (i, (name, kind)) in shown.iter().enumerate() {
        let last = i + 1 == shown.len();
        outln(&format!("{prefix}{}{name}", if last { "`-- " } else { "|-- " }));
        if *kind == KIND_DIR {
            counts.0 += 1;
            let deeper = format!("{prefix}{}", if last { "    " } else { "|   " });
            walk(&join(path, name), &deeper, depth + 1, max, dirs_only, all, counts);
        } else {
            counts.1 += 1;
        }
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
