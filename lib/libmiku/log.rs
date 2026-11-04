// Structured logging
//
// Provides log levels (ERROR, WARN, INFO, DEBUG, TRACE)
// configurable minimum level, and formatted output
// All output goes to stderr (fd 2)
// Thread-safe global configuration via atomics

use crate::io;
use crate::string;
use crate::num;
use crate::time;
use core::sync::atomic::{AtomicU8, AtomicBool, Ordering};

// log levels
pub const LOG_ERROR: u8 = 0;
pub const LOG_WARN:  u8 = 1;
pub const LOG_INFO:  u8 = 2;
pub const LOG_DEBUG: u8 = 3;
pub const LOG_TRACE: u8 = 4;

static LOG_LEVEL: AtomicU8 = AtomicU8::new(LOG_INFO);
static LOG_SHOW_TIME: AtomicBool = AtomicBool::new(true);

fn level_name(level: u8) -> &'static [u8] {
    match level {
        LOG_ERROR => b"Error",
        LOG_WARN  => b"Warn ",
        LOG_INFO  => b"Info ",
        LOG_DEBUG => b"Debug",
        LOG_TRACE => b"Trace",
        _         => b"?????",
    }
}

fn write_stderr(s: &[u8]) {
    io::miku_write(2, s.as_ptr(), s.len());
}

fn write_stderr_str(s: *const u8) {
    if s.is_null() { return; }
    let len = string::miku_strlen(s);
    io::miku_write(2, s, len);
}

// set minimum log level
#[no_mangle]
pub extern "C" fn miku_log_set_level(level: u8) {
    LOG_LEVEL.store(level, Ordering::Relaxed);
}

// get current log level
#[no_mangle]
pub extern "C" fn miku_log_get_level() -> u8 {
    LOG_LEVEL.load(Ordering::Relaxed)
}

// enable/disable timestamp in log output
#[no_mangle]
pub extern "C" fn miku_log_show_time(show: bool) {
    LOG_SHOW_TIME.store(show, Ordering::Relaxed);
}

fn log_prefix(level: u8) {
    write_stderr(b"[");
    if LOG_SHOW_TIME.load(Ordering::Relaxed) {
        let ms = time::miku_uptime_ms();
        let mut buf = [0u8; 24];
        num::miku_itoa(ms as i64, buf.as_mut_ptr());
        write_stderr_str(buf.as_ptr());
        write_stderr(b"ms] [");
    }
    write_stderr(level_name(level));
    write_stderr(b"] ");
}

fn log_tag(tag: *const u8) {
    if !tag.is_null() {
        write_stderr(b"[");
        write_stderr_str(tag);
        write_stderr(b"] ");
    }
}

// log a message at given level
#[no_mangle]
pub extern "C" fn miku_log(level: u8, tag: *const u8, msg: *const u8) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) { return; }

    log_prefix(level);
    log_tag(tag);

    if !msg.is_null() {
        write_stderr_str(msg);
    }
    write_stderr(b"\n");
}

// convenience functions
#[no_mangle]
pub extern "C" fn miku_log_error(tag: *const u8, msg: *const u8) {
    miku_log(LOG_ERROR, tag, msg);
}

#[no_mangle]
pub extern "C" fn miku_log_warn(tag: *const u8, msg: *const u8) {
    miku_log(LOG_WARN, tag, msg);
}

#[no_mangle]
pub extern "C" fn miku_log_info(tag: *const u8, msg: *const u8) {
    miku_log(LOG_INFO, tag, msg);
}

#[no_mangle]
pub extern "C" fn miku_log_debug(tag: *const u8, msg: *const u8) {
    miku_log(LOG_DEBUG, tag, msg);
}

#[no_mangle]
pub extern "C" fn miku_log_trace(tag: *const u8, msg: *const u8) {
    miku_log(LOG_TRACE, tag, msg);
}

// log with integer value
// Output: [Level] [tag] msg: value
#[no_mangle]
pub extern "C" fn miku_log_int(level: u8, tag: *const u8, msg: *const u8, val: i64) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) { return; }

    log_prefix(level);
    log_tag(tag);

    if !msg.is_null() {
        write_stderr_str(msg);
    }
    write_stderr(b": ");

    let mut buf = [0u8; 24];
    num::miku_itoa(val, buf.as_mut_ptr());
    write_stderr_str(buf.as_ptr());
    write_stderr(b"\n");
}

// log with hex value
// Output: [Level] [tag] msg: 0xHEX
#[no_mangle]
pub extern "C" fn miku_log_hex(level: u8, tag: *const u8, msg: *const u8, val: u64) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) { return; }

    log_prefix(level);
    log_tag(tag);

    if !msg.is_null() {
        write_stderr_str(msg);
    }
    write_stderr(b": 0x");

    let mut buf = [0u8; 20];
    num::utoa_hex(val, buf.as_mut_ptr());
    write_stderr_str(buf.as_ptr());
    write_stderr(b"\n");
}

// log with pointer value
#[no_mangle]
pub extern "C" fn miku_log_ptr(level: u8, tag: *const u8, msg: *const u8, ptr: *const u8) {
    miku_log_hex(level, tag, msg, ptr as u64);
}

// log two integers: msg: a, b
#[no_mangle]
pub extern "C" fn miku_log_int2(level: u8, tag: *const u8, msg: *const u8, a: i64, b: i64) {
    if level > LOG_LEVEL.load(Ordering::Relaxed) { return; }

    log_prefix(level);
    log_tag(tag);

    if !msg.is_null() {
        write_stderr_str(msg);
    }
    write_stderr(b": ");

    let mut buf = [0u8; 24];
    num::miku_itoa(a, buf.as_mut_ptr());
    write_stderr_str(buf.as_ptr());
    write_stderr(b", ");
    num::miku_itoa(b, buf.as_mut_ptr());
    write_stderr_str(buf.as_ptr());
    write_stderr(b"\n");
}
