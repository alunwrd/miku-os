//! POSIX threads for C, over `thread_create`(106) and `futex`(110), the same
//! design as `lib/rust-std/library/std/src/sys/thread/miku.rs`.
//!
//! A thread's `pthread_t` is its control block: one mapping holding the join
//! word, return value, and thread-specific values of every key. The block
//! outlives the stack, which the kernel unmaps at thread death
//! (`THREAD_STACK_OWNED`); the last of the thread and its joiner unmaps it. A
//! thread finds its block through the TCB word at `tp + 48`
//! (`docs/MikuOS_ABI.md` §3.2b). Threads not created here (the main thread,
//! `std::thread`s) get a block on first request, never freed.
//!
//! Every object is valid all-zero, as `PTHREAD_*_INITIALIZER`s are, so `init`
//! functions write only what an attribute asked. Sizes are the `libc` module's
//! (`lib/c/check/layout.c` pins the header).
//!
//! Scheduling is `SCHED_OTHER` only (`pthread_setschedparam` below).
//!
//! Absent: cancellation, scheduling attributes, process-shared objects, robust
//! mutexes.

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};

use crate::libc::timespec;

type ThreadFn = extern "C" fn(*mut u8) -> i32;

extern "C" {
    fn miku_thread_spawn_owned(entry: ThreadFn, arg: *mut u8, stack: *mut u8, size: usize) -> i64;
    fn miku_thread_exit(code: i32) -> !;
    fn miku_futex_wait(addr: *const u32, expected: u32, timeout_ns: u64) -> i64;
    fn miku_futex_wake(addr: *const u32, count: u32) -> i64;
    fn miku_mmap(addr: u64, len: usize, prot: u64) -> *mut u8;
    fn miku_munmap(addr: *mut u8, len: usize) -> i64;
}

const EINVAL: i32 = 22;
const EBUSY: i32 = 16;
const EAGAIN: i32 = 11;
const EDEADLK: i32 = 35;
const EPERM: i32 = 1;
const ETIMEDOUT: i32 = 110;
const ESRCH: i32 = 3;

const KEYS_MAX: usize = 128;
const TCB_PTHREAD: usize = 48;
const DEFAULT_STACK: usize = 1024 * 1024;
/// `lib/libmiku/thread.rs`'s guard page, which the mapping is sized for
const GUARD: usize = 4096;

fn wait(word: &AtomicU32, expected: u32, timeout_ns: u64) -> i64 {
    unsafe { miku_futex_wait(word.as_ptr(), expected, timeout_ns) }
}

fn wake(word: &AtomicU32, n: u32) {
    unsafe { miku_futex_wake(word.as_ptr(), n) };
}

fn tp() -> usize {
    let tp: usize;
    unsafe { core::arch::asm!("mov {}, fs:0", out(reg) tp, options(nostack, readonly, preserves_flags)) };
    tp
}

// threads

#[repr(C)]
struct Block {
    /// 0 while the thread runs, 1 once it has finished. Futex-waited on
    done: AtomicU32,
    /// The thread and its handle; the last one out unmaps. A thread not made
    /// here holds 1 permanently
    refs: AtomicUsize,
    ret: AtomicUsize,
    start: usize,
    arg: usize,
    specific: [AtomicUsize; KEYS_MAX],
}

const BLOCK_LEN: usize = (core::mem::size_of::<Block>() + 4095) & !4095;

fn new_block() -> *mut Block {
    let p = unsafe { miku_mmap(0, BLOCK_LEN, 3) } as *mut Block;
    if p.is_null() {
        return p;
    }
    // Zero from the kernel; refs is the only non-zero start
    unsafe { (*p).refs.store(2, Ordering::Relaxed) };
    p
}

unsafe fn release(b: *mut Block) {
    if (*b).refs.fetch_sub(1, Ordering::AcqRel) == 1 {
        miku_munmap(b as *mut u8, BLOCK_LEN);
    }
}

/// This thread's block, made on first use for a thread `pthread_create` did not
/// start
fn current() -> *mut Block {
    let slot = (tp() + TCB_PTHREAD) as *mut usize;
    let b = unsafe { *slot } as *mut Block;
    if !b.is_null() {
        return b;
    }
    let b = new_block();
    if !b.is_null() {
        unsafe {
            (*b).refs.store(1, Ordering::Relaxed);
            *slot = b as usize;
        }
    }
    b
}

