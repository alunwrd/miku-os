use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

    /// Takes the lock with no guard, for a hold spanning calls: `fork`, which
    /// takes the library's locks before the syscall and releases them after, in
    /// both processes
    pub fn lock_raw(&self) {
        core::mem::forget(self.lock());
    }

    /// Releases a `lock_raw`.
    ///
    /// # Safety
    /// The caller took it with `lock_raw` and holds it
    pub unsafe fn unlock_raw(&self) {
        self.locked.store(false, Ordering::Release);
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

/// A lock whose waiters sleep: `miku_mutex_*`'s three states below, after a
/// short spin. For a lock held across work that can be preempted
/// (`dev/std-architecture.md`, "The heap's thread caches")
pub struct Mutex<T> {
    m: MikuMutex,
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Send for Mutex<T> {}
unsafe impl<T: Send> Sync for Mutex<T> {}

/// Tries before a waiter sleeps. A running holder releases well within this
const SPIN_TRIES: u32 = 100;

impl<T> Mutex<T> {
    pub const fn new(val: T) -> Self {
        Self { m: MikuMutex { state: AtomicU32::new(MTX_FREE) }, data: UnsafeCell::new(val) }
    }

    pub fn lock(&self) -> MutexGuard<'_, T> {
        self.lock_raw();
        MutexGuard { lock: self }
    }

    /// Takes it with no guard, for `fork`, which holds the library's locks
    /// across the syscall and releases them in both processes
    pub fn lock_raw(&self) {
        let m = &self.m as *const MikuMutex as *mut MikuMutex;
        for _ in 0..SPIN_TRIES {
            if miku_mutex_trylock(m) {
                return;
            }
            core::hint::spin_loop();
        }
        miku_mutex_lock(m);
    }

    /// Releases a `lock_raw`.
    ///
    /// # Safety
    /// The caller took it with `lock_raw` and holds it
    pub unsafe fn unlock_raw(&self) {
        miku_mutex_unlock(&self.m as *const MikuMutex as *mut MikuMutex);
    }
}

pub struct MutexGuard<'a, T> {
    lock: &'a Mutex<T>,
}

impl<'a, T> core::ops::Deref for MutexGuard<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<'a, T> core::ops::DerefMut for MutexGuard<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<'a, T> Drop for MutexGuard<'a, T> {
    fn drop(&mut self) {
        unsafe { self.lock.unlock_raw() }
    }
}

// C-exported mutex API, futex-backed, with Drepper's three states. An
// uncontended unlock is one atomic store with no syscall, since the third state
// records that someone is waiting.
//
//   0  free
//   1  held, nobody waiting
//   2  held, and at least one thread is asleep on it

const MTX_FREE: u32 = 0;
const MTX_HELD: u32 = 1;
const MTX_CONTENDED: u32 = 2;

pub use miku_abi::libtypes::MikuMutex;

#[no_mangle]
pub extern "C" fn miku_mutex_init(m: *mut MikuMutex) {
    if m.is_null() { return; }
    unsafe { (*m).state = AtomicU32::new(MTX_FREE); }
}

#[no_mangle]
pub extern "C" fn miku_mutex_lock(m: *mut MikuMutex) {
    if m.is_null() { return; }
    let st = unsafe { &(*m).state };

    // The fast path, the only one an uncontended lock takes
    if st.compare_exchange(MTX_FREE, MTX_HELD, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        return;
    }

    // Contended. From here the lock is taken only as CONTENDED: taking it as
    // HELD with a sleeper present would unlock without waking them. The
    // pessimistic state costs one unneeded `futex_wake` and cannot lose a
    // waiter
    let mut c = st.swap(MTX_CONTENDED, Ordering::Acquire);
    while c != MTX_FREE {
        // A return of 0 means the sleep ended, not that the lock is free; the
        // swap below decides from the word
        crate::thread::miku_futex_wait(st.as_ptr() as *const u32, MTX_CONTENDED, 0);
        c = st.swap(MTX_CONTENDED, Ordering::Acquire);
    }
}

#[no_mangle]
pub extern "C" fn miku_mutex_unlock(m: *mut MikuMutex) {
    if m.is_null() { return; }
    let st = unsafe { &(*m).state };
    // `swap`: the previous value says if anyone needs waking, and a separate
    // read would leave a window where a waiter arrives unwoken
    if st.swap(MTX_FREE, Ordering::Release) == MTX_CONTENDED {
        crate::thread::miku_futex_wake(st.as_ptr() as *const u32, 1);
    }
}

#[no_mangle]
pub extern "C" fn miku_mutex_trylock(m: *mut MikuMutex) -> bool {
    if m.is_null() { return false; }
    unsafe {
        (*m).state.compare_exchange(
            MTX_FREE, MTX_HELD, Ordering::Acquire, Ordering::Relaxed
        ).is_ok()
    }
}

#[no_mangle]
pub extern "C" fn miku_mutex_is_locked(m: *const MikuMutex) -> bool {
    if m.is_null() { return false; }
    unsafe { (*m).state.load(Ordering::Relaxed) != MTX_FREE }
}

// C-exported atomic counter (useful for ref counting, etc.)

pub use miku_abi::libtypes::MikuAtomic;

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

pub use miku_abi::libtypes::MikuOnce;

#[no_mangle]
pub extern "C" fn miku_once_init(o: *mut MikuOnce) {
    if o.is_null() { return; }
    unsafe {
        (*o).done = AtomicBool::new(false);
        (*o).running = AtomicBool::new(false);
    }
}

// Runs the callback exactly once; concurrent callers spin until done
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
