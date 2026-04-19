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

// C-exported mutex API (opaque spinlock-based mutex)

#[repr(C)]
pub struct MikuMutex {
    locked: AtomicBool,
}

#[no_mangle]
pub extern "C" fn miku_mutex_init(m: *mut MikuMutex) {
    if m.is_null() { return; }
    unsafe { (*m).locked = AtomicBool::new(false); }
}

#[no_mangle]
pub extern "C" fn miku_mutex_lock(m: *mut MikuMutex) {
    if m.is_null() { return; }
    unsafe {
        while (*m).locked.compare_exchange_weak(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_err() {
            while (*m).locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn miku_mutex_unlock(m: *mut MikuMutex) {
    if m.is_null() { return; }
    unsafe { (*m).locked.store(false, Ordering::Release); }
}

#[no_mangle]
pub extern "C" fn miku_mutex_trylock(m: *mut MikuMutex) -> bool {
    if m.is_null() { return false; }
    unsafe {
        (*m).locked.compare_exchange(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_ok()
    }
}

#[no_mangle]
pub extern "C" fn miku_mutex_is_locked(m: *const MikuMutex) -> bool {
    if m.is_null() { return false; }
    unsafe { (*m).locked.load(Ordering::Relaxed) }
}

// C-exported atomic counter (useful for ref counting, etc.)

#[repr(C)]
pub struct MikuAtomic {
    val: core::sync::atomic::AtomicI64,
}

#[no_mangle]
pub extern "C" fn miku_atomic_init(a: *mut MikuAtomic, val: i64) {
    if a.is_null() { return; }
    unsafe { (*a).val = core::sync::atomic::AtomicI64::new(val); }
}

#[no_mangle]
pub extern "C" fn miku_atomic_load(a: *const MikuAtomic) -> i64 {
    if a.is_null() { return 0; }
    unsafe { (*a).val.load(Ordering::SeqCst) }
}

#[no_mangle]
pub extern "C" fn miku_atomic_store(a: *mut MikuAtomic, val: i64) {
    if a.is_null() { return; }
    unsafe { (*a).val.store(val, Ordering::SeqCst); }
}

#[no_mangle]
pub extern "C" fn miku_atomic_add(a: *mut MikuAtomic, val: i64) -> i64 {
    if a.is_null() { return 0; }
    unsafe { (*a).val.fetch_add(val, Ordering::SeqCst) }
}

#[no_mangle]
pub extern "C" fn miku_atomic_sub(a: *mut MikuAtomic, val: i64) -> i64 {
    if a.is_null() { return 0; }
    unsafe { (*a).val.fetch_sub(val, Ordering::SeqCst) }
}

#[no_mangle]
pub extern "C" fn miku_atomic_cas(a: *mut MikuAtomic, expected: i64, desired: i64) -> bool {
    if a.is_null() { return false; }
    unsafe {
        (*a).val.compare_exchange(
            expected, desired, Ordering::SeqCst, Ordering::Relaxed
        ).is_ok()
    }
}

#[no_mangle]
pub extern "C" fn miku_atomic_swap(a: *mut MikuAtomic, val: i64) -> i64 {
    if a.is_null() { return 0; }
    unsafe { (*a).val.swap(val, Ordering::SeqCst) }
}

// once-init flag

#[repr(C)]
pub struct MikuOnce {
    done: AtomicBool,
    running: AtomicBool,
}

#[no_mangle]
pub extern "C" fn miku_once_init(o: *mut MikuOnce) {
    if o.is_null() { return; }
    unsafe {
        (*o).done = AtomicBool::new(false);
        (*o).running = AtomicBool::new(false);
    }
}

// run callback exactly once; concurrent callers spin until done
#[no_mangle]
pub extern "C" fn miku_once_call(o: *mut MikuOnce, f: extern "C" fn()) {
    if o.is_null() { return; }
    unsafe {
        if (*o).done.load(Ordering::Acquire) { return; }
        if (*o).running.compare_exchange(
            false, true, Ordering::Acquire, Ordering::Relaxed
        ).is_ok() {
            f();
            (*o).done.store(true, Ordering::Release);
        } else {
            while !(*o).done.load(Ordering::Acquire) {
                core::hint::spin_loop();
            }
        }
    }
}

#[no_mangle]
pub extern "C" fn miku_once_done(o: *const MikuOnce) -> bool {
    if o.is_null() { return false; }
    unsafe { (*o).done.load(Ordering::Acquire) }
}
