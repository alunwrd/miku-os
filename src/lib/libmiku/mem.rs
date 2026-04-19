#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memset(dst: *mut u8, val: i32, n: usize) -> *mut u8 {
    if dst.is_null() || n == 0 { return dst; }
    let b = val as u8;
    let mut i = 0usize;

    while i < n {
        unsafe { core::ptr::write_volatile(dst.add(i), b); }
        i += 1;
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memcpy(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() || n == 0 { return dst; }
    let mut i = 0usize;
    while i < n {
        unsafe {
            let b = core::ptr::read_volatile(src.add(i));
            core::ptr::write_volatile(dst.add(i), b);
        }
        i += 1;
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memmove(dst: *mut u8, src: *const u8, n: usize) -> *mut u8 {
    if dst.is_null() || src.is_null() || n == 0 { return dst; }
    if (dst as usize) < (src as usize) || (dst as usize) >= (src as usize) + n {
        return miku_memcpy(dst, src, n);
    }
    let mut i = n;
    while i > 0 {
        i -= 1;
        unsafe {
            let b = core::ptr::read_volatile(src.add(i));
            core::ptr::write_volatile(dst.add(i), b);
        }
    }
    dst
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memcmp(a: *const u8, b: *const u8, n: usize) -> i32 {
    for i in 0..n {
        let ca = unsafe { core::ptr::read_volatile(a.add(i)) };
        let cb = unsafe { core::ptr::read_volatile(b.add(i)) };
        if ca != cb { return ca as i32 - cb as i32; }
    }
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_bzero(dst: *mut u8, n: usize) {
    miku_memset(dst, 0, n);
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    if s.is_null() { return core::ptr::null(); }
    let target = c as u8;
    for i in 0..n {
        if unsafe { core::ptr::read_volatile(s.add(i)) } == target {
            return unsafe { s.add(i) };
        }
    }
    core::ptr::null()
}

// reverse memchr - search from end
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memrchr(s: *const u8, c: i32, n: usize) -> *const u8 {
    if s.is_null() || n == 0 { return core::ptr::null(); }
    let target = c as u8;
    let mut i = n;
    while i > 0 {
        i -= 1;
        if unsafe { core::ptr::read_volatile(s.add(i)) } == target {
            return unsafe { s.add(i) };
        }
    }
    core::ptr::null()
}

// find byte sequence in memory (like strstr but for raw bytes)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_memmem(
    haystack: *const u8, hlen: usize,
    needle: *const u8, nlen: usize,
) -> *const u8 {
    if haystack.is_null() || needle.is_null() { return core::ptr::null(); }
    if nlen == 0 { return haystack; }
    if nlen > hlen { return core::ptr::null(); }

    let limit = hlen - nlen;
    for i in 0..=limit {
        if miku_memcmp(unsafe { haystack.add(i) }, needle, nlen) == 0 {
            return unsafe { haystack.add(i) };
        }
    }
    core::ptr::null()
}
