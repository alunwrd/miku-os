// Sorting and searching algorithms
// Provides qsort (quicksort), insertion sort, and binary search
// All functions work with raw byte blobs via element size and comparator

use crate::mem;

// comparator type: fn(a, b) -> i32 (negative if a < b, 0 if equal, positive if a > b)

pub type CmpFn = extern "C" fn(*const u8, *const u8) -> i32;

// internal: swap two elements of 'size' bytes

unsafe fn swap_elements(a: *mut u8, b: *mut u8, size: usize) {
    if a == b { return; }
    for i in 0..size {
        let tmp = *a.add(i);
        *a.add(i) = *b.add(i);
        *b.add(i) = tmp;
    }
}

// internal: element pointer at index

#[inline]
unsafe fn elem_at(base: *mut u8, index: usize, size: usize) -> *mut u8 {
    base.add(index * size)
}

// quicksort (Lomuto partition, median-of-three pivot)

#[no_mangle]
pub extern "C" fn miku_qsort(
    base: *mut u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) {
    if base.is_null() || count <= 1 || size == 0 { return; }
    unsafe { qsort_inner(base, 0, count - 1, size, cmp); }
}

unsafe fn qsort_inner(base: *mut u8, lo: usize, hi: usize, size: usize, cmp: CmpFn) {
    // use insertion sort for small ranges
    if hi.wrapping_sub(lo) < 16 {
        insertion_sort_range(base, lo, hi, size, cmp);
        return;
    }

    let pivot = partition(base, lo, hi, size, cmp);
    if pivot > lo {
        qsort_inner(base, lo, pivot - 1, size, cmp);
    }
    if pivot < hi {
        qsort_inner(base, pivot + 1, hi, size, cmp);
    }
}

// Lomuto partition with median-of-three pivot selection
unsafe fn partition(base: *mut u8, lo: usize, hi: usize, size: usize, cmp: CmpFn) -> usize {
    let mid = lo + (hi - lo) / 2;

    // sort lo, mid, hi - then use mid as pivot
    if cmp(elem_at(base, lo, size), elem_at(base, mid, size)) > 0 {
        swap_elements(elem_at(base, lo, size), elem_at(base, mid, size), size);
    }
    if cmp(elem_at(base, lo, size), elem_at(base, hi, size)) > 0 {
        swap_elements(elem_at(base, lo, size), elem_at(base, hi, size), size);
    }
    if cmp(elem_at(base, mid, size), elem_at(base, hi, size)) > 0 {
        swap_elements(elem_at(base, mid, size), elem_at(base, hi, size), size);
    }

    // move pivot to hi-1
    swap_elements(elem_at(base, mid, size), elem_at(base, hi, size), size);
    let pivot = elem_at(base, hi, size);

    let mut i = lo;
    for j in lo..hi {
        if cmp(elem_at(base, j, size), pivot) <= 0 {
            swap_elements(elem_at(base, i, size), elem_at(base, j, size), size);
            i += 1;
        }
    }
    swap_elements(elem_at(base, i, size), elem_at(base, hi, size), size);
    i
}

// insertion sort (stable, good for small arrays)

#[no_mangle]
pub extern "C" fn miku_insertion_sort(
    base: *mut u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) {
    if base.is_null() || count <= 1 || size == 0 { return; }
    unsafe { insertion_sort_range(base, 0, count - 1, size, cmp); }
}

unsafe fn insertion_sort_range(base: *mut u8, lo: usize, hi: usize, size: usize, cmp: CmpFn) {
    for i in (lo + 1)..=hi {
        let mut j = i;
        while j > lo && cmp(elem_at(base, j - 1, size), elem_at(base, j, size)) > 0 {
            swap_elements(elem_at(base, j - 1, size), elem_at(base, j, size), size);
            j -= 1;
        }
    }
}

//   binary search: returns pointer to found element, or null
// Array must be sorted according to 'cmp'

#[no_mangle]
pub extern "C" fn miku_bsearch(
    key: *const u8,
    base: *const u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) -> *const u8 {
    if key.is_null() || base.is_null() || count == 0 || size == 0 {
        return core::ptr::null();
    }

    let mut lo: usize = 0;
    let mut hi: usize = count;

    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = unsafe { base.add(mid * size) };
        let result = cmp(key, elem);
        if result == 0 {
            return elem;
        } else if result < 0 {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }

    core::ptr::null()
}

// reverse: reverse array in-place

#[no_mangle]
pub extern "C" fn miku_reverse(base: *mut u8, count: usize, size: usize) {
    if base.is_null() || count <= 1 || size == 0 { return; }
    let mut lo = 0usize;
    let mut hi = count - 1;
    while lo < hi {
        unsafe { swap_elements(elem_at(base, lo, size), elem_at(base, hi, size), size); }
        lo += 1;
        hi -= 1;
    }
}

