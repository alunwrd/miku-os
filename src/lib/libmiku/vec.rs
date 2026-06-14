// dynamic array (growable buffer)
// Provides a generic-ish C-compatible vector backed by miku_malloc/realloc/free.
// Default element size is u64 (8 bytes), but raw API works with any element size.

use crate::heap;
use crate::mem;

// MikuVec: raw byte-level dynamic array //

const VEC_INITIAL_CAP: usize = 8;
const VEC_GROWTH_FACTOR: usize = 2;

#[repr(C)]
pub struct MikuVec {
    data: *mut u8,
    len: usize,       // number of elements
    cap: usize,       // capacity in elements
    elem_size: usize,  // size of one element in bytes
}

// lifecycle

#[no_mangle]
pub extern "C" fn miku_vec_new(elem_size: usize) -> MikuVec {
    MikuVec {
        data: core::ptr::null_mut(),
        len: 0,
        cap: 0,
        elem_size: if elem_size == 0 { 1 } else { elem_size },
    }
}

#[no_mangle]
pub extern "C" fn miku_vec_with_capacity(elem_size: usize, cap: usize) -> MikuVec {
    let es = if elem_size == 0 { 1 } else { elem_size };
    let data = if cap > 0 {
        heap::miku_malloc(cap * es)
    } else {
        core::ptr::null_mut()
    };
    MikuVec {
        data,
        len: 0,
        cap: if data.is_null() && cap > 0 { 0 } else { cap },
        elem_size: es,
    }
}

#[no_mangle]
pub extern "C" fn miku_vec_free(v: *mut MikuVec) {
    if v.is_null() { return; }
    unsafe {
        if !(*v).data.is_null() {
            heap::miku_free((*v).data);
        }
        (*v).data = core::ptr::null_mut();
        (*v).len = 0;
        (*v).cap = 0;
    }
}

// internal: ensure capacity for at least 'needed' elements

unsafe fn vec_grow(v: *mut MikuVec, needed: usize) -> bool {
    if (*v).cap >= needed { return true; }

    let mut new_cap = if (*v).cap == 0 { VEC_INITIAL_CAP } else { (*v).cap };
    while new_cap < needed {
        new_cap *= VEC_GROWTH_FACTOR;
    }

    let new_bytes = new_cap * (*v).elem_size;
    let new_data = if (*v).data.is_null() {
        heap::miku_malloc(new_bytes)
    } else {
        heap::miku_realloc((*v).data, new_bytes)
    };

    if new_data.is_null() { return false; }
    (*v).data = new_data;
    (*v).cap = new_cap;
    true
}

// accessors

#[no_mangle]
pub extern "C" fn miku_vec_len(v: *const MikuVec) -> usize {
    if v.is_null() { return 0; }
    unsafe { (*v).len }
}

#[no_mangle]
pub extern "C" fn miku_vec_cap(v: *const MikuVec) -> usize {
    if v.is_null() { return 0; }
    unsafe { (*v).cap }
}

#[no_mangle]
pub extern "C" fn miku_vec_is_empty(v: *const MikuVec) -> bool {
    if v.is_null() { return true; }
    unsafe { (*v).len == 0 }
}

// element access: returns pointer to element at index, or null

#[no_mangle]
pub extern "C" fn miku_vec_get(v: *const MikuVec, index: usize) -> *const u8 {
    if v.is_null() { return core::ptr::null(); }
    unsafe {
        if index >= (*v).len { return core::ptr::null(); }
        (*v).data.add(index * (*v).elem_size)
    }
}

#[no_mangle]
pub extern "C" fn miku_vec_get_mut(v: *mut MikuVec, index: usize) -> *mut u8 {
    if v.is_null() { return core::ptr::null_mut(); }
    unsafe {
        if index >= (*v).len { return core::ptr::null_mut(); }
        (*v).data.add(index * (*v).elem_size)
    }
}

// push: append element to the end. 'elem' points to elem_size bytes

#[no_mangle]
pub extern "C" fn miku_vec_push(v: *mut MikuVec, elem: *const u8) -> bool {
    if v.is_null() || elem.is_null() { return false; }
    unsafe {
        if !vec_grow(v, (*v).len + 1) { return false; }
        let dst = (*v).data.add((*v).len * (*v).elem_size);
        mem::miku_memcpy(dst, elem, (*v).elem_size);
        (*v).len += 1;
        true
    }
}

// pop: remove last element, copy it to 'out' if not null

#[no_mangle]
pub extern "C" fn miku_vec_pop(v: *mut MikuVec, out: *mut u8) -> bool {
    if v.is_null() { return false; }
    unsafe {
        if (*v).len == 0 { return false; }
        (*v).len -= 1;
        if !out.is_null() {
            let src = (*v).data.add((*v).len * (*v).elem_size);
            mem::miku_memcpy(out, src, (*v).elem_size);
        }
        true
    }
}

