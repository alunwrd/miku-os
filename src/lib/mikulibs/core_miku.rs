#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// core_miku.so: syscalls, errno, sync, memory, heap, process, strings, stdio

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "../libmiku/errno.rs"] pub mod errno;
#[path = "../libmiku/sync.rs"] pub mod sync;
#[path = "../libmiku/panic.rs"] pub mod panic;
#[path = "../libmiku/mem.rs"] pub mod mem;
#[path = "../libmiku/heap.rs"] pub mod heap;
#[path = "../libmiku/io.rs"] pub mod io;
#[path = "../libmiku/proc.rs"] pub mod proc;
#[path = "../libmiku/string.rs"] pub mod string;
#[path = "../libmiku/ctype.rs"] pub mod ctype;
#[path = "../libmiku/convert.rs"] pub mod convert;
#[path = "../libmiku/num.rs"] pub mod num;
#[path = "../libmiku/stdio.rs"] pub mod stdio;
#[path = "../libmiku/time.rs"] pub mod time;

#[no_mangle]
#[link_section = ".text._libmiku_start"]
pub extern "C" fn _libmiku_start() -> ! { loop {} }

#[panic_handler]
fn rust_panic(_: &core::panic::PanicInfo) -> ! {
    io::miku_write(2, b"core_miku: panic\n".as_ptr(), 17);
    proc::miku_exit(127);
}
