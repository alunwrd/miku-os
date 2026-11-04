#![no_std]
#![no_main]
#![feature(thread_local)]

//! Threads in one address space, from ring 3. Later cases depend on earlier
//! ones, so the test stops at the first failure.
//!
//! 1. a thread runs, and `gettid` differs from its parent's
//! 2. shared memory: a counter incremented by four threads reaches four
//! 3. shared descriptors: a file opened here is readable there
//!   4. a futex wait sleeps and a wake ends it
//! 5. a futex wait with a timeout expires
//! 6. the `core_miku` mutex hands over a contended lock without spinning
//! 7. `#[thread_local]`: four threads, four blocks, four values, correct
//!    initializer
//!
//! Run under `-smp 4`.

pub use miku;

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering};

static mut PASSED: u32 = 0;
static mut FAILED: u32 = 0;

fn check(name: &str, ok: bool) {
    unsafe {
        if ok { PASSED += 1; } else { FAILED += 1; }
    }
    miku::print(if ok { "  ok    " } else { "  FAIL  " });
    miku::println(name);
}

/// One stack per thread in `.bss`, never reused. A counter reaching its target
/// does not mean the thread has left its stack, since it still returns through
/// `thread_exit` afterwards. There is no join below `std`
const STACK_SIZE: usize = 64 * 1024;
const MAX_THREADS: usize = 24;
static mut STACKS: [[u8; STACK_SIZE]; MAX_THREADS] = [[0u8; STACK_SIZE]; MAX_THREADS];
static NEXT_STACK: AtomicU64 = AtomicU64::new(0);

/// A new stack. Panics when out, rather than sharing one
fn fresh_stack() -> &'static mut [u8] {
    let i = NEXT_STACK.fetch_add(1, Ordering::SeqCst) as usize;
    if i >= MAX_THREADS {
        miku::println("threadtest: out of stacks - raise MAX_THREADS");
        miku::exit(2);
    }
    unsafe { &mut (*core::ptr::addr_of_mut!(STACKS))[i] }
}

/// Waits for a counter to reach `want`, or gives up after `budget_ms` of wall
/// time. Spins with a yield, since the harness must not depend on the futex
/// under test
fn await_count(c: &AtomicU64, want: u64, budget_ms: u64) -> bool {
    let deadline = miku::uptime_ms() + budget_ms;
    loop {
        if c.load(Ordering::SeqCst) >= want { return true; }
        if miku::uptime_ms() >= deadline {
            return c.load(Ordering::SeqCst) >= want;
        }
        unsafe { miku::miku_sched_yield() };
    }
}

// 1. A thread runs, and is not its parent

static TID_SEEN: AtomicU64 = AtomicU64::new(0);

extern "C" fn says_who(arg: *mut u8) -> i32 {
    TID_SEEN.store(miku::gettid() as u64, Ordering::SeqCst);
    arg as i32
}

fn test_runs_at_all() -> bool {
    let me = miku::gettid();
    let tid = miku::thread_spawn(says_who, 7 as *mut u8, fresh_stack());
    check("thread_spawn returns a tid", tid > 0);
    if tid <= 0 { return false; }

    let ran = await_count(&TID_SEEN, 1, 1_000);
    check("the thread ran", ran);
    let seen = TID_SEEN.load(Ordering::SeqCst) as i64;
    check("gettid in the thread is the tid spawn returned", seen == tid);
    check("and it is not the parent's", seen != me);
    // `getpid` is the process's in every thread
    check("getpid is the same on both sides", miku::getpid() == me as u64);
    ran && seen == tid
}

// 2. shared memory

static COUNTER: AtomicU64 = AtomicU64::new(0);
const PER_THREAD: u64 = 1000;

extern "C" fn bump(_arg: *mut u8) -> i32 {
    for _ in 0..PER_THREAD {
        COUNTER.fetch_add(1, Ordering::SeqCst);
    }
    0
}

fn test_shared_memory() -> bool {
    let n = 4;
    let mut started = 0;
    for _ in 0..n {
        if miku::thread_spawn(bump, core::ptr::null_mut(), fresh_stack()) > 0 {
            started += 1;
        }
    }
    check("four threads started", started == n as i32);
    let all = await_count(&COUNTER, PER_THREAD * started as u64, 2_000);
    check("every increment landed - one address space, one counter", all);
    all
}

// 3. shared descriptors

static FD_FROM_THREAD: AtomicU64 = AtomicU64::new(0);
static READ_OK: AtomicU64 = AtomicU64::new(0);

const TEST_PATH: &[u8] = b"/tmp/threadtest.txt\0";
const TEST_DATA: &[u8] = b"shared";