// is_sorted: check if array is sorted according to 'cmp'

#[no_mangle]
pub extern "C" fn miku_is_sorted(
    base: *const u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) -> bool {
    if base.is_null() || count <= 1 { return true; }
    for i in 0..(count - 1) {
        unsafe {
            if cmp(base.add(i * size), base.add((i + 1) * size)) > 0 {
                return false;
            }
        }
    }
    true
}

// convenience comparators for common types

#[no_mangle]
pub extern "C" fn miku_cmp_i64(a: *const u8, b: *const u8) -> i32 {
    let va = unsafe { *(a as *const i64) };
    let vb = unsafe { *(b as *const i64) };
    if va < vb { -1 } else if va > vb { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn miku_cmp_u64(a: *const u8, b: *const u8) -> i32 {
    let va = unsafe { *(a as *const u64) };
    let vb = unsafe { *(b as *const u64) };
    if va < vb { -1 } else if va > vb { 1 } else { 0 }
}

#[no_mangle]
pub extern "C" fn miku_cmp_str(a: *const u8, b: *const u8) -> i32 {
    // a and b are pointers to (pointer to string) - dereference once
    let sa = unsafe { *(a as *const *const u8) };
    let sb = unsafe { *(b as *const *const u8) };
    crate::string::miku_strcmp(sa, sb)
}

// lower_bound: first position where key could be inserted (leftmost)
// Returns index (0..count), Array must be sorted
#[no_mangle]
pub extern "C" fn miku_lower_bound(
    key: *const u8,
    base: *const u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) -> usize {
    if key.is_null() || base.is_null() || size == 0 { return 0; }
    let mut lo = 0usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = unsafe { base.add(mid * size) };
        if cmp(elem, key) < 0 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

//   upper_bound: first position past key (rightmost insertion)
// Returns index (0..count), Array must be sorted
#[no_mangle]
pub extern "C" fn miku_upper_bound(
    key: *const u8,
    base: *const u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) -> usize {
    if key.is_null() || base.is_null() || size == 0 { return 0; }
    let mut lo = 0usize;
    let mut hi = count;
    while lo < hi {
        let mid = lo + (hi - lo) / 2;
        let elem = unsafe { base.add(mid * size) };
        if cmp(elem, key) <= 0 {
            lo = mid + 1;
        } else {
            hi = mid;
        }
    }
    lo
}

//   unique: remove consecutive duplicates in-place
// Returns new count, Array must be sorted
#[no_mangle]
pub extern "C" fn miku_unique(
    base: *mut u8,
    count: usize,
    size: usize,
    cmp: CmpFn,
) -> usize {
    if base.is_null() || count <= 1 || size == 0 { return count; }
    let mut write = 1usize;
    for read in 1..count {
        unsafe {
            if cmp(elem_at(base, write - 1, size), elem_at(base, read, size)) != 0 {
                if write != read {
                    mem::miku_memcpy(elem_at(base, write, size), elem_at(base, read, size), size);
                }
                write += 1;
            }
        }
    }
    write
}

//   nth_element: partial sort so that element at nth is in correct position
// Elements before nth are <= nth, elements after are >= nth (not fully sorted)
#[no_mangle]
pub extern "C" fn miku_nth_element(
    base: *mut u8,
    count: usize,
    size: usize,
    nth: usize,
    cmp: CmpFn,
) {
    if base.is_null() || count <= 1 || size == 0 || nth >= count { return; }
    unsafe { nth_inner(base, 0, count - 1, size, nth, cmp); }
}

unsafe fn nth_inner(base: *mut u8, lo: usize, hi: usize, size: usize, nth: usize, cmp: CmpFn) {
    if lo >= hi { return; }
    let pivot = partition(base, lo, hi, size, cmp);
    if nth < pivot && pivot > 0 {
        nth_inner(base, lo, pivot - 1, size, nth, cmp);
    } else if nth > pivot {
        nth_inner(base, pivot + 1, hi, size, nth, cmp);
    }
}

// comparator for i32
#[no_mangle]
pub extern "C" fn miku_cmp_i32(a: *const u8, b: *const u8) -> i32 {
    let va = unsafe { *(a as *const i32) };
    let vb = unsafe { *(b as *const i32) };
    if va < vb { -1 } else if va > vb { 1 } else { 0 }
}

// comparator for u32
#[no_mangle]
pub extern "C" fn miku_cmp_u32(a: *const u8, b: *const u8) -> i32 {
    let va = unsafe { *(a as *const u32) };
    let vb = unsafe { *(b as *const u32) };
    if va < vb { -1 } else if va > vb { 1 } else { 0 }
}