// insert: insert element at index, shifting everything after

#[no_mangle]
pub extern "C" fn miku_vec_insert(v: *mut MikuVec, index: usize, elem: *const u8) -> bool {
    if v.is_null() || elem.is_null() { return false; }
    unsafe {
        if index > (*v).len { return false; }
        if !vec_grow(v, (*v).len + 1) { return false; }

        let es = (*v).elem_size;
        if index < (*v).len {
            // shift elements right
            let src = (*v).data.add(index * es);
            let dst = (*v).data.add((index + 1) * es);
            let move_bytes = ((*v).len - index) * es;
            mem::miku_memmove(dst, src, move_bytes);
        }

        let dst = (*v).data.add(index * es);
        mem::miku_memcpy(dst, elem, es);
        (*v).len += 1;
        true
    }
}

// remove: remove element at index, shifting everything after

#[no_mangle]
pub extern "C" fn miku_vec_remove(v: *mut MikuVec, index: usize) -> bool {
    if v.is_null() { return false; }
    unsafe {
        if index >= (*v).len { return false; }

        let es = (*v).elem_size;
        if index < (*v).len - 1 {
            let dst = (*v).data.add(index * es);
            let src = (*v).data.add((index + 1) * es);
            let move_bytes = ((*v).len - index - 1) * es;
            mem::miku_memmove(dst, src, move_bytes);
        }

        (*v).len -= 1;
        true
    }
}

// swap_remove: O(1) remove by swapping with last element

#[no_mangle]
pub extern "C" fn miku_vec_swap_remove(v: *mut MikuVec, index: usize) -> bool {
    if v.is_null() { return false; }
    unsafe {
        if index >= (*v).len { return false; }

        let es = (*v).elem_size;
        let last = (*v).len - 1;
        if index != last {
            let a = (*v).data.add(index * es);
            let b = (*v).data.add(last * es);
            // swap bytes
            for i in 0..es {
                let tmp = *a.add(i);
                *a.add(i) = *b.add(i);
                *b.add(i) = tmp;
            }
        }
        (*v).len -= 1;
        true
    }
}

// clear: set length to 0 without freeing memory

#[no_mangle]
pub extern "C" fn miku_vec_clear(v: *mut MikuVec) {
    if v.is_null() { return; }
    unsafe { (*v).len = 0; }
}

// reserve: ensure capacity for at least 'additional' more elements

#[no_mangle]
pub extern "C" fn miku_vec_reserve(v: *mut MikuVec, additional: usize) -> bool {
    if v.is_null() { return false; }
    unsafe { vec_grow(v, (*v).len + additional) }
}

// shrink_to_fit: reduce capacity to match length

#[no_mangle]
pub extern "C" fn miku_vec_shrink(v: *mut MikuVec) -> bool {
    if v.is_null() { return false; }
    unsafe {
        if (*v).len == 0 {
            if !(*v).data.is_null() {
                heap::miku_free((*v).data);
                (*v).data = core::ptr::null_mut();
            }
            (*v).cap = 0;
            return true;
        }

        if (*v).len == (*v).cap { return true; }

        let new_bytes = (*v).len * (*v).elem_size;
        let new_data = heap::miku_realloc((*v).data, new_bytes);
        if new_data.is_null() { return false; }
        (*v).data = new_data;
        (*v).cap = (*v).len;
        true
    }
}

// data: raw pointer to underlying buffer

#[no_mangle]
pub extern "C" fn miku_vec_data(v: *const MikuVec) -> *const u8 {
    if v.is_null() { return core::ptr::null(); }
    unsafe { (*v).data }
}

// contains: linear search for element

#[no_mangle]
pub extern "C" fn miku_vec_contains(v: *const MikuVec, elem: *const u8) -> bool {
    if v.is_null() || elem.is_null() { return false; }
    unsafe {
        for i in 0..(*v).len {
            let ptr = (*v).data.add(i * (*v).elem_size);
            if mem::miku_memcmp(ptr, elem, (*v).elem_size) == 0 {
                return true;
            }
        }
        false
    }
}

// convenience: push/pop for u64 values (most common case)

#[no_mangle]
pub extern "C" fn miku_vec_push_u64(v: *mut MikuVec, val: u64) -> bool {
    miku_vec_push(v, &val as *const u64 as *const u8)
}

#[no_mangle]
pub extern "C" fn miku_vec_get_u64(v: *const MikuVec, index: usize) -> u64 {
    let ptr = miku_vec_get(v, index);
    if ptr.is_null() { return 0; }
    unsafe { *(ptr as *const u64) }
}
