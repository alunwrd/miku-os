#![no_std]
#![no_main]

//! kill: sends a signal
//!
//! Usage: kill [-s sig] pid... | kill -sig pid... | kill -l
//!
//! A negative pid is a process group.

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

const SIGNALS: &[(&str, u64)] = &[
    ("HUP", 1), ("INT", 2), ("QUIT", 3), ("ILL", 4), ("TRAP", 5), ("ABRT", 6),
    ("FPE", 8), ("KILL", 9), ("USR1", 10), ("SEGV", 11), ("USR2", 12),
    ("PIPE", 13), ("ALRM", 14), ("TERM", 15), ("CHLD", 17), ("CONT", 18),
    ("STOP", 19), ("TSTP", 20), ("TTIN", 21), ("TTOU", 22), ("URG", 23),
    ("WINCH", 28),
];

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut sig: u64 = 15;
    let mut targets: Vec<&str> = Vec::new();
    let mut want_sig = false;

    for a in args.iter().skip(1) {
        if want_sig {
            match signal_number(a) {
                Some(n) => sig = n,
                None => { errln(&format!("kill: {a}: no such signal")); miku::exit(2); }
            }
            want_sig = false;
            continue;
        }
        match *a {
            "-s" => want_sig = true,
            "-l" => { for (n, v) in SIGNALS { outln(&format!("{:>2}  SIG{}", v, n)); } miku::exit(0); }
            "-h" | "--help" => {
                outln("usage: kill [-s sig] pid... | kill -sig pid... | kill -l");
                outln("  a negative pid is a process group");
                miku::exit(0);
            }
            // `-9`, `-TERM`, `-sigterm`, before any target. Afterwards a
            // leading minus is a process group
            other if other.starts_with('-') && other.len() > 1 && targets.is_empty() => {
                match signal_number(&other[1..]) {
                    Some(n) => sig = n,
                    None => { errln(&format!("kill: {other}: no such signal")); miku::exit(2); }
                }
            }
            other => targets.push(other),
        }
    }
    if targets.is_empty() {
        errln("usage: kill [-s sig] pid... | kill -sig pid... | kill -l");
        miku::exit(2);
    }

    let mut bad = false;
    for t in targets {
        let (is_group, digits) = match t.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, t),
        };
        let n = match parse_u64(digits) {
            Some(n) if n > 0 => n,
            _ => { errln(&format!("kill: {t}: not a pid")); bad = true; continue; }
        };
        let rc = if is_group {
            unsafe { miku::miku_killpg(n, sig) }
        } else {
            unsafe { miku::miku_kill(n, sig) }
        };
        if rc < 0 { complain("kill", t, &reason(rc)); bad = true; }
    }
    miku::exit(if bad { 1 } else { 0 });
}

fn signal_number(s: &str) -> Option<u64> {
    if let Some(n) = parse_u64(s) {
        return if n > 0 && n < 32 { Some(n) } else { None };
    }
    let upper: String = s.chars().map(|c| c.to_ascii_uppercase()).collect();
    let name = upper.strip_prefix("SIG").unwrap_or(&upper).to_string();
    SIGNALS.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
