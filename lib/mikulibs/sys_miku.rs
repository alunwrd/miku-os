#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// sys_miku.so: signals, environment, events, timers, logging, tests, datetime

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/sync.rs"] pub mod sync;
#[path = "shim/mem.rs"] pub mod mem;
// `env` needs `core_miku`'s allocator, through the shim
#[path = "shim/heap.rs"] pub mod heap;
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

// This library's locks, taken for `fork` (`miku_fork`): the environment outside
// the heap, and signal tables reset in the child
extern "C" fn fork_parent() {
    unsafe { env::fork_unlock_env() };
}

extern "C" fn fork_child() {
    unsafe {
        signal::fork_child_reset_tables();
        env::fork_unlock_env();
    }
}

extern "C" fn register_fork_hooks() {
    proc::miku_fork_hooks(
        env::fork_lock_env as usize,
        0,
        fork_parent as usize,
        fork_child as usize,
    );
}

#[used]
#[link_section = ".init_array"]
static FORK_HOOKS_CTOR: extern "C" fn() = register_fork_hooks;

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
