//! The C `printf` family over a real `va_list`.
//!
//! One formatter, `format`, over `core::ffi::VaList`, writing into an `Out`;
//! each exported function is a sink and a call. Conversions are C99's:
//! `d i u o x X c s p n % f F e E g G a A`, the flags `- + space # 0`, `*`
//! widths and precisions, and the lengths `hh h l ll j z t q`. Floats go
//! through `core::fmt`, which rounds correctly; this file spells the output as
//! C does (`1.5e3` is `1.500000e+03`).
//!
//! Refused: `L`, since a `long double` is 80 bits in memory that `VaList`
//! cannot read, and the wide `%lc`/`%ls`. Either returns -1 with `EINVAL`,
//! printing nothing further.
//!
//! `miku_printf` and others in `fmt.rs` are MikuOS's own API.

use core::ffi::VaList;
use core::fmt::Write;

use crate::libc::{FILE, fwrite, malloc, set_errno};

const EINVAL: i32 = 22;
const EOVERFLOW: i32 = 75;

/// Where formatted bytes go. `put` may be handed any length, including zero
pub trait Out {
    fn put(&mut self, s: &[u8]);
}

/// Counts and keeps nothing, for the first pass when the length is needed first
/// (padding a float, sizing `asprintf`'s buffer)
struct Count(usize);

impl Out for Count {
    fn put(&mut self, s: &[u8]) {
        self.0 += s.len();
    }
}

/// `snprintf`'s buffer: keeps what fits, with room for the NUL
struct Buf {
    ptr: *mut u8,
    room: usize,
    len: usize,
}

impl Out for Buf {
    fn put(&mut self, s: &[u8]) {
        let n = s.len().min(self.room - self.len.min(self.room));
        if n > 0 {
            unsafe { core::ptr::copy_nonoverlapping(s.as_ptr(), self.ptr.add(self.len), n) };
            self.len += n;
        }
    }
}

/// A stdio stream, so `printf` and `puts` interleave in written order
struct Stream(*mut FILE);

impl Out for Stream {
    fn put(&mut self, s: &[u8]) {
        if !s.is_empty() {
            fwrite(s.as_ptr(), 1, s.len(), self.0);
        }
    }
}

/// A bare descriptor, gathered into 256-byte writes
struct Fd {
    fd: i32,
    buf: [u8; 256],
    len: usize,
}

impl Fd {
    fn flush(&mut self) {
        let mut off = 0;
        while off < self.len {
            let r = crate::libc::write(self.fd, unsafe { self.buf.as_ptr().add(off) }, self.len - off);
            if r <= 0 {
                break;
            }
            off += r as usize;
        }
        self.len = 0;
    }
}

impl Out for Fd {
    fn put(&mut self, mut s: &[u8]) {
        while !s.is_empty() {
            if self.len == self.buf.len() {
                self.flush();
            }
            let n = s.len().min(self.buf.len() - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&s[..n]);
            self.len += n;
            s = &s[n..];
        }
    }
}

/// Everything `format` writes passes here, so the total is exact
struct Counted<'a> {
    out: &'a mut dyn Out,
    total: usize,
}

