#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// libc_miku.so: POSIX/C compatibility layer

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/string.rs"] pub mod string;
#[path = "shim/ctype.rs"] pub mod ctype;
#[path = "shim/convert.rs"] pub mod convert;
#[path = "shim/num.rs"] pub mod num;
#[path = "shim/io.rs"] pub mod io;
#[path = "shim/file.rs"] pub mod file;
#[path = "shim/proc.rs"] pub mod proc;
#[path = "shim/env.rs"] pub mod env;
#[path = "shim/signal.rs"] pub mod signal;
#[path = "shim/time.rs"] pub mod time;
#[path = "shim/math.rs"] pub mod math;
#[path = "shim/random.rs"] pub mod random;
#[path = "shim/errno.rs"] pub mod errno;
#[path = "../libmiku/libc.rs"] pub mod libc;

static PANIC_MSG: &[u8] = b"libc_miku: panic\n";

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
