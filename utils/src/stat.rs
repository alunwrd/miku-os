#![no_std]
#![no_main]

//! stat: prints file metadata
//!
//! Usage: stat file...

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
    let mut files: Vec<&str> = Vec::new();
    for a in args.iter().skip(1) {
        match *a {
            "-h" | "--help" => { outln("usage: stat file..."); miku::exit(0); }
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("stat: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }
    if files.is_empty() { errln("usage: stat file..."); miku::exit(2); }

    let accounts = Accounts::load();
    let mut bad = false;
    for (i, f) in files.iter().enumerate() {
        if i > 0 { outln(""); }
        match stat(f) {
            Ok(st) => {
                outln(&format!("  File: {f}"));
                outln(&format!("  Size: {}\tBlocks: {}\tIO Block: 4096   {}",
                               st.size, st.blocks, kind_name(st.kind)));
                if st.kind == KIND_CHAR || st.kind == KIND_BLOCK {
                    outln(&format!("Device: {},{}\tInode: {}\tLinks: {}",
                                   st.dev_major, st.dev_minor, st.inode_id, st.nlinks));
                } else {
                    outln(&format!("Device: fs type {}\tInode: {}\tLinks: {}",
                                   st.fs_type, st.inode_id, st.nlinks));
                }
                outln(&format!("Access: ({:04o}/{})  Uid: ({:>5}/{:>8})   Gid: ({:>5}/{:>8})",
                               st.mode, mode_string(st.kind, st.mode),
                               st.uid, accounts.user(st.uid),
                               st.gid, accounts.group(st.gid)));
                outln(&format!("Access: {}", timestamp_full(st.atime)));
                outln(&format!("Modify: {}", timestamp_full(st.mtime)));
                outln(&format!("Change: {}", timestamp_full(st.ctime)));
            }
            Err(rc) => { complain("stat", f, &reason(rc)); bad = true; }
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn kind_name(k: u8) -> &'static str {
    match k {
        KIND_REGULAR => "regular file",
        KIND_DIR     => "directory",
        KIND_SYMLINK => "symbolic link",
        KIND_CHAR    => "character device",
        KIND_BLOCK   => "block device",
        KIND_PIPE    => "pipe",
        KIND_SOCKET  => "socket",
        KIND_FIFO    => "fifo",
        _            => "unknown",
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
