// Dynamic string with formatting
//
// Heap-allocated mutable string with printf-like formatting,
// split, join, trim, replace, and other common operations
// This is a higher-level complement to format.rs (StringBuilder)

use crate::heap;
use crate::mem;
use crate::string;

#[repr(C)]
pub struct MikuStr {
    data: *mut u8,
    len: usize,
    cap: usize,
}

fn grow(s: &mut MikuStr, needed: usize) -> bool {
    if s.len + needed <= s.cap { return true; }
    let new_cap = (s.len + needed).max(s.cap * 2).max(32);
    let new_data = heap::miku_realloc(s.data, new_cap);
    if new_data.is_null() { return false; }
    s.data = new_data;
    s.cap = new_cap;
    true
}

// create empty string
#[no_mangle]
pub extern "C" fn miku_str_new() -> MikuStr {
    let data = heap::miku_malloc(32);
    if data.is_null() {
        return MikuStr { data: core::ptr::null_mut(), len: 0, cap: 0 };
    }
    unsafe { *data = 0; }
    MikuStr { data, len: 0, cap: 32 }
}

// create string from C string
#[no_mangle]
pub extern "C" fn miku_str_from(s: *const u8) -> MikuStr {
    let mut str = miku_str_new();
    if !s.is_null() {
        let len = string::miku_strlen(s);
        if grow(&mut str, len + 1) {
            unsafe {
                mem::miku_memcpy(str.data, s, len);
                *str.data.add(len) = 0;
            }
            str.len = len;
        }
    }
    str
}

// create string with given capacity
#[no_mangle]
pub extern "C" fn miku_str_with_capacity(cap: usize) -> MikuStr {
    let actual = if cap < 1 { 1 } else { cap };
    let data = heap::miku_malloc(actual);
    if data.is_null() {
        return MikuStr { data: core::ptr::null_mut(), len: 0, cap: 0 };
    }
    unsafe { *data = 0; }
    MikuStr { data, len: 0, cap: actual }
}

// free string
#[no_mangle]
pub extern "C" fn miku_str_free(s: *mut MikuStr) {
    if s.is_null() { return; }
    unsafe {
        if !(*s).data.is_null() {
            heap::miku_free((*s).data);
        }
        (*s).data = core::ptr::null_mut();
        (*s).len = 0;
        (*s).cap = 0;
    }
}

// get C string pointer
#[no_mangle]
pub extern "C" fn miku_str_cstr(s: *const MikuStr) -> *const u8 {
    if s.is_null() { return core::ptr::null(); }
    unsafe { (*s).data }
}

// get length
#[no_mangle]
pub extern "C" fn miku_str_len(s: *const MikuStr) -> usize {
    if s.is_null() { return 0; }
    unsafe { (*s).len }
}

// check if empty
#[no_mangle]
pub extern "C" fn miku_str_empty(s: *const MikuStr) -> bool {
    if s.is_null() { return true; }
    unsafe { (*s).len == 0 }
}

// append C string
#[no_mangle]
pub extern "C" fn miku_str_push(s: *mut MikuStr, text: *const u8) -> bool {
    if s.is_null() || text.is_null() { return false; }
    let s = unsafe { &mut *s };
    let tlen = string::miku_strlen(text);
    if !grow(s, tlen + 1) { return false; }
    unsafe {
        mem::miku_memcpy(s.data.add(s.len), text, tlen);
        s.len += tlen;
        *s.data.add(s.len) = 0;
    }
    true
}

// append single char
#[no_mangle]
pub extern "C" fn miku_str_push_char(s: *mut MikuStr, c: u8) -> bool {
    if s.is_null() { return false; }
    let s = unsafe { &mut *s };
    if !grow(s, 2) { return false; }
    unsafe {
        *s.data.add(s.len) = c;
        s.len += 1;
        *s.data.add(s.len) = 0;
    }
    true
}

