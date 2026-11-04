// sscanf and vsscanf: numbers and words from a string
//
// String forms only; on a stream the one character of lookahead `%d` needs
// would be consumed. `aws-lc` parses its CPU capability override with `sscanf`.
//
// C99: whitespace in the format skips any in the input, other characters must
// match, and a conversion is `%[*][width][length]spec` with `d i u o x X`
// (integers, `i` taking a 0x or 0 prefix), `a e f g` (via `strtod`), `s`, `c`,
// `[set]`, `p`, `n`, and `%%`. Returns the conversions stored, or `EOF` if the
// input ran out before the first.

use core::ffi::VaList;

const EOF: i32 = -1;

#[derive(Clone, Copy, PartialEq)]
enum Len {
    Hh,
    H,
    Default,
    L,
    Ll,
    Size,
    Big, // long double, as a double here
}

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\x0b' | b'\x0c' | b'\r')
}

fn digit(c: u8, base: u32) -> Option<u32> {
    let d = match c {
        b'0'..=b'9' => (c - b'0') as u32,
        b'a'..=b'z' => (c - b'a') as u32 + 10,
        b'A'..=b'Z' => (c - b'A') as u32 + 10,
        _ => return None,
    };
    if d < base { Some(d) } else { None }
}

/// An integer at `s[*at..]`, at most `width` characters. `base` 0 is `%i`'s: a
/// 0x prefix is hex, a leading 0 octal
fn scan_int(s: &[u8], at: &mut usize, width: usize, mut base: u32) -> Option<(bool, u64)> {
    let end = (*at).saturating_add(width).min(s.len());
    let mut i = *at;
    let mut neg = false;
    if i < end && (s[i] == b'+' || s[i] == b'-') {
        neg = s[i] == b'-';
        i += 1;
    }
    if (base == 0 || base == 16) && i + 1 < end && s[i] == b'0' && (s[i + 1] | 0x20) == b'x'
        && i + 2 < end && digit(s[i + 2], 16).is_some()
    {
        i += 2;
        base = 16;
    } else if base == 0 {
        base = if i < end && s[i] == b'0' { 8 } else { 10 };
    }
    let start = i;
    let mut v: u64 = 0;
    while i < end {
        let Some(d) = digit(s[i], base) else { break };
        v = v.wrapping_mul(base as u64).wrapping_add(d as u64);
        i += 1;
    }
    if i == start {
        return None;
    }
    *at = i;
    Some((neg, v))
}

unsafe fn store_int(ap: &mut VaList, len: Len, v: u64) {
    unsafe {
        match len {
            Len::Hh => *ap.next_arg::<*mut u8>() = v as u8,
            Len::H => *ap.next_arg::<*mut u16>() = v as u16,
            Len::Default => *ap.next_arg::<*mut u32>() = v as u32,
            _ => *ap.next_arg::<*mut u64>() = v,
        }
    }
}

/// `[set]` starting just after `[`; returns the set and where the format
/// continues
fn parse_set(f: &[u8], mut i: usize) -> ([bool; 256], usize) {
    let mut set = [false; 256];
    let mut negate = false;
    if i < f.len() && f[i] == b'^' {
        negate = true;
        i += 1;
    }
    let first = i;
    while i < f.len() && (f[i] != b']' || i == first) {
        if i + 2 < f.len() && f[i + 1] == b'-' && f[i + 2] != b']' {
            for c in f[i]..=f[i + 2] {
                set[c as usize] = true;
            }
            i += 3;
        } else {
            set[f[i] as usize] = true;
            i += 1;
        }
    }
    if negate {
        for b in set.iter_mut() {
            *b = !*b;
        }
    }
    (set, (i + 1).min(f.len()))
}

#[no_mangle]
pub unsafe extern "C" fn vsscanf(src: *const u8, fmt: *const u8, mut ap: VaList) -> i32 {
    unsafe { scan(src, fmt, &mut ap) }
}

#[no_mangle]
pub unsafe extern "C" fn sscanf(src: *const u8, fmt: *const u8, mut args: ...) -> i32 {
    unsafe { scan(src, fmt, &mut args) }
}

