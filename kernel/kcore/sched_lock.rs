// A mutex that yields the CPU instead of spinning on it.
//
// `spin::Mutex` suits a lock held for a handful of instructions. The VFS lock
// is held across block-device I/O: while one thread waits on a disk read, every
// other CPU wanting the filesystem burns its timeslice in a `pause` loop, and
// on a single CPU a lower-priority holder cannot be scheduled to finish.
//
// `SchedMutex` keeps a test-and-set core and, on contention, spins for a moment
// and then sleeps on the lock's own queue until the holder releases it. Before
// the scheduler exists (early boot), and in the idle task, which has nowhere to
// sleep to, it spins and yields. A task already marked dead spins and never
// yields: the scheduler does not resume a dead task, so one releasing the
// processor here would leave what it was finishing (an exit closing its
// descriptors) undone.
//
// No wake is lost between looking and sleeping: a waiter counts itself in
// `waiting`, joins the queue and asks to sleep before it looks at `locked` a
// last time, and the unlock clears `locked` before it reads `waiting`
// (`sched::sleep_until_unless` has the other half). All sequentially
// consistent, the store-then-load order both sides depend on. A backstop bounds
// the sleep anyway.
//
// Not interrupt-safe: never take one of these from an interrupt handler. Locks
// shared with interrupt context use 'kcore::irq_lock::IrqMutex'.
//
// Every one keeps statistics (`LockStats`, `/proc/lockstat`): how often it was
// taken, waited for and slept on, and with `lockstat` on the boot line how long
// it was held and where (`dev/kernel-core.md`, "Lock statistics").

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::panic::Location;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};

use crate::sched::WaitQueue;

/// How many times a waiter spins before it sleeps. Short critical sections
/// finish inside this window with no context switch; ring 0 is not preempted,
/// so a holder is on a processor until it releases or sleeps
const SPIN_BEFORE_SLEEP: u32 = 256;

/// The longest one sleep waits for an unlock that did not wake it, in ns.
/// Matters only on a bug
const BACKSTOP_NS: u64 = 20_000_000;

pub struct SchedMutex<T: ?Sized> {
    locked: AtomicBool,
    /// Waiters between counting themselves in and leaving the queue: what an
    /// unlock reads to know there is anyone to wake
    waiting: AtomicU32,
    waiters: WaitQueue,
    /// What it has cost, for observation
    stats: LockStats,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for SchedMutex<T> {}
unsafe impl<T: ?Sized + Send> Sync for SchedMutex<T> {}

impl<T> SchedMutex<T> {
    pub const fn new(value: T) -> Self {
        Self::named("?", value)
    }

