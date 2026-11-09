#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// algo_miku.so: sorting, hashing, bit ops, math, random, endian

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(mem);
shim!(string);
shim!(proc);
shim!(sync);
shim!(time);
shim!(heap);
pub mod sort;
pub mod hash;
pub mod bitops;
pub mod math;
pub mod random;
pub mod endian;

static PANIC_MSG: &[u8] = b"algo_miku: panic\n";

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
