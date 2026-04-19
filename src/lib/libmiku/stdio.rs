#[no_mangle]
pub extern "C" fn miku_print(s: *const u8) {
    if s.is_null() { return; }
    let len = crate::string::miku_strlen(s);
    if len > 0 { crate::io::miku_write(1, s, len); }
}

#[no_mangle]
pub extern "C" fn miku_println(s: *const u8) {
    miku_print(s);
    crate::io::miku_write(1, b"\n".as_ptr(), 1);
}

#[no_mangle]
pub extern "C" fn miku_puts(s: *const u8) -> i32 {
    miku_println(s);
    0
}

#[no_mangle]
pub extern "C" fn miku_eprint(s: *const u8) {
    if s.is_null() { return; }
    let len = crate::string::miku_strlen(s);
    if len > 0 { crate::io::miku_write(2, s, len); }
}

#[no_mangle]
pub extern "C" fn miku_eprintln(s: *const u8) {
    miku_eprint(s);
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
}

#[no_mangle]
pub extern "C" fn miku_putchar(c: i32) -> i32 {
    let b = c as u8;
    crate::io::miku_write(1, &b as *const u8, 1);
    c
}

#[no_mangle]
pub extern "C" fn miku_getchar() -> i32 {
    let mut b: u8 = 0;
    let n = crate::io::miku_read(0, &mut b as *mut u8, 1);
    if n <= 0 { -1 } else { b as i32 }
}

#[no_mangle]
pub extern "C" fn miku_readline(buf: *mut u8, max_len: usize) -> i32 {
    if buf.is_null() || max_len < 2 { return -1; }
    let mut pos = 0usize;
    let limit = max_len - 1;
    loop {
        let mut byte: u8 = 0;
        let n = crate::io::miku_read(0, &mut byte as *mut u8, 1);
        if n <= 0 {
            if pos == 0 { return -1; }
            break;
        }
        if byte == b'\n' || byte == b'\r' { break; }
        if byte == 0x7F || byte == 0x08 {
            if pos > 0 { pos -= 1; }
            continue;
        }
        if byte < 0x20 { continue; }
        if pos < limit {
            unsafe { *buf.add(pos) = byte; }
            pos += 1;
        }
    }
    unsafe { *buf.add(pos) = 0; }
    pos as i32
}

#[no_mangle]
pub extern "C" fn miku_getline() -> *mut u8 {
    let mut cap: usize = 128;
    let mut buf = crate::heap::miku_malloc(cap);
    if buf.is_null() { return core::ptr::null_mut(); }
    let mut pos = 0usize;
    loop {
        let mut byte: u8 = 0;
        let n = crate::io::miku_read(0, &mut byte as *mut u8, 1);
        if n <= 0 {
            if pos == 0 { crate::heap::miku_free(buf); return core::ptr::null_mut(); }
            break;
        }
        if byte == b'\n' || byte == b'\r' { break; }
        if byte == 0x7F || byte == 0x08 {
            if pos > 0 { pos -= 1; }
            continue;
        }
        if byte < 0x20 { continue; }
        if pos + 1 >= cap {
            let new_cap = cap * 2;
            let new_buf = crate::heap::miku_realloc(buf, new_cap);
            if new_buf.is_null() { break; }
            buf = new_buf;
            cap = new_cap;
        }
        unsafe { *buf.add(pos) = byte; }
        pos += 1;
    }
    unsafe { *buf.add(pos) = 0; }
    buf
}

pub fn write_u64(fd: u64, val: u64) {
    let mut buf = [0u8; 21];
    crate::num::miku_utoa(val, buf.as_mut_ptr());
    let len = crate::string::miku_strlen(buf.as_ptr());
    crate::io::miku_write(fd, buf.as_ptr(), len);
}

pub fn write_i64(fd: u64, val: i64) {
    let mut buf = [0u8; 24];
    crate::num::miku_itoa(val, buf.as_mut_ptr());
    let len = crate::string::miku_strlen(buf.as_ptr());
    crate::io::miku_write(fd, buf.as_ptr(), len);
}
