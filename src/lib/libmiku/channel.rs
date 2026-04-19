// Lock-free ring buffer based message passing between producer
// and consumer Useful for inter-task communication in the Miku-OS
// Fixed capacity, no dynamic allocation after creation

use crate::heap;
use crate::mem;
use core::sync::atomic::{AtomicUsize, Ordering};

// bounded SPSC channel
#[repr(C)]
pub struct MikuChannel {
    buf: *mut u8,
    cap: usize,          // number of slots
    elem_size: usize,
    head: AtomicUsize,   // write position (producer)
    tail: AtomicUsize,   // read position (consumer)
}

fn slot_ptr(ch: &MikuChannel, idx: usize) -> *mut u8 {
    unsafe { ch.buf.add((idx % ch.cap) * ch.elem_size) }
}

// create channel with given capacity and element size
#[no_mangle]
pub extern "C" fn miku_chan_new(elem_size: usize, capacity: usize) -> MikuChannel {
    let cap = if capacity < 2 { 2 } else { capacity };
    // allocate cap+1 slots to distinguish full from empty
    let total_cap = cap + 1;
    let buf = heap::miku_calloc(total_cap, elem_size);
    MikuChannel {
        buf,
        cap: total_cap,
        elem_size,
        head: AtomicUsize::new(0),
        tail: AtomicUsize::new(0),
    }
}

// free channel
#[no_mangle]
pub extern "C" fn miku_chan_free(ch: *mut MikuChannel) {
    if ch.is_null() {
        return;
    }
    unsafe {
        if !(*ch).buf.is_null() {
            heap::miku_free((*ch).buf);
            (*ch).buf = core::ptr::null_mut();
        }
    }
}

// try to send an element (non-blocking)
// Returns true on success, false if channel is full
#[no_mangle]
pub extern "C" fn miku_chan_send(ch: *mut MikuChannel, data: *const u8) -> bool {
    if ch.is_null() || data.is_null() {
        return false;
    }
    unsafe {
        let c = &mut *ch;
        let head = c.head.load(Ordering::Relaxed);
        let next_head = (head + 1) % c.cap;
        let tail = c.tail.load(Ordering::Acquire);
        if next_head == tail {
            return false; // full
        }
        mem::miku_memcpy(slot_ptr(c, head), data, c.elem_size);
        c.head.store(next_head, Ordering::Release);
        true
    }
}

// try to receive an element (non-blocking)
// Returns true on success, false if channel is empty
#[no_mangle]
pub extern "C" fn miku_chan_recv(ch: *mut MikuChannel, out: *mut u8) -> bool {
    if ch.is_null() || out.is_null() {
        return false;
    }
    unsafe {
        let c = &mut *ch;
        let tail = c.tail.load(Ordering::Relaxed);
        let head = c.head.load(Ordering::Acquire);
        if tail == head {
            return false; // empty
        }
        mem::miku_memcpy(out, slot_ptr(c, tail), c.elem_size);
        c.tail.store((tail + 1) % c.cap, Ordering::Release);
        true
    }
}

// number of pending elements
#[no_mangle]
pub extern "C" fn miku_chan_len(ch: *const MikuChannel) -> usize {
    if ch.is_null() {
        return 0;
    }
    unsafe {
        let c = &*ch;
        let head = c.head.load(Ordering::Relaxed);
        let tail = c.tail.load(Ordering::Relaxed);
        if head >= tail {
            head - tail
        } else {
            c.cap - tail + head
        }
    }
}

// check if channel is empty
#[no_mangle]
pub extern "C" fn miku_chan_is_empty(ch: *const MikuChannel) -> bool {
    if ch.is_null() {
        return true;
    }
    unsafe {
        let c = &*ch;
        c.head.load(Ordering::Relaxed) == c.tail.load(Ordering::Relaxed)
    }
}

// check if channel is full
#[no_mangle]
pub extern "C" fn miku_chan_is_full(ch: *const MikuChannel) -> bool {
    if ch.is_null() {
        return true;
    }
    unsafe {
        let c = &*ch;
        let next = (c.head.load(Ordering::Relaxed) + 1) % c.cap;
        next == c.tail.load(Ordering::Relaxed)
    }
}

// available space for sending
#[no_mangle]
pub extern "C" fn miku_chan_available(ch: *const MikuChannel) -> usize {
    if ch.is_null() {
        return 0;
    }
    unsafe {
        let c = &*ch;
        // max usable slots is cap - 1
        (c.cap - 1) - miku_chan_len(ch)
    }
}

// convenience: u64 channel
// create u64 channel
#[no_mangle]
pub extern "C" fn miku_chan_new_u64(capacity: usize) -> MikuChannel {
    miku_chan_new(8, capacity)
}

// send u64
#[no_mangle]
pub extern "C" fn miku_chan_send_u64(ch: *mut MikuChannel, val: u64) -> bool {
    miku_chan_send(ch, &val as *const u64 as *const u8)
}

// receive u64
#[no_mangle]
pub extern "C" fn miku_chan_recv_u64(ch: *mut MikuChannel) -> u64 {
    let mut val: u64 = 0;
    if miku_chan_recv(ch, &mut val as *mut u64 as *mut u8) {
        val
    } else {
        u64::MAX
    }
}