// append bytes
#[no_mangle]
pub extern "C" fn miku_str_push_bytes(s: *mut MikuStr, data: *const u8, len: usize) -> bool {
    if s.is_null() || data.is_null() { return false; }
    let s = unsafe { &mut *s };
    if !grow(s, len + 1) { return false; }
    unsafe {
        mem::miku_memcpy(s.data.add(s.len), data, len);
        s.len += len;
        *s.data.add(s.len) = 0;
    }
    true
}

// append integer as decimal
#[no_mangle]
pub extern "C" fn miku_str_push_int(s: *mut MikuStr, val: i64) -> bool {
    let mut buf = [0u8; 24];
    crate::num::miku_itoa(val, buf.as_mut_ptr());
    miku_str_push(s, buf.as_ptr())
}

// clear string (keep capacity)
#[no_mangle]
pub extern "C" fn miku_str_clear(s: *mut MikuStr) {
    if s.is_null() { return; }
    let s = unsafe { &mut *s };
    s.len = 0;
    if !s.data.is_null() {
        unsafe { *s.data = 0; }
    }
}

// compare with C string
#[no_mangle]
pub extern "C" fn miku_str_eq(s: *const MikuStr, other: *const u8) -> bool {
    if s.is_null() || other.is_null() { return false; }
    let s = unsafe { &*s };
    if s.data.is_null() { return false; }
    string::miku_strcmp(s.data, other) == 0
}

// check if string starts with prefix
#[no_mangle]
pub extern "C" fn miku_str_starts_with(s: *const MikuStr, prefix: *const u8) -> bool {
    if s.is_null() || prefix.is_null() { return false; }
    let s = unsafe { &*s };
    if s.data.is_null() { return false; }
    let plen = string::miku_strlen(prefix);
    if plen > s.len { return false; }
    string::miku_strncmp(s.data, prefix, plen) == 0
}

// check if string ends with suffix
#[no_mangle]
pub extern "C" fn miku_str_ends_with(s: *const MikuStr, suffix: *const u8) -> bool {
    if s.is_null() || suffix.is_null() { return false; }
    let s = unsafe { &*s };
    if s.data.is_null() { return false; }
    let slen = string::miku_strlen(suffix);
    if slen > s.len { return false; }
    unsafe {
        string::miku_strncmp(s.data.add(s.len - slen), suffix, slen) == 0
    }
}

// find substring, returns offset or -1
#[no_mangle]
pub extern "C" fn miku_str_find(s: *const MikuStr, needle: *const u8) -> i32 {
    if s.is_null() || needle.is_null() { return -1; }
    let s = unsafe { &*s };
    if s.data.is_null() { return -1; }
    let nlen = string::miku_strlen(needle);
    if nlen == 0 { return 0; }
    if nlen > s.len { return -1; }

    unsafe {
        for i in 0..=(s.len - nlen) {
            if string::miku_strncmp(s.data.add(i), needle, nlen) == 0 {
                return i as i32;
            }
        }
    }
    -1
}

// check if string contains substring
#[no_mangle]
pub extern "C" fn miku_str_contains(s: *const MikuStr, needle: *const u8) -> bool {
    miku_str_find(s, needle) >= 0
}

// get character at index
#[no_mangle]
pub extern "C" fn miku_str_at(s: *const MikuStr, index: usize) -> u8 {
    if s.is_null() { return 0; }
    let s = unsafe { &*s };
    if index >= s.len || s.data.is_null() { return 0; }
    unsafe { *s.data.add(index) }
}

// trim whitespace from both ends (in-place)
#[no_mangle]
pub extern "C" fn miku_str_trim(s: *mut MikuStr) {
    if s.is_null() { return; }
    let s = unsafe { &mut *s };
    if s.data.is_null() || s.len == 0 { return; }

    // trim leading
    let mut start = 0usize;
    unsafe {
        while start < s.len {
            let c = *s.data.add(start);
            if c != b' ' && c != b'\t' && c != b'\r' && c != b'\n' { break; }
            start += 1;
        }
    }

    // trim trailing
    let mut end = s.len;
    unsafe {
        while end > start {
            let c = *s.data.add(end - 1);
            if c != b' ' && c != b'\t' && c != b'\r' && c != b'\n' { break; }
            end -= 1;
        }
    }

    if start > 0 {
        unsafe {
            mem::miku_memmove(s.data, s.data.add(start), end - start);
        }
    }
    s.len = end - start;
    unsafe { *s.data.add(s.len) = 0; }
}

