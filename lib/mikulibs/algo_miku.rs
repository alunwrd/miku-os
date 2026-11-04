#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// algo_miku.so: sorting, hashing, bit ops, math, random, endian

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/string.rs"] pub mod string;
#[path = "shim/proc.rs"] pub mod proc;
#[path = "shim/sync.rs"] pub mod sync;
#[path = "shim/time.rs"] pub mod time;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "../libmiku/sort.rs"] pub mod sort;
#[path = "../libmiku/hash.rs"] pub mod hash;
#[path = "../libmiku/bitops.rs"] pub mod bitops;
#[path = "../libmiku/math.rs"] pub mod math;
#[path = "../libmiku/random.rs"] pub mod random;
#[path = "../libmiku/endian.rs"] pub mod endian;

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
