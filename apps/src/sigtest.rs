// Signal delivery test. Exit codes (serial: "[syscall] exit"):
// 0 = handler ran and execution resumed after sigreturn
//   1 = handler did not run
//   2 = miku_kill(self, SIGUSR1) failed
// 3 = local state corrupted across delivery
#![no_std]
#![no_main]

pub use miku;

use core::sync::atomic::{AtomicU32, Ordering};

static FIRED: AtomicU32 = AtomicU32::new(0);

extern "C" fn on_usr1(sig: u32) {
    FIRED.store(sig, Ordering::SeqCst);
}

const SIGUSR1: u32 = miku_abi::signal::SIGUSR1 as u32;

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    unsafe {
        miku::miku_signal(SIGUSR1, Some(on_usr1));

        // Canaries in callee-saved registers or on the stack must survive the
        // handler
        let canary_a: u64 = 0xDEAD_BEEF_CAFE_0001;
        let canary_b: u64 = 0x1234_5678_9ABC_0002;

        let me = miku::getpid();
        if miku::miku_kill(me, SIGUSR1 as u64) < 0 {
            miku::exit(2);
        }
        // the pending signal is delivered at the next syscall boundary
        miku::sleep_ms(50);

        if core::hint::black_box(canary_a) != 0xDEAD_BEEF_CAFE_0001
            || core::hint::black_box(canary_b) != 0x1234_5678_9ABC_0002
        {
            miku::exit(3);
        }
        if FIRED.load(Ordering::SeqCst) != SIGUSR1 {
            miku::exit(1);
        }
        miku::println("sigtest: handler ran, context restored");
    }
    miku::exit(0);
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::exit(9);
}
