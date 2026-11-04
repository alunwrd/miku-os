//! `std::thread` on MikuOS.
//!
//! The kernel gives a thread (`thread_create`, 106) and a way to sleep on a
//! word (`futex`, 110). A joinable thread is built here out of those two.
//!
//! Three pieces of memory per thread:
//!
//! - the stack, `mmap`ed here and unmapped by the kernel when the thread dies,
//!   since a thread cannot unmap the stack it is standing on.
//!   `THREAD_STACK_OWNED` is the request.
//! - the join word, a small mapping of its own that outlives the stack, freed
//!   by whichever of the thread and the joiner is last.
//! - the `ThreadInit` box, handed to the new thread as its argument and
//!   consumed by it.

use crate::ffi::CStr;
use crate::io;
use crate::num::NonZero;
use crate::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use crate::sys::pal::abi;
use crate::thread::ThreadInit;
use crate::time::Duration;

/// Linux's figure, since programs are written against it. The stack is mapped
/// lazily, so a thread pays for the pages it touches
pub const DEFAULT_MIN_STACK_SIZE: usize = 2 * 1024 * 1024;

/// The page `miku_thread_spawn_owned` turns into the stack's guard; the mapping
/// is this much bigger than the stack asked for
const GUARD: usize = 4096;

const PROT_READ: u64 = 1;
const PROT_WRITE: u64 = 2;

/// The word a joiner sleeps on, and the reference count deciding who frees it.
///
/// A mapping of its own: the kernel unmaps the stack the moment the thread
/// dies, which is when the joiner reads this.
#[repr(C)]
struct Join {
    /// 0 while the thread runs, 1 once it has finished. Futex-waited on.
    done: AtomicU32,
    /// Two owners: the thread and the handle. The last one out unmaps
    refs: AtomicUsize,
}

fn map(len: usize) -> io::Result<*mut u8> {
    let p = abi::miku_mmap(0, len, PROT_READ | PROT_WRITE);
    if p.is_null() { Err(io::Error::from_raw_os_error(12)) } else { Ok(p) } // ENOMEM
}

unsafe fn release(join: *mut Join) {
    // SAFETY: the caller holds one of the two references
    if unsafe { (*join).refs.fetch_sub(1, Ordering::AcqRel) } == 1 {
        abi::miku_munmap(join as *mut u8, size_of::<Join>());
    }
}

pub struct Thread {
    join: *mut Join,
    /// The kernel's id for it, what `std::os::unix::thread::RawPthread` is
    tid: u64,
}

// SAFETY: the only shared state is the `Join` block, and every field of it is
// an atomic
unsafe impl Send for Thread {}
unsafe impl Sync for Thread {}

/// What the new thread runs. Its argument is the `Start` block below, which it
/// owns and frees
struct Start {
    init: *mut ThreadInit,
    join: *mut Join,
}

extern "C" fn thread_start(arg: *mut u8) -> i32 {
    // SAFETY: `arg` is the `Start` that `Thread::new` leaked into this thread
    let start = unsafe { Box::from_raw(arg as *mut Start) };
    // SAFETY: the box was made by `Box::into_raw` in `Thread::new`
    let init = unsafe { Box::from_raw(start.init) };
    let run = init.init();
    run();

    // Thread-local destructors and the runtime's own cleanup. `std` is the
    // runtime here, and `sys/thread_local/mod.rs` puts MikuOS in the arm that
    // says so
    unsafe {
        crate::sys::thread_local::destructors::run();
    }
    crate::rt::thread_cleanup();

    // Only now: a joiner woken earlier would see the thread finished while its
    // thread-locals are still being dropped
    let join = start.join;
    unsafe {
        (*join).done.store(1, Ordering::Release);
        abi::miku_futex_wake((*join).done.as_ptr() as *const u32, u32::MAX);
        release(join);
    }
    0
}

impl Thread {
    /// # Safety
    /// See `thread::Builder::spawn_unchecked`.
    pub unsafe fn new(stack: usize, init: Box<ThreadInit>) -> io::Result<Thread> {
        let stack = stack.max(DEFAULT_MIN_STACK_SIZE).next_multiple_of(4096);

        let join = map(size_of::<Join>())? as *mut Join;
        // SAFETY: freshly mapped, zeroed by the kernel, and ours alone
        unsafe {
            (*join).done = AtomicU32::new(0);
            (*join).refs = AtomicUsize::new(2); // the thread and this handle
        }

        let stack = stack + GUARD;
        let mem = match map(stack) {
            Ok(m) => m,
            Err(e) => {
                abi::miku_munmap(join as *mut u8, size_of::<Join>());
                return Err(e);
            }
        };

        let start = Box::into_raw(Box::new(Start { init: Box::into_raw(init), join }));
        let tid = abi::miku_thread_spawn_owned(thread_start, start as *mut u8, mem, stack);
        if tid < 0 {
            // Nothing ran, so everything here is still ours to undo.
            // SAFETY: `start` came from `Box::into_raw` a line ago and nothing
            // else has seen it
            unsafe {
                let start = Box::from_raw(start);
                drop(Box::from_raw(start.init));
            }
            abi::miku_munmap(mem, stack);
            abi::miku_munmap(join as *mut u8, size_of::<Join>());
            return Err(io::Error::from_raw_os_error(-tid as i32));
        }

        Ok(Thread { join, tid: tid as u64 })
    }

    /// The thread's id, for `JoinHandleExt::as_pthread_t`
    pub fn id(&self) -> u64 {
        self.tid
    }

    /// The same, giving the handle up. MikuOS cannot join a thread by id, so
    /// the handle detaches, as dropping it would, and only the id goes back
    pub fn into_id(self) -> u64 {
        self.tid
    }

    pub fn join(self) {
        let join = self.join;
        // SAFETY: this handle holds one of the two references
        unsafe {
            while (*join).done.load(Ordering::Acquire) == 0 {
                // A return says the sleep ended, never that the word changed,
                // so the loop re-reads it
                abi::miku_futex_wait((*join).done.as_ptr() as *const u32, 0, 0);
            }
            release(join);
        }
        crate::mem::forget(self);
    }
}

impl Drop for Thread {
    /// A handle dropped without joining detaches the thread.
    ///
    /// The thread keeps running: the kernel unmaps its stack when it dies, and
    /// the last of the two unmaps the join word.
    fn drop(&mut self) {
        // SAFETY: this handle holds one of the two references
        unsafe { release(self.join) };
    }
}

pub fn available_parallelism() -> io::Result<NonZero<usize>> {
    // No call answers this: the CPU count stays in ring 0. `Err` is what `std`
    // documents for "the platform cannot tell", and callers have a default
    Err(io::Error::UNKNOWN_THREAD_COUNT)
}

pub fn current_os_id() -> Option<u64> {
    let tid = abi::miku_gettid();
    if tid < 0 { None } else { Some(tid as u64) }
}

pub fn yield_now() {
    abi::miku_sched_yield();
}

pub fn set_name(_name: &CStr) {
    // A task's name is set by `exec` only. `sched::set_name` has no syscall,
    // since `ps` reads this name
}

pub fn sleep(dur: Duration) {
    // Nanoseconds: the kernel's timer sleeps to the deadline, not to the next 4
    // ms tick. A sub-nanosecond sleep is still a sleep, not a yield
    let ns = dur.as_nanos().min(u64::MAX as u128) as u64;
    let ns = if ns == 0 && !dur.is_zero() { 1 } else { ns };
    abi::miku_sleep_ns(ns);
}
