#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_itoa(val: i64, buf: *mut u8) {
    if buf.is_null() { return; }
    let mut pos = 0usize;
    let mut num: u64;
    if val < 0 {
        unsafe { *buf = b'-'; }
        pos = 1;
        num = (-(val + 1)) as u64 + 1;
    } else {
        num = val as u64;
    }
    let start = pos;
    if num == 0 {
        unsafe { *buf.add(pos) = b'0'; }
        pos += 1;
    } else {
        while num > 0 {
            unsafe { *buf.add(pos) = b'0' + (num % 10) as u8; }
            pos += 1;
            num /= 10;
        }
        reverse_bytes(buf, start, pos);
    }
    unsafe { *buf.add(pos) = 0; }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_utoa(val: u64, buf: *mut u8) {
    if buf.is_null() { return; }
    let mut num = val;
    let mut pos = 0usize;
    if num == 0 {
        unsafe { *buf = b'0'; *buf.add(1) = 0; }
        return;
    }
    while num > 0 {
        unsafe { *buf.add(pos) = b'0' + (num % 10) as u8; }
        pos += 1;
        num /= 10;
    }
    reverse_bytes(buf, 0, pos);
    unsafe { *buf.add(pos) = 0; }
}

pub fn utoa_hex(val: u64, buf: *mut u8) -> usize {
    if buf.is_null() { return 0; }
    let mut n = val;
    if n == 0 {
        unsafe { *buf = b'0'; *buf.add(1) = 0; }
        return 1;
    }
    let mut pos = 0usize;
    while n > 0 {
        let d = (n & 0xF) as u8;
        unsafe { *buf.add(pos) = if d < 10 { b'0' + d } else { b'a' + d - 10 }; }
        pos += 1;
        n >>= 4;
    }
    reverse_bytes(buf, 0, pos);
    unsafe { *buf.add(pos) = 0; }
    pos
}

pub fn utoa_hex_upper(val: u64, buf: *mut u8) -> usize {
    if buf.is_null() { return 0; }
    let mut n = val;
    if n == 0 {
        unsafe { *buf = b'0'; *buf.add(1) = 0; }
        return 1;
    }
    let mut pos = 0usize;
    while n > 0 {
        let d = (n & 0xF) as u8;
        unsafe { *buf.add(pos) = if d < 10 { b'0' + d } else { b'A' + d - 10 }; }
        pos += 1;
        n >>= 4;
    }
    reverse_bytes(buf, 0, pos);
    unsafe { *buf.add(pos) = 0; }
    pos
}

pub fn utoa_oct(val: u64, buf: *mut u8) -> usize {
    if buf.is_null() { return 0; }
    let mut n = val;
    if n == 0 {
        unsafe { *buf = b'0'; *buf.add(1) = 0; }
        return 1;
    }
    let mut pos = 0usize;
    while n > 0 {
        unsafe { *buf.add(pos) = b'0' + (n & 7) as u8; }
        pos += 1;
        n >>= 3;
    }
    reverse_bytes(buf, 0, pos);
    unsafe { *buf.add(pos) = 0; }
    pos
}

fn reverse_bytes(buf: *mut u8, start: usize, end: usize) {
    let mut l = start;
    let mut r = end - 1;
    while l < r {
        unsafe {
            let tmp = *buf.add(l);
            *buf.add(l) = *buf.add(r);
            *buf.add(r) = tmp;
        }
        l += 1;
        r -= 1;
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_atoi(s: *const u8) -> i64 {
    crate::convert::miku_strtol(s, core::ptr::null_mut(), 10)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_print_int(val: i64) {
    let mut buf = [0u8; 24];
    miku_itoa(val, buf.as_mut_ptr());
    crate::stdio::miku_print(buf.as_ptr());
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_print_hex(val: u64) {
    let mut buf = [0u8; 19];
    buf[0] = b'0';
    buf[1] = b'x';
    let len = utoa_hex(val, unsafe { buf.as_mut_ptr().add(2) });
    unsafe { *buf.as_mut_ptr().add(2 + len) = 0; }
    crate::stdio::miku_print(buf.as_ptr());
}
