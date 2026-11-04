#![no_std]
#![no_main]

//! date: prints the current time
//!
//! Usage: date [+seconds]
//!
//! Read-only. `ntpdate` sets the clock, through `clock_settime` (79) with
//! `CAP_SYS_TIME`.

extern crate alloc;

pub use miku;

use alloc::format;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut raw = false;
    for a in args.iter().skip(1) {
        match *a {
            "+seconds" | "+%s" => raw = true,
            // The machine keeps UTC only
            "-u" => {}
            "-h" | "--help" => { outln("usage: date [+seconds]"); miku::exit(0); }
            other => { errln(&format!("date: unknown option {other}")); miku::exit(2); }
        }
    }

    let mut ts = miku::time::Timespec::default();
    if miku::time::clock_gettime(miku::time::CLOCK_REALTIME, &mut ts) < 0 {
        errln("date: the clock did not answer");
        miku::exit(1);
    }
    if raw { outln(&format!("{}", ts.tv_sec)); miku::exit(0); }

    let dt = unsafe { miku::miku_dt_from_timestamp(ts.tv_sec) };
    let mut buf = [0u8; 64];
    let n = unsafe { miku::miku_dt_format(&dt as *const _, buf.as_mut_ptr(), buf.len()) };
    match core::str::from_utf8(&buf[..n]) {
        Ok(s) if n > 0 => outln(s),
        // Falls back to seconds if the date cannot be represented
        _ => outln(&format!("{} seconds since the epoch", ts.tv_sec)),
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
