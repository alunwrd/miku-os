// Sleeps about 30 s in 100 ms slices, exits 42 if not interrupted. For testing
// Ctrl+C / SIGINT (expected exit: 130)
#![no_std]
#![no_main]

pub use miku;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("spin: sleeping 30s, press Ctrl+C...");
    for _ in 0..300 {
        miku::sleep_ms(100);
    }
    miku::exit(42);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(9); }
