#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// ds_miku.so: data structures and allocator arenas

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/string.rs"] pub mod string;
#[path = "../libmiku/vec.rs"] pub mod vec;
#[path = "../libmiku/list.rs"] pub mod list;
#[path = "../libmiku/queue.rs"] pub mod queue;
#[path = "../libmiku/hashmap.rs"] pub mod hashmap;
#[path = "../libmiku/treemap.rs"] pub mod treemap;
#[path = "../libmiku/trie.rs"] pub mod trie;
#[path = "../libmiku/ringbuf.rs"] pub mod ringbuf;
#[path = "../libmiku/ringbuf2.rs"] pub mod ringbuf2;
#[path = "../libmiku/bitset.rs"] pub mod bitset;
#[path = "../libmiku/heap_queue.rs"] pub mod heap_queue;
#[path = "../libmiku/channel.rs"] pub mod channel;
#[path = "../libmiku/arena.rs"] pub mod arena;
#[path = "../libmiku/slab.rs"] pub mod slab;
#[path = "../libmiku/pool.rs"] pub mod pool;

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
