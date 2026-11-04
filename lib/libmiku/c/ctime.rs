//! `<time.h>` and `gettimeofday`: calendar time for C.
//!
//! Local time is UTC. There is no zone database or `TZ`, so `localtime_r` is
//! `gmtime_r`, `mktime` is `timegm`, and `%Z` prints "UTC".
//!
//! Calendar arithmetic is Howard Hinnant's `civil_from_days` and
//! `days_from_civil`, exact over the proleptic Gregorian range.

use crate::libc::{clock_gettime, fail, set_errno, timespec};

const EINVAL: i32 = 22;
const EOVERFLOW: i32 = 75;
const CLOCK_REALTIME: i32 = 0;
const CLOCK_MONOTONIC: i32 = 1;
const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Tm {
    pub tm_sec: i32,
    pub tm_min: i32,
    pub tm_hour: i32,
    pub tm_mday: i32,
    pub tm_mon: i32,
    pub tm_year: i32,
    pub tm_wday: i32,
    pub tm_yday: i32,
    pub tm_isdst: i32,
}

#[repr(C)]
pub struct Timeval {
    pub tv_sec: i64,
    pub tv_usec: i64,
}

fn now(clock: i32) -> Option<timespec> {
    let mut ts = timespec { tv_sec: 0, tv_nsec: 0 };
    if clock_gettime(clock, &mut ts) == 0 { Some(ts) } else { None }
}

#[no_mangle]
pub unsafe extern "C" fn time(out: *mut i64) -> i64 {
    let t = match now(CLOCK_REALTIME) {
        Some(ts) => ts.tv_sec,
        None => return -1,
    };
    if !out.is_null() {
        *out = t;
    }
    t
}

#[no_mangle]
pub unsafe extern "C" fn gettimeofday(tv: *mut Timeval, _tz: *mut u8) -> i32 {
    let ts = match now(CLOCK_REALTIME) {
        Some(ts) => ts,
        None => return -1,
    };
    if !tv.is_null() {
        (*tv).tv_sec = ts.tv_sec;
        (*tv).tv_usec = ts.tv_nsec / 1000;
    }
    0
}

/// Processor time in microseconds (`CLOCKS_PER_SEC` is a million): the process
/// clock if kept, else time since boot, an upper bound
#[no_mangle]
pub extern "C" fn clock() -> i64 {
    match now(CLOCK_PROCESS_CPUTIME_ID).or_else(|| now(CLOCK_MONOTONIC)) {
        Some(ts) => ts.tv_sec * 1_000_000 + ts.tv_nsec / 1000,
        None => -1,
    }
}

