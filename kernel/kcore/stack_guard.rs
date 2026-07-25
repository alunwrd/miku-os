// BSP kernel-stack overflow detection
//
// The kernel stack lives at the end of .bss (see linker.ld) and the whole
// kernel image is mapped by one 1 GiB huge page, so an overflow does not
// fault - it just keeps writing downwards into whatever statics the linker
// placed below '_stack_bottom'. That is how a ~0.5 MiB stack temporary in
// 'init_vfs' once zeroed 'apic::LAPIC_BASE_VIRT' / 'LAPIC_TICKS_PER_HZ' and
// left the kernel hanging on the first `sti` with no hint as to why.
//
// 'linker.ld' therefore reserves one page directly below the stack.
// 'init()' fills it with a magic pattern and 'check()' verifies it; the
// boot-step reporter calls 'check()' after every init step, so an overflow
// is reported at the step that caused it

use core::sync::atomic::{AtomicBool, Ordering};

unsafe extern "C" {
    static _stack_guard: u8;
}

const GUARD_SIZE: usize = 0x1000;
const GUARD_MAGIC: u64 = 0x5354_4143_4B47_5244; // "STACKGRD"

static ARMED: AtomicBool = AtomicBool::new(false);

fn guard_words() -> *mut u64 {
    core::ptr::addr_of!(_stack_guard) as *mut u64
}

/// Poison the guard page. Call once, as early in 'kernel_main' as possible
pub fn init() {
    let words = guard_words();
    unsafe {
        for i in 0..(GUARD_SIZE / 8) {
            core::ptr::write_volatile(words.add(i), GUARD_MAGIC);
        }
    }
    ARMED.store(true, Ordering::Release);
}

/// True while the guard page is still intact (or not yet armed)
pub fn intact() -> bool {
    if !ARMED.load(Ordering::Acquire) {
        return true;
    }
    let words = guard_words();
    // Walk from the top of the guard (closest to the stack) downwards - an
    // overflow always chews through that end first
    for i in (0..(GUARD_SIZE / 8)).rev() {
        if unsafe { core::ptr::read_volatile(words.add(i)) } != GUARD_MAGIC {
            return false;
        }
    }
    true
}

/// Panic with a diagnosis if the stack has run past '_stack_bottom'
pub fn check(where_: &str) {
    if intact() {
        return;
    }
    ARMED.store(false, Ordering::Release);
    crate::serial_println!(
        "[stack] BSP kernel stack overflowed during '{}' - guard page at {:#x} is corrupted",
        where_,
        core::ptr::addr_of!(_stack_guard) as usize
    );
    crate::cprintln!(255, 70, 70, "[stack] kernel stack overflow during '{}'", where_);
    panic!("kernel stack overflow during '{}'", where_);
}
