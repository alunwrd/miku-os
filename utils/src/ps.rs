#![no_std]
#![no_main]

//! ps: lists processes
//!
//! Usage: ps [-f]
//!
//! Default columns: pid, terminal, CPU time, command. `-f` adds owner, parent,
//! job, and state. Data from `procinfo` (103). Addresses and sizes are not
//! shown.

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
    let mut full = false;
    for a in args.iter().skip(1) {
        match *a {
            "-f" | "-l" | "-ef" | "-aux" | "aux" => full = true,
            "-e" | "-a" | "-A" => {}   // everything is listed either way
            "-h" | "--help" => { outln("usage: ps [-f]"); miku::exit(0); }
            other => { errln(&format!("ps: unknown option {other}")); miku::exit(2); }
        }
    }

    let mut buf: Vec<miku::proc::ProcInfo> = alloc::vec![Default::default(); 256];
    let n = miku::proc::procinfo(&mut buf);
    if n == 0 {
        errln("ps: the kernel listed no processes");
        miku::exit(1);
    }
    let hz = proc_value("/proc/stat", "hz").unwrap_or(250);

    if !full {
        outln("    PID TTY          TIME CMD");
        for p in &buf[..n] {
            outln(&format!("{} {} {} {}",
                pad_left(&format!("{}", p.pid), 7),
                pad_right(&tty_name(p.tty), 8),
                cpu_time(p.cpu_time, hz),
                p.name()));
        }
        miku::exit(0);
    }

    let accounts = Accounts::load();
    outln("UID          PID   PPID   PGID S PRI  %CPU TTY          TIME CMD");
    for p in &buf[..n] {
        // The `exec` path. A kernel thread has only its short name
        let cmd = miku::proc::path_of(p.pid).unwrap_or_else(|| String::from(p.name()));
        outln(&format!("{} {} {} {} {} {} {} {} {} {}",
            pad_right(&accounts.user(p.uid), 8),
            pad_left(&format!("{}", p.pid), 6),
            pad_left(&format!("{}", p.ppid), 6),
            pad_left(&format!("{}", p.pgid), 6),
            p.state as char,
            pad_left(&format!("{}", p.priority), 3),
            pad_left(&format!("{}.{}", p.cpu_pct_x10 / 10, p.cpu_pct_x10 % 10), 5),
            pad_right(&tty_name(p.tty), 8),
            cpu_time(p.cpu_time, hz),
            cmd));
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
