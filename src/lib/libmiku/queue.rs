// FIFO queue and stack
//
// Generic fixed-element-size queue backed by ring buffer
// Also provides stack (LIFO) operations on the same structure

use crate::heap;
use crate::mem;

// queue structure
#[repr(C)]
pub struct MikuQueue {
    data: *mut u8,
    elem_size: usize,
    cap: usize,      // max number of elements
    head: usize,     // read position
    tail: usize,     // write position
    count: usize,
}

// create queue with given element size and capacity
#[no_mangle]
pub extern "C" fn miku_queue_new(elem_size: usize, capacity: usize) -> MikuQueue {
    if elem_size == 0 || capacity == 0 {
        return MikuQueue {
            data: core::ptr::null_mut(), elem_size: 0,
            cap: 0, head: 0, tail: 0, count: 0,
        };
    }
    let total = elem_size * capacity;
    let data = heap::miku_malloc(total) as *mut u8;
    MikuQueue {
        data,
        elem_size,
        cap: capacity,
        head: 0,
        tail: 0,
        count: 0,
    }
}

// free queue
#[no_mangle]
pub extern "C" fn miku_queue_free(q: *mut MikuQueue) {
    if q.is_null() { return; }
    let q = unsafe { &mut *q };
    if !q.data.is_null() {
        heap::miku_free(q.data);
        q.data = core::ptr::null_mut();
    }
    q.count = 0;
}

// push element to back (enqueue)
#[no_mangle]
pub extern "C" fn miku_queue_push(q: *mut MikuQueue, elem: *const u8) -> bool {
    if q.is_null() || elem.is_null() { return false; }
    let q = unsafe { &mut *q };
    if q.data.is_null() || q.count >= q.cap { return false; }

    let offset = q.tail * q.elem_size;
    unsafe { mem::miku_memcpy(q.data.add(offset), elem, q.elem_size); }
    q.tail = (q.tail + 1) % q.cap;
    q.count += 1;
    true
}

// pop element from front (dequeue)
#[no_mangle]
pub extern "C" fn miku_queue_pop(q: *mut MikuQueue, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() { return false; }
    let q = unsafe { &mut *q };
    if q.count == 0 { return false; }

    let offset = q.head * q.elem_size;
    unsafe { mem::miku_memcpy(out, q.data.add(offset), q.elem_size); }
    q.head = (q.head + 1) % q.cap;
    q.count -= 1;
    true
}

// peek at front element without removing
#[no_mangle]
pub extern "C" fn miku_queue_peek(q: *const MikuQueue, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() { return false; }
    let q = unsafe { &*q };
    if q.count == 0 { return false; }

    let offset = q.head * q.elem_size;
    unsafe { mem::miku_memcpy(out, q.data.add(offset), q.elem_size); }
    true
}

// peek at back element
#[no_mangle]
pub extern "C" fn miku_queue_peek_back(q: *const MikuQueue, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() { return false; }
    let q = unsafe { &*q };
    if q.count == 0 { return false; }

    let idx = if q.tail == 0 { q.cap - 1 } else { q.tail - 1 };
    let offset = idx * q.elem_size;
    unsafe { mem::miku_memcpy(out, q.data.add(offset), q.elem_size); }
    true
}

// pop from back (stack-like LIFO)
#[no_mangle]
pub extern "C" fn miku_queue_pop_back(q: *mut MikuQueue, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() { return false; }
    let q = unsafe { &mut *q };
    if q.count == 0 { return false; }

    q.tail = if q.tail == 0 { q.cap - 1 } else { q.tail - 1 };
    let offset = q.tail * q.elem_size;
    unsafe { mem::miku_memcpy(out, q.data.add(offset), q.elem_size); }
    q.count -= 1;
    true
}

// number of elements
#[no_mangle]
pub extern "C" fn miku_queue_len(q: *const MikuQueue) -> usize {
    if q.is_null() { return 0; }
    unsafe { (*q).count }
}

// capacity
#[no_mangle]
pub extern "C" fn miku_queue_capacity(q: *const MikuQueue) -> usize {
    if q.is_null() { return 0; }
    unsafe { (*q).cap }
}

// is empty
#[no_mangle]
pub extern "C" fn miku_queue_is_empty(q: *const MikuQueue) -> bool {
    if q.is_null() { return true; }
    unsafe { (*q).count == 0 }
}

// is full
#[no_mangle]
pub extern "C" fn miku_queue_is_full(q: *const MikuQueue) -> bool {
    if q.is_null() { return true; }
    let q = unsafe { &*q };
    q.count >= q.cap
}

// clear all elements
#[no_mangle]
pub extern "C" fn miku_queue_clear(q: *mut MikuQueue) {
    if q.is_null() { return; }
    let q = unsafe { &mut *q };
    q.head = 0;
    q.tail = 0;
    q.count = 0;
}

// get element at index (0 = front)
#[no_mangle]
pub extern "C" fn miku_queue_at(q: *const MikuQueue, idx: usize, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() { return false; }
    let q = unsafe { &*q };
    if idx >= q.count { return false; }

    let real_idx = (q.head + idx) % q.cap;
    let offset = real_idx * q.elem_size;
    unsafe { mem::miku_memcpy(out, q.data.add(offset), q.elem_size); }
    true
}
