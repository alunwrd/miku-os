#[no_mangle]
pub extern "C" fn miku_assert_fail(expr: *const u8, file: *const u8, line: i32) {
    crate::io::miku_write(2, b"Assert Failed: ".as_ptr(), 15);
    if !expr.is_null() {
        let len = crate::string::miku_strlen(expr);
        crate::io::miku_write(2, expr, len);
    }
    if !file.is_null() {
        crate::io::miku_write(2, b" at ".as_ptr(), 4);
        let len = crate::string::miku_strlen(file);
        crate::io::miku_write(2, file, len);
        crate::io::miku_write(2, b":".as_ptr(), 1);
        let mut buf = [0u8; 12];
        crate::num::miku_itoa(line as i64, buf.as_mut_ptr());
        let blen = crate::string::miku_strlen(buf.as_ptr());
        crate::io::miku_write(2, buf.as_ptr(), blen);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

#[no_mangle]
pub extern "C" fn miku_panic(msg: *const u8) -> ! {
    crate::io::miku_write(2, b"panic: ".as_ptr(), 7);
    if !msg.is_null() {
        let len = crate::string::miku_strlen(msg);
        crate::io::miku_write(2, msg, len);
    } else {
        crate::io::miku_write(2, b"(no message)".as_ptr(), 12);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

#[no_mangle]
pub extern "C" fn miku_abort() -> ! {
    crate::io::miku_write(2, b"abort()\n".as_ptr(), 8);
    crate::proc::miku_exit(134);
}

pub fn panic_fmt(prefix: &[u8], msg: &[u8]) -> ! {
    crate::io::miku_write(2, prefix.as_ptr(), prefix.len());
    crate::io::miku_write(2, msg.as_ptr(), msg.len());
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

// assert two integers are equal, print both if they differ
#[no_mangle]
pub extern "C" fn miku_assert_eq(a: i64, b: i64, file: *const u8, line: i32) {
    if a == b { return; }
    crate::io::miku_write(2, b"Assert Failed: ".as_ptr(), 15);
    let mut buf = [0u8; 24];
    crate::num::miku_itoa(a, buf.as_mut_ptr());
    let len = crate::string::miku_strlen(buf.as_ptr());
    crate::io::miku_write(2, buf.as_ptr(), len);
    crate::io::miku_write(2, b" != ".as_ptr(), 4);
    crate::num::miku_itoa(b, buf.as_mut_ptr());
    let len = crate::string::miku_strlen(buf.as_ptr());
    crate::io::miku_write(2, buf.as_ptr(), len);
    if !file.is_null() {
        crate::io::miku_write(2, b" at ".as_ptr(), 4);
        let flen = crate::string::miku_strlen(file);
        crate::io::miku_write(2, file, flen);
        crate::io::miku_write(2, b":".as_ptr(), 1);
        crate::num::miku_itoa(line as i64, buf.as_mut_ptr());
        let blen = crate::string::miku_strlen(buf.as_ptr());
        crate::io::miku_write(2, buf.as_ptr(), blen);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

// assert pointer is not null
#[no_mangle]
pub extern "C" fn miku_assert_not_null(ptr: *const u8, name: *const u8, file: *const u8, line: i32) {
    if !ptr.is_null() { return; }
    crate::io::miku_write(2, b"Assert Failed: ".as_ptr(), 15);
    if !name.is_null() {
        let len = crate::string::miku_strlen(name);
        crate::io::miku_write(2, name, len);
    }
    crate::io::miku_write(2, b" is null".as_ptr(), 8);
    if !file.is_null() {
        crate::io::miku_write(2, b" at ".as_ptr(), 4);
        let flen = crate::string::miku_strlen(file);
        crate::io::miku_write(2, file, flen);
        crate::io::miku_write(2, b":".as_ptr(), 1);
        let mut buf = [0u8; 12];
        crate::num::miku_itoa(line as i64, buf.as_mut_ptr());
        let blen = crate::string::miku_strlen(buf.as_ptr());
        crate::io::miku_write(2, buf.as_ptr(), blen);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

// unreachable code marker
#[no_mangle]
pub extern "C" fn miku_unreachable(file: *const u8, line: i32) -> ! {
    crate::io::miku_write(2, b"unreachable code reached".as_ptr(), 24);
    if !file.is_null() {
        crate::io::miku_write(2, b" at ".as_ptr(), 4);
        let flen = crate::string::miku_strlen(file);
        crate::io::miku_write(2, file, flen);
        crate::io::miku_write(2, b":".as_ptr(), 1);
        let mut buf = [0u8; 12];
        crate::num::miku_itoa(line as i64, buf.as_mut_ptr());
        let blen = crate::string::miku_strlen(buf.as_ptr());
        crate::io::miku_write(2, buf.as_ptr(), blen);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}

// todo marker - prints message and aborts
#[no_mangle]
pub extern "C" fn miku_todo(msg: *const u8) -> ! {
    crate::io::miku_write(2, b"TODO: ".as_ptr(), 6);
    if !msg.is_null() {
        let len = crate::string::miku_strlen(msg);
        crate::io::miku_write(2, msg, len);
    } else {
        crate::io::miku_write(2, b"not yet implemented".as_ptr(), 19);
    }
    crate::io::miku_write(2, b"\n".as_ptr(), 1);
    crate::proc::miku_exit(134);
}