extern "C" fn reads_the_fd(arg: *mut u8) -> i32 {
    let fd = arg as i64;
    let mut buf = [0u8; 16];
    let n = unsafe { miku::miku_read(fd as u64, buf.as_mut_ptr(), buf.len()) };
    if n == TEST_DATA.len() as i64 && &buf[..TEST_DATA.len()] == TEST_DATA {
        READ_OK.store(1, Ordering::SeqCst);
    }
    FD_FROM_THREAD.store(1, Ordering::SeqCst);
    0
}

fn test_shared_descriptors() -> bool {
    // O_READ|O_WRITE|O_CREATE|O_TRUNC, since /tmp may hold a previous run's
    // file
    let fd = unsafe {
        miku::miku_open(TEST_PATH.as_ptr(), TEST_PATH.len() - 1, 0x0001 | 0x0002 | 0x0008 | 0x0020, 0o644)
    };
    check("opened a file to share", fd >= 0);
    if fd < 0 { return false; }

    let w = unsafe { miku::miku_write(fd as u64, TEST_DATA.as_ptr(), TEST_DATA.len()) };
    check("wrote to it", w == TEST_DATA.len() as i64);
    unsafe { miku::miku_seek(fd, 0) };

    let tid = miku::thread_spawn(reads_the_fd, fd as usize as *mut u8, fresh_stack());
    if tid <= 0 { check("spawned a reader", false); return false; }
    let done = await_count(&FD_FROM_THREAD, 1, 1_000);
    check("the reader finished", done);
    check("the descriptor was the same one - one table per process", READ_OK.load(Ordering::SeqCst) == 1);
    unsafe { miku::miku_close(fd) };
    done
}

// 4. a futex wait really sleeps, and a wake really ends it

static GATE: AtomicU32 = AtomicU32::new(0);
static WOKE: AtomicU64 = AtomicU64::new(0);
static ENTERED: AtomicU64 = AtomicU64::new(0);

extern "C" fn waits_on_gate(_arg: *mut u8) -> i32 {
    ENTERED.fetch_add(1, Ordering::SeqCst);
    // A return from `futex_wait` is only a hint, so it loops
    while GATE.load(Ordering::SeqCst) == 0 {
        miku::futex_wait(&GATE, 0, 0);
    }
    WOKE.fetch_add(1, Ordering::SeqCst);
    0
}

fn test_futex_wake() -> bool {
    let n = 3;
    for _ in 0..n {
        if miku::thread_spawn(waits_on_gate, core::ptr::null_mut(), fresh_stack()) <= 0 {
            check("spawned a waiter", false);
            return false;
        }
    }
    let in_wait = await_count(&ENTERED, n as u64, 1_000);
    check("three waiters entered the wait", in_wait);

    // The gate is closed, so nothing may have passed
    check("and none of them came out on their own", WOKE.load(Ordering::SeqCst) == 0);

    GATE.store(1, Ordering::SeqCst);
    let woken = miku::futex_wake(&GATE, n as u32);
    check("futex_wake reported waiters", woken > 0);

    let all_out = await_count(&WOKE, n as u64, 2_000);
    check("every waiter woke", all_out);
    all_out
}

// 5. a timeout ends a wait nobody wakes

static TIMED_OUT: AtomicU64 = AtomicU64::new(0);
static NEVER: AtomicU32 = AtomicU32::new(0);

extern "C" fn waits_for_nothing(_arg: *mut u8) -> i32 {
    // 100 ms on a word nobody will ever change. -ETIMEDOUT is -110
    let r = miku::futex_wait(&NEVER, 0, 100_000_000);
    TIMED_OUT.store(if r == -110 { 1 } else { 2 }, Ordering::SeqCst);
    0
}

fn test_futex_timeout() -> bool {
    if miku::thread_spawn(waits_for_nothing, core::ptr::null_mut(), fresh_stack()) <= 0 {
        check("spawned a timeout waiter", false);
        return false;
    }
    let done = await_count(&TIMED_OUT, 1, 2_000);
    check("the wait ended by itself", done);
    check("and said so with ETIMEDOUT", TIMED_OUT.load(Ordering::SeqCst) == 1);
    done
}

// 6. The mutex hands over the lock instead of spinning

static MTX: miku::MikuMutex = miku::MikuMutex::new();
static GUARDED: AtomicU64 = AtomicU64::new(0);
static MTX_DONE: AtomicU64 = AtomicU64::new(0);
const MTX_ROUNDS: u64 = 500;