    /// A lock `/proc/lockstat` lists as `name`
    pub const fn named(name: &'static str, value: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            waiting: AtomicU32::new(0),
            waiters: WaitQueue::new(),
            stats: LockStats::new(name),
            data: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> SchedMutex<T> {
    #[track_caller]
    pub fn lock(&self) -> SchedMutexGuard<'_, T> {
        let mut spins = 0u32;
        let mut waited_from = 0u64;
        loop {
            if self
                .locked
                .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_ok()
            {
                self.stats.taken(Location::caller(), waited_from);
                return SchedMutexGuard { lock: self };
            }
            if waited_from == 0 {
                waited_from = self.stats.waiting();
            }
            while self.locked.load(Ordering::Relaxed) {
                spins += 1;
                if spins < SPIN_BEFORE_SLEEP || !crate::sched::started() {
                    core::hint::spin_loop();
                    continue;
                }
                spins = 0;
                if crate::sched::current_is_dead() {
                    // Off the processor it would stay off: a dead task spins to
                    // the end of what it is finishing
                    core::hint::spin_loop();
                    continue;
                }
                self.stats.slept.fetch_add(1, Ordering::Relaxed);
                if crate::sched::current_is_idle() {
                    crate::sched::yield_now();
                } else {
                    self.sleep_while_locked();
                }
            }
        }
    }

    /// Off the processor until an unlock wakes this waiter, or not at all if
    /// the lock is free by the time the sleep is asked for
    #[cold]
    fn sleep_while_locked(&self) {
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let deadline = crate::kcore::time::now_ns().saturating_add(BACKSTOP_NS);
        self.waiters.wait_unless(deadline, || !self.locked.load(Ordering::SeqCst));
        self.waiting.fetch_sub(1, Ordering::SeqCst);
    }

    fn unlock(&self) {
        self.stats.released();
        self.locked.store(false, Ordering::SeqCst);
        if self.waiting.load(Ordering::SeqCst) != 0 {
            self.waiters.wake_one();
        }
    }

    #[track_caller]
    pub fn try_lock(&self) -> Option<SchedMutexGuard<'_, T>> {
        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            self.stats.taken(Location::caller(), 0);
            Some(SchedMutexGuard { lock: self })
        } else {
            None
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    /// Times a waiter went to sleep waiting for this lock. Quick growth is the
    /// signal to split the lock
    #[inline]
    pub fn contention_yields(&self) -> u64 {
        self.stats.slept.load(Ordering::Relaxed)
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
        self.lock.unlock();
    }
}

// Statistics

/// What one lock has cost. The counts always; the times only with `lockstat` on
/// the boot line, when every take and release reads the clock.
///
/// A lock many may hold to read, or one to write (`dev/vfs-locking.md`, "4c").
/// `SchedMutex` for data mostly read: the VFS's tree, walked by every `stat`
/// and changed by far fewer calls.
///
/// Writers come first: a reader finding a writer waiting waits behind it, so a
/// stream of readers cannot keep a writer out. Waiters spin a little and then
/// sleep on one queue, and every release that may let one in wakes them all to
/// look again; there are a handful at most.
///
/// Neither half nests: a task holding it in either mode that asks again waits
/// for itself.
///
/// `/proc/lockstat` counts both kinds of take and their waits; hold times and
/// the longest hold are the writers' (readers overlap, and one clock per lock
/// cannot time them)
pub struct SchedRwLock<T: ?Sized> {
    /// Readers holding it, or `RW_WRITER` while a writer does
    state: AtomicU32,
    /// Writers waiting for it: new readers wait while there are any
    writers_waiting: AtomicU32,
    /// Tasks asleep on `waiters`, for a release to know whether to wake
    waiting: AtomicU32,
    waiters: WaitQueue,
    stats: LockStats,
    data: UnsafeCell<T>,
}

const RW_WRITER: u32 = 1 << 31;

unsafe impl<T: ?Sized + Send> Send for SchedRwLock<T> {}
unsafe impl<T: ?Sized + Send + Sync> Sync for SchedRwLock<T> {}

impl<T> SchedRwLock<T> {
    /// A lock `/proc/lockstat` lists as `name`
    pub const fn named(name: &'static str, value: T) -> Self {
        Self {
            state: AtomicU32::new(0),
            writers_waiting: AtomicU32::new(0),
            waiting: AtomicU32::new(0),
            waiters: WaitQueue::new(),
            stats: LockStats::new(name),
            data: UnsafeCell::new(value),
        }
    }
}

impl<T: ?Sized> SchedRwLock<T> {
    /// Held to read, alongside any other reader
    #[track_caller]
    pub fn read(&self) -> SchedRwReadGuard<'_, T> {
        let mut waited_from = 0u64;
        let mut spins = 0u32;
        loop {
            let s = self.state.load(Ordering::Relaxed);
            if s & RW_WRITER == 0
                && self.writers_waiting.load(Ordering::Relaxed) == 0
                && self.state.compare_exchange_weak(s, s + 1, Ordering::Acquire, Ordering::Relaxed).is_ok()
            {
                self.stats.taken_shared(waited_from);
                return SchedRwReadGuard { lock: self };
            }
            if waited_from == 0 {
                waited_from = self.stats.waiting();
            }
            self.pause(&mut spins, || {
                self.state.load(Ordering::SeqCst) & RW_WRITER == 0 && self.writers_waiting.load(Ordering::SeqCst) == 0
            });
        }
    }

    /// Held alone, to write
    #[track_caller]
    pub fn lock(&self) -> SchedRwWriteGuard<'_, T> {
        if self.state.compare_exchange(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            self.stats.taken(Location::caller(), 0);
            return SchedRwWriteGuard { lock: self };
        }
        let waited_from = self.stats.waiting();
        self.writers_waiting.fetch_add(1, Ordering::SeqCst);
        let mut spins = 0u32;
        loop {
            if self.state.compare_exchange_weak(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed).is_ok() {
                self.writers_waiting.fetch_sub(1, Ordering::SeqCst);
                self.stats.taken(Location::caller(), waited_from);
                return SchedRwWriteGuard { lock: self };
            }
            self.pause(&mut spins, || self.state.load(Ordering::SeqCst) == 0);
        }
    }

