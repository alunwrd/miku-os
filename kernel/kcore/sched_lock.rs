// A mutex that yields the CPU instead of spinning on it.
//
// `spin::Mutex` is the right tool for a lock held for a handful of
// instructions. It is the wrong tool for the VFS lock, which is held across
// block-device I/O: while one thread waits on a disk read, every other CPU
// that wants the filesystem burns its entire timeslice in a `pause` loop,
// and on a single CPU a lower-priority holder cannot even be scheduled to
// finish, because the waiter never gives the CPU back.
//
// `SchedMutex` keeps the same simple test-and-set core but hands the CPU to
// the scheduler on contention, so waiting time turns into work for someone
// else. Before the scheduler exists (early boot) it degrades to a plain
// spin, which is correct because nothing else is runnable yet.
//
// Deliberately not interrupt-safe: never take one of these from an
// interrupt handler - yielding from an ISR is not a thing. Locks shared
// with interrupt context want 'kcore::irq_lock::IrqMutex' instead

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How many times a waiter spins before it bothers the scheduler. Short
/// critical sections (the common case) finish inside this window and never
/// pay for a context switch
const SPIN_BEFORE_YIELD: u32 = 40;

pub struct SchedMutex<T: ?Sized> {
    locked: AtomicBool,
    /// Cumulative number of times a waiter had to yield. Exposed through
    /// 'contention_yields()' so the cost of a lock can be observed rather
    /// than guessed at
    yields: AtomicU64,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for SchedMutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for SchedMutex<T> {}

impl<T> SchedMutex<T> {
    pub const fn new(value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            yields: AtomicU64::new(0),
            data: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> SchedMutex<T> {
    pub fn lock(&self) -> SchedMutexGuard<'_, T> {
        let mut spins = 0u32;
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                return SchedMutexGuard { lock: self };
            }
            while self.locked.load(Ordering::Relaxed) {
                spins += 1;
                if spins >= SPIN_BEFORE_YIELD && crate::sched::started() {
                    spins = 0;
                    self.yields.fetch_add(1, Ordering::Relaxed);
                    crate::sched::yield_now();
                } else {
                    core::hint::spin_loop();
                }
            }
        }
    }

    pub fn try_lock(&self) -> Option<SchedMutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SchedMutexGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// Times a waiter gave up its slice waiting for this lock. A number that
    /// grows quickly is the signal to split the lock, not to spin harder
    #[inline]
    pub fn contention_yields(&self) -> u64 {
        self.yields.load(Ordering::Relaxed)
    }
}

pub struct SchedMutexGuard<'a, T: ?Sized + 'a> {
    lock: &'a SchedMutex<T>,
}

impl<'a, T: ?Sized> Deref for SchedMutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> DerefMut for SchedMutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T: ?Sized> Drop for SchedMutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}
