#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// ds_miku.so: data structures and allocator arenas

#[macro_use]
#[path = "../shim/macros.rs"]
mod shim_macro;

pub use miku_abi::sys;
shim!(heap);
shim!(mem);
shim!(string);
pub mod vec;
pub mod list;
pub mod queue;
pub mod hashmap;
pub mod treemap;
pub mod trie;
pub mod ringbuf;
pub mod bitset;
pub mod heap_queue;
pub mod channel;
pub mod arena;
pub mod slab;
pub mod pool;

static PANIC_MSG: &[u8] = b"ds_miku: panic\n";

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
