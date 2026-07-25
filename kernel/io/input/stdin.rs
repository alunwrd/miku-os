use core::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

const BUF_SIZE: usize = 64;

static BUF:   [AtomicU8; BUF_SIZE] = {
    const Z: AtomicU8 = AtomicU8::new(0);
    [Z; BUF_SIZE]
};
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

pub fn push(byte: u8) {
    let tail = TAIL.load(Ordering::Relaxed);
    let next = (tail + 1) % BUF_SIZE;
    if next == HEAD.load(Ordering::Acquire) {
        return; 
    }
    BUF[tail].store(byte, Ordering::Relaxed);
    TAIL.store(next, Ordering::Release);
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
