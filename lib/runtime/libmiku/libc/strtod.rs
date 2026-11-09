//! `strtod`, `strtof`, `atof`.
//!
//! C's syntax is scanned here: leading space, a sign,
//! `inf`/`infinity`/`nan`/`nan(chars)`, hexadecimal (`0x1.8p3`), decimal with
//! an optional exponent. Decimal digits go to `core`'s parser, which rounds
//! correctly (Eisel-Lemire with a fallback). SQLite reads every REAL back
//! through this.
//!
//! No `strtold`: a `long double` is 80 bits returned in `st(0)`, which Rust
//! cannot produce. `<stdlib.h>` does not declare it.

use crate::libc::set_errno;

use miku_abi::errno::ERANGE;

fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

fn eq_ci(p: *const u8, word: &[u8]) -> bool {
    word.iter().enumerate().all(|(i, &w)| unsafe { (*p.add(i)).to_ascii_lowercase() == w })
}

/// What was scanned: the value and the bytes used, zero when no number was
/// found
unsafe fn scan(s: *const u8, single: bool) -> (f64, usize) {
    let mut p = s;
    while is_space(*p) {
        p = p.add(1);
    }
    let neg = *p == b'-';
    if *p == b'-' || *p == b'+' {
        p = p.add(1);
    }
    let sign = |v: f64| if neg { -v } else { v };

    if eq_ci(p, b"inf") {
        let n = if eq_ci(p, b"infinity") { 8 } else { 3 };
        return (sign(f64::INFINITY), p.add(n).offset_from(s) as usize);
    }
    if eq_ci(p, b"nan") {
        let mut q = p.add(3);
        if *q == b'(' {
            let mut r = q.add(1);
            while (*r).is_ascii_alphanumeric() || *r == b'_' {
                r = r.add(1);
            }
            if *r == b')' {
                q = r.add(1);
            }
        }
        return (sign(f64::NAN), q.offset_from(s) as usize);
    }
    if *p == b'0' && (*p.add(1) | 0x20) == b'x' {
        if let Some((v, end)) = hex(p.add(2)) {
            return (sign(v), end.offset_from(s) as usize);
        }
        // "0x" with no hex digits after it is the number 0, ending at the 'x'
        return (sign(0.0), p.add(1).offset_from(s) as usize);
    }

    // Decimal: digits, a point, digits, with at least one digit, and an
    // exponent only if it has digits
    let start = p;
    let mut digits = 0;
    while (*p).is_ascii_digit() {
        p = p.add(1);
        digits += 1;
    }
    if *p == b'.' {
        p = p.add(1);
        while (*p).is_ascii_digit() {
            p = p.add(1);
            digits += 1;
        }
    }
    if digits == 0 {
        return (0.0, 0);
    }
    if (*p | 0x20) == b'e' {
        let mut q = p.add(1);
        if *q == b'-' || *q == b'+' {
            q = q.add(1);
        }
        if (*q).is_ascii_digit() {
            while (*q).is_ascii_digit() {
                q = q.add(1);
            }
            p = q;
        }
    }
    let text = core::str::from_utf8_unchecked(core::slice::from_raw_parts(start, p.offset_from(start) as usize));
    let v = if single {
        text.parse::<f32>().map(|x| x as f64).unwrap_or(0.0)
    } else {
        text.parse::<f64>().unwrap_or(0.0)
    };
    // Out of range is ERANGE: overflow to infinity from finite digits, or a
    // non-zero number becoming zero
    let nonzero = text.bytes().take_while(|&c| c != b'e' && c != b'E').any(|c| (b'1'..=b'9').contains(&c));
    if v.is_infinite() || (v == 0.0 && nonzero) {
        set_errno(ERANGE);
    }
    (sign(v), p.offset_from(s) as usize)
}

/// After "0x": hex digits, an optional point and more, then an optional binary
/// exponent. The mantissa keeps 60 bits and a sticky bit, so the one rounding,
/// `u64` to `f64`, is correct
unsafe fn hex(mut p: *const u8) -> Option<(f64, *const u8)> {
    let mut mant: u64 = 0;
    let mut sticky = false;
    let mut exp: i64 = 0;
    let mut any = false;
    let mut after_point = false;
    loop {
        let c = *p;
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            b'.' if !after_point => {
                after_point = true;
                p = p.add(1);
                continue;
            }
            _ => break,
        };
        any = true;
        if mant >> 56 == 0 {
            mant = (mant << 4) | d as u64;
            if after_point {
                exp -= 4;
            }
        } else {
            sticky |= d != 0;
            if !after_point {
                exp += 4;
            }
        }
        p = p.add(1);
    }
    if !any {
        return None;
    }
    if (*p | 0x20) == b'p' {
        let mut q = p.add(1);
        let neg = *q == b'-';
        if *q == b'-' || *q == b'+' {
            q = q.add(1);
        }
        if (*q).is_ascii_digit() {
            let mut e: i64 = 0;
            while (*q).is_ascii_digit() {
                e = (e * 10 + (*q - b'0') as i64).min(1 << 20);
                q = q.add(1);
            }
            exp += if neg { -e } else { e };
            p = q;
        }
    }
    if sticky {
        mant |= 1;
    }
    let mut v = mant as f64;
    // Scaled in steps a double can take exactly
    while exp > 0 {
        let k = exp.min(1000);
        v *= f64::from_bits(((1023 + k) as u64) << 52);
        exp -= k;
    }
    while exp < 0 {
        let k = (-exp).min(1000);
        v /= f64::from_bits(((1023 + k) as u64) << 52);
        exp += k;
    }
    if v.is_infinite() || (v == 0.0 && mant != 0) {
        set_errno(ERANGE);
    }
    Some((v, p))
}

#[no_mangle]
pub unsafe extern "C" fn strtod(s: *const u8, end: *mut *mut u8) -> f64 {
    let (v, n) = scan(s, false);
    if !end.is_null() {
        *end = s.add(n) as *mut u8;
    }
    v
}

#[no_mangle]
pub unsafe extern "C" fn strtof(s: *const u8, end: *mut *mut u8) -> f32 {
    let (v, n) = scan(s, true);
    if !end.is_null() {
        *end = s.add(n) as *mut u8;
    }
    v as f32
}

#[no_mangle]
pub unsafe extern "C" fn atof(s: *const u8) -> f64 {
    scan(s, false).0
}
