// Binary min-heap priority queue
// Generic element size with user-supplied comparator
// Backed by a dynamic array, Elements with lowest comparator
// value are dequeued first

use crate::heap;
use crate::mem;

pub type CmpFn = extern "C" fn(*const u8, *const u8) -> i32;

// priority queue backed by binary heap
#[repr(C)]
pub struct MikuHeapQueue {
    data: *mut u8,
    len: usize,
    cap: usize,
    elem_size: usize,
    cmp: CmpFn,
}

unsafe fn elem_ptr(q: *const MikuHeapQueue, i: usize) -> *mut u8 {
    (*q).data.add(i * (*q).elem_size)
}

unsafe fn swap_elems(q: *mut MikuHeapQueue, a: usize, b: usize) {
    if a == b {
        return;
    }
    let size = (*q).elem_size;
    let pa = elem_ptr(q, a);
    let pb = elem_ptr(q, b);
    if size <= 256 {
        let mut stack_buf = [0u8; 256];
        let tmp = stack_buf.as_mut_ptr();
        mem::miku_memcpy(tmp, pa, size);
        mem::miku_memcpy(pa, pb, size);
        mem::miku_memcpy(pb, tmp, size);
    } else {
        for i in 0..size {
            let t = *pa.add(i);
            *pa.add(i) = *pb.add(i);
            *pb.add(i) = t;
        }
    }
}

unsafe fn sift_up(q: *mut MikuHeapQueue, mut idx: usize) {
    let cmp = (*q).cmp;
    while idx > 0 {
        let parent = (idx - 1) / 2;
        if cmp(elem_ptr(q, idx), elem_ptr(q, parent)) < 0 {
            swap_elems(q, idx, parent);
            idx = parent;
        } else {
            break;
        }
    }
}

unsafe fn sift_down(q: *mut MikuHeapQueue, mut idx: usize) {
    let cmp = (*q).cmp;
    let len = (*q).len;
    loop {
        let left = 2 * idx + 1;
        let right = 2 * idx + 2;
        let mut smallest = idx;

        if left < len && cmp(elem_ptr(q, left), elem_ptr(q, smallest)) < 0 {
            smallest = left;
        }
        if right < len && cmp(elem_ptr(q, right), elem_ptr(q, smallest)) < 0 {
            smallest = right;
        }
        if smallest == idx {
            break;
        }
        swap_elems(q, idx, smallest);
        idx = smallest;
    }
}

unsafe fn grow(q: *mut MikuHeapQueue) -> bool {
    let new_cap = if (*q).cap == 0 { 4 } else { (*q).cap * 2 };
    let new_data = heap::miku_realloc((*q).data, new_cap * (*q).elem_size);
    if new_data.is_null() {
        return false;
    }
    (*q).data = new_data;
    (*q).cap = new_cap;
    true
}

// create a new priority queue
#[no_mangle]
pub extern "C" fn miku_pq_new(elem_size: usize, cmp: CmpFn) -> MikuHeapQueue {
    MikuHeapQueue {
        data: core::ptr::null_mut(),
        len: 0,
        cap: 0,
        elem_size,
        cmp,
    }
}

// free priority queue
#[no_mangle]
pub extern "C" fn miku_pq_free(q: *mut MikuHeapQueue) {
    if q.is_null() {
        return;
    }
    unsafe {
        if !(*q).data.is_null() {
            heap::miku_free((*q).data);
        }
        (*q).data = core::ptr::null_mut();
        (*q).len = 0;
        (*q).cap = 0;
    }
}

// push element into queue
#[no_mangle]
pub extern "C" fn miku_pq_push(q: *mut MikuHeapQueue, elem: *const u8) -> bool {
    if q.is_null() || elem.is_null() {
        return false;
    }
    unsafe {
        if (*q).len >= (*q).cap && !grow(q) {
            return false;
        }
        let idx = (*q).len;
        mem::miku_memcpy(elem_ptr(q, idx), elem, (*q).elem_size);
        (*q).len += 1;
        sift_up(q, idx);
        true
    }
}

// peek at top element without removing
#[no_mangle]
pub extern "C" fn miku_pq_peek(q: *const MikuHeapQueue) -> *const u8 {
    if q.is_null() {
        return core::ptr::null();
    }
    unsafe {
        if (*q).len == 0 {
            return core::ptr::null();
        }
        elem_ptr(q, 0)
    }
}

// pop top (minimum) element
#[no_mangle]
pub extern "C" fn miku_pq_pop(q: *mut MikuHeapQueue, out: *mut u8) -> bool {
    if q.is_null() || out.is_null() {
        return false;
    }
    unsafe {
        if (*q).len == 0 {
            return false;
        }
        mem::miku_memcpy(out, elem_ptr(q, 0), (*q).elem_size);
        (*q).len -= 1;
        if (*q).len > 0 {
            mem::miku_memcpy(elem_ptr(q, 0), elem_ptr(q, (*q).len), (*q).elem_size);
            sift_down(q, 0);
        }
        true
    }
}

// number of elements
#[no_mangle]
pub extern "C" fn miku_pq_len(q: *const MikuHeapQueue) -> usize {
    if q.is_null() {
        return 0;
    }
    unsafe { (*q).len }
}

// check if queue is empty
#[no_mangle]
pub extern "C" fn miku_pq_is_empty(q: *const MikuHeapQueue) -> bool {
    if q.is_null() {
        return true;
    }
    unsafe { (*q).len == 0 }
}

// clear all elements
#[no_mangle]
pub extern "C" fn miku_pq_clear(q: *mut MikuHeapQueue) {
    if q.is_null() {
        return;
    }
    unsafe {
        (*q).len = 0;
    }
}

// convenience: i64 priority queue
// comparison function for i64 min-heap
#[no_mangle]
pub extern "C" fn miku_pq_cmp_i64(a: *const u8, b: *const u8) -> i32 {
    unsafe {
        let va = *(a as *const i64);
        let vb = *(b as *const i64);
        if va < vb { -1 } else if va > vb { 1 } else { 0 }
    }
}

// create i64 priority queue
#[no_mangle]
pub extern "C" fn miku_pq_new_i64() -> MikuHeapQueue {
    miku_pq_new(8, miku_pq_cmp_i64)
}

// push i64 value
#[no_mangle]
pub extern "C" fn miku_pq_push_i64(q: *mut MikuHeapQueue, val: i64) -> bool {
    miku_pq_push(q, &val as *const i64 as *const u8)
}

// pop i64 value
#[no_mangle]
pub extern "C" fn miku_pq_pop_i64(q: *mut MikuHeapQueue) -> i64 {
    let mut val: i64 = 0;
    if miku_pq_pop(q, &mut val as *mut i64 as *mut u8) {
        val
    } else {
        i64::MAX
    }
}
