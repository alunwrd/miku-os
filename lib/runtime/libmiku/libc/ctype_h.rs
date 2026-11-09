//! `<ctype.h>`

use crate::libc::*;

// printf family: `lib/runtime/libmiku/libc/printf.rs`, over a real `va_list`

#[no_mangle]
#[inline(never)]
pub extern "C" fn isdigit(c: i32) -> i32 { ctype::miku_isdigit(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isalpha(c: i32) -> i32 { ctype::miku_isalpha(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isalnum(c: i32) -> i32 { ctype::miku_isalnum(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isspace(c: i32) -> i32 { ctype::miku_isspace(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isupper(c: i32) -> i32 { ctype::miku_isupper(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn islower(c: i32) -> i32 { ctype::miku_islower(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isprint(c: i32) -> i32 { ctype::miku_isprint(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn ispunct(c: i32) -> i32 { ctype::miku_ispunct(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iscntrl(c: i32) -> i32 { ctype::miku_iscntrl(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isxdigit(c: i32) -> i32 { ctype::miku_isxdigit(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isgraph(c: i32) -> i32 { ctype::miku_isgraph(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isblank(c: i32) -> i32 { ctype::miku_isblank(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn isascii(c: i32) -> i32 { ctype::miku_isascii(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn toupper(c: i32) -> i32 { ctype::miku_toupper(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn tolower(c: i32) -> i32 { ctype::miku_tolower(c) }

#[no_mangle]
#[inline(never)]
pub extern "C" fn toascii(c: i32) -> i32 { ctype::miku_toascii(c) }
