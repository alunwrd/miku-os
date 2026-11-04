#![no_std]
#![no_main]
// `...` parameters and `VaList` for lib/libmiku/c/printf.rs
#![feature(c_variadic)]
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
#[path = "shim/auxv.rs"] pub mod auxv;
#[path = "../libmiku/libc.rs"] pub mod libc;
// C math library, using musl's algorithms
#[path = "../libmiku/cmath.rs"] pub mod cmath;
// C-facing areas (dev/c-architecture.md)
#[path = "../libmiku/c/printf.rs"] pub mod cprintf;
#[path = "../libmiku/c/strtod.rs"] pub mod cstrtod;
#[path = "../libmiku/c/ctime.rs"] pub mod ctime;
#[path = "../libmiku/c/pthread.rs"] pub mod pthread;
#[path = "../libmiku/c/netdb.rs"] pub mod netdb;
#[path = "../libmiku/c/termios.rs"] pub mod ctermios;
#[path = "../libmiku/c/scanf.rs"] pub mod cscanf;
#[path = "../libmiku/c/pty.rs"] pub mod pty;
#[path = "../libmiku/c/accounts.rs"] pub mod accounts;
#[path = "../libmiku/c/sysinfo.rs"] pub mod sysinfo;
#[path = "../libmiku/c/at.rs"] pub mod at;
#[path = "../libmiku/c/setjmp.rs"] pub mod setjmp;

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