impl Counted<'_> {
    fn put(&mut self, s: &[u8]) {
        self.total += s.len();
        self.out.put(s);
    }

    fn pad(&mut self, c: u8, n: usize) {
        let block = [c; 32];
        let mut left = n;
        while left > 0 {
            let k = left.min(block.len());
            self.put(&block[..k]);
            left -= k;
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Len {
    None,
    Hh,
    H,
    L,
    Ll,
    J,
    Z,
    T,
    BigL,
}

#[derive(Clone, Copy)]
struct Spec {
    minus: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    prec: Option<usize>,
    len: Len,
    conv: u8,
}

/// Formats `fmt` with `ap` into `out`. Returns the byte count or `Err(errno)`.
///
/// # Safety
/// `fmt` is a NUL-terminated string and `ap` holds the arguments it names
pub unsafe fn format(out: &mut dyn Out, fmt: *const u8, ap: &mut VaList) -> Result<usize, i32> {
    let mut w = Counted { out, total: 0 };
    let mut p = fmt;
    loop {
        // The literal run up to the next '%'
        let start = p;
        while *p != 0 && *p != b'%' {
            p = p.add(1);
        }
        if p > start {
            w.put(core::slice::from_raw_parts(start, p.offset_from(start) as usize));
        }
        if *p == 0 {
            break;
        }
        let directive = p;
        p = p.add(1);

        let mut s = Spec {
            minus: false, plus: false, space: false, alt: false, zero: false,
            width: 0, prec: None, len: Len::None, conv: 0,
        };
        loop {
            match *p {
                b'-' => s.minus = true,
                b'+' => s.plus = true,
                b' ' => s.space = true,
                b'#' => s.alt = true,
                b'0' => s.zero = true,
                _ => break,
            }
            p = p.add(1);
        }
        if *p == b'*' {
            let v: i32 = ap.next_arg();
            // A negative width is a '-' flag and its magnitude, per C99
            if v < 0 {
                s.minus = true;
            }
            s.width = v.unsigned_abs() as usize;
            p = p.add(1);
        } else {
            while (*p).is_ascii_digit() {
                s.width = s.width.saturating_mul(10).saturating_add((*p - b'0') as usize);
                p = p.add(1);
            }
        }
        if *p == b'.' {
            p = p.add(1);
            if *p == b'*' {
                let v: i32 = ap.next_arg();
                // A negative precision is as if none were given
                s.prec = if v < 0 { None } else { Some(v as usize) };
                p = p.add(1);
            } else {
                let mut v = 0usize;
                while (*p).is_ascii_digit() {
                    v = v.saturating_mul(10).saturating_add((*p - b'0') as usize);
                    p = p.add(1);
                }
                s.prec = Some(v);
            }
        }
        s.len = match *p {
            b'h' if *p.add(1) == b'h' => { p = p.add(2); Len::Hh }
            b'h' => { p = p.add(1); Len::H }
            b'l' if *p.add(1) == b'l' => { p = p.add(2); Len::Ll }
            b'l' => { p = p.add(1); Len::L }
            b'q' => { p = p.add(1); Len::Ll }
            b'j' => { p = p.add(1); Len::J }
            b'z' => { p = p.add(1); Len::Z }
            b't' => { p = p.add(1); Len::T }
            b'L' => { p = p.add(1); Len::BigL }
            _ => Len::None,
        };
        s.conv = *p;
        if s.conv == 0 {
            // A format ending inside a directive prints what was there
            w.put(core::slice::from_raw_parts(directive, p.offset_from(directive) as usize));
            break;
        }
        p = p.add(1);

        match s.conv {
            b'%' => w.put(b"%"),
            b'd' | b'i' => {
                let v: i64 = match s.len {
                    Len::Hh => ap.next_arg::<i32>() as i8 as i64,
                    Len::H => ap.next_arg::<i32>() as i16 as i64,
                    Len::None => ap.next_arg::<i32>() as i64,
                    Len::BigL => return Err(EINVAL),
                    _ => ap.next_arg::<i64>(),
                };
                let sign: &[u8] = if v < 0 { b"-" } else if s.plus { b"+" } else if s.space { b" " } else { b"" };
                int(&mut w, &s, sign, b"", v.unsigned_abs(), 10, false);
            }
            b'u' | b'o' | b'x' | b'X' => {
                let v: u64 = match s.len {
                    Len::Hh => ap.next_arg::<u32>() as u8 as u64,
                    Len::H => ap.next_arg::<u32>() as u16 as u64,
                    Len::None => ap.next_arg::<u32>() as u64,
                    Len::BigL => return Err(EINVAL),
                    _ => ap.next_arg::<u64>(),
                };
                let (base, upper) = match s.conv {
                    b'u' => (10, false),
                    b'o' => (8, false),
                    b'x' => (16, false),
                    _ => (16, true),
                };
                let prefix: &[u8] = match (s.alt && v != 0, s.conv) {
                    (true, b'x') => b"0x",
                    (true, b'X') => b"0X",
                    _ => b"",
                };
                int(&mut w, &s, b"", prefix, v, base, upper);
            }
            b'p' => {
                let v = ap.next_arg::<*const u8>() as u64;
                if v == 0 {
                    let q = Spec { prec: None, zero: false, ..s };
                    text(&mut w, &q, b"(nil)");
                } else {
                    let q = Spec { alt: false, ..s };
                    int(&mut w, &q, b"", b"0x", v, 16, false);
                }
            }
            b'c' => {
                if s.len == Len::L {
                    return Err(EINVAL);
                }
                let c = ap.next_arg::<i32>() as u8;
                let q = Spec { prec: None, ..s };
                text(&mut w, &q, &[c]);
            }
            b's' => {
                if s.len == Len::L {
                    return Err(EINVAL);
                }
                let ptr = ap.next_arg::<*const u8>();
                let bytes: &[u8] = if ptr.is_null() {
                    b"(null)"
                } else {
                    // Never reads past the precision: `%.3s` allows an array
                    // with no NUL
                    let max = s.prec.unwrap_or(usize::MAX);
                    let mut n = 0;
                    while n < max && *ptr.add(n) != 0 {
                        n += 1;
                    }
                    core::slice::from_raw_parts(ptr, n)
                };
                let q = Spec { prec: None, ..s };
                text(&mut w, &q, bytes);
            }
            b'n' => {
                let t = w.total;
                match s.len {
                    Len::Hh => *ap.next_arg::<*mut i8>() = t as i8,
                    Len::H => *ap.next_arg::<*mut i16>() = t as i16,
                    Len::None => *ap.next_arg::<*mut i32>() = t as i32,
                    _ => *ap.next_arg::<*mut i64>() = t as i64,
                }
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' | b'a' | b'A' => {
                if s.len == Len::BigL {
                    return Err(EINVAL);
                }
                let v: f64 = ap.next_arg();
                float(&mut w, &s, v);
            }
            _ => {
                // Unknown: printed as written, as glibc does
                w.put(core::slice::from_raw_parts(directive, p.offset_from(directive) as usize));
            }
        }
    }
    Ok(w.total)
}

/// A field: `sign`, `prefix`, then `body`, padded to the width
fn field(w: &mut Counted, s: &Spec, sign: &[u8], prefix: &[u8], zeros: usize, body: &[u8], zero_pad: bool) {
    let len = sign.len() + prefix.len() + zeros + body.len();
    let pad = s.width.saturating_sub(len);
    if !s.minus && !zero_pad {
        w.pad(b' ', pad);
    }
    w.put(sign);
    w.put(prefix);
    if !s.minus && zero_pad {
        w.pad(b'0', pad);
    }
    w.pad(b'0', zeros);
    w.put(body);
    if s.minus {
        w.pad(b' ', pad);
    }
}

fn text(w: &mut Counted, s: &Spec, body: &[u8]) {
    field(w, s, b"", b"", 0, body, false);
}

fn int(w: &mut Counted, s: &Spec, sign: &[u8], prefix: &[u8], v: u64, base: u64, upper: bool) {
    let digits: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let mut buf = [0u8; 24];
    let mut i = buf.len();
    let mut x = v;
    // `%.0d` of zero is no digits at all
    if !(v == 0 && s.prec == Some(0)) {
        loop {
            i -= 1;
            buf[i] = digits[(x % base) as usize];
            x /= base;
            if x == 0 {
                break;
            }
        }
    }
    let body = &buf[i..];
    let mut zeros = s.prec.unwrap_or(0).saturating_sub(body.len());
    // `%#o` gives a leading zero, adding one only if absent
    if base == 8 && s.alt && zeros == 0 && body.first() != Some(&b'0') {
        zeros = 1;
    }
    // A precision turns the '0' flag off for integers
    field(w, s, sign, prefix, zeros, body, s.zero && s.prec.is_none());
}

/// `core::fmt::Write` onto an `Out`, for what `core` formats
struct Via<'a>(&'a mut dyn Out);

impl Write for Via<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.0.put(s.as_bytes());
        Ok(())
    }
}

