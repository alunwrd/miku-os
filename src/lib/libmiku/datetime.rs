// Converts timestamps to human-readable date/time strings
// No heap allocation - all formatting into caller buffers

use crate::time;
use crate::num;
use crate::mem;

// date/time components
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuDateTime {
    pub year: i32,
    pub month: u8,    // 1-12
    pub day: u8,      // 1-31
    pub hour: u8,     // 0-23
    pub minute: u8,   // 0-59
    pub second: u8,   // 0-59
    pub weekday: u8,  // 0=Sunday, 6=Saturday
    pub yearday: u16, // 0-365
}

const EMPTY_DT: MikuDateTime = MikuDateTime {
    year: 1970, month: 1, day: 1,
    hour: 0, minute: 0, second: 0,
    weekday: 4, yearday: 0,
};

// days per month (non-leap)
static DAYS_IN_MONTH: [u8; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];

fn is_leap(y: i32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

fn days_in_month(m: u8, y: i32) -> u8 {
    if m == 2 && is_leap(y) { 29 } else { DAYS_IN_MONTH[(m - 1) as usize] }
}

// convert timestamp to datetime
#[no_mangle]
pub extern "C" fn miku_dt_from_timestamp(ts: i64) -> MikuDateTime {
    let mut dt = EMPTY_DT;

    let secs = if ts >= 0 { ts } else { 0 } as u64;
    let days = (secs / 86400) as i64;
    let day_secs = (secs % 86400) as u32;

    dt.hour = (day_secs / 3600) as u8;
    dt.minute = ((day_secs % 3600) / 60) as u8;
    dt.second = (day_secs % 60) as u8;

    // weekday: Jan 1 1970 was Thursday (4)
    dt.weekday = ((days % 7 + 4) % 7) as u8;

    // year/month/day from days since epoch
    let mut y = 1970i32;
    let mut remaining = days;

    loop {
        let year_days = if is_leap(y) { 366i64 } else { 365i64 };
        if remaining < year_days { break; }
        remaining -= year_days;
        y += 1;
    }

    dt.year = y;
    dt.yearday = remaining as u16;

    let mut m = 1u8;
    while m <= 12 {
        let md = days_in_month(m, y) as i64;
        if remaining < md { break; }
        remaining -= md;
        m += 1;
    }

    dt.month = m;
    dt.day = remaining as u8 + 1;

    dt
}

// convert datetime to Unix timestamp
#[no_mangle]
pub extern "C" fn miku_dt_to_timestamp(dt: *const MikuDateTime) -> i64 {
    if dt.is_null() { return 0; }
    let dt = unsafe { &*dt };

    let mut days = 0i64;
    // years
    let mut y = 1970i32;
    while y < dt.year {
        days += if is_leap(y) { 366 } else { 365 };
        y += 1;
    }
    // months
    let mut m = 1u8;
    while m < dt.month {
        days += days_in_month(m, dt.year) as i64;
        m += 1;
    }
    // days
    days += (dt.day as i64) - 1;

    days * 86400
        + dt.hour as i64 * 3600
        + dt.minute as i64 * 60
        + dt.second as i64
}

// get current datetime
#[no_mangle]
pub extern "C" fn miku_dt_now() -> MikuDateTime {
    let ts = time::miku_uptime_ms() / 1000;
    miku_dt_from_timestamp(ts as i64)
}

// format datetime to string
// Format: "YYYY-MM-DD HH:MM:SS"
// Returns bytes written
#[no_mangle]
pub extern "C" fn miku_dt_format(
    dt: *const MikuDateTime,
    buf: *mut u8,
    buf_len: usize,
) -> usize {
    if dt.is_null() || buf.is_null() || buf_len < 20 { return 0; }
    let dt = unsafe { &*dt };

    let mut pos = 0usize;

    // year
    pos += write_padded_4(unsafe { buf.add(pos) }, dt.year as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;

    // month
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.month as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;

    // day
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.day as u32);
    unsafe { *buf.add(pos) = b' '; }
    pos += 1;

    // hour
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.hour as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;

    // minute
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.minute as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;

    // second
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.second as u32);
    unsafe { *buf.add(pos) = 0; }

    pos
}

// format date only: "YYYY-MM-DD"
#[no_mangle]
pub extern "C" fn miku_dt_format_date(
    dt: *const MikuDateTime,
    buf: *mut u8,
    buf_len: usize,
) -> usize {
    if dt.is_null() || buf.is_null() || buf_len < 11 { return 0; }
    let dt = unsafe { &*dt };

    let mut pos = 0usize;
    pos += write_padded_4(unsafe { buf.add(pos) }, dt.year as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.month as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.day as u32);
    unsafe { *buf.add(pos) = 0; }
    pos
}