extern "C" fn thread_start(arg: *mut u8) -> i32 {
    let b = arg as *mut Block;
    unsafe {
        *((tp() + TCB_PTHREAD) as *mut usize) = b as usize;
        let f: extern "C" fn(*mut u8) -> *mut u8 = core::mem::transmute((*b).start);
        let r = f((*b).arg as *mut u8);
        finish(b, r)
    }
}

unsafe fn finish(b: *mut Block, r: *mut u8) -> ! {
    run_destructors(b);
    (*b).ret.store(r as usize, Ordering::Relaxed);
    (*b).done.store(1, Ordering::Release);
    wake(&(*b).done, u32::MAX);
    release(b);
    miku_thread_exit(0)
}

#[no_mangle]
pub unsafe extern "C" fn pthread_create(
    out: *mut usize,
    attr: *const [u64; 7],
    start: extern "C" fn(*mut u8) -> *mut u8,
    arg: *mut u8,
) -> i32 {
    let mut stack = DEFAULT_STACK;
    let mut detached = false;
    if !attr.is_null() {
        if (*attr)[0] != 0 {
            stack = (*attr)[0] as usize;
        }
        detached = (*attr)[1] != 0;
    }
    let stack = stack.max(64 * 1024).next_multiple_of(4096);

    let b = new_block();
    if b.is_null() {
        return EAGAIN;
    }
    (*b).start = start as usize;
    (*b).arg = arg as usize;
    // Size asked for, plus the guard page `miku_thread_spawn_owned` makes of
    // the mapping's bottom
    let len = stack + GUARD;
    let mem = miku_mmap(0, len, 3);
    if mem.is_null() {
        miku_munmap(b as *mut u8, BLOCK_LEN);
        return EAGAIN;
    }
    let tid = miku_thread_spawn_owned(thread_start, b as *mut u8, mem, len);
    if tid < 0 {
        miku_munmap(mem, len);
        miku_munmap(b as *mut u8, BLOCK_LEN);
        return EAGAIN;
    }
    if detached {
        release(b);
    }
    *out = b as usize;
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_join(t: usize, ret: *mut *mut u8) -> i32 {
    let b = t as *mut Block;
    if b.is_null() {
        return ESRCH;
    }
    if b == current() {
        return EDEADLK;
    }
    while (*b).done.load(Ordering::Acquire) == 0 {
        wait(&(*b).done, 0, 0);
    }
    if !ret.is_null() {
        *ret = (*b).ret.load(Ordering::Relaxed) as *mut u8;
    }
    release(b);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_detach(t: usize) -> i32 {
    if t == 0 {
        return ESRCH;
    }
    release(t as *mut Block);
    0
}

#[no_mangle]
pub extern "C" fn pthread_self() -> usize {
    current() as usize
}

#[no_mangle]
pub extern "C" fn pthread_equal(a: usize, b: usize) -> i32 {
    (a == b) as i32
}

/// Ends a thread `pthread_create` started, with its value for a joiner. From
/// another thread it runs the key destructors and ends that thread; from the
/// main thread, by the kernel's rule, the whole process
#[no_mangle]
pub unsafe extern "C" fn pthread_exit(r: *mut u8) -> ! {
    let b = current();
    if !b.is_null() && (*b).start != 0 {
        finish(b, r)
    }
    if !b.is_null() {
        run_destructors(b);
    }
    miku_thread_exit(0)
}

// attributes: [stack size, detached, ...]

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_init(a: *mut [u64; 7]) -> i32 {
    *a = [0; 7];
    0
}

#[no_mangle]
pub extern "C" fn pthread_attr_destroy(_a: *mut [u64; 7]) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setstacksize(a: *mut [u64; 7], size: usize) -> i32 {
    if size < 16384 {
        return EINVAL;
    }
    (*a)[0] = size as u64;
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_getstacksize(a: *const [u64; 7], size: *mut usize) -> i32 {
    *size = if (*a)[0] == 0 { DEFAULT_STACK } else { (*a)[0] as usize };
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_attr_setdetachstate(a: *mut [u64; 7], state: i32) -> i32 {
    match state {
        0 | 1 => {
            (*a)[1] = state as u64;
            0
        }
        _ => EINVAL,
    }
}

// mutexes: [state u32 | kind u32, owner, depth, -, -]
//
// state is Drepper's: 0 free, 1 held, 2 held with waiters. The owner is the
// holder's thread pointer, unique among live threads, kept only for kinds
// needing it

const NORMAL: u32 = 0;
const RECURSIVE: u32 = 1;
const ERRORCHECK: u32 = 2;

#[repr(C)]
pub struct Mutex {
    state: AtomicU32,
    kind: AtomicU32,
    owner: AtomicUsize,
    depth: AtomicU64,
    _pad: [u64; 2],
}

fn lock_word(state: &AtomicU32) {
    if state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        return;
    }
    // Contended: marks it and sleeps until the holder's unlock wakes us
    while state.swap(2, Ordering::Acquire) != 0 {
        wait(state, 2, 0);
    }
}

fn unlock_word(state: &AtomicU32) {
    if state.swap(0, Ordering::Release) == 2 {
        wake(state, 1);
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_init(m: *mut Mutex, attr: *const [u64; 4]) -> i32 {
    core::ptr::write_bytes(m, 0, 1);
    if !attr.is_null() {
        (*m).kind.store((*attr)[0] as u32, Ordering::Relaxed);
    }
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutex_destroy(_m: *mut Mutex) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_lock(m: *mut Mutex) -> i32 {
    let m = &*m;
    let kind = m.kind.load(Ordering::Relaxed);
    if kind != NORMAL {
        let me = tp();
        if m.owner.load(Ordering::Relaxed) == me {
            if kind == RECURSIVE {
                m.depth.fetch_add(1, Ordering::Relaxed);
                return 0;
            }
            return EDEADLK;
        }
        lock_word(&m.state);
        m.owner.store(me, Ordering::Relaxed);
        m.depth.store(1, Ordering::Relaxed);
        return 0;
    }
    lock_word(&m.state);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_trylock(m: *mut Mutex) -> i32 {
    let m = &*m;
    let kind = m.kind.load(Ordering::Relaxed);
    let me = tp();
    if kind == RECURSIVE && m.owner.load(Ordering::Relaxed) == me {
        m.depth.fetch_add(1, Ordering::Relaxed);
        return 0;
    }
    if m.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Relaxed).is_err() {
        return EBUSY;
    }
    if kind != NORMAL {
        m.owner.store(me, Ordering::Relaxed);
        m.depth.store(1, Ordering::Relaxed);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutex_unlock(m: *mut Mutex) -> i32 {
    let m = &*m;
    if m.kind.load(Ordering::Relaxed) != NORMAL {
        if m.owner.load(Ordering::Relaxed) != tp() {
            return EPERM;
        }
        if m.depth.fetch_sub(1, Ordering::Relaxed) > 1 {
            return 0;
        }
        m.owner.store(0, Ordering::Relaxed);
    }
    unlock_word(&m.state);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_init(a: *mut [u64; 4]) -> i32 {
    *a = [0; 4];
    0
}

#[no_mangle]
pub extern "C" fn pthread_mutexattr_destroy(_a: *mut [u64; 4]) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_settype(a: *mut [u64; 4], kind: i32) -> i32 {
    match kind as u32 {
        NORMAL | RECURSIVE | ERRORCHECK => {
            (*a)[0] = kind as u64;
            0
        }
        _ => EINVAL,
    }
}

/// Also process-shared. A lock word sleeps through `futex`(110), and the kernel
/// keys a word in a `MAP_SHARED` tmpfs page on the file's frame
/// (`sched::futex`), shared by every process mapping the file. LMDB keeps its
/// reader-table mutex in such a file (`lock.mdb`).
///
/// Does not hold for a mapping that is not one frame for all: on ext2,
/// `MAP_SHARED` is a copy per mapping. And only a `NORMAL` mutex works across
/// processes, since the recorded owner is a per-address-space thread pointer
#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_setpshared(a: *mut [u64; 4], pshared: i32) -> i32 {
    const PRIVATE: i32 = 0;
    const SHARED: i32 = 1;
    match pshared {
        PRIVATE | SHARED => {
            if !a.is_null() {
                (*a)[1] = pshared as u64;
            }
            0
        }
        _ => EINVAL,
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_mutexattr_getpshared(a: *const [u64; 4], out: *mut i32) -> i32 {
    if !out.is_null() {
        *out = if a.is_null() { 0 } else { (*a)[1] as i32 };
    }
    0
}

// condition variables: [seq u32, ...]
//
// A waiter reads the sequence, releases the mutex, and sleeps on the sequence
// staying what it read. A signal between the unlock and the sleep moves it, so
// the sleep returns at once

#[repr(C)]
pub struct Cond {
    seq: AtomicU32,
    /// Clock of `pthread_cond_timedwait`'s deadline: 0 is `CLOCK_REALTIME`, so
    /// all-zero `PTHREAD_COND_INITIALIZER` is POSIX's default; 1 is
    /// `CLOCK_MONOTONIC`, from the condattr
    clock: AtomicU32,
    _pad: [u32; 10],
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_init(c: *mut Cond, attr: *const [u64; 4]) -> i32 {
    core::ptr::write_bytes(c, 0, 1);
    if !attr.is_null() {
        (*c).clock.store((*attr)[0] as u32, Ordering::Relaxed);
    }
    0
}

#[no_mangle]
pub extern "C" fn pthread_cond_destroy(_c: *mut Cond) -> i32 {
    0
}

unsafe fn cond_wait(c: *mut Cond, m: *mut Mutex, timeout_ns: u64) -> i32 {
    let seq = (*c).seq.load(Ordering::Relaxed);
    // A recursive mutex is released and retaken whole, whatever its depth
    let depth = (*m).depth.swap(1, Ordering::Relaxed);
    let r = pthread_mutex_unlock(m);
    if r != 0 {
        (*m).depth.store(depth, Ordering::Relaxed);
        return r;
    }
    let w = wait(&(*c).seq, seq, timeout_ns);
    pthread_mutex_lock(m);
    (*m).depth.store(depth, Ordering::Relaxed);
    if w == -(ETIMEDOUT as i64) { ETIMEDOUT } else { 0 }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_wait(c: *mut Cond, m: *mut Mutex) -> i32 {
    cond_wait(c, m, 0)
}

/// `abstime` is on the condattr's clock: `CLOCK_REALTIME` unless it chose
/// `CLOCK_MONOTONIC`, as `parking_lot` does
#[no_mangle]
pub unsafe extern "C" fn pthread_cond_timedwait(c: *mut Cond, m: *mut Mutex, abstime: *const timespec) -> i32 {
    if abstime.is_null() || (*abstime).tv_nsec < 0 || (*abstime).tv_nsec >= 1_000_000_000 {
        return EINVAL;
    }
    let mut now = timespec { tv_sec: 0, tv_nsec: 0 };
    crate::libc::clock_gettime((*c).clock.load(Ordering::Relaxed) as i32, &mut now);
    let left = ((*abstime).tv_sec - now.tv_sec) as i128 * 1_000_000_000
        + ((*abstime).tv_nsec - now.tv_nsec) as i128;
    if left <= 0 {
        return ETIMEDOUT;
    }
    // Zero means no timeout to futex(110), so the shortest real wait is 1
    cond_wait(c, m, (left as u64).max(1))
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_signal(c: *mut Cond) -> i32 {
    (*c).seq.fetch_add(1, Ordering::Release);
    wake(&(*c).seq, 1);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_cond_broadcast(c: *mut Cond) -> i32 {
    (*c).seq.fetch_add(1, Ordering::Release);
    wake(&(*c).seq, u32::MAX);
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_init(a: *mut [u64; 4]) -> i32 {
    *a = [0; 4];
    0
}

#[no_mangle]
pub extern "C" fn pthread_condattr_destroy(_a: *mut [u64; 4]) -> i32 {
    0
}

/// Clock a condvar made from this waits on: `CLOCK_REALTIME` (0) or
/// `CLOCK_MONOTONIC` (1), in the attribute's first word
#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_setclock(a: *mut [u64; 4], clock: i32) -> i32 {
    if a.is_null() || !(0..=1).contains(&clock) {
        return EINVAL;
    }
    (*a)[0] = clock as u64;
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_condattr_getclock(a: *const [u64; 4], clock: *mut i32) -> i32 {
    if a.is_null() || clock.is_null() {
        return EINVAL;
    }
    *clock = (*a)[0] as i32;
    0
}

/// The mask is process-wide on MikuOS, so this is `sigprocmask` returning the
/// error
#[no_mangle]
pub extern "C" fn pthread_sigmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32 {
    match crate::libc::change_mask(how, set, old) {
        Ok(()) => 0,
        Err(e) => e,
    }
}

// read-write locks: [state u32, ...]: readers counted in the low bits, a writer
// as the top bit. A waiting writer does not hold back readers, so a stream of
// readers can starve one

const WRITER: u32 = 1 << 31;

#[repr(C)]
pub struct RwLock {
    state: AtomicU32,
    _pad: [u32; 13],
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_init(l: *mut RwLock, _attr: *const [u64; 4]) -> i32 {
    core::ptr::write_bytes(l, 0, 1);
    0
}

#[no_mangle]
pub extern "C" fn pthread_rwlock_destroy(_l: *mut RwLock) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_tryrdlock(l: *mut RwLock) -> i32 {
    let s = &(*l).state;
    let v = s.load(Ordering::Relaxed);
    if v & WRITER == 0 && s.compare_exchange(v, v + 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
        0
    } else {
        EBUSY
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_rdlock(l: *mut RwLock) -> i32 {
    let s = &(*l).state;
    loop {
        let v = s.load(Ordering::Relaxed);
        if v & WRITER != 0 {
            wait(s, v, 0);
        } else if s.compare_exchange_weak(v, v + 1, Ordering::Acquire, Ordering::Relaxed).is_ok() {
            return 0;
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_trywrlock(l: *mut RwLock) -> i32 {
    match (*l).state.compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed) {
        Ok(_) => 0,
        Err(_) => EBUSY,
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_wrlock(l: *mut RwLock) -> i32 {
    let s = &(*l).state;
    loop {
        match s.compare_exchange_weak(0, WRITER, Ordering::Acquire, Ordering::Relaxed) {
            Ok(_) => return 0,
            Err(v) if v != 0 => {
                wait(s, v, 0);
            }
            Err(_) => {}
        }
    }
}

#[no_mangle]
pub unsafe extern "C" fn pthread_rwlock_unlock(l: *mut RwLock) -> i32 {
    let s = &(*l).state;
    let v = s.load(Ordering::Relaxed);
    let after = if v & WRITER != 0 {
        s.store(0, Ordering::Release);
        0
    } else {
        s.fetch_sub(1, Ordering::Release) - 1
    };
    if after == 0 {
        wake(s, u32::MAX);
    }
    0
}

// once: 0 not run, 1 running, 2 done

#[no_mangle]
pub unsafe extern "C" fn pthread_once(once: *mut i32, f: extern "C" fn()) -> i32 {
    let w = &*(once as *const AtomicI32 as *const AtomicU32);
    if w.load(Ordering::Acquire) == 2 {
        return 0;
    }
    if w.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire).is_ok() {
        f();
        w.store(2, Ordering::Release);
        wake(w, u32::MAX);
        return 0;
    }
    while w.load(Ordering::Acquire) == 1 {
        wait(w, 1, 0);
    }
    0
}

// thread-specific data. A key indexes every block's `specific`, with its
// destructor here. `pthread_key_delete` clears only the calling thread's value;
// other threads' values under a deleted key are undefined by POSIX, and a
// reused key may see them

static KEY_USED: [AtomicU32; KEYS_MAX] = [const { AtomicU32::new(0) }; KEYS_MAX];
static KEY_DTOR: [AtomicUsize; KEYS_MAX] = [const { AtomicUsize::new(0) }; KEYS_MAX];

#[no_mangle]
pub unsafe extern "C" fn pthread_key_create(key: *mut u32, dtor: Option<extern "C" fn(*mut u8)>) -> i32 {
    for (i, used) in KEY_USED.iter().enumerate() {
        if used.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
            KEY_DTOR[i].store(dtor.map_or(0, |f| f as usize), Ordering::Release);
            *key = i as u32;
            return 0;
        }
    }
    EAGAIN
}

#[no_mangle]
pub extern "C" fn pthread_key_delete(key: u32) -> i32 {
    let i = key as usize;
    if i >= KEYS_MAX || KEY_USED[i].load(Ordering::Acquire) == 0 {
        return EINVAL;
    }
    KEY_DTOR[i].store(0, Ordering::Release);
    let b = current();
    if !b.is_null() {
        unsafe { (*b).specific[i].store(0, Ordering::Relaxed) };
    }
    KEY_USED[i].store(0, Ordering::Release);
    0
}

#[no_mangle]
pub extern "C" fn pthread_getspecific(key: u32) -> *mut u8 {
    let b = current();
    if b.is_null() || key as usize >= KEYS_MAX {
        return core::ptr::null_mut();
    }
    unsafe { (*b).specific[key as usize].load(Ordering::Relaxed) as *mut u8 }
}

#[no_mangle]
pub extern "C" fn pthread_setspecific(key: u32, v: *const u8) -> i32 {
    if key as usize >= KEYS_MAX || KEY_USED[key as usize].load(Ordering::Acquire) == 0 {
        return EINVAL;
    }
    let b = current();
    if b.is_null() {
        return 12; // ENOMEM
    }
    unsafe { (*b).specific[key as usize].store(v as usize, Ordering::Relaxed) };
    0
}

/// POSIX's four rounds: a destructor may set a value again and gets
/// `PTHREAD_DESTRUCTOR_ITERATIONS` chances
unsafe fn run_destructors(b: *mut Block) {
    for _ in 0..4 {
        let mut any = false;
        for i in 0..KEYS_MAX {
            let v = (*b).specific[i].swap(0, Ordering::Relaxed);
            let d = KEY_DTOR[i].load(Ordering::Acquire);
            if v != 0 && d != 0 {
                let f: extern "C" fn(*mut u8) = core::mem::transmute(d);
                f(v as *mut u8);
                any = true;
            }
        }
        if !any {
            break;
        }
    }
}

// pthread_atfork
//
// Handlers `fork` runs around itself, so a library can lock before the copy and
// fix up in both processes: every `prepare` last-registered first, then in the
// parent every `parent`, and in the child every `child`, first-registered
// first. `rand`'s reseeding generator registers one. A slot is published by
// `ATFORK_READY` after its three handlers are stored, so a racing `fork` sees
// it whole or not at all.

const ATFORK_MAX: usize = 32;
type ForkFn = extern "C" fn();
static ATFORK_USED: AtomicUsize = AtomicUsize::new(0);
static ATFORK_READY: [AtomicU32; ATFORK_MAX] = [const { AtomicU32::new(0) }; ATFORK_MAX];
static ATFORK: [[AtomicUsize; 3]; ATFORK_MAX] =
    [const { [const { AtomicUsize::new(0) }; 3] }; ATFORK_MAX];

#[no_mangle]
pub extern "C" fn pthread_atfork(
    prepare: Option<ForkFn>,
    parent: Option<ForkFn>,
    child: Option<ForkFn>,
) -> i32 {
    let i = ATFORK_USED.fetch_add(1, Ordering::AcqRel);
    if i >= ATFORK_MAX {
        ATFORK_USED.fetch_sub(1, Ordering::AcqRel);
        return crate::libc::ENOMEM;
    }
    for (slot, f) in ATFORK[i].iter().zip([prepare, parent, child]) {
        slot.store(f.map_or(0, |f| f as usize), Ordering::Relaxed);
    }
    ATFORK_READY[i].store(1, Ordering::Release);
    0
}

/// Which handlers `fork` is at
#[derive(Clone, Copy)]
pub(crate) enum ForkPhase {
    Prepare = 0,
    Parent = 1,
    Child = 2,
}

pub(crate) fn run_atfork(phase: ForkPhase) {
    let n = ATFORK_USED.load(Ordering::Acquire).min(ATFORK_MAX);
    let run = |i: usize| {
        if ATFORK_READY[i].load(Ordering::Acquire) == 0 { return; }
        let f = ATFORK[i][phase as usize].load(Ordering::Relaxed);
        if f != 0 {
            let f: ForkFn = unsafe { core::mem::transmute(f) };
            f();
        }
    };
    match phase {
        ForkPhase::Prepare => (0..n).rev().for_each(run),
        _ => (0..n).for_each(run),
    }
}

// Scheduling policy //

/// `struct sched_param`: one `int`
#[repr(C)]
pub struct sched_param {
    pub sched_priority: i32,
}

const SCHED_OTHER: i32 = 0;
const ENOTSUP: i32 = 95;

/// Every thread is `SCHED_OTHER`. The kernel keeps a per-task priority that no
/// call changes
#[no_mangle]
#[inline(never)]
pub extern "C" fn pthread_getschedparam(_thread: usize, policy: *mut i32, param: *mut sched_param) -> i32 {
    if policy.is_null() || param.is_null() { return crate::libc::EINVAL; }
    unsafe {
        *policy = SCHED_OTHER;
        (*param).sched_priority = 0;
    }
    0
}

/// Only `SCHED_OTHER` at priority 0. A realtime policy is `ENOTSUP`, as the
/// scheduler has no such class
#[no_mangle]
#[inline(never)]
pub extern "C" fn pthread_setschedparam(_thread: usize, policy: i32, param: *const sched_param) -> i32 {
    if param.is_null() { return crate::libc::EINVAL; }
    if policy == SCHED_OTHER && unsafe { (*param).sched_priority } == 0 { 0 } else { ENOTSUP }
}
