// Wall-clock time.
//
// Kept as an epoch anchor plus the scheduler tick at which the anchor was
// taken; reading adds the elapsed ticks. Any source can move the anchor (the
// CMOS RTC at boot, NTP once the network is up)

use core::sync::atomic::{AtomicI32, AtomicU64, Ordering};

use crate::kcore::irq_lock::IrqMutex;

#[derive(Clone, Copy)]
struct Anchor {
    /// Unix seconds at the moment the anchor was taken
    epoch_secs: u64,
    /// Timer tick at that same moment
    tick: u64,
}

static ANCHOR: IrqMutex<Option<Anchor>> = IrqMutex::new(None);
/// Set once a real source has been seen. Separate, so the common read path can
/// bail out without the lock
static HAVE_TIME: AtomicU64 = AtomicU64::new(0);

/// Source that last set the clock, for reporting
static SOURCE: IrqMutex<&'static str> = IrqMutex::new("none");

/// Minutes ahead of UTC where this machine is. The clock is UTC, for every
/// timestamp, file and comparison; this is only how a date is written for
/// reading. The installer asks
static TZ_MINUTES: AtomicI32 = AtomicI32::new(0);

fn hz() -> u64 {
    let hz = crate::arch::x86_64::apic::timer_hz() as u64;
    if hz == 0 { 250 } else { hz }
}

/// Anchor the wall clock to `unix_secs` as of now. Later sources override
/// earlier ones: NTP re-anchors over the CMOS
pub fn set_epoch(unix_secs: u64, source: &'static str) {
    let tick = crate::arch::x86_64::interrupts::get_tick();
    *ANCHOR.lock() = Some(Anchor { epoch_secs: unix_secs, tick });
    *SOURCE.lock() = source;
    HAVE_TIME.store(1, Ordering::Release);
    // Every process reads the wall clock out of the vDSO's data page, so a
    // correction stopping here would leave ring 3 with the old anchor
    crate::process::vdso::publish();
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
/// resolution is one timer period (4 ms at 250 Hz)
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


/// Where the machine is, in minutes ahead of UTC
pub fn set_tz_offset(minutes: i32) {
    TZ_MINUTES.store(minutes, Ordering::Release);
}

pub fn tz_offset() -> i32 {
    TZ_MINUTES.load(Ordering::Acquire)
}

/// The same split, in the chosen zone. Before one is chosen this is
/// `civil_from_unix`: the offset starts at zero
pub fn local_from_unix(secs: u64) -> (u16, u8, u8, u8, u8, u8) {
    let shifted = secs as i64 + tz_offset() as i64 * 60;
    civil_from_unix(shifted.max(0) as u64)
}

/// Split Unix seconds into (year, month, day, hour, minute, second) UTC
pub fn civil_from_unix(secs: u64) -> (u16, u8, u8, u8, u8, u8) {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (y, m, d) = miku_base::date::civil_from_days(days);
    let (m, d) = (m as u32, d as u32);
    (
        y as u16,
        m as u8,
        d as u8,
        (rem / 3600) as u8,
        ((rem % 3600) / 60) as u8,
        (rem % 60) as u8,
    )
}
