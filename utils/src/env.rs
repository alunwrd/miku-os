#![no_std]
#![no_main]

//! env: prints the environment
//!
//! Usage: env

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[path = "common.rs"]
mod common;
use common::*;

/// Filled by the `miku_env_iter` callback
static mut LINES: Option<Vec<String>> = None;

extern "C" fn take(key: *const u8, value: *const u8, _ctx: *mut u8) {
    let k = cstr_to_str(key);
    let v = cstr_to_str(value);
    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(LINES);
        if let Some(list) = slot.as_mut() {
            list.push(format!("{k}={v}"));
        }
    }
}

fn cstr_to_str(p: *const u8) -> String {
    if p.is_null() { return String::new(); }
    let mut len = 0usize;
    while len < 4096 && unsafe { *p.add(len) } != 0 { len += 1; }
    let bytes = unsafe { core::slice::from_raw_parts(p, len) };
    String::from_utf8_lossy(bytes).into_owned()
}

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    for a in args.iter().skip(1) {
        if *a == "-h" || *a == "--help" { outln("usage: env"); miku::exit(0); }
        errln(&format!("env: {a}: starting a program with a changed environment is the shell's job"));
        miku::exit(2);
    }

    unsafe {
        let slot = &mut *core::ptr::addr_of_mut!(LINES);
        *slot = Some(Vec::new());
        miku::miku_env_iter(take, core::ptr::null_mut());
        if let Some(list) = slot.as_ref() {
            for l in list { outln(l); }
        }
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(1);
}
