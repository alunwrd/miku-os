use core::sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering};
use x86_64::instructions::interrupts;

const BUF_SIZE: usize = 64;

static BUF:   [AtomicU8; BUF_SIZE] = {
    const Z: AtomicU8 = AtomicU8::new(0);
    [Z; BUF_SIZE]
};
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

/// Serialises the two producers; see `push`
static PUSH_LOCK: AtomicBool = AtomicBool::new(false);

pub fn push(byte: u8) {
    // The display protocol gets first refusal. While a client holds the
    // keyboard the shell must not also see the keys. Runs in the keyboard
    // interrupt: atomics only
    if crate::mayland::input::intercept(byte) {
        return;
    }
    // One consumer and two producers: the PS/2 keyboard interrupt and the usbd
    // thread carrying xHCI HID reports. Claiming a slot with a plain load and
    // publishing with a plain store is correct only with one producer: on two
    // cores both take the same slot, one byte is lost and the other may be read
    // before it is written.
    //
    // Interrupts off before the lock: the usbd thread holding it can be
    // interrupted on its own core by the keyboard interrupt wanting it
    interrupts::without_interrupts(|| {
        while PUSH_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        let tail = TAIL.load(Ordering::Relaxed);
        let next = (tail + 1) % BUF_SIZE;
        if next != HEAD.load(Ordering::Acquire) {
            BUF[tail].store(byte, Ordering::Relaxed);
            TAIL.store(next, Ordering::Release);
        }
        PUSH_LOCK.store(false, Ordering::Release);
    });
}

pub fn pop() -> Option<u8> {
    let head = HEAD.load(Ordering::Relaxed);
    if head == TAIL.load(Ordering::Acquire) {
        return None;
    }
    let byte = BUF[head].load(Ordering::Relaxed);
    HEAD.store((head + 1) % BUF_SIZE, Ordering::Release);
    Some(byte)
}

pub fn is_empty() -> bool {
    HEAD.load(Ordering::Relaxed) == TAIL.load(Ordering::Acquire)
}
