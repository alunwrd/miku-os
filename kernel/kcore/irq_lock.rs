// Interrupt-safe spin lock.
//
// A plain 'spin::Mutex' is only safe when *nobody* takes it from interrupt
// context. The moment an ISR touches the same lock the kernel can deadlock
// against itself: thread context takes the lock with IF=1, the timer fires
// on that very CPU, the handler spins on a lock its own CPU holds, and the
// only code that could release it never runs again. On a uniprocessor that
// is an instant hard hang; on SMP it is a rare hang that looks like random
// corruption
//
// 'IrqMutex' closes that window by disabling interrupts for the whole
// critical section and restoring the previous IF state on unlock. Nesting is
// handled: an inner lock taken while interrupts are already off leaves them
// off when it unlocks
//
// Use it for every lock reachable from an interrupt handler (the physical
// frame allocator, the swap reverse map, per-CPU bookkeeping). Locks that
// are only ever taken by threads can stay 'spin::Mutex'
//
// It does not make holding a lock across blocking I/O safe - keep the
// critical sections short, since interrupts (including the scheduler tick)
// are off for their whole duration

use core::mem::ManuallyDrop;
use core::ops::{Deref, DerefMut};

use spin::{Mutex, MutexGuard};
use x86_64::instructions::interrupts;

pub struct IrqMutex<T: ?Sized> {
    inner: Mutex<T>,
}

unsafe impl<T: ?Sized + Send> Send for IrqMutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for IrqMutex<T> {}

impl<T> IrqMutex<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: Mutex::new(value) }
    }
}

impl<T: ?Sized> IrqMutex<T> {
    /// Disable interrupts, then take the lock. Interrupts stay off until the
    /// returned guard is dropped, and are only re-enabled if they were on
    /// when 'lock()' was called
    #[inline]
    pub fn lock(&self) -> IrqMutexGuard<'_, T> {
        let was_enabled = interrupts::are_enabled();
        interrupts::disable();
        IrqMutexGuard {
            guard: ManuallyDrop::new(self.inner.lock()),
            reenable: was_enabled,
        }
    }

    /// Non-blocking variant. Interrupts are restored immediately when the
    /// lock is already held, so a failed attempt costs nothing
    #[inline]
    pub fn try_lock(&self) -> Option<IrqMutexGuard<'_, T>> {
        let was_enabled = interrupts::are_enabled();
        interrupts::disable();
        match self.inner.try_lock() {
            Some(guard) => Some(IrqMutexGuard {
                guard: ManuallyDrop::new(guard),
                reenable: was_enabled,
            }),
            None => {
                if was_enabled {
                    interrupts::enable();
                }
                None
            }
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.inner.is_locked()
    }
}

pub struct IrqMutexGuard<'a, T: ?Sized + 'a> {
    guard: ManuallyDrop<MutexGuard<'a, T>>,
    reenable: bool,
}

impl<'a, T: ?Sized> Deref for IrqMutexGuard<'a, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.guard
    }
}

impl<'a, T: ?Sized> DerefMut for IrqMutexGuard<'a, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        &mut self.guard
    }
}

impl<'a, T: ?Sized> Drop for IrqMutexGuard<'a, T> {
    #[inline]
    fn drop(&mut self) {
        // Release the lock before letting interrupts back in, otherwise an
        // ISR could fire on this CPU while the lock still looks held
        unsafe { ManuallyDrop::drop(&mut self.guard) };
        if self.reenable {
            interrupts::enable();
        }
    }
}