/// `core`'s `{:e}` (`1.5e3`) in C's form (`1.5e+03`), with `#` keeping the
/// point when there is no fraction
struct Exp<'a> {
    out: &'a mut dyn Out,
    upper: bool,
    alt_point: bool,
    in_exp: bool,
    exp: [u8; 8],
    exp_len: usize,
}

impl Write for Exp<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for &b in s.as_bytes() {
            if self.in_exp {
                if self.exp_len < self.exp.len() {
                    self.exp[self.exp_len] = b;
                    self.exp_len += 1;
                }
            } else if b == b'e' {
                self.in_exp = true;
            } else {
                self.out.put(&[b]);
            }
        }
        Ok(())
    }
}

impl Exp<'_> {
    fn finish(self) {
        if self.alt_point {
            self.out.put(b".");
        }
        self.out.put(if self.upper { b"E" } else { b"e" });
        let e = &self.exp[..self.exp_len];
        let (sign, digits): (&[u8], &[u8]) = match e.first() {
            Some(b'-') => (b"-", &e[1..]),
            _ => (b"+", e),
        };
        self.out.put(sign);
        if digits.len() < 2 {
            self.out.put(b"0");
        }
        self.out.put(digits);
    }
}

fn fixed(out: &mut dyn Out, v: f64, prec: usize, alt: bool) {
    let _ = write!(Via(out), "{:.*}", prec, v);
    if alt && prec == 0 {
        out.put(b".");
    }
}