// format time only: "HH:MM:SS"
#[no_mangle]
pub extern "C" fn miku_dt_format_time(
    dt: *const MikuDateTime,
    buf: *mut u8,
    buf_len: usize,
) -> usize {
    if dt.is_null() || buf.is_null() || buf_len < 9 { return 0; }
    let dt = unsafe { &*dt };

    let mut pos = 0usize;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.hour as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.minute as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.second as u32);
    unsafe { *buf.add(pos) = 0; }
    pos
}

// format ISO 8601: "YYYY-MM-DDTHH:MM:SSZ"
#[no_mangle]
pub extern "C" fn miku_dt_format_iso(
    dt: *const MikuDateTime,
    buf: *mut u8,
    buf_len: usize,
) -> usize {
    if dt.is_null() || buf.is_null() || buf_len < 21 { return 0; }
    let dt = unsafe { &*dt };

    let mut pos = 0usize;
    pos += write_padded_4(unsafe { buf.add(pos) }, dt.year as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.month as u32);
    unsafe { *buf.add(pos) = b'-'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.day as u32);
    unsafe { *buf.add(pos) = b'T'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.hour as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.minute as u32);
    unsafe { *buf.add(pos) = b':'; }
    pos += 1;
    pos += write_padded_2(unsafe { buf.add(pos) }, dt.second as u32);
    unsafe { *buf.add(pos) = b'Z'; }
    pos += 1;
    unsafe { *buf.add(pos) = 0; }
    pos
}

// day of week name
#[no_mangle]
pub extern "C" fn miku_dt_weekday_name(day: u8) -> *const u8 {
    match day {
        0 => b"Sunday\0".as_ptr(),
        1 => b"Monday\0".as_ptr(),
        2 => b"Tuesday\0".as_ptr(),
        3 => b"Wednesday\0".as_ptr(),
        4 => b"Thursday\0".as_ptr(),
        5 => b"Friday\0".as_ptr(),
        6 => b"Saturday\0".as_ptr(),
        _ => b"Unknown\0".as_ptr(),
    }
}

// month name
#[no_mangle]
pub extern "C" fn miku_dt_month_name(month: u8) -> *const u8 {
    match month {
        1 => b"January\0".as_ptr(),
        2 => b"February\0".as_ptr(),
        3 => b"March\0".as_ptr(),
        4 => b"April\0".as_ptr(),
        5 => b"May\0".as_ptr(),
        6 => b"June\0".as_ptr(),
        7 => b"July\0".as_ptr(),
        8 => b"August\0".as_ptr(),
        9 => b"September\0".as_ptr(),
        10 => b"October\0".as_ptr(),
        11 => b"November\0".as_ptr(),
        12 => b"December\0".as_ptr(),
        _ => b"Unknown\0".as_ptr(),
    }
}

// difference in seconds between two datetimes
#[no_mangle]
pub extern "C" fn miku_dt_diff_secs(a: *const MikuDateTime, b: *const MikuDateTime) -> i64 {
    miku_dt_to_timestamp(a) - miku_dt_to_timestamp(b)
}

// add seconds to datetime
#[no_mangle]
pub extern "C" fn miku_dt_add_secs(dt: *const MikuDateTime, secs: i64) -> MikuDateTime {
    if dt.is_null() { return EMPTY_DT; }
    let ts = miku_dt_to_timestamp(dt) + secs;
    miku_dt_from_timestamp(ts)
}

// add days to datetime
#[no_mangle]
pub extern "C" fn miku_dt_add_days(dt: *const MikuDateTime, days: i32) -> MikuDateTime {
    miku_dt_add_secs(dt, days as i64 * 86400)
}

// helpers for zero-padded numbers
fn write_padded_2(buf: *mut u8, val: u32) -> usize {
    let v = val % 100;
    unsafe {
        *buf = b'0' + (v / 10) as u8;
        *buf.add(1) = b'0' + (v % 10) as u8;
    }
    2
}

fn write_padded_4(buf: *mut u8, val: u32) -> usize {
    let v = val % 10000;
    unsafe {
        *buf = b'0' + (v / 1000) as u8;
        *buf.add(1) = b'0' + ((v / 100) % 10) as u8;
        *buf.add(2) = b'0' + ((v / 10) % 10) as u8;
        *buf.add(3) = b'0' + (v % 10) as u8;
    }
    4
}

