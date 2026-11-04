#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// parse_miku.so: json, csv, ini, getopt, args

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/num.rs"] pub mod num;
#[path = "shim/string.rs"] pub mod string;
#[path = "shim/utf8.rs"] pub mod utf8;
#[path = "../libmiku/json.rs"] pub mod json;
#[path = "../libmiku/csv.rs"] pub mod csv;
#[path = "../libmiku/ini.rs"] pub mod ini;
#[path = "../libmiku/getopt.rs"] pub mod getopt;
#[path = "../libmiku/args.rs"] pub mod args;

static PANIC_MSG: &[u8] = b"parse_miku: panic\n";

#[no_mangle]
#[link_section = ".text._libmiku_start"]
pub extern "C" fn _libmiku_start() -> ! { loop {} }

#[panic_handler]
fn rust_panic(_: &core::panic::PanicInfo) -> ! {
    unsafe {
        sys::sc3(sys::SYS_WRITE, 2, PANIC_MSG.as_ptr() as u64, PANIC_MSG.len() as u64);
        sys::sc1(sys::SYS_EXIT, 127);
    }
    loop {}
}