fn sci(out: &mut dyn Out, v: f64, prec: usize, alt: bool, upper: bool) {
    let mut e = Exp { out, upper, alt_point: alt && prec == 0, in_exp: false, exp: [0; 8], exp_len: 0 };
    let _ = write!(e, "{:.*e}", prec, v);
    e.finish();
}

/// Decimal exponent `%e` would print for `v` at `prec`, after rounding, as
/// `%g`'s choice requires
fn exponent_of(v: f64, prec: usize) -> i32 {
    if v == 0.0 {
        return 0;
    }
    struct Scan {
        seen: bool,
        neg: bool,
        e: i32,
    }
    impl Write for Scan {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            for b in s.bytes() {
                if self.seen {
                    if b == b'-' {
                        self.neg = true;
                    } else {
                        self.e = self.e * 10 + (b - b'0') as i32;
                    }
                } else if b == b'e' {
                    self.seen = true;
                }
            }
            Ok(())
        }
    }
    let mut sc = Scan { seen: false, neg: false, e: 0 };
    let _ = write!(sc, "{:.*e}", prec, v);
    if sc.neg { -sc.e } else { sc.e }
}

/// `%g`: `%e` or `%f` by the exponent, then trailing zeros dropped unless `#`
fn general(out: &mut dyn Out, v: f64, prec: Option<usize>, alt: bool, upper: bool) {
    let p = match prec {
        None => 6,
        Some(0) => 1,
        // No more significant digits exist in an f64 past this, and it bounds
        // the buffer below
        Some(p) => p.min(330),
    };
    let x = exponent_of(v, p - 1);
    let mut buf = [0u8; 400];
    let mut b = Buf { ptr: buf.as_mut_ptr(), room: buf.len(), len: 0 };
    let use_exp = !(x < p as i32 && x >= -4);
    if use_exp {
        sci(&mut b, v, p - 1, alt, upper);
    } else {
        fixed(&mut b, v, (p as i32 - 1 - x) as usize, alt);
    }
    let s = &buf[..b.len];
    if alt {
        out.put(s);
        return;
    }
    // Strips the fraction's trailing zeros, and the point if nothing is left
    let (mant, rest) = match s.iter().position(|&c| c == b'e' || c == b'E') {
        Some(i) => s.split_at(i),
        None => (s, &[][..]),
    };
    let mant = if mant.contains(&b'.') {
        let mut end = mant.len();
        while end > 0 && mant[end - 1] == b'0' {
            end -= 1;
        }
        if end > 0 && mant[end - 1] == b'.' {
            end -= 1;
        }
        &mant[..end]
    } else {
        mant
    };
    out.put(mant);
    out.put(rest);
}

