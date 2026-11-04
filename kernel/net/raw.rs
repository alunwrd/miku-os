// Raw ethernet frames, for a stack that does not live here
//
// With the network stack in a program, the kernel owns the card and nothing
// else: it hands up what arrived and sends down what was given. A frame goes in
// whole and comes out whole, with no parsing and no demux.
//
// Not a second receive path: `rx::pump` still drains the driver once, in one
// place, and delivers the batch here instead of into the demux when the card
// has been handed out.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use spin::Mutex;

/// Frames held for the reader before the oldest is dropped.
///
/// A reader 64 frames behind will not catch up, and more backlog only makes the
/// data older. TCP recovers from a drop
const BACKLOG: usize = 64;

/// The largest frame this will carry. 1514 is an untagged ethernet MTU frame;
/// the rest is room for a VLAN tag and a driver that hands up its padding
const MAX_FRAME: usize = 2048;

/// Whether the card belongs to a program rather than to the in-kernel stack.
/// Set once at boot from `net=raw` on the command line
static ENABLED: AtomicBool = AtomicBool::new(false);

static QUEUE: Mutex<VecDeque<Vec<u8>>> = Mutex::new(VecDeque::new());

/// Frames thrown away because the reader was behind
static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Frames thrown away because the reader offered a buffer too small to hold
/// one. Counted apart from a backlog drop: this is a bug in the reader, not a
/// slow reader
static UNDERSIZED: AtomicU64 = AtomicU64::new(0);
/// Frames handed to the reader, and frames the reader sent
static READ: AtomicU64 = AtomicU64::new(0);
static WRITTEN: AtomicU64 = AtomicU64::new(0);

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Give the card to whoever opens /dev/net0. Called once during boot, before
/// the in-kernel stack has started, so there is never a moment where both are
/// draining the same receive ring
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Take one frame from the receive path. Called from `rx::pump` with the batch
/// it just drained from the driver
pub fn push(frame: &[u8]) {
    if frame.len() > MAX_FRAME {
        UNDERSIZED.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let mut q = QUEUE.lock();
    if q.len() >= BACKLOG {
        q.pop_front();
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    q.push_back(Vec::from(frame));
}

/// One frame into `buf`, or 0 when there is nothing waiting.
///
/// A frame that does not fit is dropped rather than truncated: half an ethernet
/// frame has a plausible header and a severed body. A 2 KiB buffer always fits
pub fn take(buf: &mut [u8]) -> usize {
    let mut q = QUEUE.lock();
    loop {
        let frame = match q.pop_front() {
            Some(f) => f,
            None => return 0,
        };
        if frame.len() > buf.len() {
            UNDERSIZED.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        buf[..frame.len()].copy_from_slice(&frame);
        READ.fetch_add(1, Ordering::Relaxed);
        return frame.len();
    }
}

/// Whether a read would return something, for `poll`
pub fn pending() -> bool {
    !QUEUE.lock().is_empty()
}

/// Sends one frame as given. No header is added and none is checked: the source
/// MAC is the sender's to get right
pub fn send(frame: &[u8]) -> bool {
    if frame.is_empty() || frame.len() > MAX_FRAME {
        return false;
    }
    let sent = super::tx_frame(frame);
    if sent {
        WRITTEN.fetch_add(1, Ordering::Relaxed);
    }
    sent
}

/// read, written, dropped, undersized, for `/proc/net`
pub fn stats() -> (u64, u64, u64, u64) {
    (
        READ.load(Ordering::Relaxed),
        WRITTEN.load(Ordering::Relaxed),
        DROPPED.load(Ordering::Relaxed),
        UNDERSIZED.load(Ordering::Relaxed),
    )
}
