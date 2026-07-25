#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// codec_miku.so: base64, hex, checksums, sha256, lz, uuid

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "shim/heap.rs"] pub mod heap;
#[path = "shim/mem.rs"] pub mod mem;
#[path = "shim/random.rs"] pub mod random;
#[path = "shim/string.rs"] pub mod string;
#[path = "../libmiku/base64.rs"] pub mod base64;
#[path = "../libmiku/hex.rs"] pub mod hex;
#[path = "../libmiku/checksum.rs"] pub mod checksum;
#[path = "../libmiku/sha256.rs"] pub mod sha256;
#[path = "../libmiku/lz.rs"] pub mod lz;
#[path = "../libmiku/uuid.rs"] pub mod uuid;

static PANIC_MSG: &[u8] = b"codec_miku: panic\n";

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
