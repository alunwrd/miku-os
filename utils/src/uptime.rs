#![no_std]
#![no_main]

//! uptime: time of day, uptime, and process count from `/proc/stat`
//!
//! Usage: uptime

extern crate alloc;

pub use miku;

use alloc::format;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(_argc: i32, _argv: *const *const u8) -> ! {
    // The time of day first, as `uptime` has always started
    let now = now_secs();
    let clock = if now == 0 {
        alloc::string::String::from("--:--:--")
    } else {
        let dt = unsafe { miku::miku_dt_from_timestamp(now as i64) };
        format!("{:02}:{:02}:{:02}", dt.hour, dt.minute, dt.second)
    };

    let hz = proc_value("/proc/stat", "hz").unwrap_or(250);
    let secs = match proc_value("/proc/stat", "ticks") {
        Some(t) if hz > 0 => t / hz,
        // `/proc/uptime` is Linux's two numbers, seconds up first
        _ => uptime_secs().unwrap_or(0),
    };
    let procs = proc_value("/proc/stat", "procs").unwrap_or(0);

    // No load average, since nothing computes one
    outln(&format!(" {clock} up {}, {} processes", elapsed(secs), procs));
    miku::exit(0);
}

/// Whole seconds of `/proc/uptime`'s first number
fn uptime_secs() -> Option<u64> {
    let data = read_file("/proc/uptime").ok()?;
    let text = core::str::from_utf8(&data).ok()?;
    let digits: alloc::string::String = text.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect();
    parse_u64(&digits)
}

/// `3 min`, `2:05`, `1 day, 2:05`
fn elapsed(secs: u64) -> alloc::string::String {
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    if days > 0 {
        format!("{days} day{}, {hours}:{mins:02}", if days == 1 { "" } else { "s" })
    } else if hours > 0 {
        format!("{hours}:{mins:02}")
    } else {
        format!("{mins} min")
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