// convert to uppercase (in-place)
#[no_mangle]
pub extern "C" fn miku_str_to_upper(s: *mut MikuStr) {
    if s.is_null() { return; }
    let s = unsafe { &mut *s };
    if s.data.is_null() { return; }
    unsafe {
        for i in 0..s.len {
            let c = *s.data.add(i);
            if c >= b'a' && c <= b'z' {
                *s.data.add(i) = c - 32;
            }
        }
    }
}

// convert to lowercase (in-place)
#[no_mangle]
pub extern "C" fn miku_str_to_lower(s: *mut MikuStr) {
    if s.is_null() { return; }
    let s = unsafe { &mut *s };
    if s.data.is_null() { return; }
    unsafe {
        for i in 0..s.len {
            let c = *s.data.add(i);
            if c >= b'A' && c <= b'Z' {
                *s.data.add(i) = c + 32;
            }
        }
    }
}

// create substring (new allocation)
#[no_mangle]
pub extern "C" fn miku_str_substr(s: *const MikuStr, start: usize, len: usize) -> MikuStr {
    if s.is_null() { return miku_str_new(); }
    let s = unsafe { &*s };
    if s.data.is_null() || start >= s.len { return miku_str_new(); }

    let actual = len.min(s.len - start);
    let mut result = miku_str_with_capacity(actual + 1);
    unsafe {
        mem::miku_memcpy(result.data, s.data.add(start), actual);
        *result.data.add(actual) = 0;
    }
    result.len = actual;
    result
}

// clone string
#[no_mangle]
pub extern "C" fn miku_str_clone(s: *const MikuStr) -> MikuStr {
    if s.is_null() { return miku_str_new(); }
    let s = unsafe { &*s };
    if s.data.is_null() { return miku_str_new(); }
    miku_str_substr(core::ptr::addr_of!(*s), 0, s.len)
}

// insert C string at position
#[no_mangle]
pub extern "C" fn miku_str_insert(s: *mut MikuStr, pos: usize, text: *const u8) -> bool {
    if s.is_null() || text.is_null() { return false; }
    let s = unsafe { &mut *s };
    let tlen = string::miku_strlen(text);
    if tlen == 0 { return true; }
    let pos = if pos > s.len { s.len } else { pos };
    if !grow(s, tlen + 1) { return false; }
    unsafe {
        // shift existing data right
        mem::miku_memmove(s.data.add(pos + tlen), s.data.add(pos), s.len - pos);
        mem::miku_memcpy(s.data.add(pos), text, tlen);
        s.len += tlen;
        *s.data.add(s.len) = 0;
    }
    true
}

// remove range [start..start+count)
#[no_mangle]
pub extern "C" fn miku_str_remove(s: *mut MikuStr, start: usize, count: usize) -> bool {
    if s.is_null() { return false; }
    let s = unsafe { &mut *s };
    if s.data.is_null() || start >= s.len { return false; }
    let count = count.min(s.len - start);
    unsafe {
        mem::miku_memmove(
            s.data.add(start),
            s.data.add(start + count),
            s.len - start - count,
        );
        s.len -= count;
        *s.data.add(s.len) = 0;
    }
    true
}

// replace first occurrence of needle with replacement
#[no_mangle]
pub extern "C" fn miku_str_replace(s: *mut MikuStr, needle: *const u8, replacement: *const u8) -> bool {
    if s.is_null() || needle.is_null() || replacement.is_null() { return false; }
    let pos = miku_str_find(s, needle);
    if pos < 0 { return false; }
    let nlen = string::miku_strlen(needle);
    let rlen = string::miku_strlen(replacement);
    let s = unsafe { &mut *s };
    let start = pos as usize;

    if rlen > nlen {
        if !grow(s, rlen - nlen + 1) { return false; }
    }
    unsafe {
        // shift tail
        mem::miku_memmove(
            s.data.add(start + rlen),
            s.data.add(start + nlen),
            s.len - start - nlen,
        );
        mem::miku_memcpy(s.data.add(start), replacement, rlen);
        s.len = s.len - nlen + rlen;
        *s.data.add(s.len) = 0;
    }
    true
}

