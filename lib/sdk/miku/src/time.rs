// Clock reads through the vDSO, found by libmiku (`lib/runtime/libmiku/core/auxv.rs`)

pub use miku_abi::time::CLOCK_REALTIME;
pub use miku_abi::time::CLOCK_MONOTONIC;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct Timespec {
    pub tv_sec:  i64,
    pub tv_nsec: i64,
}

extern "C" {
    fn miku_getcpu() -> i64;
    fn miku_vdso_available() -> u64;
}

/// Wall clock or time since boot, in seconds and nanoseconds. Returns 0 or a
/// negative errno
pub fn clock_gettime(clock: i32, ts: &mut Timespec) -> i32 {
    // The same two words as libmiku's `MikuTimespec`
    unsafe { crate::miku_clock_gettime(clock as u64, ts as *mut Timespec as *mut crate::MikuTimespec) as i32 }
}

/// Milliseconds since boot
pub fn monotonic_ms() -> u64 {
    let mut ts = Timespec::default();
    if clock_gettime(CLOCK_MONOTONIC, &mut ts) != 0 {
        return 0;
    }
    (ts.tv_sec as u64) * 1000 + (ts.tv_nsec as u64) / 1_000_000
}

/// Seconds since the Unix epoch, or 0 if unset
pub fn realtime_secs() -> u64 {
    let mut ts = Timespec::default();
    if clock_gettime(CLOCK_REALTIME, &mut ts) != 0 {
        return 0;
    }
    ts.tv_sec as u64
}

/// Current CPU, as a hint: the thread may migrate at once. None without the
/// vDSO
pub fn getcpu() -> Option<u32> {
    match unsafe { miku_getcpu() } {
        c if c >= 0 => Some(c as u32),
        _ => None,
    }
}

/// Whether this process reads the clock through the vDSO
pub fn available() -> bool {
    unsafe { miku_vdso_available() != 0 }
}