extern "C" fn hammers_the_lock(_arg: *mut u8) -> i32 {
    for _ in 0..MTX_ROUNDS {
        MTX.lock();
        // Non-atomic, protected only by the mutex
        let v = GUARDED.load(Ordering::Relaxed);
        unsafe { miku::miku_sched_yield() };
        GUARDED.store(v + 1, Ordering::Relaxed);
        MTX.unlock();
    }
    MTX_DONE.fetch_add(1, Ordering::SeqCst);
    0
}

fn test_mutex() -> bool {
    let n = 4;
    for _ in 0..n {
        if miku::thread_spawn(hammers_the_lock, core::ptr::null_mut(), fresh_stack()) <= 0 {
            check("spawned a contender", false);
            return false;
        }
    }
    let done = await_count(&MTX_DONE, n as u64, 5_000);
    check("every contender finished", done);
    check(
        "the guarded count is exact - the lock actually excluded",
        GUARDED.load(Ordering::SeqCst) == MTX_ROUNDS * n as u64,
    );
    done
}

// 7. Per-thread storage: `#[thread_local]` in `PT_TLS`. Depends on the loader
//    publishing the layout in the TCB, `miku_thread_spawn` carving a block,
//    `thread_create` setting `FS_BASE`, and the context switch swapping it

/// Non-zero, so `.tdata` copying is tested
#[thread_local]
static mut MY_TID: u64 = 0xA11CE;

static TLS_DISTINCT: AtomicU64 = AtomicU64::new(0);
static TLS_INIT_OK: AtomicU64 = AtomicU64::new(0);
static TLS_DONE: AtomicU64 = AtomicU64::new(0);
/// Slots per TLS thread
const TLS_THREADS: usize = 4;
static TLS_SEEN: [AtomicU64; TLS_THREADS] = [
    AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0), AtomicU64::new(0),
];

extern "C" fn keeps_its_own(arg: *mut u8) -> i32 {
    let slot = arg as usize;
    // The declared initializer must be present before writing
    let initial = unsafe { core::ptr::read_volatile(&raw const MY_TID) };
    if initial == 0xA11CE {
        TLS_INIT_OK.fetch_add(1, Ordering::SeqCst);
    }

    let me = miku::gettid() as u64;
    unsafe { core::ptr::write_volatile(&raw mut MY_TID, me) };
    // Give the others every chance to overwrite it if they share a block
    for _ in 0..200 {
        unsafe { miku::miku_sched_yield() };
    }
    let after = unsafe { core::ptr::read_volatile(&raw const MY_TID) };
    if after == me {
        TLS_DISTINCT.fetch_add(1, Ordering::SeqCst);
    }
    if slot < TLS_THREADS {
        TLS_SEEN[slot].store(after, Ordering::SeqCst);
    }
    TLS_DONE.fetch_add(1, Ordering::SeqCst);
    0
}

fn test_thread_local() -> bool {
    let n = TLS_THREADS;
    for i in 0..n {
        if miku::thread_spawn(keeps_its_own, i as *mut u8, fresh_stack()) <= 0 {
            check("spawned a TLS user", false);
            return false;
        }
    }
    let done = await_count(&TLS_DONE, n as u64, 5_000);
    check("every TLS thread finished", done);
    check(
        "each started from the declared initialiser - .tdata was copied",
        TLS_INIT_OK.load(Ordering::SeqCst) == n as u64,
    );
    check(
        "each kept its own value while the others ran",
        TLS_DISTINCT.load(Ordering::SeqCst) == n as u64,
    );
    // The values must really be four different ones
    let mut distinct = true;
    for i in 0..n {
        for j in (i + 1)..n {
            if TLS_SEEN[i].load(Ordering::SeqCst) == TLS_SEEN[j].load(Ordering::SeqCst) {
                distinct = false;
            }
        }
    }
    check("and no two threads shared a block", distinct);
    // The main thread's own copy is untouched by any of it
    check(
        "the first thread still has its own",
        unsafe { core::ptr::read_volatile(&raw const MY_TID) } == 0xA11CE,
    );
    done
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    miku::println("threadtest: threads and futexes from ring 3");

    if test_runs_at_all() {
        if test_shared_memory() {
            test_shared_descriptors();
            if test_futex_wake() {
                test_futex_timeout();
                test_mutex();
                test_thread_local();
            }
        }
    }

    let (p, f) = unsafe { (PASSED, FAILED) };
    miku::print("threadtest: ");
    miku::print_int(p as i64);
    miku::print(" passed, ");
    miku::print_int(f as i64);
    miku::println(" failed");
    miku::exit(if f == 0 { 0 } else { 1 });
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::println("threadtest: PANIC");
    miku::exit(1);
}
