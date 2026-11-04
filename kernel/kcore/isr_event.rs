// An event an interrupt handler announces and threads sleep on
//
// A sequence number and a few waiter slots. The handler bumps the number and
// wakes whoever is in a slot; a thread wanting the next event publishes its pid
// in a slot, looks once more, and sleeps. Users: the GSP message queue, the
// display's vertical blank, the GSP event thread, GPU fences
// (`dev/gpu-memory-compute.md`, 3.5a).
//
// Not `sched::WaitQueue`: waking one returns its list of pids to be freed, and
// nothing is freed in an interrupt handler. Here nothing is allocated: the
// slots are atomics, and `sched::wakeup` is safe from an ISR
// (`sched/control.rs`).
//
// The races, and what closes them:
//   - an event between a waiter's last look and its publishing a pid: the
//     waiter looks again after publishing, before it sleeps;
//   - an event between that look and the context switch: `wakeup` cancels a
//     sleep asked for and not yet taken;
//   - everything else (more waiters than slots, a handler that never runs):
//     the sleep is bounded, so a missed wake costs its backstop.

use core::sync::atomic::{AtomicU64, Ordering};

pub struct IsrEvent<const N: usize> {
    seq: AtomicU64,
    waiters: [AtomicU64; N],
}

impl<const N: usize> IsrEvent<N> {
    pub const fn new() -> Self {
        IsrEvent { seq: AtomicU64::new(0), waiters: [const { AtomicU64::new(0) }; N] }
    }

    /// From interrupt context: count one, and wake everybody waiting. Atomics
    /// and `sched::wakeup` only
    pub fn notify(&self) {
        self.seq.fetch_add(1, Ordering::Release);
        for w in &self.waiters {
            let pid = w.swap(0, Ordering::AcqRel);
            if pid != 0 {
                crate::sched::wakeup(pid);
            }
        }
    }

    /// How many times it has been notified
    pub fn seq(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    /// Sleep until the event moves past `seen`, `ready()` says there is no
    /// need, or `ticks` go by. True if the event moved.
    ///
    /// `seen` is a `seq()` the caller read before it last checked its own
    /// condition, which makes an event landing in between visible here. With
    /// every slot taken the caller still sleeps, bounded: a poll, never worse
    /// than the backstop
    pub fn sleep_unless(&self, seen: u64, ticks: u64, ready: impl Fn() -> bool) -> bool {
        self.sleep_outcome(seen, ticks, ready) == Woke::Moved
    }

    /// The same sleep, saying how it ended, for a caller counting which waits
    /// an interrupt ended and which only the backstop did
    pub fn sleep_outcome(&self, seen: u64, ticks: u64, ready: impl Fn() -> bool) -> Woke {
        let pid = crate::sched::current_pid();
        let slot = self
            .waiters
            .iter()
            .find(|w| w.compare_exchange(0, pid, Ordering::AcqRel, Ordering::Relaxed).is_ok());
        let slept = self.seq.load(Ordering::Acquire) == seen && !ready();
        if slept {
            crate::sched::sleep(ticks.max(1));
        }
        if let Some(w) = slot {
            // Still ours only if no notify took it; either way it is free after
            let _ = w.compare_exchange(pid, 0, Ordering::AcqRel, Ordering::Relaxed);
        }
        if self.seq.load(Ordering::Acquire) != seen {
            Woke::Moved
        } else if slept {
            Woke::TimedOut
        } else {
            Woke::NotNeeded
        }
    }
}

/// How `sleep_outcome` ended
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Woke {
    /// The event moved past `seen`
    Moved,
    /// `ready()` held before the sleep, so there was none
    NotNeeded,
    /// It slept the whole backstop and the event did not move
    TimedOut,
}