// replace all occurrences of needle with replacement
#[no_mangle]
pub extern "C" fn miku_str_replace_all(s: *mut MikuStr, needle: *const u8, replacement: *const u8) -> i32 {
    if s.is_null() || needle.is_null() || replacement.is_null() { return 0; }
    let nlen = string::miku_strlen(needle);
    if nlen == 0 { return 0; }
    let rlen = string::miku_strlen(replacement);
    let s = unsafe { &mut *s };
    if s.data.is_null() { return 0; }

    let mut count = 0i32;
    let mut pos = 0usize;
    unsafe {
        while pos + nlen <= s.len {
            if string::miku_strncmp(s.data.add(pos), needle, nlen) != 0 {
                pos += 1;
                continue;
            }
            if rlen > nlen && !grow(s, rlen - nlen + 1) { break; }
            mem::miku_memmove(
                s.data.add(pos + rlen),
                s.data.add(pos + nlen),
                s.len - pos - nlen,
            );
            mem::miku_memcpy(s.data.add(pos), replacement, rlen);
            s.len = s.len - nlen + rlen;
            *s.data.add(s.len) = 0;
            count += 1;
            pos += rlen;
        }
    }
    count
}

// reverse string in-place
#[no_mangle]
pub extern "C" fn miku_str_reverse(s: *mut MikuStr) {
    if s.is_null() { return; }
    let s = unsafe { &mut *s };
    if s.data.is_null() || s.len < 2 { return; }
    let mut i = 0usize;
    let mut j = s.len - 1;
    unsafe {
        while i < j {
            let tmp = *s.data.add(i);
            *s.data.add(i) = *s.data.add(j);
            *s.data.add(j) = tmp;
            i += 1;
            j -= 1;
        }
    }
}

// count occurrences of substring
#[no_mangle]
pub extern "C" fn miku_str_count(s: *const MikuStr, needle: *const u8) -> i32 {
    if s.is_null() || needle.is_null() { return 0; }
    let s = unsafe { &*s };
    if s.data.is_null() { return 0; }
    let nlen = string::miku_strlen(needle);
    if nlen == 0 || nlen > s.len { return 0; }
    let mut count = 0i32;
    let mut i = 0usize;
    unsafe {
        while i + nlen <= s.len {
            if string::miku_strncmp(s.data.add(i), needle, nlen) == 0 {
                count += 1;
                i += nlen;
            } else {
                i += 1;
            }
        }
    }
    count
}

// repeat string n times (returns new string)
#[no_mangle]
pub extern "C" fn miku_str_repeat_new(text: *const u8, n: u32) -> MikuStr {
    if text.is_null() || n == 0 { return miku_str_new(); }
    let tlen = string::miku_strlen(text);
    let total = tlen * n as usize;
    let mut result = miku_str_with_capacity(total + 1);
    for _ in 0..n {
        miku_str_push(&mut result, text);
    }
    result
}

//   split string by delimiter, calls callback for each part
// callback(part_ptr, part_len, user_data) - parts are NOT null-terminated
#[no_mangle]
pub extern "C" fn miku_str_split(
    s: *const MikuStr,
    delim: u8,
    cb: extern "C" fn(*const u8, usize, usize),
    user_data: usize,
) -> i32 {
    if s.is_null() { return 0; }
    let s = unsafe { &*s };
    if s.data.is_null() || s.len == 0 { return 0; }
    let mut count = 0i32;
    let mut start = 0usize;
    unsafe {
        for i in 0..s.len {
            if *s.data.add(i) == delim {
                cb(s.data.add(start), i - start, user_data);
                count += 1;
                start = i + 1;
            }
        }
        // last part
        cb(s.data.add(start), s.len - start, user_data);
        count += 1;
    }
    count
}
