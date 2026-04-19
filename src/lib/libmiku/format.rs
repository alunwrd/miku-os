// Higher-level string building on top of heap allocation
// Useful for constructing messages, log lines, and paths
// without manual buffer management

use crate::heap;
use crate::mem;
use crate::string;
use crate::num;

// dynamic string builder
#[repr(C)]
pub struct MikuStringBuilder {
    buf: *mut u8,
    len: usize,
    cap: usize,
}

unsafe fn sb_grow(sb: *mut MikuStringBuilder, needed: usize) -> bool {
    let total = (*sb).len + needed;
    if total <= (*sb).cap {
        return true;
    }
    let new_cap = if (*sb).cap == 0 { 64 } else { (*sb).cap };
    let mut cap = new_cap;
    while cap < total {
        cap *= 2;
    }
    let new_buf = heap::miku_realloc((*sb).buf, cap);
    if new_buf.is_null() {
        return false;
    }
    (*sb).buf = new_buf;
    (*sb).cap = cap;
    true
}

// create new string builder
#[no_mangle]
pub extern "C" fn miku_sb_new() -> MikuStringBuilder {
    MikuStringBuilder {
        buf: core::ptr::null_mut(),
        len: 0,
        cap: 0,
    }
}

// create string builder with initial capacity
#[no_mangle]
pub extern "C" fn miku_sb_with_capacity(cap: usize) -> MikuStringBuilder {
    let c = if cap == 0 { 64 } else { cap };
    let buf = heap::miku_malloc(c);
    if buf.is_null() {
        return miku_sb_new();
    }
    MikuStringBuilder { buf, len: 0, cap: c }
}

// free string builder
#[no_mangle]
pub extern "C" fn miku_sb_free(sb: *mut MikuStringBuilder) {
    if sb.is_null() {
        return;
    }
    unsafe {
        if !(*sb).buf.is_null() {
            heap::miku_free((*sb).buf);
        }
        (*sb).buf = core::ptr::null_mut();
        (*sb).len = 0;
        (*sb).cap = 0;
    }
}

// append c-string
#[no_mangle]
pub extern "C" fn miku_sb_append(sb: *mut MikuStringBuilder, s: *const u8) -> bool {
    if sb.is_null() || s.is_null() {
        return false;
    }
    let len = string::miku_strlen(s);
    if len == 0 {
        return true;
    }
    unsafe {
        if !sb_grow(sb, len) {
            return false;
        }
        mem::miku_memcpy((*sb).buf.add((*sb).len), s, len);
        (*sb).len += len;
        true
    }
}

// append raw bytes
#[no_mangle]
pub extern "C" fn miku_sb_append_bytes(sb: *mut MikuStringBuilder, data: *const u8, len: usize) -> bool {
    if sb.is_null() || data.is_null() || len == 0 {
        return sb.is_null() || len == 0;
    }
    unsafe {
        if !sb_grow(sb, len) {
            return false;
        }
        mem::miku_memcpy((*sb).buf.add((*sb).len), data, len);
        (*sb).len += len;
        true
    }
}

// append single character
#[no_mangle]
pub extern "C" fn miku_sb_append_char(sb: *mut MikuStringBuilder, c: u8) -> bool {
    if sb.is_null() {
        return false;
    }
    unsafe {
        if !sb_grow(sb, 1) {
            return false;
        }
        *(*sb).buf.add((*sb).len) = c;
        (*sb).len += 1;
        true
    }
}

// append integer as decimal string
#[no_mangle]
pub extern "C" fn miku_sb_append_int(sb: *mut MikuStringBuilder, val: i64) -> bool {
    if sb.is_null() {
        return false;
    }
    let mut buf = [0u8; 24];
    num::miku_itoa(val, buf.as_mut_ptr());
    miku_sb_append(sb, buf.as_ptr())
}

// append unsigned integer as decimal string
#[no_mangle]
pub extern "C" fn miku_sb_append_uint(sb: *mut MikuStringBuilder, val: u64) -> bool {
    if sb.is_null() {
        return false;
    }
    let mut buf = [0u8; 24];
    num::miku_utoa(val, buf.as_mut_ptr());
    miku_sb_append(sb, buf.as_ptr())
}

// append n copies of character c
#[no_mangle]
pub extern "C" fn miku_sb_repeat(sb: *mut MikuStringBuilder, c: u8, n: usize) -> bool {
    if sb.is_null() || n == 0 {
        return true;
    }
    unsafe {
        if !sb_grow(sb, n) {
            return false;
        }
        mem::miku_memset((*sb).buf.add((*sb).len), c as i32, n);
        (*sb).len += n;
        true
    }
}

// finalize: return heap-allocated null-terminated string
// The string builder is reset after this call, caller must free
#[no_mangle]
pub extern "C" fn miku_sb_finish(sb: *mut MikuStringBuilder) -> *mut u8 {
    if sb.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        let len = (*sb).len;
        let result = heap::miku_malloc(len + 1);
        if result.is_null() {
            return core::ptr::null_mut();
        }
        if len > 0 && !(*sb).buf.is_null() {
            mem::miku_memcpy(result, (*sb).buf, len);
        }
        *result.add(len) = 0;

        // reset builder
        (*sb).len = 0;
        result
    }
}

// get current length
#[no_mangle]
pub extern "C" fn miku_sb_len(sb: *const MikuStringBuilder) -> usize {
    if sb.is_null() {
        return 0;
    }
    unsafe { (*sb).len }
}

// clear without freeing buffer
#[no_mangle]
pub extern "C" fn miku_sb_clear(sb: *mut MikuStringBuilder) {
    if sb.is_null() {
        return;
    }
    unsafe { (*sb).len = 0; }
}

// get read-only pointer to current content (not null-terminated)
#[no_mangle]
pub extern "C" fn miku_sb_data(sb: *const MikuStringBuilder) -> *const u8 {
    if sb.is_null() {
        return core::ptr::null();
    }
    unsafe { (*sb).buf }
}

// standalone string formatting helpers //

// join array of c-strings with separator
// Returns heap-allocated result, caller must free
#[no_mangle]
pub extern "C" fn miku_str_join(
    strs: *const *const u8,
    count: usize,
    sep: *const u8,
) -> *mut u8 {
    if strs.is_null() || count == 0 {
        let r = heap::miku_malloc(1);
        if !r.is_null() {
            unsafe { *r = 0; }
        }
        return r;
    }

    let mut sb = miku_sb_new();
    let sep_len = if sep.is_null() { 0 } else { string::miku_strlen(sep) };

    unsafe {
        for i in 0..count {
            if i > 0 && sep_len > 0 {
                miku_sb_append(&mut sb, sep);
            }
            let s = *strs.add(i);
            if !s.is_null() {
                miku_sb_append(&mut sb, s);
            }
        }
    }

    let result = miku_sb_finish(&mut sb);
    miku_sb_free(&mut sb);
    result
}

// repeat string n times
// Returns heap-allocated result, caller must free
#[no_mangle]
pub extern "C" fn miku_str_repeat(s: *const u8, n: usize) -> *mut u8 {
    if s.is_null() || n == 0 {
        let r = heap::miku_malloc(1);
        if !r.is_null() {
            unsafe { *r = 0; }
        }
        return r;
    }
    let slen = string::miku_strlen(s);
    let total = slen * n;
    let out = heap::miku_malloc(total + 1);
    if out.is_null() {
        return core::ptr::null_mut();
    }
    unsafe {
        for i in 0..n {
            mem::miku_memcpy(out.add(i * slen), s, slen);
        }
        *out.add(total) = 0;
    }
    out
}