/// `%a`: `0x1.8p+1`. Rounds half up when a precision cuts the fraction
fn hex(out: &mut dyn Out, v: f64, prec: Option<usize>, upper: bool) {
    let digits: &[u8; 16] = if upper { b"0123456789ABCDEF" } else { b"0123456789abcdef" };
    let bits = v.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1u64 << 52) - 1);
    let (mut lead, exp) = if raw_exp == 0 {
        if mant == 0 { (0u64, 0) } else { (0u64, -1022) }
    } else {
        (1u64, raw_exp - 1023)
    };
    let mut nib = 13usize;
    if let Some(p) = prec {
        if p < 13 {
            let drop = (13 - p) * 4;
            let half = 1u64 << (drop - 1);
            let rounded = mant + half;
            mant = rounded >> drop;
            if mant >> (p * 4) != 0 {
                mant &= (1u64 << (p * 4)) - 1;
                lead += 1;
            }
            nib = p;
        }
    } else {
        while nib > 0 && (mant & 0xf) == 0 {
            mant >>= 4;
            nib -= 1;
        }
    }
    out.put(if upper { b"0X" } else { b"0x" });
    out.put(&[digits[lead as usize]]);
    let shown = prec.unwrap_or(nib);
    if shown > 0 {
        out.put(b".");
        for i in (0..nib).rev() {
            out.put(&[digits[((mant >> (i * 4)) & 0xf) as usize]]);
        }
        for _ in nib..shown {
            out.put(b"0");
        }
    }
    let _ = write!(Via(out), "{}{:+}", if upper { "P" } else { "p" }, exp);
}

fn float(w: &mut Counted, s: &Spec, v: f64) {
    let upper = s.conv.is_ascii_uppercase();
    let sign: &[u8] = if v.is_sign_negative() && !v.is_nan() {
        b"-"
    } else if s.plus {
        b"+"
    } else if s.space {
        b" "
    } else {
        b""
    };
    if !v.is_finite() {
        let body: &[u8] = match (v.is_nan(), upper) {
            (true, false) => b"nan",
            (true, true) => b"NAN",
            (false, false) => b"inf",
            (false, true) => b"INF",
        };
        field(w, s, sign, b"", 0, body, false);
        return;
    }
    let a = v.abs();
    let render = |out: &mut dyn Out| match s.conv {
        b'f' | b'F' => fixed(out, a, s.prec.unwrap_or(6), s.alt),
        b'e' | b'E' => sci(out, a, s.prec.unwrap_or(6), s.alt, upper),
        b'g' | b'G' => general(out, a, s.prec, s.alt, upper),
        _ => hex(out, a, s.prec, upper),
    };
    let mut c = Count(0);
    render(&mut c);
    let len = sign.len() + c.0;
    let pad = s.width.saturating_sub(len);
    if !s.minus && !s.zero {
        w.pad(b' ', pad);
    }
    w.put(sign);
    if !s.minus && s.zero {
        w.pad(b'0', pad);
    }
    render(&mut CountedOut(w));
    if s.minus {
        w.pad(b' ', pad);
    }
}

/// A `Counted` seen as an `Out`, so float renderers count into the total
struct CountedOut<'a, 'b>(&'a mut Counted<'b>);

