////////////////////////////////////////////////////////////////
// Generic byte ring buffer with advanced operations          //
//                                                            //
// Heap-allocated circular byte buffer                        //
// Supports peek, skip, read-line, write-at-once semantics    //
// Useful for I/O buffering, protocol parsing, logging        //
////////////////////////////////////////////////////////////////

use crate::heap;
use crate::mem;

// byte ring buffer
#[repr(C)]
pub struct MikuByteRing {
    data: *mut u8,
    cap: usize,     // allocated capacity (power of 2)
    head: usize,    // read position
    tail: usize,    // write position
}

fn next_pow2(mut n: usize) -> usize {
    if n == 0 { return 1; }
    n -= 1;
    n |= n >> 1;
    n |= n >> 2;
    n |= n >> 4;
    n |= n >> 8;
    n |= n >> 16;
    n |= n >> 32;
    n + 1
}

// create byte ring buffer with given minimum capacity
#[no_mangle]
pub extern "C" fn miku_bring_new(min_cap: usize) -> MikuByteRing {
    let cap = next_pow2(if min_cap < 16 { 16 } else { min_cap });
    let data = heap::miku_malloc(cap);
    MikuByteRing {
        data,
        cap: if data.is_null() { 0 } else { cap },
        head: 0,
        tail: 0,
    }
}

// free ring buffer
#[no_mangle]
pub extern "C" fn miku_bring_free(r: *mut MikuByteRing) {
    if r.is_null() { return; }
    unsafe {
        if !(*r).data.is_null() {
            heap::miku_free((*r).data);
            (*r).data = core::ptr::null_mut();
        }
        (*r).cap = 0;
        (*r).head = 0;
        (*r).tail = 0;
    }
}

#[inline]
fn mask(r: &MikuByteRing) -> usize {
    r.cap - 1 // works because cap is power of 2
}

// number of bytes available to read
#[no_mangle]
pub extern "C" fn miku_bring_len(r: *const MikuByteRing) -> usize {
    if r.is_null() { return 0; }
    unsafe { used(&*r) }
}

fn used(r: &MikuByteRing) -> usize {
    if r.cap == 0 { return 0; }
    r.tail.wrapping_sub(r.head)
}

fn avail(r: &MikuByteRing) -> usize {
    if r.cap == 0 { return 0; }
    r.cap - 1 - used(r)
}

// available space for writing
#[no_mangle]
pub extern "C" fn miku_bring_avail(r: *const MikuByteRing) -> usize {
    if r.is_null() { return 0; }
    unsafe { avail(&*r) }
}

// check if empty
#[no_mangle]
pub extern "C" fn miku_bring_is_empty(r: *const MikuByteRing) -> bool {
    if r.is_null() { return true; }
    unsafe { used(&*r) == 0 }
}

// Write bytes into ring buffer
// Returns number of bytes actually written
#[no_mangle]
pub extern "C" fn miku_bring_write(
    r: *mut MikuByteRing,
    data: *const u8,
    len: usize,
) -> usize {
    if r.is_null() || data.is_null() || len == 0 { return 0; }
    unsafe {
        let r = &mut *r;
        if r.data.is_null() { return 0; }
        let space = avail(r);
        let to_write = if len > space { space } else { len };
        let m = mask(r);
        for i in 0..to_write {
            let pos = (r.tail + i) & m;
            *r.data.add(pos) = *data.add(i);
        }
        r.tail = r.tail.wrapping_add(to_write);
        to_write
    }
}

// Read bytes from ring buffer
// Returns number of bytes actually read, Advances head
#[no_mangle]
pub extern "C" fn miku_bring_read(
    r: *mut MikuByteRing,
    out: *mut u8,
    len: usize,
) -> usize {
    if r.is_null() || out.is_null() || len == 0 { return 0; }
    unsafe {
        let r = &mut *r;
        if r.data.is_null() { return 0; }
        let have = used(r);
        let to_read = if len > have { have } else { len };
        let m = mask(r);
        for i in 0..to_read {
            let pos = (r.head + i) & m;
            *out.add(i) = *r.data.add(pos);
        }
        r.head = r.head.wrapping_add(to_read);
        to_read
    }
}

// Peek at bytes without consuming
#[no_mangle]
pub extern "C" fn miku_bring_peek(
    r: *const MikuByteRing,
    out: *mut u8,
    len: usize,
) -> usize {
    if r.is_null() || out.is_null() || len == 0 { return 0; }
    unsafe {
        let r = &*r;
        if r.data.is_null() { return 0; }
        let have = used(r);
        let to_peek = if len > have { have } else { len };
        let m = mask(r);
        for i in 0..to_peek {
            let pos = (r.head + i) & m;
            *out.add(i) = *r.data.add(pos);
        }
        to_peek
    }
}

// skip bytes (advance read head without copying)
#[no_mangle]
pub extern "C" fn miku_bring_skip(r: *mut MikuByteRing, len: usize) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        let r = &mut *r;
        let have = used(r);
        let to_skip = if len > have { have } else { len };
        r.head = r.head.wrapping_add(to_skip);
        to_skip
    }
}

// write single byte
#[no_mangle]
pub extern "C" fn miku_bring_put(r: *mut MikuByteRing, byte: u8) -> bool {
    if r.is_null() { return false; }
    unsafe {
        let r = &mut *r;
        if r.data.is_null() || avail(r) == 0 { return false; }
        let pos = r.tail & mask(r);
        *r.data.add(pos) = byte;
        r.tail = r.tail.wrapping_add(1);
        true
    }
}

// read single byte
#[no_mangle]
pub extern "C" fn miku_bring_get(r: *mut MikuByteRing, out: *mut u8) -> bool {
    if r.is_null() || out.is_null() { return false; }
    unsafe {
        let r = &mut *r;
        if r.data.is_null() || used(r) == 0 { return false; }
        let pos = r.head & mask(r);
        *out = *r.data.add(pos);
        r.head = r.head.wrapping_add(1);
        true
    }
}

// find byte in buffer, returns offset from head or -1
#[no_mangle]
pub extern "C" fn miku_bring_find(r: *const MikuByteRing, byte: u8) -> i32 {
    if r.is_null() { return -1; }
    unsafe {
        let r = &*r;
        if r.data.is_null() { return -1; }
        let have = used(r);
        let m = mask(r);
        for i in 0..have {
            let pos = (r.head + i) & m;
            if *r.data.add(pos) == byte {
                return i as i32;
            }
        }
    }
    -1
}

// Read a line (up to and including newline)
// Returns bytes read (including newline), or 0 if no complete line
#[no_mangle]
pub extern "C" fn miku_bring_readline(
    r: *mut MikuByteRing,
    out: *mut u8,
    max_len: usize,
) -> usize {
    if r.is_null() || out.is_null() || max_len == 0 { return 0; }

    let nl_pos = miku_bring_find(r, b'\n');
    if nl_pos < 0 { return 0; }

    let line_len = (nl_pos as usize) + 1;
    if line_len > max_len { return 0; }

    miku_bring_read(r, out, line_len)
}

// clear all data
#[no_mangle]
pub extern "C" fn miku_bring_clear(r: *mut MikuByteRing) {
    if r.is_null() { return; }
    unsafe {
        (*r).head = 0;
        (*r).tail = 0;
    }
}

// capacity of the buffer
#[no_mangle]
pub extern "C" fn miku_bring_capacity(r: *const MikuByteRing) -> usize {
    if r.is_null() { return 0; }
    unsafe {
        if (*r).cap == 0 { 0 } else { (*r).cap - 1 }
    }
}
