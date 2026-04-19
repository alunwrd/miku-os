// Software timer and stopwatch utilities
//
// Provides elapsed time measurement, countdown timers,
// and periodic timer callbacks.
// Uses miku_uptime_ms() as the time source.

use crate::time;

// stopwatch

#[repr(C)]
pub struct MikuStopwatch {
    start_ms: u64,
    paused_elapsed: u64,
    running: bool,
}

// create and start a stopwatch
#[no_mangle]
pub extern "C" fn miku_sw_start() -> MikuStopwatch {
    MikuStopwatch {
        start_ms: time::miku_uptime_ms(),
        paused_elapsed: 0,
        running: true,
    }
}

// get elapsed milliseconds
#[no_mangle]
pub extern "C" fn miku_sw_elapsed_ms(sw: *const MikuStopwatch) -> u64 {
    if sw.is_null() { return 0; }
    let sw = unsafe { &*sw };
    if sw.running {
        sw.paused_elapsed + time::miku_uptime_ms().saturating_sub(sw.start_ms)
    } else {
        sw.paused_elapsed
    }
}

// get elapsed seconds
#[no_mangle]
pub extern "C" fn miku_sw_elapsed_sec(sw: *const MikuStopwatch) -> u64 {
    miku_sw_elapsed_ms(sw) / 1000
}

// pause stopwatch
#[no_mangle]
pub extern "C" fn miku_sw_pause(sw: *mut MikuStopwatch) {
    if sw.is_null() { return; }
    let sw = unsafe { &mut *sw };
    if sw.running {
        sw.paused_elapsed += time::miku_uptime_ms().saturating_sub(sw.start_ms);
        sw.running = false;
    }
}

// resume stopwatch
#[no_mangle]
pub extern "C" fn miku_sw_resume(sw: *mut MikuStopwatch) {
    if sw.is_null() { return; }
    let sw = unsafe { &mut *sw };
    if !sw.running {
        sw.start_ms = time::miku_uptime_ms();
        sw.running = true;
    }
}

// reset stopwatch
#[no_mangle]
pub extern "C" fn miku_sw_reset(sw: *mut MikuStopwatch) {
    if sw.is_null() { return; }
    let sw = unsafe { &mut *sw };
    sw.start_ms = time::miku_uptime_ms();
    sw.paused_elapsed = 0;
    sw.running = true;
}

// check if running
#[no_mangle]
pub extern "C" fn miku_sw_running(sw: *const MikuStopwatch) -> bool {
    if sw.is_null() { return false; }
    unsafe { (*sw).running }
}

// countdown timer

#[repr(C)]
pub struct MikuTimer {
    deadline_ms: u64,
    duration_ms: u64,
    repeat: bool,
    expired: bool,
}

// create one-shot timer
#[no_mangle]
pub extern "C" fn miku_timer_once(duration_ms: u64) -> MikuTimer {
    let now = time::miku_uptime_ms();
    MikuTimer {
        deadline_ms: now.saturating_add(duration_ms),
        duration_ms,
        repeat: false,
        expired: false,
    }
}

// create repeating timer
#[no_mangle]
pub extern "C" fn miku_timer_repeat(interval_ms: u64) -> MikuTimer {
    let now = time::miku_uptime_ms();
    MikuTimer {
        deadline_ms: now.saturating_add(interval_ms),
        duration_ms: interval_ms,
        repeat: true,
        expired: false,
    }
}

//   check if timer has expired
// For repeating timers, resets deadline when expired
#[no_mangle]
pub extern "C" fn miku_timer_check(t: *mut MikuTimer) -> bool {
    if t.is_null() { return false; }
    let t = unsafe { &mut *t };
    if t.expired && !t.repeat { return true; }

    let now = time::miku_uptime_ms();
    if now >= t.deadline_ms {
        if t.repeat {
            t.deadline_ms = now.saturating_add(t.duration_ms);
        } else {
            t.expired = true;
        }
        return true;
    }
    false
}

// remaining time in ms
#[no_mangle]
pub extern "C" fn miku_timer_remaining(t: *const MikuTimer) -> u64 {
    if t.is_null() { return 0; }
    let t = unsafe { &*t };
    let now = time::miku_uptime_ms();
    t.deadline_ms.saturating_sub(now)
}

// reset timer
#[no_mangle]
pub extern "C" fn miku_timer_reset(t: *mut MikuTimer) {
    if t.is_null() { return; }
    let t = unsafe { &mut *t };
    t.deadline_ms = time::miku_uptime_ms().saturating_add(t.duration_ms);
    t.expired = false;
}

// check if expired (one-shot only, no side effects)
#[no_mangle]
pub extern "C" fn miku_timer_expired(t: *const MikuTimer) -> bool {
    if t.is_null() { return false; }
    let t = unsafe { &*t };
    if t.expired { return true; }
    time::miku_uptime_ms() >= t.deadline_ms
}

// simple delay //

// busy-wait for given milliseconds
#[no_mangle]
pub extern "C" fn miku_delay_ms(ms: u64) {
    let target = time::miku_uptime_ms().saturating_add(ms);
    while time::miku_uptime_ms() < target {
        core::hint::spin_loop();
    }
}

// sleep for given milliseconds (yields to scheduler)
#[no_mangle]
pub extern "C" fn miku_delay_sleep(ms: u64) {
    time::miku_sleep_ms(ms);
}
