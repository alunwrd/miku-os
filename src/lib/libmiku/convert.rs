// String-to-number conversion (strtol, strtoul)

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strtol(s: *const u8, endptr: *mut *const u8, base: i32) -> i64 {
    if s.is_null() { return 0; }
    let mut i = 0usize;
    unsafe { while crate::ctype::miku_isspace(*s.add(i) as i32) != 0 { i += 1; } }

    let neg = unsafe { *s.add(i) } == b'-';
    if neg || unsafe { *s.add(i) } == b'+' { i += 1; }

    let mut radix = base as u64;
    if radix == 0 {
        if unsafe { *s.add(i) } == b'0' {
            i += 1;
            let next = unsafe { *s.add(i) };
            if next == b'x' || next == b'X' { i += 1; radix = 16; }
            else { radix = 8; }
        } else {
            radix = 10;
        }
    } else if radix == 16 {
        if unsafe { *s.add(i) } == b'0' {
            let next = unsafe { *s.add(i + 1) };
            if next == b'x' || next == b'X' { i += 2; }
        }
    }

    let mut result: i64 = 0;
    unsafe {
        loop {
            let c = *s.add(i);
            let digit = if c >= b'0' && c <= b'9' { (c - b'0') as u64 }
                else if c >= b'a' && c <= b'f' { (c - b'a' + 10) as u64 }
                else if c >= b'A' && c <= b'F' { (c - b'A' + 10) as u64 }
                else { break; };
            if digit >= radix { break; }
            result = result.wrapping_mul(radix as i64).wrapping_add(digit as i64);
            i += 1;
        }
        if !endptr.is_null() { *endptr = s.add(i); }
    }
    // wrapping_neg: -result overflows for i64::MIN; wrapping is the C/POSIX behavior
    if neg { result.wrapping_neg() } else { result }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strtoul(s: *const u8, endptr: *mut *const u8, base: i32) -> u64 {
    if s.is_null() { return 0; }
    let mut i = 0usize;
    unsafe { while crate::ctype::miku_isspace(*s.add(i) as i32) != 0 { i += 1; } }

    // unsigned: no minus sign; a leading '+' is still accepted
    if unsafe { *s.add(i) } == b'+' { i += 1; }

    let mut radix = base as u64;
    if radix == 0 {
        if unsafe { *s.add(i) } == b'0' {
            i += 1;
            let next = unsafe { *s.add(i) };
            if next == b'x' || next == b'X' { i += 1; radix = 16; }
            else { radix = 8; }
        } else {
            radix = 10;
        }
    } else if radix == 16 {
        if unsafe { *s.add(i) } == b'0' {
            let next = unsafe { *s.add(i + 1) };
            if next == b'x' || next == b'X' { i += 2; }
        }
    }

    let mut result: u64 = 0;
    unsafe {
        loop {
            let c = *s.add(i);
            let digit = if c >= b'0' && c <= b'9' { (c - b'0') as u64 }
                else if c >= b'a' && c <= b'f' { (c - b'a' + 10) as u64 }
                else if c >= b'A' && c <= b'F' { (c - b'A' + 10) as u64 }
                else { break; };
            if digit >= radix { break; }
            result = result.wrapping_mul(radix).wrapping_add(digit);
            i += 1;
        }
        if !endptr.is_null() { *endptr = s.add(i); }
    }
    result
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_strtod(s: *const u8, endptr: *mut *const u8) -> i64 {
    // integer-only stub: parse as integer, no floating point in no_std
    miku_strtol(s, endptr, 10)
}

// integer to string with arbitrary base (2..36)
// writes into buf, returns pointer to first digit in buf
// buf must be at least 66 bytes (64 bits + sign + null)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_itoa_base(val: i64, buf: *mut u8, base: i32) -> *mut u8 {
    if buf.is_null() || base < 2 || base > 36 { return core::ptr::null_mut(); }

    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let radix = base as u64;
    let neg = val < 0 && base == 10;
    let mut uval = if neg { (val as i128).wrapping_neg() as u64 } else { val as u64 };

    // write digits backwards starting at end of buffer
    let mut pos = 65usize;
    unsafe {
        *buf.add(pos) = 0; // null terminator
        pos -= 1;
        if uval == 0 {
            *buf.add(pos) = b'0';
        } else {
            while uval > 0 {
                *buf.add(pos) = DIGITS[(uval % radix) as usize];
                uval /= radix;
                if uval > 0 { pos -= 1; }
            }
        }
        if neg {
            pos -= 1;
            *buf.add(pos) = b'-';
        }
        buf.add(pos)
    }
}

// unsigned integer to string with arbitrary base (2..36)
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_utoa_base(val: u64, buf: *mut u8, base: i32) -> *mut u8 {
    if buf.is_null() || base < 2 || base > 36 { return core::ptr::null_mut(); }

    const DIGITS: &[u8] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let radix = base as u64;
    let mut uval = val;

    let mut pos = 65usize;
    unsafe {
        *buf.add(pos) = 0;
        pos -= 1;
        if uval == 0 {
            *buf.add(pos) = b'0';
        } else {
            while uval > 0 {
                *buf.add(pos) = DIGITS[(uval % radix) as usize];
                uval /= radix;
                if uval > 0 { pos -= 1; }
            }
        }
        buf.add(pos)
    }
}

