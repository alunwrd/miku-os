//! `<arpa/inet.h>`

use crate::libc::*;

//
// Address text, IPv4 only: `AF_INET6` is `EAFNOSUPPORT`. Four decimal parts,
// each at most 255 and unsigned, the same parse as `getaddrinfo`
pub(crate) use miku_abi::socket::AF_INET as AF_INET_C;

#[no_mangle]
#[inline(never)]
pub extern "C" fn inet_pton(af: i32, src: *const u8, dst: *mut u8) -> i32 {
    if af != AF_INET_C { return fail(97); } // EAFNOSUPPORT
    if src.is_null() || dst.is_null() { return fail(EFAULT); }
    let s = unsafe { core::slice::from_raw_parts(src, strlen(src)) };
    let mut out = [0u8; 4];
    let mut parts = s.split(|&b| b == b'.');
    for o in out.iter_mut() {
        let Some(p) = parts.next() else { return 0 };
        if p.is_empty() || p.len() > 3 || !p.iter().all(u8::is_ascii_digit) { return 0; }
        let v = p.iter().fold(0u32, |a, &d| a * 10 + (d - b'0') as u32);
        if v > 255 { return 0; }
        *o = v as u8;
    }
    if parts.next().is_some() { return 0; }
    unsafe { core::ptr::copy_nonoverlapping(out.as_ptr(), dst, 4) };
    1
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn inet_ntop(af: i32, src: *const u8, dst: *mut u8, size: u32) -> *const u8 {
    if af != AF_INET_C { fail(97); return core::ptr::null(); }
    if src.is_null() || dst.is_null() { fail(EFAULT); return core::ptr::null(); }
    let a = unsafe { core::slice::from_raw_parts(src, 4) };
    let mut text = [0u8; 16];
    let mut n = 0;
    for (i, &b) in a.iter().enumerate() {
        if i > 0 { text[n] = b'.'; n += 1; }
        let mut digits = [0u8; 3];
        let mut k = 0;
        let mut v = b;
        loop {
            digits[k] = b'0' + v % 10;
            k += 1;
            v /= 10;
            if v == 0 { break; }
        }
        while k > 0 { k -= 1; text[n] = digits[k]; n += 1; }
    }
    if n + 1 > size as usize { fail(28); return core::ptr::null(); } // ENOSPC
    unsafe {
        core::ptr::copy_nonoverlapping(text.as_ptr(), dst, n);
        *dst.add(n) = 0;
    }
    dst
}
