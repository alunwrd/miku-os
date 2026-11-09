// plain (non-C-ABI) helpers duplicated from num.rs: pure functions, no state

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
