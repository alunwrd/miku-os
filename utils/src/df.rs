#![no_std]
#![no_main]

//! df: free space on a filesystem
//!
//! Usage: df [-k] [-b] [path...]
//!
//! One line per path, in kilobytes, or bytes with `-b`. `-k` is accepted for
//! POSIX. With no path, the root.

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
    let mut bytes = false;
    let mut paths: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-b" => bytes = true,
            "-k" => bytes = false,
            "-h" | "--help" => { outln("usage: df [-k] [-b] [path...]"); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("df: unknown option {other}"));
                miku::exit(2);
            }
            other => paths.push(other),
        }
    }
    if paths.is_empty() { paths.push("/"); }

    outln(&format!("{} {} {} {} {} {}",
                   pad_right("Filesystem", 12),
                   pad_left(if bytes { "bytes" } else { "1K-blocks" }, 11),
                   pad_left("Used", 9),
                   pad_left("Available", 9),
                   pad_left("Use%", 4),
                   "Mounted on"));

    let mut bad = false;
    for p in paths {
        let c = match cpath(p) { Some(c) => c, None => { complain("df", p, "name too long"); bad = true; continue; } };
        let mut sf = miku::MikuStatfs::default();
        let rc = unsafe { miku::miku_statfs(c.as_ptr(), &mut sf as *mut _ as *mut u8) };
        if rc < 0 { complain("df", p, &reason(rc)); bad = true; continue; }

        let bs = sf.block_size.max(1) as u64;
        let unit = if bytes { 1 } else { 1024 };
        let total = sf.total_blocks.saturating_mul(bs) / unit;
        let free  = sf.free_blocks.saturating_mul(bs) / unit;
        let used  = total.saturating_sub(free);
        let pct   = if total == 0 { 0 } else { used * 100 / total };
        outln(&format!("{} {} {} {} {} {}",
                       pad_right(fs_name(sf.magic), 12),
                       pad_left(&format!("{total}"), 11),
                       pad_left(&format!("{used}"), 9),
                       pad_left(&format!("{free}"), 9),
                       pad_left(&format!("{pct}%"), 4),
                       p));
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// The magic numbers the kernel's `FsType::magic()` hands out
fn fs_name(magic: u32) -> &'static str {
    match magic {
        0xEF53     => "ext",
        0x01021994 => "tmpfs",
        0x1373     => "devfs",
        0x9fa0     => "procfs",
        _          => "fs",
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
