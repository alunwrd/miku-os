#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// fs_miku.so: files, buffered io, directories, paths

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/errno.rs"] pub mod errno;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/io.rs"] pub mod io;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/string.rs"] pub mod string;
#[path = "../libmiku/file.rs"] pub mod file;
#[path = "../libmiku/bufio.rs"] pub mod bufio;
#[path = "../libmiku/dir.rs"] pub mod dir;
#[path = "../libmiku/path.rs"] pub mod path;

static PANIC_MSG: &[u8] = b"fs_miku: panic\n";

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