// validate datetime fields
#[no_mangle]
pub extern "C" fn miku_dt_valid(dt: *const MikuDateTime) -> bool {
    if dt.is_null() { return false; }
    let dt = unsafe { &*dt };
    dt.month >= 1 && dt.month <= 12
        && dt.day >= 1 && dt.day <= days_in_month(dt.month, dt.year)
        && dt.hour <= 23
        && dt.minute <= 59
        && dt.second <= 59
}

// check if year is leap
#[no_mangle]
pub extern "C" fn miku_dt_is_leap_year(year: i32) -> bool {
    is_leap(year)
}

// days in given month (1-12) for given year
#[no_mangle]
pub extern "C" fn miku_dt_days_in_month(month: u8, year: i32) -> u8 {
    if month < 1 || month > 12 { return 0; }
    days_in_month(month, year)
}

// days in given year
#[no_mangle]
pub extern "C" fn miku_dt_days_in_year(year: i32) -> u16 {
    if is_leap(year) { 366 } else { 365 }
}

// short weekday name (3 chars)
#[no_mangle]
pub extern "C" fn miku_dt_weekday_short(day: u8) -> *const u8 {
    match day {
        0 => b"Sun\0".as_ptr(),
        1 => b"Mon\0".as_ptr(),
        2 => b"Tue\0".as_ptr(),
        3 => b"Wed\0".as_ptr(),
        4 => b"Thu\0".as_ptr(),
        5 => b"Fri\0".as_ptr(),
        6 => b"Sat\0".as_ptr(),
        _ => b"???\0".as_ptr(),
    }
}

// short month name (3 chars)
#[no_mangle]
pub extern "C" fn miku_dt_month_short(month: u8) -> *const u8 {
    match month {
        1 => b"Jan\0".as_ptr(),
        2 => b"Feb\0".as_ptr(),
        3 => b"Mar\0".as_ptr(),
        4 => b"Apr\0".as_ptr(),
        5 => b"May\0".as_ptr(),
        6 => b"Jun\0".as_ptr(),
        7 => b"Jul\0".as_ptr(),
        8 => b"Aug\0".as_ptr(),
        9 => b"Sep\0".as_ptr(),
        10 => b"Oct\0".as_ptr(),
        11 => b"Nov\0".as_ptr(),
        12 => b"Dec\0".as_ptr(),
        _ => b"???\0".as_ptr(),
    }
}

// compare two datetimes: returns -1, 0, 1
#[no_mangle]
pub extern "C" fn miku_dt_cmp(a: *const MikuDateTime, b: *const MikuDateTime) -> i32 {
    let ta = miku_dt_to_timestamp(a);
    let tb = miku_dt_to_timestamp(b);
    if ta < tb { -1 } else if ta > tb { 1 } else { 0 }
}

// format RFC 2822: "Thu, 01 Jan 1970 00:00:00 +0000"
#[no_mangle]
pub extern "C" fn miku_dt_format_rfc2822(
    dt: *const MikuDateTime,
    buf: *mut u8,
    buf_len: usize,
) -> usize {
    if dt.is_null() || buf.is_null() || buf_len < 32 { return 0; }
    let dt = unsafe { &*dt };
    let wday = miku_dt_weekday_short(dt.weekday);
    let mon = miku_dt_month_short(dt.month);

    let mut pos = 0usize;
    unsafe {
        // "Thu, "
        for i in 0..3 { *buf.add(pos) = *wday.add(i); pos += 1; }
        *buf.add(pos) = b','; pos += 1;
        *buf.add(pos) = b' '; pos += 1;

        // "01 "
        pos += write_padded_2(buf.add(pos), dt.day as u32);
        *buf.add(pos) = b' '; pos += 1;

        // "Jan "
        for i in 0..3 { *buf.add(pos) = *mon.add(i); pos += 1; }
        *buf.add(pos) = b' '; pos += 1;

        // "1970 "
        pos += write_padded_4(buf.add(pos), dt.year as u32);
        *buf.add(pos) = b' '; pos += 1;

        // "00:00:00"
        pos += write_padded_2(buf.add(pos), dt.hour as u32);
        *buf.add(pos) = b':'; pos += 1;
        pos += write_padded_2(buf.add(pos), dt.minute as u32);
        *buf.add(pos) = b':'; pos += 1;
        pos += write_padded_2(buf.add(pos), dt.second as u32);

        // " +0000"
        *buf.add(pos) = b' '; pos += 1;
        *buf.add(pos) = b'+'; pos += 1;
        *buf.add(pos) = b'0'; pos += 1;
        *buf.add(pos) = b'0'; pos += 1;
        *buf.add(pos) = b'0'; pos += 1;
        *buf.add(pos) = b'0'; pos += 1;
        *buf.add(pos) = 0;
    }
    pos
}
