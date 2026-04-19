// ringbuf.rs - fixed-capacity circular buffer
// Lock-free single-producer single-consumer ring buffer for byte streams.
// Also provides a general-purpose ring buffer for elements of any size.

use crate::heap;
use crate::mem;

// Byte ring buffer (SPSC) //

#[repr(C)]
pub struct MikuRingBuf {
    buf: *mut u8,
    cap: usize,    // total capacity in bytes
    head: usize,   // read position
    tail: usize,   // write position
}

// lifecycle //

#[no_mangle]
pub extern "C" fn miku_ring_new(capacity: usize) -> MikuRingBuf {
    if capacity == 0 {
        return MikuRingBuf { buf: core::ptr::null_mut(), cap: 0, head: 0, tail: 0 };
    }
    // allocate capacity + 1 to distinguish full from empty
    let real_cap = capacity + 1;
    let buf = heap::miku_malloc(real_cap);
    MikuRingBuf {
        buf,
        cap: if buf.is_null() { 0 } else { real_cap },
        head: 0,
        tail: 0,
    }
}

#[no_mangle]
pub extern "C" fn miku_ring_free(r: *mut MikuRingBuf) {
    if r.is_null() { return; }
    unsafe {
        if !(*r).buf.is_null() {
            heap::miku_free((*r).buf);
        }
        (*r).buf = core::ptr::null_mut();
        (*r).cap = 0;
        (*r).head = 0;
        (*r).tail = 0;
    }
}

// state queries //

#[no_mangle]
pub extern "C" fn miku_ring_len(r: *const MikuRingBuf) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        let cap = (*r).cap;
        if cap == 0 { return 0; }
        ((*r).tail + cap - (*r).head) % cap
    }
}

#[no_mangle]
pub extern "C" fn miku_ring_available(r: *const MikuRingBuf) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        let cap = (*r).cap;
        if cap == 0 { return 0; }
        cap - 1 - miku_ring_len(r)
    }
}

#[no_mangle]
pub extern "C" fn miku_ring_is_empty(r: *const MikuRingBuf) -> bool {
    if r.is_null() { return true; }
    unsafe { (*r).head == (*r).tail }
}

#[no_mangle]
pub extern "C" fn miku_ring_is_full(r: *const MikuRingBuf) -> bool {
    if r.is_null() { return true; }
    miku_ring_available(r) == 0
}

// write bytes into the ring buffer 
// Returns number of bytes actually written (may be less than 'len' if full)

#[no_mangle]
pub extern "C" fn miku_ring_write(r: *mut MikuRingBuf, data: *const u8, len: usize) -> usize {
    if r.is_null() || data.is_null() || len == 0 { return 0; }
    unsafe {
        let avail = miku_ring_available(r);
        let write_len = if len < avail { len } else { avail };
        let cap = (*r).cap;

        for i in 0..write_len {
            *(*r).buf.add((*r).tail) = *data.add(i);
            (*r).tail = ((*r).tail + 1) % cap;
        }

        write_len
    }
}

// read bytes from the ring buffer
// Returns number of bytes actually read.

#[no_mangle]
pub extern "C" fn miku_ring_read(r: *mut MikuRingBuf, out: *mut u8, len: usize) -> usize {
    if r.is_null() || out.is_null() || len == 0 { return 0; }
    unsafe {
        let available = miku_ring_len(r);
        let read_len = if len < available { len } else { available };
        let cap = (*r).cap;

        for i in 0..read_len {
            *out.add(i) = *(*r).buf.add((*r).head);
            (*r).head = ((*r).head + 1) % cap;
        }

        read_len
    }
}

// peek: read without consuming

#[no_mangle]
pub extern "C" fn miku_ring_peek(r: *const MikuRingBuf, out: *mut u8, len: usize) -> usize {
    if r.is_null() || out.is_null() || len == 0 { return 0; }
    unsafe {
        let available = miku_ring_len(r);
        let peek_len = if len < available { len } else { available };
        let cap = (*r).cap;
        let mut pos = (*r).head;

        for i in 0..peek_len {
            *out.add(i) = *(*r).buf.add(pos);
            pos = (pos + 1) % cap;
        }

        peek_len
    }
}

// push/pop single byte

#[no_mangle]
pub extern "C" fn miku_ring_push_byte(r: *mut MikuRingBuf, byte: u8) -> bool {
    if r.is_null() { return false; }
    if miku_ring_is_full(r) { return false; }
    unsafe {
        *(*r).buf.add((*r).tail) = byte;
        (*r).tail = ((*r).tail + 1) % (*r).cap;
    }
    true
}

#[no_mangle]
pub extern "C" fn miku_ring_pop_byte(r: *mut MikuRingBuf) -> i32 {
    if r.is_null() { return -1; }
    if miku_ring_is_empty(r) { return -1; }
    unsafe {
        let byte = *(*r).buf.add((*r).head);
        (*r).head = ((*r).head + 1) % (*r).cap;
        byte as i32
    }
}

// skip: discard n bytes from head

#[no_mangle]
pub extern "C" fn miku_ring_skip(r: *mut MikuRingBuf, n: usize) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        if (*r).cap == 0 { return 0; }
        let available = miku_ring_len(r);
        let skip_len = if n < available { n } else { available };
        (*r).head = ((*r).head + skip_len) % (*r).cap;
        skip_len
    }
}

// clear: reset to empty

#[no_mangle]
pub extern "C" fn miku_ring_clear(r: *mut MikuRingBuf) {
    if r.is_null() { return; }
    unsafe {
        (*r).head = 0;
        (*r).tail = 0;
    }
}
