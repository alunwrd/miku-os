// The server lock, and who held it too long
//
// Every frame takes the server lock twice: to hand buffers back (`frame_done`)
// and to flatten the scene (`presentable`), and waits as long as somebody else
// holds it. So the lock remembers where it was taken (`#[track_caller]`) and
// when, and a hold past `SLOW_HOLD_US` says so in `dmesg debug`:
// `[mayland] the server lock was held N us, taken at file:line`.
//
// The cost is a timestamp on each lock and a compare on each unlock, with no
// table and no lock of its own.

use core::ops::{Deref, DerefMut};
use core::panic::Location;

/// Longer than this, a hold gets a line. A frame at 240 Hz is 4.2 ms
pub const SLOW_HOLD_US: u64 = 5_000;

pub struct Held<T> {
    inner: spin::Mutex<T>,
}

impl<T> Held<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: spin::Mutex::new(value) }
    }

    /// Take the lock. Where it is taken from is what a slow hold reports
    #[track_caller]
    pub fn lock(&self) -> HeldGuard<'_, T> {
        let guard = self.inner.lock();
        HeldGuard {
            guard: Some(guard),
            since: crate::kcore::time::Stopwatch::start(),
            site: Location::caller(),
        }
    }
}

pub struct HeldGuard<'a, T> {
    guard: Option<spin::MutexGuard<'a, T>>,
    since: crate::kcore::time::Stopwatch,
    site: &'static Location<'static>,
}

impl<T> Deref for HeldGuard<'_, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.guard.as_ref().expect("a held guard is never empty before drop")
    }
}

impl<T> DerefMut for HeldGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.guard.as_mut().expect("a held guard is never empty before drop")
    }
}

impl<T> Drop for HeldGuard<'_, T> {
    fn drop(&mut self) {
        let us = self.since.elapsed_us();
        // Lets go first, then reports: the line goes through the log, which
        // should not run under this lock
        drop(self.guard.take());
        if us >= SLOW_HOLD_US {
            crate::kdebug!("[mayland] the server lock was held {} us, taken at {}:{}",
                           us, self.site.file(), self.site.line());
        }
    }
}