unsafe fn scan(src: *const u8, fmt: *const u8, ap: &mut VaList) -> i32 {
    if src.is_null() || fmt.is_null() {
        return EOF;
    }
    let s = unsafe { core::slice::from_raw_parts(src, crate::libc::strlen(src)) };
    let f = unsafe { core::slice::from_raw_parts(fmt, crate::libc::strlen(fmt)) };
    let (mut si, mut fi) = (0usize, 0usize);
    let mut stored = 0i32;
    let mut converted_any = false;

    while fi < f.len() {
        let c = f[fi];
        if is_space(c) {
            while fi < f.len() && is_space(f[fi]) { fi += 1; }
            while si < s.len() && is_space(s[si]) { si += 1; }
            continue;
        }
        if c != b'%' {
            if si < s.len() && s[si] == c { si += 1; fi += 1; continue; }
            break;
        }
        fi += 1;
        if fi < f.len() && f[fi] == b'%' {
            while si < s.len() && is_space(s[si]) { si += 1; }
            if si < s.len() && s[si] == b'%' { si += 1; fi += 1; continue; }
            break;
        }
        let suppress = fi < f.len() && f[fi] == b'*';
        if suppress { fi += 1; }
        let mut width = 0usize;
        while fi < f.len() && f[fi].is_ascii_digit() {
            width = width * 10 + (f[fi] - b'0') as usize;
            fi += 1;
        }
        let mut len = Len::Default;
        if fi < f.len() {
            match f[fi] {
                b'h' => { fi += 1; len = if fi < f.len() && f[fi] == b'h' { fi += 1; Len::Hh } else { Len::H }; }
                b'l' => { fi += 1; len = if fi < f.len() && f[fi] == b'l' { fi += 1; Len::Ll } else { Len::L }; }
                b'j' | b'z' | b't' => { fi += 1; len = Len::Size; }
                b'L' | b'q' => { fi += 1; len = Len::Big; }
                _ => {}
            }
        }
        if fi >= f.len() { break; }
        let spec = f[fi];
        fi += 1;
        if spec != b'c' && spec != b'[' && spec != b'n' {
            while si < s.len() && is_space(s[si]) { si += 1; }
        }
        if si >= s.len() && spec != b'n' {
            return if converted_any || stored > 0 { stored } else { EOF };
        }
        let w = if width == 0 { usize::MAX } else { width };
        match spec {
            b'd' | b'i' | b'u' | b'o' | b'x' | b'X' | b'p' => {
                let base = match spec { b'd' | b'u' => 10, b'i' => 0, b'o' => 8, _ => 16 };
                let Some((neg, v)) = scan_int(s, &mut si, w, base) else { break };
                converted_any = true;
                if !suppress {
                    let v = if neg { (v as i64).wrapping_neg() as u64 } else { v };
                    let len = if spec == b'p' { Len::Ll } else { len };
                    unsafe { store_int(ap, len, v) };
                    stored += 1;
                }
            }
            b'a' | b'e' | b'f' | b'g' | b'A' | b'E' | b'F' | b'G' => {
                let mut end: *mut u8 = core::ptr::null_mut();
                let start = unsafe { src.add(si) };
                let v = unsafe { crate::cstrtod::strtod(start, &mut end) };
                let used = (end as usize).saturating_sub(start as usize);
                if used == 0 { break; }
                si += used;
                converted_any = true;
                if !suppress {
                    unsafe {
                        match len {
                            Len::L | Len::Big => *ap.next_arg::<*mut f64>() = v,
                            _ => *ap.next_arg::<*mut f32>() = v as f32,
                        }
                    }
                    stored += 1;
                }
            }
            b's' => {
                let start = si;
                while si < s.len() && !is_space(s[si]) && si - start < w { si += 1; }
                converted_any = true;
                if !suppress {
                    let out = unsafe { ap.next_arg::<*mut u8>() };
                    unsafe {
                        core::ptr::copy_nonoverlapping(s[start..].as_ptr(), out, si - start);
                        *out.add(si - start) = 0;
                    }
                    stored += 1;
                }
            }
            b'c' => {
                let n = if width == 0 { 1 } else { width };
                if si + n > s.len() { break; }
                converted_any = true;
                if !suppress {
                    let out = unsafe { ap.next_arg::<*mut u8>() };
                    unsafe { core::ptr::copy_nonoverlapping(s[si..].as_ptr(), out, n) };
                    stored += 1;
                }
                si += n;
            }
            b'[' => {
                let (set, next) = parse_set(f, fi);
                fi = next;
                let start = si;
                while si < s.len() && set[s[si] as usize] && si - start < w { si += 1; }
                if si == start { break; }
                converted_any = true;
                if !suppress {
                    let out = unsafe { ap.next_arg::<*mut u8>() };
                    unsafe {
                        core::ptr::copy_nonoverlapping(s[start..].as_ptr(), out, si - start);
                        *out.add(si - start) = 0;
                    }
                    stored += 1;
                }
            }
            b'n' => {
                if !suppress {
                    unsafe { store_int(ap, len, si as u64) };
                }
            }
            _ => break,
        }
    }
    stored
}