    /// Held alone if nobody holds it now
    #[track_caller]
    pub fn try_lock(&self) -> Option<SchedRwWriteGuard<'_, T>> {
        if self.state.compare_exchange(0, RW_WRITER, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            self.stats.taken(Location::caller(), 0);
            Some(SchedRwWriteGuard { lock: self })
        } else {
            None
        }
    }

    /// A moment's spin, then asleep until a release wakes this waiter or `free`
    /// says it may go, as `SchedMutex::lock` does
    fn pause(&self, spins: &mut u32, free: impl Fn() -> bool) {
        *spins += 1;
        if *spins < SPIN_BEFORE_SLEEP || !crate::sched::started() || crate::sched::current_is_dead() {
            core::hint::spin_loop();
            return;
        }
        *spins = 0;
        self.stats.slept.fetch_add(1, Ordering::Relaxed);
        if crate::sched::current_is_idle() {
            crate::sched::yield_now();
            return;
        }
        self.waiting.fetch_add(1, Ordering::SeqCst);
        let deadline = crate::kcore::time::now_ns().saturating_add(BACKSTOP_NS);
        self.waiters.wait_unless(deadline, free);
        self.waiting.fetch_sub(1, Ordering::SeqCst);
    }

    fn wake(&self) {
        if self.waiting.load(Ordering::SeqCst) != 0 {
            self.waiters.wake_all();
        }
    }

    fn read_unlock(&self) {
        // The last reader out may be what a writer waits for
        if self.state.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.wake();
        }
    }

    fn write_unlock(&self) {
        self.stats.released();
        self.state.store(0, Ordering::SeqCst);
        self.wake();
    }
}

pub struct SchedRwReadGuard<'a, T: ?Sized + 'a> {
    lock: &'a SchedRwLock<T>,
}

impl<T: ?Sized> Deref for SchedRwReadGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SchedRwReadGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.read_unlock();
    }
}

pub struct SchedRwWriteGuard<'a, T: ?Sized + 'a> {
    lock: &'a SchedRwLock<T>,
}

impl<T: ?Sized> Deref for SchedRwWriteGuard<'_, T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T: ?Sized> DerefMut for SchedRwWriteGuard<'_, T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T: ?Sized> Drop for SchedRwWriteGuard<'_, T> {
    #[inline]
    fn drop(&mut self) {
        self.lock.write_unlock();
    }
}

pub struct LockStats {
    name: &'static str,
    registered: AtomicBool,
    taken: AtomicU64,
    /// Takes that found it held and waited, spinning or asleep
    contended: AtomicU64,
    /// Waits that went to sleep
    slept: AtomicU64,
    wait_ns: AtomicU64,
    /// When the holder took it, and where
    since: AtomicU64,
    site: AtomicPtr<Location<'static>>,
    longest: AtomicU64,
    longest_site: AtomicPtr<Location<'static>>,
    /// Holds under 1 µs, 10 µs, 100 µs, 1 ms, and longer
    holds: [AtomicU64; 5],
    /// When this lock's last long hold was logged
    last_long: AtomicU64,
}

/// `lockstat`: 0 not read yet, 1 off, 2 on
static TIMING: AtomicU8 = AtomicU8::new(0);

