#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// text_miku.so: utf8, string builders, formatting, glob, regex

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/num.rs"] pub mod num;
#[path = "shim/string.rs"] pub mod string;
#[path = "shim/io.rs"] pub mod io;
#[path = "../libmiku/utf8.rs"] pub mod utf8;
#[path = "../libmiku/strbuf.rs"] pub mod strbuf;
#[path = "../libmiku/format.rs"] pub mod format;
#[path = "../libmiku/fmt.rs"] pub mod fmt;
#[path = "../libmiku/glob.rs"] pub mod glob;
#[path = "../libmiku/regex.rs"] pub mod regex;

static PANIC_MSG: &[u8] = b"text_miku: panic\n";

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
