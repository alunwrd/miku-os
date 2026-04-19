#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isdigit(c: i32) -> i32 {
    if c >= b'0' as i32 && c <= b'9' as i32 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isalpha(c: i32) -> i32 {
    if (c >= b'a' as i32 && c <= b'z' as i32)
        || (c >= b'A' as i32 && c <= b'Z' as i32) { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isalnum(c: i32) -> i32 {
    if miku_isalpha(c) != 0 || miku_isdigit(c) != 0 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isspace(c: i32) -> i32 {
    match c as u8 {
        b' ' | b'\t' | b'\n' | b'\r' | 0x0B | 0x0C => 1,
        _ => 0,
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isupper(c: i32) -> i32 {
    if c >= b'A' as i32 && c <= b'Z' as i32 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_islower(c: i32) -> i32 {
    if c >= b'a' as i32 && c <= b'z' as i32 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isprint(c: i32) -> i32 {
    if c >= 0x20 && c <= 0x7E { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_ispunct(c: i32) -> i32 {
    if miku_isprint(c) != 0 && miku_isalnum(c) == 0 && c != b' ' as i32 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_iscntrl(c: i32) -> i32 {
    if (c >= 0 && c < 0x20) || c == 0x7F { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isxdigit(c: i32) -> i32 {
    if miku_isdigit(c) != 0
        || (c >= b'a' as i32 && c <= b'f' as i32)
        || (c >= b'A' as i32 && c <= b'F' as i32) { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_toupper(c: i32) -> i32 {
    if c >= b'a' as i32 && c <= b'z' as i32 { c - 32 } else { c }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_tolower(c: i32) -> i32 {
    if c >= b'A' as i32 && c <= b'Z' as i32 { c + 32 } else { c }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isgraph(c: i32) -> i32 {
    if c > 0x20 && c <= 0x7E { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isblank(c: i32) -> i32 {
    if c == b' ' as i32 || c == b'\t' as i32 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_isascii(c: i32) -> i32 {
    if c >= 0 && c <= 127 { 1 } else { 0 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_toascii(c: i32) -> i32 {
    c & 0x7F
}