impl Out for CountedOut<'_, '_> {
    fn put(&mut self, s: &[u8]) {
        self.0.put(s);
    }
}

fn finish(r: Result<usize, i32>) -> i32 {
    match r {
        Ok(n) if n <= i32::MAX as usize => n as i32,
        Ok(_) => {
            set_errno(EOVERFLOW);
            -1
        }
        Err(e) => {
            set_errno(e);
            -1
        }
    }
}

unsafe fn to_buf(buf: *mut u8, size: usize, fmt: *const u8, ap: &mut VaList) -> i32 {
    let mut b = Buf { ptr: buf, room: size.saturating_sub(1), len: 0 };
    if buf.is_null() {
        b.room = 0;
    }
    let r = format(&mut b, fmt, ap);
    if !buf.is_null() && size > 0 {
        *buf.add(b.len) = 0;
    }
    finish(r)
}

unsafe fn to_fd(fd: i32, fmt: *const u8, ap: &mut VaList) -> i32 {
    let mut o = Fd { fd, buf: [0; 256], len: 0 };
    let r = format(&mut o, fmt, ap);
    o.flush();
    finish(r)
}

unsafe fn to_stream(f: *mut FILE, fmt: *const u8, ap: &mut VaList) -> i32 {
    finish(format(&mut Stream(f), fmt, ap))
}

unsafe fn to_new(out: *mut *mut u8, fmt: *const u8, ap: &mut VaList) -> i32 {
    let mut counting = ap.clone();
    let n = match format(&mut Count(0), fmt, &mut counting) {
        Ok(n) => n,
        Err(e) => return finish(Err(e)),
    };
    let p = malloc(n + 1);
    if p.is_null() {
        set_errno(12);
        return -1;
    }
    let r = to_buf(p, n + 1, fmt, ap);
    *out = p;
    r
}

#[no_mangle]
pub unsafe extern "C" fn vsnprintf(buf: *mut u8, size: usize, fmt: *const u8, mut ap: VaList) -> i32 {
    to_buf(buf, size, fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn snprintf(buf: *mut u8, size: usize, fmt: *const u8, mut args: ...) -> i32 {
    to_buf(buf, size, fmt, &mut args)
}

#[no_mangle]
pub unsafe extern "C" fn vsprintf(buf: *mut u8, fmt: *const u8, mut ap: VaList) -> i32 {
    to_buf(buf, usize::MAX, fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn sprintf(buf: *mut u8, fmt: *const u8, mut args: ...) -> i32 {
    to_buf(buf, usize::MAX, fmt, &mut args)
}

#[no_mangle]
pub unsafe extern "C" fn vdprintf(fd: i32, fmt: *const u8, mut ap: VaList) -> i32 {
    to_fd(fd, fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn dprintf(fd: i32, fmt: *const u8, mut args: ...) -> i32 {
    to_fd(fd, fmt, &mut args)
}

#[no_mangle]
pub unsafe extern "C" fn vfprintf(f: *mut FILE, fmt: *const u8, mut ap: VaList) -> i32 {
    to_stream(f, fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn fprintf(f: *mut FILE, fmt: *const u8, mut args: ...) -> i32 {
    to_stream(f, fmt, &mut args)
}

#[no_mangle]
pub unsafe extern "C" fn vprintf(fmt: *const u8, mut ap: VaList) -> i32 {
    to_stream(crate::libc::__miku_stdfile(1), fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn printf(fmt: *const u8, mut args: ...) -> i32 {
    to_stream(crate::libc::__miku_stdfile(1), fmt, &mut args)
}

#[no_mangle]
pub unsafe extern "C" fn vasprintf(out: *mut *mut u8, fmt: *const u8, mut ap: VaList) -> i32 {
    to_new(out, fmt, &mut ap)
}

#[no_mangle]
pub unsafe extern "C" fn asprintf(out: *mut *mut u8, fmt: *const u8, mut args: ...) -> i32 {
    to_new(out, fmt, &mut args)
}
