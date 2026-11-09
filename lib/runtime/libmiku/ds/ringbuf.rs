// Fixed-capacity circular byte buffer, single producer and single consumer: peek,
// skip, find, read a line

use crate::heap;
use crate::mem;

// Byte ring buffer (SPSC) //

pub use miku_abi::libtypes::MikuRingBuf;

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

// Finds a byte in the buffered data: its offset from the read position, or -1
#[no_mangle]
pub extern "C" fn miku_ring_find(r: *const MikuRingBuf, byte: u8) -> i32 {
    if r.is_null() { return -1; }
    unsafe {
        let r = &*r;
        if r.buf.is_null() || r.cap == 0 { return -1; }
        let have = miku_ring_len(r);
        for i in 0..have {
            if *r.buf.add((r.head + i) % r.cap) == byte {
                return i as i32;
            }
        }
    }
    -1
}

// Reads a line, up to and including the newline. The byte count, or 0 if there
// is no complete line or it does not fit
#[no_mangle]
pub extern "C" fn miku_ring_readline(r: *mut MikuRingBuf, out: *mut u8, max_len: usize) -> usize {
    if r.is_null() || out.is_null() || max_len == 0 { return 0; }
    let nl = miku_ring_find(r, b'\n');
    if nl < 0 { return 0; }
    let line_len = nl as usize + 1;
    if line_len > max_len { return 0; }
    miku_ring_read(r, out, line_len)
}

// How many bytes the buffer can hold
#[no_mangle]
pub extern "C" fn miku_ring_capacity(r: *const MikuRingBuf) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        if (*r).cap == 0 { 0 } else { (*r).cap - 1 }
    }
}
