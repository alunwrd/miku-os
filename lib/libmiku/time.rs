use crate::sys::*;

/// One scheduler tick. `sleep`(7) and `uptime`(8) speak in ticks, so this must
/// match `PIT_HZ = 250` in `kernel/arch/x86_64/interrupts.rs`: 4 ms
pub const TICK_MS: u64 = 4;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sleep(ticks: u64) {
    unsafe { sc1(SYS_SLEEP, ticks); }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sleep_ms(ms: u64) {
    miku_sleep_ns(ms.saturating_mul(1_000_000));
}

/// A sleep of `ns` nanoseconds; zero is a yield
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sleep_ns(ns: u64) {
    if ns == 0 {
        unsafe { sc1(SYS_SLEEP, 0); }
        return;
    }
    unsafe { sc1(SYS_SLEEP_NS, ns); }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_uptime() -> u64 {
    unsafe { sc0(SYS_UPTIME) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_uptime_ms() -> u64 {
    (unsafe { sc0(SYS_UPTIME) } as u64) * TICK_MS
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_yield() {
    unsafe { sc1(SYS_SLEEP, 0); }
}

/// Seconds since the Unix epoch, or a negative errno when the clock was never
/// set. `miku_uptime` counts ticks since boot instead
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_realtime() -> i64 {
    let mut ts = [0i64; 2];
    let rc = unsafe { sc2(SYS_CLOCK_GETTIME, CLOCK_REALTIME, ts.as_mut_ptr() as u64) };
    let rc = rc as i64;
    if rc < 0 { return rc; }
    ts[0]
}

/// A `timespec`, as syscall 70 writes one.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MikuTimespec {
    pub sec: i64,
    pub nsec: i64,
}

/// The full clock, the primitive behind `Instant` and `SystemTime`.
///
/// Through the vDSO (`auxv.rs`) when the process has one, with no kernel entry.
/// The syscall otherwise, with the same answers; the vDSO itself falls back for
/// a clock it cannot read.
///
/// `clock` is `CLOCK_REALTIME` (0) or `CLOCK_MONOTONIC` (1). Returns 0 or a
/// negative errno: `-EINVAL` for REALTIME before the clock was set
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_clock_gettime(clock: u64, out: *mut MikuTimespec) -> i64 {
    if out.is_null() { return crate::errno::EINVAL; }
    if let Some(f) = crate::auxv::clock_gettime() {
        // The same two words, `tv_sec` and `tv_nsec`
        return unsafe { f(clock as i32, out as *mut crate::auxv::VdsoTimespec) } as i64;
    }
    unsafe { sc2(SYS_CLOCK_GETTIME, clock, out as u64) as i64 }
}

/// The processor this thread runs on, from the vDSO; -1 without one. A hint, as
/// the thread may move
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_getcpu() -> i64 {
    let Some(f) = crate::auxv::getcpu() else { return -1 };
    let mut cpu = 0u32;
    if unsafe { f(&mut cpu, core::ptr::null_mut(), core::ptr::null_mut()) } != 0 {
        return -1;
    }
    cpu as i64
}

/// Whether this process reads the clock through the vDSO: 1 or 0, for a test
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_vdso_available() -> u64 {
    crate::auxv::clock_gettime().is_some() as u64
}

/// Sets the wall clock. Root only
#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_set_realtime(unix_secs: i64) -> i64 {
    let ts = [unix_secs, 0i64];
    unsafe { sc2(SYS_CLOCK_SETTIME, CLOCK_REALTIME, ts.as_ptr() as u64) as i64 }
}
