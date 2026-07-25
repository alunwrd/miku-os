// shim for `sync`: SpinLock/SpinGuard are generic, so they are duplicated
// here by value (pure code, per-lock state); the C-ABI mutex/atomic/once
// objects resolve to core_miku.so at load time via ld-miku.

use core::sync::atomic::{AtomicBool, Ordering};
use core::cell::UnsafeCell;

pub struct SpinLock<T> {
    locked: AtomicBool,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for SpinLock<T> {}
unsafe impl<T: Send> Sync for SpinLock<T> {}

impl<T> SpinLock<T> {
    pub const fn new(val: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(val),
        }
    }

    pub fn lock(&self) -> SpinGuard<'_, T> {
        while self.locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
        SpinGuard { lock: self }
    }
}

pub struct SpinGuard<'a, T> {
    lock: &'a SpinLock<T>,
}

impl<'a, T> core::ops::Deref for SpinGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for SpinGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for SpinGuard<'a, T> {
    fn drop(&mut self) {
        self.lock.locked.store(false, Ordering::Release);
    }
}
