#![no_std]
#![no_main]

//! cat: writes files to standard output
//!
//! Usage: cat [-n] [file...]
//!
//! With no file, copies its input. Binary data passes through unchanged.

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
    let mut number = false;
    let mut files: Vec<&str> = Vec::new();

    for a in args.iter().skip(1) {
        match *a {
            "-n" => number = true,
            "-h" | "--help" => { outln("usage: cat [-n] [file...]"); miku::exit(0); }
            "-" => files.push("-"),
            other if other.starts_with('-') && other.len() > 1 => {
                errln(&format!("cat: unknown option {other}"));
                miku::exit(2);
            }
            other => files.push(other),
        }
    }

    let mut state = Numbering { line: 1, at_line_start: true, on: number };
    let mut bad = false;
    if files.is_empty() { files.push("-"); }

    for f in files {
        if let Err(rc) = pour(f, &mut state) {
            complain("cat", f, &reason(rc));
            bad = true;
        }
    }
    miku::exit(if bad { 1 } else { 0 });
}

/// Where the line numbering has got to, across files and across reads
struct Numbering { line: u64, at_line_start: bool, on: bool }

/// Copies one file in chunks, so unbounded inputs need no memory growth
fn pour(path: &str, num: &mut Numbering) -> Result<(), i64> {
    let fd = if path == "-" { 0 } else { open(path, O_READ, 0)? };
    let mut buf = [0u8; 4096];
    let r = loop {
        let n = unsafe { miku::miku_read(fd as u64, buf.as_mut_ptr(), buf.len()) };
        if n < 0 { break Err(n); }
        if n == 0 { break Ok(()); }
        let chunk = &buf[..n as usize];
        if !num.on {
            if let Err(e) = write_all(1, chunk) { break Err(e); }
            continue;
        }
        // Numbers lines across chunk boundaries
        let mut start = 0usize;
        let mut failed: Option<i64> = None;
        for i in 0..chunk.len() {
            if num.at_line_start {
                if start < i {
                    if let Err(e) = write_all(1, &chunk[start..i]) { failed = Some(e); break; }
                }
                start = i;
                out(&format!("{:>6}  ", num.line));
                num.line += 1;
                num.at_line_start = false;
            }
            if chunk[i] == b'\n' { num.at_line_start = true; }
        }
        if let Some(e) = failed { break Err(e); }
        if start < chunk.len() {
            if let Err(e) = write_all(1, &chunk[start..]) { break Err(e); }
        }
    };
    if fd != 0 { miku::close(fd); }
    r
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
