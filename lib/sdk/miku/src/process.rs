//! The process, its threads, time

use crate::*;

pub fn exit(code: i64) -> ! {
    unsafe { miku_exit(code) }
}

/// Program arguments as strings, borrowed from the initial stack. Non-UTF-8
/// arguments are empty strings, keeping positions.
///
/// # Safety
/// `argv` must be the pointer `_start_main` was given.
pub fn args(argc: i32, argv: *const *const u8) -> alloc::vec::Vec<&'static str> {
    let mut out = alloc::vec::Vec::new();
    if argc <= 0 || argv.is_null() {
        return out;
    }
    for i in 0..argc as usize {
        let p = unsafe { *argv.add(i) };
        if p.is_null() {
            break;
        }
        let mut len = 0usize;
        // Bounded in case of a missing terminator
        while len < 4096 && unsafe { *p.add(len) } != 0 {
            len += 1;
        }
        let bytes = unsafe { core::slice::from_raw_parts(p, len) };
        out.push(core::str::from_utf8(bytes).unwrap_or(""));
    }
    out
}

/// Fills `buf` with kernel entropy. False on failure, with no weaker fallback
pub fn getrandom(buf: &mut [u8]) -> bool {
    if buf.is_empty() { return true; }
    unsafe { miku_getrandom(buf.as_mut_ptr(), buf.len()) == buf.len() as i64 }
}

/// Seconds since the Unix epoch, or None if unset
pub fn realtime() -> Option<u64> {
    let s = unsafe { miku_realtime() };
    if s < 0 { None } else { Some(s as u64) }
}
pub fn sleep(ticks: u64) {
    unsafe {
        miku_sleep(ticks);
    }
}
pub fn sleep_ms(ms: u64) {
    unsafe {
        miku_sleep_ms(ms);
    }
}
pub fn uptime() -> u64 {
    unsafe { miku_uptime() }
}
pub fn uptime_ms() -> u64 {
    unsafe { miku_uptime_ms() }
}
pub fn yield_now() {
    unsafe {
        miku_yield();
    }
}
pub fn getpid() -> u64 {
    unsafe { miku_getpid() }
}

/// This thread's id, which for a single-threaded program is its pid.
pub fn gettid() -> i64 {
    unsafe { miku_gettid() }
}

/// Starts a thread on a caller-owned stack, which must outlive it. Returns the
/// thread id or a negative errno.
pub fn thread_spawn(
    entry: extern "C" fn(*mut u8) -> i32,
    arg: *mut u8,
    stack: &mut [u8],
) -> i64 {
    unsafe { miku_thread_spawn(entry, arg, stack.as_mut_ptr(), stack.len()) }
}

/// Sleeps while the word holds `expected`. A wake, timeout, or nothing may end
/// it, so the caller rechecks in a loop. `timeout_ns` 0 means none.
pub fn futex_wait(addr: &core::sync::atomic::AtomicU32, expected: u32, timeout_ns: u64) -> i64 {
    unsafe { miku_futex_wait(addr.as_ptr() as *const u32, expected, timeout_ns) }
}

/// Wakes at most `count` sleepers. Returns how many.
pub fn futex_wake(addr: &core::sync::atomic::AtomicU32, count: u32) -> i64 {
    unsafe { miku_futex_wake(addr.as_ptr() as *const u32, count) }
}
pub fn brk(addr: u64) -> u64 {
    unsafe { miku_brk(addr) }
}

/// A `MikuMutex` as a lock
pub trait MutexOps {
    /// Takes the lock, sleeping if held
    fn lock(&self);
    /// Gives it back, waking one sleeper if there is one
    fn unlock(&self);
    /// Takes it if it is free, without waiting
    fn try_lock(&self) -> bool;
}

impl MutexOps for MikuMutex {
    fn lock(&self) {
        unsafe { miku_mutex_lock(self as *const _ as *mut MikuMutex) }
    }
    fn unlock(&self) {
        unsafe { miku_mutex_unlock(self as *const _ as *mut MikuMutex) }
    }
    fn try_lock(&self) -> bool {
        unsafe { miku_mutex_trylock(self as *const _ as *mut MikuMutex) }
    }
}
