//! Arithmetic, strings, and safe forms of the data libraries' functions

use crate::*;

/// Directory names with kinds, without `.` and `..`, following nothing. Empty
/// if the directory will not open
/// Program arguments as strings, borrowed from the initial stack. Non-UTF-8
/// arguments are empty strings, keeping positions.
///
/// # Safety
/// `argv` must be the pointer `_start_main` was given.
/// Fills `buf` with kernel entropy. False on failure, with no weaker fallback
/// Seconds since the Unix epoch, or None if unset
/// Left-aligned decimal. Returns bytes written, or 0 if it does not fit
/// The same with a sign
/// This thread's id, which for a single-threaded program is its pid.
/// Starts a thread on a caller-owned stack, which must outlive it. Returns the
/// thread id or a negative errno.
/// Sleeps while the word holds `expected`. A wake, timeout, or nothing may end
/// it, so the caller rechecks in a loop. `timeout_ns` 0 means none.
/// Wakes at most `count` sleepers. Returns how many.
pub fn abs(x: i64) -> i64 {
    unsafe { miku_abs(x) }
}
pub fn min(a: i64, b: i64) -> i64 {
    unsafe { miku_min(a, b) }
}
pub fn max(a: i64, b: i64) -> i64 {
    unsafe { miku_max(a, b) }
}
pub fn clamp(val: i64, lo: i64, hi: i64) -> i64 {
    unsafe { miku_clamp(val, lo, hi) }
}
pub fn umin(a: u64, b: u64) -> u64 {
    unsafe { miku_umin(a, b) }
}
pub fn umax(a: u64, b: u64) -> u64 {
    unsafe { miku_umax(a, b) }
}
pub fn rand() -> u64 {
    unsafe { miku_rand() }
}
pub fn srand(seed: u64) {
    unsafe {
        miku_srand(seed);
    }
}
pub fn rand_range(lo: u64, hi: u64) -> u64 {
    unsafe { miku_rand_range(lo, hi) }
}

pub fn strlen(s: &[u8]) -> usize {
    unsafe { miku_strlen(s.as_ptr()) }
}
pub fn streq(a: &[u8], b: &[u8]) -> bool {
    unsafe { miku_strcmp(a.as_ptr(), b.as_ptr()) == 0 }
}

pub unsafe fn malloc(size: usize) -> *mut u8 {
    miku_malloc(size)
}
pub unsafe fn free(ptr: *mut u8) {
    miku_free(ptr)
}
pub unsafe fn realloc(ptr: *mut u8, new_size: usize) -> *mut u8 {
    miku_realloc(ptr, new_size)
}
pub unsafe fn calloc(count: usize, size: usize) -> *mut u8 {
    miku_calloc(count, size)
}

// bitops safe wrappers
pub fn popcount(x: u64) -> u64 {
    unsafe { miku_popcount64(x) }
}
pub fn clz(x: u64) -> u64 {
    unsafe { miku_clz64(x) }
}
pub fn ctz(x: u64) -> u64 {
    unsafe { miku_ctz64(x) }
}
pub fn log2(x: u64) -> u64 {
    unsafe { miku_log2(x) }
}
pub fn is_power_of_two(x: u64) -> bool {
    unsafe { miku_is_power_of_two(x) }
}
pub fn next_power_of_two(x: u64) -> u64 {
    unsafe { miku_next_power_of_two(x) }
}
pub fn align_up(val: u64, align: u64) -> u64 {
    unsafe { miku_align_up(val, align) }
}
pub fn align_down(val: u64, align: u64) -> u64 {
    unsafe { miku_align_down(val, align) }
}

// hash safe wrappers
pub fn hash_bytes(data: &[u8]) -> u64 {
    unsafe { miku_hash_bytes(data.as_ptr(), data.len()) }
}
pub fn crc32(data: &[u8]) -> u32 {
    unsafe { miku_crc32(data.as_ptr(), data.len()) }
}
pub fn fnv1a(data: &[u8]) -> u64 {
    unsafe { miku_fnv1a_64(data.as_ptr(), data.len()) }
}

// base64 safe wrappers
pub fn base64_encode(data: &[u8], out: &mut [u8]) -> i32 {
    unsafe { miku_base64_encode(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len()) }
}
pub fn base64_decode(data: &[u8], out: &mut [u8]) -> i32 {
    unsafe { miku_base64_decode(data.as_ptr(), data.len(), out.as_mut_ptr(), out.len()) }
}

// utf8 safe wrappers
pub fn utf8_valid(s: &[u8]) -> bool {
    unsafe { miku_utf8_valid(s.as_ptr(), s.len()) }
}
pub fn utf8_len(s: &[u8]) -> usize {
    unsafe { miku_utf8_len(s.as_ptr(), s.len()) }
}

// path safe wrappers
pub fn path_is_absolute(path: &[u8]) -> bool {
    unsafe { miku_path_is_absolute(path.as_ptr()) }
}
pub fn path_depth(path: &[u8]) -> usize {
    unsafe { miku_path_depth(path.as_ptr()) }
}
