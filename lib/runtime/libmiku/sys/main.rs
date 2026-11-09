#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// sys_miku.so: signals, environment, events, timers, logging, tests, datetime

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(sync);
shim!(mem);
// `env` needs `core_miku`'s allocator, through the shim
shim!(heap);
shim!(string);
shim!(io);
shim!(num);
shim!(time);
shim!(proc);
pub mod signal;
pub mod env;
pub mod event;
pub mod timer;
pub mod log;
pub mod test;
pub mod datetime;

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
