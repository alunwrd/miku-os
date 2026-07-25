#![no_std]
#![no_main]
#![allow(dead_code, unused)]

// net_miku.so: TCP/UDP sockets

#[path = "../libmiku/sys.rs"] pub mod sys;
#[path = "../libmiku/net.rs"] pub mod net;

static PANIC_MSG: &[u8] = b"net_miku: panic\n";

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