fn timing() -> bool {
    match TIMING.load(Ordering::Relaxed) {
        // Not before the command line can be read: the first locks are taken
        // before it is
        0 if !crate::kcore::boot_state::is_done() => false,
        0 => {
            let on = crate::arch::x86_64::grub::cmdline_flag("lockstat");
            TIMING.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        v => v == 2,
    }
}

/// The locks taken at least once, for `/proc/lockstat`
const MAX_LOCKS: usize = 128;
static REGISTRY: [AtomicPtr<LockStats>; MAX_LOCKS] = [const { AtomicPtr::new(core::ptr::null_mut()) }; MAX_LOCKS];
static REGISTERED: AtomicUsize = AtomicUsize::new(0);


/// A hold this long is logged with its place
const LONG_HOLD_NS: u64 = 1_000_000;

impl LockStats {
    const fn new(name: &'static str) -> Self {
        Self {
            name,
            registered: AtomicBool::new(false),
            taken: AtomicU64::new(0),
            contended: AtomicU64::new(0),
            slept: AtomicU64::new(0),
            wait_ns: AtomicU64::new(0),
            since: AtomicU64::new(0),
            site: AtomicPtr::new(core::ptr::null_mut()),
            longest: AtomicU64::new(0),
            longest_site: AtomicPtr::new(core::ptr::null_mut()),
            holds: [const { AtomicU64::new(0) }; 5],
            last_long: AtomicU64::new(0),
        }
    }

    /// A taker found it held: when the wait started, or 1 when nobody is timing
    /// (any non-zero means it waited)
    fn waiting(&self) -> u64 {
        self.contended.fetch_add(1, Ordering::Relaxed);
        if timing() { crate::kcore::time::now_ns().max(1) } else { 1 }
    }

    #[inline]
    fn taken(&self, site: &'static Location<'static>, waited_from: u64) {
        self.taken.fetch_add(1, Ordering::Relaxed);
        if !self.registered.load(Ordering::Relaxed) {
            self.register();
        }
        if timing() {
            let now = crate::kcore::time::now_ns();
            if waited_from > 1 {
                self.wait_ns.fetch_add(now.saturating_sub(waited_from), Ordering::Relaxed);
            }
            self.since.store(now, Ordering::Relaxed);
            self.site.store(site as *const _ as *mut _, Ordering::Relaxed);
        }
    }

    /// A reader got in: counted, its wait timed, its hold not
    #[inline]
    fn taken_shared(&self, waited_from: u64) {
        self.taken.fetch_add(1, Ordering::Relaxed);
        if !self.registered.load(Ordering::Relaxed) {
            self.register();
        }
        if waited_from > 1 && timing() {
            let now = crate::kcore::time::now_ns();
            self.wait_ns.fetch_add(now.saturating_sub(waited_from), Ordering::Relaxed);
        }
    }

    #[inline]
    fn released(&self) {
        let since = self.since.load(Ordering::Relaxed);
        if since == 0 || !timing() {
            return;
        }
        self.since.store(0, Ordering::Relaxed);
        let held = crate::kcore::time::now_ns().saturating_sub(since);
        let bucket = match held {
            0..=999 => 0,
            1_000..=9_999 => 1,
            10_000..=99_999 => 2,
            100_000..=999_999 => 3,
            _ => 4,
        };
        self.holds[bucket].fetch_add(1, Ordering::Relaxed);
        let site = self.site.load(Ordering::Relaxed);
        if held > self.longest.fetch_max(held, Ordering::Relaxed) {
            self.longest_site.store(site, Ordering::Relaxed);
        }
        if held >= LONG_HOLD_NS {
            let now = since + held;
            // Ten a second for each lock, so one often long does not hide
            // another's
            let last = self.last_long.load(Ordering::Relaxed);
            if now.saturating_sub(last) >= 100_000_000
                && self.last_long.compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed).is_ok()
            {
                // SAFETY: only ever a `&'static Location`
                let at = unsafe { site.as_ref() };
                crate::kwarn!("[lockstat] {} held {} us, taken at {}", self.name, held / 1000,
                    at.map_or(alloc::format!("?"), |l| alloc::format!("{}:{}", l.file(), l.line())));
            }
        }
    }

    #[cold]
    fn register(&self) {
        if self.registered.swap(true, Ordering::AcqRel) {
            return;
        }
        let i = REGISTERED.fetch_add(1, Ordering::AcqRel);
        if i < MAX_LOCKS {
            REGISTRY[i].store(self as *const _ as *mut _, Ordering::Release);
        }
    }
}

/// `/proc/lockstat`: one line a name, locks of one name added together
pub fn report(out: &mut dyn core::fmt::Write) {
    use core::fmt::Write;
    let n = REGISTERED.load(Ordering::Acquire).min(MAX_LOCKS);
    let _ = writeln!(out, "# name taken contended slept wait_us longest_us longest_at | holds <1us <10us <100us <1ms >=1ms{}",
        if timing() { "" } else { " (times need lockstat on the boot line)" });
    let mut done: alloc::vec::Vec<&'static str> = alloc::vec::Vec::new();
    for i in 0..n {
        // SAFETY: registered locks are statics, or live in tables that are
        let Some(first) = (unsafe { REGISTRY[i].load(Ordering::Acquire).as_ref() }) else { continue };
        if done.contains(&first.name) {
            continue;
        }
        done.push(first.name);
        let (mut taken, mut contended, mut slept, mut wait, mut longest) = (0u64, 0u64, 0u64, 0u64, 0u64);
        let mut longest_site: *mut Location<'static> = core::ptr::null_mut();
        let mut holds = [0u64; 5];
        for j in i..n {
            let Some(s) = (unsafe { REGISTRY[j].load(Ordering::Acquire).as_ref() }) else { continue };
            if s.name != first.name {
                continue;
            }
            taken += s.taken.load(Ordering::Relaxed);
            contended += s.contended.load(Ordering::Relaxed);
            slept += s.slept.load(Ordering::Relaxed);
            wait += s.wait_ns.load(Ordering::Relaxed);
            let l = s.longest.load(Ordering::Relaxed);
            if l > longest {
                longest = l;
                longest_site = s.longest_site.load(Ordering::Relaxed);
            }
            for (b, h) in holds.iter_mut().zip(s.holds.iter()) {
                *b += h.load(Ordering::Relaxed);
            }
        }
        // SAFETY: only ever a `&'static Location`
        let at = unsafe { longest_site.as_ref() };
        let _ = write!(out, "{} {} {} {} {} {} ", first.name, taken, contended, slept, wait / 1000, longest / 1000);
        let _ = match at {
            Some(l) => write!(out, "{}:{}", l.file(), l.line()),
            None => write!(out, "-"),
        };
        let _ = writeln!(out, " | {} {} {} {} {}", holds[0], holds[1], holds[2], holds[3], holds[4]);
    }
}