#[no_mangle]
pub extern "C" fn difftime(a: i64, b: i64) -> f64 {
    a as f64 - b as f64
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn broken_down(t: i64) -> Option<Tm> {
    let days = t.div_euclid(86400);
    let secs = t.rem_euclid(86400);
    let (y, m, d) = civil_from_days(days);
    let year = i32::try_from(y - 1900).ok()?;
    let yday = days - days_from_civil(y, 1, 1);
    Some(Tm {
        tm_sec: (secs % 60) as i32,
        tm_min: (secs / 60 % 60) as i32,
        tm_hour: (secs / 3600) as i32,
        tm_mday: d as i32,
        tm_mon: (m - 1) as i32,
        tm_year: year,
        // 1970-01-01 was a Thursday
        tm_wday: (days + 4).rem_euclid(7) as i32,
        tm_yday: yday as i32,
        tm_isdst: 0,
    })
}

#[no_mangle]
pub unsafe extern "C" fn gmtime_r(t: *const i64, out: *mut Tm) -> *mut Tm {
    if t.is_null() || out.is_null() {
        set_errno(EINVAL);
        return core::ptr::null_mut();
    }
    match broken_down(*t) {
        Some(tm) => {
            *out = tm;
            out
        }
        None => {
            set_errno(EOVERFLOW);
            core::ptr::null_mut()
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn localtime_r(t: *const i64, out: *mut Tm) -> *mut Tm {
    gmtime_r(t, out)
}

/// The shared result of `gmtime` and `localtime`, as C specifies; threaded code
/// uses the `_r` forms
static mut SHARED_TM: Tm = Tm {
    tm_sec: 0, tm_min: 0, tm_hour: 0, tm_mday: 0, tm_mon: 0,
    tm_year: 0, tm_wday: 0, tm_yday: 0, tm_isdst: 0,
};

#[no_mangle]
pub unsafe extern "C" fn gmtime(t: *const i64) -> *mut Tm {
    gmtime_r(t, &raw mut SHARED_TM)
}

#[no_mangle]
pub unsafe extern "C" fn localtime(t: *const i64) -> *mut Tm {
    gmtime_r(t, &raw mut SHARED_TM)
}

/// `timegm`: normalizes fields as `mktime` must (month 13 is January of the
/// next year, second 75 is a minute and fifteen) and writes them back
#[no_mangle]
pub unsafe extern "C" fn timegm(tm: *mut Tm) -> i64 {
    if tm.is_null() {
        fail(EINVAL);
        return -1;
    }
    let t = &*tm;
    let mon = t.tm_mon as i64;
    let y = 1900 + t.tm_year as i64 + mon.div_euclid(12);
    let m = mon.rem_euclid(12) + 1;
    let days = days_from_civil(y, m, 1) + t.tm_mday as i64 - 1;
    let secs = days * 86400 + t.tm_hour as i64 * 3600 + t.tm_min as i64 * 60 + t.tm_sec as i64;
    match broken_down(secs) {
        Some(n) => {
            *tm = n;
            secs
        }
        None => {
            fail(EOVERFLOW);
            -1
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn mktime(tm: *mut Tm) -> i64 {
    timegm(tm)
}

const DAYS: [&[u8]; 7] = [b"Sunday", b"Monday", b"Tuesday", b"Wednesday", b"Thursday", b"Friday", b"Saturday"];
const MONTHS: [&[u8]; 12] = [
    b"January", b"February", b"March", b"April", b"May", b"June",
    b"July", b"August", b"September", b"October", b"November", b"December",
];

struct Out {
    ptr: *mut u8,
    cap: usize,
    len: usize,
    full: bool,
}

impl Out {
    fn put(&mut self, s: &[u8]) {
        for &b in s {
            // One byte is always kept for the NUL
            if self.len + 1 >= self.cap {
                self.full = true;
                return;
            }
            unsafe { *self.ptr.add(self.len) = b };
            self.len += 1;
        }
    }

    fn num(&mut self, v: i64, width: usize, pad: u8) {
        let mut buf = [0u8; 24];
        let mut i = buf.len();
        let neg = v < 0;
        let mut x = v.unsigned_abs();
        loop {
            i -= 1;
            buf[i] = b'0' + (x % 10) as u8;
            x /= 10;
            if x == 0 {
                break;
            }
        }
        if neg {
            self.put(b"-");
        }
        let digits = buf.len() - i;
        for _ in digits..width {
            self.put(&[pad]);
        }
        self.put(&buf[i..]);
    }
}

fn name(list: &[&'static [u8]], i: i32) -> &'static [u8] {
    list.get(i as usize).copied().unwrap_or(b"?")
}

unsafe fn strf(o: &mut Out, fmt: *const u8, tm: &Tm) {
    let mut p = fmt;
    while *p != 0 {
        if *p != b'%' {
            o.put(&[*p]);
            p = p.add(1);
            continue;
        }
        p = p.add(1);
        // E and O name alternative representations; the "C" locale has none, so
        // they are read and ignored
        if *p == b'E' || *p == b'O' {
            p = p.add(1);
        }
        let year = tm.tm_year as i64 + 1900;
        match *p {
            b'a' => o.put(&name(&DAYS, tm.tm_wday)[..3]),
            b'A' => o.put(name(&DAYS, tm.tm_wday)),
            b'b' | b'h' => o.put(&name(&MONTHS, tm.tm_mon)[..3]),
            b'B' => o.put(name(&MONTHS, tm.tm_mon)),
            b'c' => strf(o, b"%a %b %e %H:%M:%S %Y\0".as_ptr(), tm),
            b'C' => o.num(year.div_euclid(100), 2, b'0'),
            b'd' => o.num(tm.tm_mday as i64, 2, b'0'),
            b'D' => strf(o, b"%m/%d/%y\0".as_ptr(), tm),
            b'e' => o.num(tm.tm_mday as i64, 2, b' '),
            b'F' => strf(o, b"%Y-%m-%d\0".as_ptr(), tm),
            b'H' => o.num(tm.tm_hour as i64, 2, b'0'),
            b'I' => o.num(((tm.tm_hour + 11) % 12 + 1) as i64, 2, b'0'),
            b'j' => o.num(tm.tm_yday as i64 + 1, 3, b'0'),
            b'm' => o.num(tm.tm_mon as i64 + 1, 2, b'0'),
            b'M' => o.num(tm.tm_min as i64, 2, b'0'),
            b'n' => o.put(b"\n"),
            b'p' => o.put(if tm.tm_hour < 12 { b"AM" } else { b"PM" }),
            b'r' => strf(o, b"%I:%M:%S %p\0".as_ptr(), tm),
            b'R' => strf(o, b"%H:%M\0".as_ptr(), tm),
            b's' => {
                let mut t = *tm;
                o.num(timegm(&mut t), 0, b'0');
            }
            b'S' => o.num(tm.tm_sec as i64, 2, b'0'),
            b't' => o.put(b"\t"),
            b'T' => strf(o, b"%H:%M:%S\0".as_ptr(), tm),
            b'u' => o.num(if tm.tm_wday == 0 { 7 } else { tm.tm_wday as i64 }, 1, b'0'),
            b'w' => o.num(tm.tm_wday as i64, 1, b'0'),
            b'x' => strf(o, b"%m/%d/%y\0".as_ptr(), tm),
            b'X' => strf(o, b"%H:%M:%S\0".as_ptr(), tm),
            b'y' => o.num(year.rem_euclid(100), 2, b'0'),
            b'Y' => o.num(year, 1, b'0'),
            b'z' => o.put(b"+0000"),
            b'Z' => o.put(b"UTC"),
            b'%' => o.put(b"%"),
            0 => return,
            c => o.put(&[b'%', c]),
        }
        p = p.add(1);
    }
}

/// Length written, or 0 if it did not fit, per C
#[no_mangle]
pub unsafe extern "C" fn strftime(buf: *mut u8, max: usize, fmt: *const u8, tm: *const Tm) -> usize {
    if buf.is_null() || max == 0 || fmt.is_null() || tm.is_null() {
        return 0;
    }
    let mut o = Out { ptr: buf, cap: max, len: 0, full: false };
    strf(&mut o, fmt, &*tm);
    if o.full {
        *buf = 0;
        return 0;
    }
    *buf.add(o.len) = 0;
    o.len
}
