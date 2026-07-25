#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// sys_miku.so: signals, environment, events, timers, logging, tests, datetime

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/sync.rs"] pub mod sync;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/string.rs"] pub mod string;
#[path = "shim/io.rs"] pub mod io;
#[path = "shim/num.rs"] pub mod num;
#[path = "shim/time.rs"] pub mod time;
#[path = "shim/proc.rs"] pub mod proc;
#[path = "../libmiku/signal.rs"] pub mod signal;
#[path = "../libmiku/env.rs"] pub mod env;
#[path = "../libmiku/event.rs"] pub mod event;
#[path = "../libmiku/timer.rs"] pub mod timer;
#[path = "../libmiku/log.rs"] pub mod log;
#[path = "../libmiku/test.rs"] pub mod test;
#[path = "../libmiku/datetime.rs"] pub mod datetime;

static PANIC_MSG: &[u8] = b"sys_miku: panic\n";

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
