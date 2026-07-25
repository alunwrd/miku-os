// Wall-clock time.
//
// 'procfs::set_wall_clock()' stored a bare seconds value that nothing ever
// advanced, so after NTP set it the clock was simply frozen at the instant
// of the reply - every file created afterwards carried the same timestamp,
// and 'uptime'-based code had no relation to it at all.
//
// The clock is kept as an epoch anchor plus the scheduler tick at which the
// anchor was taken; reading it adds the elapsed ticks. Any source can move
// the anchor (the CMOS RTC at boot, NTP once the network is up) without the
// readers caring which one won

use core::sync::atomic::{AtomicU64, Ordering};

use crate::kcore::irq_lock::IrqMutex;

#[derive(Clone, Copy)]
struct Anchor {
    /// Unix seconds at the moment the anchor was taken
    epoch_secs: u64,
    /// Timer tick at that same moment
    tick: u64,
}

static ANCHOR: IrqMutex<Option<Anchor>> = IrqMutex::new(None);
/// Set once a source better than "no idea" has been seen. Kept separate so
/// the common read path can bail out without taking the lock
static HAVE_TIME: AtomicU64 = AtomicU64::new(0);

/// Source that last set the clock, for reporting
static SOURCE: IrqMutex<&'static str> = IrqMutex::new("none");

fn hz() -> u64 {
    let hz = crate::arch::x86_64::apic::timer_hz() as u64;
    if hz == 0 { 250 } else { hz }
}

/// Anchor the wall clock to `unix_secs` as of now. Later sources override
/// earlier ones: NTP is more accurate than the CMOS, so it simply re-anchors
pub fn set_epoch(unix_secs: u64, source: &'static str) {
    let tick = crate::arch::x86_64::interrupts::get_tick();
    *ANCHOR.lock() = Some(Anchor { epoch_secs: unix_secs, tick });
    *SOURCE.lock() = source;
    HAVE_TIME.store(1, Ordering::Release);
}

/// Seconds since the Unix epoch, or 0 when the clock was never set
pub fn now_unix() -> u64 {
    if HAVE_TIME.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let anchor = match *ANCHOR.lock() {
        Some(a) => a,
        None => return 0,
    };
    let now_tick = crate::arch::x86_64::interrupts::get_tick();
    let elapsed = now_tick.saturating_sub(anchor.tick) / hz();
    anchor.epoch_secs.saturating_add(elapsed)
}

/// Nanoseconds within the current second. Derived from the tick, so the
/// resolution is one timer period (4 ms at 250 Hz) - honest, not invented
pub fn now_nanos_frac() -> u64 {
    if HAVE_TIME.load(Ordering::Acquire) == 0 {
        return 0;
    }
    let anchor = match *ANCHOR.lock() {
        Some(a) => a,
        None => return 0,
    };
    let hz = hz();
    let elapsed_ticks = crate::arch::x86_64::interrupts::get_tick().saturating_sub(anchor.tick);
    (elapsed_ticks % hz) * (1_000_000_000 / hz)
}

pub fn is_set() -> bool {
    HAVE_TIME.load(Ordering::Acquire) != 0
}

pub fn source() -> &'static str {
    *SOURCE.lock()
}

/// Seed the clock from the platform RTC. Called early in boot, long before
/// any network exists
pub fn init_from_rtc() {
    match crate::arch::x86_64::rtc::read() {
        Some(dt) => {
            let secs = crate::arch::x86_64::rtc::to_unix(&dt);
            set_epoch(secs, "rtc");
            crate::serial_println!(
                "[clock] RTC {:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC (unix {})",
                dt.year, dt.month, dt.day, dt.hour, dt.minute, dt.second, secs
            );
        }
        None => {
            crate::serial_println!("[clock] no usable RTC; wall clock stays unset until NTP");
        }
    }
}

// Calendar conversion, for anything that wants to print a date

/// Inverse of 'rtc::days_from_civil' - same shifted-year trick
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;                                     // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);              // [0, 365]
    let mp = (5 * doy + 2) / 153;                                   // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;                  // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;           // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Split Unix seconds into (year, month, day, hour, minute, second) UTC
pub fn civil_from_unix(secs: u64) -> (u16, u8, u8, u8, u8, u8) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = civil_from_days(days);
    (
        y as u16,
        m as u8,
        d as u8,
        (rem / 3600) as u8,
        ((rem % 3600) / 60) as u8,
        (rem % 60) as u8,
    )
}
