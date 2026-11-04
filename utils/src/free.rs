#![no_std]
#![no_main]

//! free: memory usage from `/proc/meminfo`, in kilobytes
//!
//! Usage: free

extern crate alloc;

pub use miku;

use alloc::format;

#[path = "common.rs"]
mod common;
use common::*;

#[no_mangle]
pub extern "C" fn _start_main(_argc: i32, _argv: *const *const u8) -> ! {
    // Linux field names
    let total = proc_value("/proc/meminfo", "MemTotal");
    let free  = proc_value("/proc/meminfo", "MemAvailable");
    let (total, free) = match (total, free) {
        (Some(t), Some(f)) => (t, f),
        // Reports missing fields rather than printing zeroes
        _ => {
            errln("free: /proc/meminfo did not say MemTotal and MemAvailable");
            miku::exit(1);
        }
    };
    let used = total.saturating_sub(free);

    outln(&format!("{} {} {} {}",
                   pad_right("", 8),
                   pad_left("total", 11), pad_left("used", 11), pad_left("free", 11)));
    outln(&format!("{} {} {} {}",
                   pad_right("Mem:", 8),
                   pad_left(&format!("{total}"), 11),
                   pad_left(&format!("{used}"), 11),
                   pad_left(&format!("{free}"), 11)));
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
