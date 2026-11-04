#![no_std]
#![no_main]

pub use miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("Hello from Rust on MikuOS!");
    miku::print("pid = ");
    miku::print_int(miku::getpid() as i64);
    miku::println("");
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("panic!");
    miku::exit(1);
}
