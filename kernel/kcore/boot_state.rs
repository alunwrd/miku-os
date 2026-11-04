use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

pub type InitResult = Result<(), &'static str>;

static BOOT_DONE: AtomicBool = AtomicBool::new(false);

pub fn is_done() -> bool {
    BOOT_DONE.load(Ordering::Acquire)
}

pub fn mark_done() {
    BOOT_DONE.store(true, Ordering::Release);
    let total = crate::kcore::time::since_boot_ms();
    if total != 0 {
        crate::serial_println!(
            "[boot] up in {} ms ({} ms of it inside init steps)", total, elapsed_ms());
    }
}

/// Milliseconds spent inside every step so far, for the closing line's total
static BOOT_MS: AtomicU64 = AtomicU64::new(0);

/// Steps at or above this get their duration printed; below it the line stays
/// short, as a line down the wire is not free
const SLOW_STEP_MS: u64 = 5;

pub fn record(name: &str, result: InitResult) {
    record_timed(name, result, 0)
}

/// `record` plus how long the step took. Separate: the timing is taken around
/// the expression, which only the macro can do
pub fn record_timed(name: &str, result: InitResult, ms: u64) {
    // Every init step could have run a fat stack frame off the bottom of the
    // BSP stack; checked here
    crate::kcore::stack_guard::check(name);
    BOOT_MS.fetch_add(ms, Ordering::Relaxed);
    match result {
        Ok(()) => {
            crate::cprint!(100, 100, 100, "[");
            crate::cprint!(100, 220, 150, "  ok  ");
            crate::cprint!(100, 100, 100, "] ");
            if ms >= SLOW_STEP_MS {
                crate::cprint!(100, 170, 255, "{}", name);
                crate::cprintln!(120, 120, 120, "  ({} ms)", ms);
                crate::serial_println!("[boot] ok  {} ({} ms)", name, ms);
            } else {
                crate::cprintln!(100, 170, 255, "{}", name);
                crate::serial_println!("[boot] ok  {}", name);
            }
        }
        Err(reason) => {
            crate::cprint!(100, 100, 100, "[");
            crate::cprint!(255, 70, 70, " fail ");
            crate::cprint!(100, 100, 100, "] ");
            crate::cprint!(100, 170, 255, "{}", name);
            crate::cprintln!(120, 120, 120, ": {}", reason);
            crate::serial_println!("[boot] fail {} : {}", name, reason);
        }
    }
}

/// Total measured time across every `boot_step!` so far, in milliseconds.
///
/// Steps run before `time::calibrate_early()` succeeded contribute nothing, so
/// this is a floor on the real figure
pub fn elapsed_ms() -> u64 {
    BOOT_MS.load(Ordering::Relaxed)
}

/// Run one init step, print its result, and say how long it took.
///
/// The clock is read around the expression, not inside `record`. `Stopwatch`
/// reads zero until the TSC is calibrated, which `kernel_main` does before the
/// first step; a step run earlier reports nothing
#[macro_export]
macro_rules! boot_step {
    ($name:expr, $expr:expr) => {{
        let __sw = $crate::kcore::time::Stopwatch::start();
        let __r = $expr;
        let __ms = if $crate::kcore::time::tsc_khz() != 0 { __sw.elapsed_ms() } else { 0 };
        $crate::kcore::boot_state::record_timed($name, __r, __ms)
    }};
}
