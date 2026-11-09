//! `<string.h>`

use crate::libc::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlen(s: *const u8) -> usize {
    string::miku_strlen(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strnlen(s: *const u8, maxlen: usize) -> usize {
    string::miku_strnlen(s, maxlen)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcmp(s1: *const u8, s2: *const u8) -> i32 {
    string::miku_strcmp(s1, s2)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    string::miku_strncmp(s1, s2, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcasecmp(s1: *const u8, s2: *const u8) -> i32 {
    string::miku_strcasecmp(s1, s2)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncasecmp(s1: *const u8, s2: *const u8, n: usize) -> i32 {
    string::miku_strncasecmp(s1, s2, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcpy(dst: *mut u8, src: *const u8) -> *mut u8 {
    string::miku_strcpy(dst, src)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    string::miku_strncpy(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlcpy(dst: *mut u8, src: *const u8, size: usize) -> usize {
    string::miku_strlcpy(dst, src, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcat(dst: *mut u8, src: *const u8) -> *mut u8 {
    string::miku_strcat(dst, src)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strncat(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    string::miku_strncat(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strlcat(dst: *mut u8, src: *const u8, size: usize) -> usize {
    string::miku_strlcat(dst, src, size)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strchr(s: *const u8, c: i32) -> *const u8 {
    string::miku_strchr(s, c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strrchr(s: *const u8, c: i32) -> *const u8 {
    string::miku_strrchr(s, c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strstr(haystack: *const u8, needle: *const u8) -> *const u8 {
    string::miku_strstr(haystack, needle)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strpbrk(s: *const u8, accept: *const u8) -> *const u8 {
    string::miku_strpbrk(s, accept)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strspn(s: *const u8, accept: *const u8) -> usize {
    string::miku_strspn(s, accept)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strcspn(s: *const u8, reject: *const u8) -> usize {
    string::miku_strcspn(s, reject)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strdup(s: *const u8) -> *mut u8 {
    string::miku_strdup(s)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strndup(s: *const u8, n: usize) -> *mut u8 {
    string::miku_strndup(s, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtok(s: *mut u8, delim: *const u8) -> *mut u8 {
    string::miku_strtok(s, delim)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strtok_r(s: *mut u8, delim: *const u8, saveptr: *mut *mut u8) -> *mut u8 {
    string::miku_strtok_r(s, delim, saveptr)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn strsep(stringp: *mut *mut u8, delim: *const u8) -> *mut u8 {
    string::miku_strsep(stringp, delim)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    mem::miku_memset(dst, val, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    mem::miku_memcpy(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    mem::miku_memmove(dst, src, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    mem::miku_memcmp(a, b, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    mem::miku_memchr(s, c, n)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn memmem(
    haystack: *const u8, hlen: usize,
    needle: *const u8, nlen: usize,
) -> *const u8 {
    mem::miku_memmem(haystack, hlen, needle, nlen)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn bzero(dst: *mut u8, n: usize) {
    mem::miku_bzero(dst, n);
}

// The POSIX names Rust crates expect
//
// Written for crates saying `cfg(unix)`. `poll` and `fcntl` are essential:
// `polling`, and so `async-io` and `smol`, is a reactor over `epoll`, `kqueue`,
// or `poll(2)`, and here it is the third.
//
// These declare what MikuOS does. Where it differs (a socket is not a
// descriptor in the VFS table, no `shutdown`, no IPv6), the function is absent;
// see `dev/std-architecture.md`.

/// `strerror_r`, the reentrant one. XSI semantics: 0 on success, `ERANGE` when
/// the buffer is too small, always NUL-terminated
#[no_mangle]
#[inline(never)]
pub extern "C" fn strerror_r(errnum: i32, buf: *mut u8, buflen: usize) -> i32 {
    if buf.is_null() || buflen == 0 { return EINVAL; }
    let src = strerror(errnum as i64);
    let len = string::miku_strlen(src);
    if len + 1 > buflen {
        unsafe {
            mem::miku_memcpy(buf, src, buflen - 1);
            *buf.add(buflen - 1) = 0;
        }
        return ERANGE;
    }
    unsafe {
        mem::miku_memcpy(buf, src, len);
        *buf.add(len) = 0;
    }
    0
}

/// With one locale, collation is byte order
#[no_mangle]
#[inline(never)]
pub extern "C" fn strcoll(a: *const u8, b: *const u8) -> i32 {
    strcmp(a, b)
}
