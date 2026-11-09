//! `<wctype.h>`

use crate::libc::*;

// wctype.h: Unicode classification from `core`'s tables, the same as Rust code
// in this process. Not a locale; there is one
pub(crate) fn wide(c: u32) -> Option<char> {
    char::from_u32(c)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswalpha(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_alphabetic()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswalnum(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_alphanumeric()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswdigit(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_ascii_digit()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswxdigit(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_ascii_hexdigit()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswspace(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_whitespace()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswblank(c: u32) -> i32 {
    wide(c).is_some_and(|c| c == '\t' || (c.is_whitespace() && !matches!(c, '\n' | '\x0b' | '\x0c' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}'))) as i32
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswcntrl(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_control()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswlower(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_lowercase()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswupper(c: u32) -> i32 { wide(c).is_some_and(|c| c.is_uppercase()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswprint(c: u32) -> i32 { wide(c).is_some_and(|c| !c.is_control()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswgraph(c: u32) -> i32 { wide(c).is_some_and(|c| !c.is_control() && !c.is_whitespace()) as i32 }

#[no_mangle]
#[inline(never)]
pub extern "C" fn iswpunct(c: u32) -> i32 {
    wide(c).is_some_and(|c| !c.is_control() && !c.is_whitespace() && !c.is_alphanumeric()) as i32
}

/// The one-character mapping. A character mapping to several (German sharp s to
/// "SS") is returned unchanged, as C requires
#[no_mangle]
#[inline(never)]
pub extern "C" fn towlower(c: u32) -> u32 {
    match wide(c) {
        Some(ch) => {
            let mut it = ch.to_lowercase();
            match (it.next(), it.next()) {
                (Some(l), None) => l as u32,
                _ => c,
            }
        }
        None => c,
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn towupper(c: u32) -> u32 {
    match wide(c) {
        Some(ch) => {
            let mut it = ch.to_uppercase();
            match (it.next(), it.next()) {
                (Some(u), None) => u as u32,
                _ => c,
            }
        }
        None => c,
    }
}
