//! `Instant` and `SystemTime`, from `clock_gettime`(70).
//!
//! `Instant` is MONOTONIC: it counts from boot and never goes backwards.
//! `SystemTime` is REALTIME and may not exist yet: MikuOS does not guess a wall
//! clock, so the syscall answers `-EINVAL` until something sets it.

use crate::sys::pal::abi;
use crate::time::Duration;

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::ZERO);

fn read(clock: u64) -> Option<Duration> {
    let mut ts = abi::MikuTimespec::default();
    if abi::miku_clock_gettime(clock, &mut ts) < 0 {
        return None;
    }
    if ts.sec < 0 || ts.nsec < 0 || ts.nsec >= 1_000_000_000 {
        return None;
    }
    Some(Duration::new(ts.sec as u64, ts.nsec as u32))
}

impl Instant {
    pub fn now() -> Instant {
        // A monotonic clock that cannot be read is a machine without a timer
        Instant(read(abi::CLOCK_MONOTONIC).expect("CLOCK_MONOTONIC is unreadable"))
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_sub(*other)?))
    }
}

impl SystemTime {
    /// A whole-second timestamp, for `sys::fs`. The VFS stores seconds and
    /// nothing finer
    pub fn from_secs(secs: u64) -> SystemTime {
        SystemTime(Duration::from_secs(secs))
    }

    /// The other way, for `sys::fs` setting a file's times
    pub fn as_secs(&self) -> u64 {
        self.0.as_secs()
    }

    pub const MAX: SystemTime = SystemTime(Duration::MAX);
    pub const MIN: SystemTime = SystemTime(Duration::ZERO);

    pub fn now() -> SystemTime {
        // The wall clock may not be set. `SystemTime::now()` has no way to say
        // so, so it reports the epoch, and a difference of two is still correct
        SystemTime(read(abi::CLOCK_REALTIME).unwrap_or(Duration::ZERO))
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_sub(*other)?))
    }
}
