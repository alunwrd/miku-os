use crate::sys::*;

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sleep(ticks: u64) {
    unsafe { sc1(SYS_SLEEP, ticks); }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_sleep_ms(ms: u64) {
    let ticks = (ms + 9) / 10;
    unsafe { sc1(SYS_SLEEP, ticks); }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_uptime() -> u64 {
    unsafe { sc0(SYS_UPTIME) as u64 }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_uptime_ms() -> u64 {
    (unsafe { sc0(SYS_UPTIME) } as u64) * 10
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn miku_yield() {
    unsafe { sc1(SYS_SLEEP, 0); }
}
