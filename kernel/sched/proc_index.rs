// Process registry: PID -> *mut Process index plus the owning BTreeMap
//
// Two cooperating containers:
//   PROC_TABLE  owns the boxed Process; access guarded by a Mutex
//   PROC_INDEX  flat pid-keyed pointer array, lock-free reads from any context
//               (including ISRs). Updated only with interrupts disabled to keep
//               ISR readers consistent with the table

extern crate alloc;

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;
use x86_64::instructions::interrupts;

use crate::arch::x86_64::percpu;
use crate::process::Process;

/// Linux's default `pid_max`. A thread is a task (`dev/kernel-core.md`, "The
/// process table"). The index stays a static array, 256 KiB: allocated at boot,
/// every lookup, the timer's and the scheduler's among them, would check first
pub const MAX_PROCS: usize = 32768;

pub struct ProcIndex(pub(super) UnsafeCell<[*mut Process; MAX_PROCS]>);
unsafe impl Sync for ProcIndex {}
unsafe impl Send for ProcIndex {}

pub static PROC_INDEX: ProcIndex =
    ProcIndex(UnsafeCell::new([null_mut::<Process>(); MAX_PROCS]));

pub static PROC_TABLE: Mutex<BTreeMap<u64, Box<Process>>> = Mutex::new(BTreeMap::new());

/// Which pids are in use: one bit per pid, set from `alloc_pid` until
/// `free_pid`.
///
/// `PROC_INDEX` is a flat array of `MAX_PROCS`, and both `set` and `get_raw`
/// return quietly for a pid past the end, so pids have to stay inside it. As in
/// Linux, a cursor rotates over the whole space and skips what is live, so a
/// pid is reused as late as possible: the further apart two uses of one number
/// are, the smaller the window in which a stale pid means somebody else.
///
/// The words are atomic because the bitmap is also the list of live tasks:
/// `for_each_task` walks the set bits without a lock. Bit 0 is pid 0 (the boot
/// CPU's idle task) and is set from the start, so it is never handed out
const PID_WORDS: usize = MAX_PROCS / 64;

static PID_USED: [AtomicU64; PID_WORDS] = {
    let mut used = [const { AtomicU64::new(0) }; PID_WORDS];
    used[0] = AtomicU64::new(1);
    used
};

/// Where the last search stopped (the rotation delays reuse), and the lock
/// making one allocation at a time
static PID_CURSOR: Mutex<usize> = Mutex::new(1);

/// One past the largest pid handed out so far, which bounds the walk of the
/// bitmap: a machine that has allocated three pids looks at one word.
///
/// An atomic of its own so nothing interrupting a holder of the allocator's
/// lock has to take it
static HIGH_WATER: AtomicU64 = AtomicU64::new(1);

#[inline]
fn pid_bit(pid: usize) -> (usize, u64) {
    (pid / 64, 1u64 << (pid % 64))
}

/// The first clear bit in `lo..hi`, a word at a time
fn first_free(lo: usize, hi: usize) -> Option<usize> {
    let mut i = lo;
    while i < hi {
        let w = i / 64;
        let free = !PID_USED[w].load(Ordering::Relaxed) & (!0u64 << (i % 64));
        if free != 0 {
            let pid = w * 64 + free.trailing_zeros() as usize;
            return (pid < hi).then_some(pid);
        }
        i = (w + 1) * 64;
    }
    None
}

/// Takes a pid, or `None` when every one is live.
///
/// `None` rather than a panic: `thread_create` is a syscall, so a program
/// asking for forty thousand threads gets an error. From the cursor to the end
/// of the space, then from 1 back up to the cursor
pub fn alloc_pid() -> Option<u64> {
    let mut cursor = PID_CURSOR.lock();
    let start = *cursor;
    let pid = first_free(start, MAX_PROCS).or_else(|| first_free(1, start))?;
    let (w, bit) = pid_bit(pid);
    PID_USED[w].fetch_or(bit, Ordering::AcqRel);
    *cursor = if pid + 1 >= MAX_PROCS { 1 } else { pid + 1 };
    let p = pid as u64;
    HIGH_WATER.fetch_max(p + 1, Ordering::Release);
    Some(p)
}

/// Gives a pid back, once nothing can name it any more.
///
/// Called from the reaper, after the task is out of `PROC_TABLE` and
/// `PROC_INDEX`. Earlier, the number could go to a new task while a signal, a
/// `wait4` or a run queue still meant the old one. No lock: clearing a bit
/// cannot meet an allocation, which only sets bits it found clear
pub fn free_pid(pid: u64) {
    if pid == 0 || pid as usize >= MAX_PROCS { return; }
    let (w, bit) = pid_bit(pid as usize);
    PID_USED[w].fetch_and(!bit, Ordering::AcqRel);
}

/// Forgets every pid, for the one caller that forgets every process.
///
/// `reinit_scheduler` empties `PROC_TABLE` and `PROC_INDEX` without reaping, so
/// nothing calls `free_pid`, and the next allocation after a shutdown-reboot
/// would fail
pub fn reset_pids() {
    let mut cursor = PID_CURSOR.lock();
    for w in PID_USED.iter() {
        w.store(0, Ordering::Relaxed);
    }
    PID_USED[0].store(1, Ordering::Release);
    *cursor = 1;
    HIGH_WATER.store(1, Ordering::Release);
    JOBS.lock().clear();
}

/// One past the largest pid ever handed out, capped at the table.
///
/// It only grows, and since the cursor rotates over the whole space, on a
/// machine that has run long enough it is the whole space. Bound a walk with
/// `for_each_task`
pub fn pid_high_water() -> u64 {
    HIGH_WATER.load(Ordering::Acquire)
}

/// The first live task `f` says yes to, walking the pid bitmap without a lock
/// (`dev/kernel-core.md`, "The process table"): 512 words for the whole space,
/// and one look at the index per live task. Safe from a fault handler.
///
/// A task registered or reaped during the walk may or may not be seen. One that
/// is seen is as good as a `get_raw` of it: valid until it is reaped
pub fn find_task(mut f: impl FnMut(&Process) -> bool) -> Option<*mut Process> {
    let words = (pid_high_water() as usize).div_ceil(64).min(PID_WORDS);
    for (w, word) in PID_USED[..words].iter().enumerate() {
        let mut bits = word.load(Ordering::Acquire);
        while bits != 0 {
            let pid = w * 64 + bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let p = unsafe { PROC_INDEX.get_raw(pid as u64) };
            if !p.is_null() && f(unsafe { &*p }) {
                return Some(p);
            }
        }
    }
    None
}

/// Every live task, as `find_task` walks them
pub fn for_each_task(mut f: impl FnMut(&Process)) {
    find_task(|p| {
        f(p);
        false
    });
}

/// Jobs: every process (a thread group's leader) as `(pgid, pid)`, so a job's
/// members are one range of the set (`dev/kernel-core.md`, "The process
/// table"). Added at registration, moved by `set_job`, removed at the reap.
/// Threads are not members: a signal goes to a process.
///
/// Taken with interrupts off: Ctrl-C reaches it from the keyboard thread.
/// Nothing it is held across takes another lock
static JOBS: Mutex<BTreeSet<(u64, u64)>> = Mutex::new(BTreeSet::new());

/// `p` into its job, if it is a process and in one
fn job_add(p: &Process) {
    let pgid = p.pgid.load(Ordering::Relaxed);
    if pgid == 0 || p.is_thread() { return; }
    interrupts::without_interrupts(|| JOBS.lock().insert((pgid, p.pid)));
}

/// `p` out of its job, as it leaves the table
fn job_remove(p: &Process) {
    if p.is_thread() { return; }
    interrupts::without_interrupts(|| {
        let mut jobs = JOBS.lock();
        jobs.remove(&(p.pgid.load(Ordering::Relaxed), p.pid));
    });
}

/// Moves process `pid` into job `pgid`: the field and the index together, under
/// the index's lock. False when `pid` has left the index, looked at again under
/// the lock, because the reap takes the lock before it frees
pub fn set_job(pid: u64, pgid: u64) -> bool {
    interrupts::without_interrupts(|| {
        let mut jobs = JOBS.lock();
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return false; }
        let p = unsafe { &*ptr };
        let old = p.pgid.swap(pgid, Ordering::Relaxed);
        jobs.remove(&(old, pid));
        if pgid != 0 && !p.is_thread() {
            jobs.insert((pgid, pid));
        }
        true
    })
}

/// The processes of job `pgid` from pid `start` on, in pid order, each to `f`
/// until it says stop. No allocation. Returns the pid `f` stopped at (not
/// taken, where a next pass starts), or `None` when the job is done
pub fn job_members(pgid: u64, start: u64, mut f: impl FnMut(u64) -> bool) -> Option<u64> {
    interrupts::without_interrupts(|| {
        let jobs = JOBS.lock();
        for &(_, pid) in jobs.range((pgid, start)..(pgid.saturating_add(1), 0)) {
            if !f(pid) {
                return Some(pid);
            }
        }
        None
    })
}

impl ProcIndex {
    #[inline]
    pub unsafe fn get_raw(&self, pid: u64) -> *mut Process {
        if pid as usize >= MAX_PROCS { return null_mut(); }
        (*self.0.get())[pid as usize]
    }

    pub fn set(&self, pid: u64, p: *mut Process) {
        if pid as usize >= MAX_PROCS { return; }
        interrupts::without_interrupts(|| unsafe {
            (*self.0.get())[pid as usize] = p;
        });
    }

    pub fn clear(&self, pid: u64) { self.set(pid, null_mut()); }

    /// Drop every entry. Safe only when no other CPU can be inspecting the
    /// table - used by reinit_scheduler during shutdown reboots
    pub fn clear_all(&self) {
        interrupts::without_interrupts(|| unsafe {
            for slot in (*self.0.get()).iter_mut() { *slot = null_mut(); }
        });
    }
}

/// Insert a process into the registry and return a stable raw pointer to it
pub fn register_process(p: Box<Process>) -> *mut Process {
    let pid = p.pid;
    let mut table = PROC_TABLE.lock();
    table.insert(pid, p);
    let raw: *mut Process = table.get_mut(&pid).unwrap().as_mut();
    drop(table);
    PROC_INDEX.set(pid, raw);
    job_add(unsafe { &*raw });
    raw
}

/// Out of the index, for the reaper: nothing finds `pid` by number after
/// this, and it has left its job. The `Process` itself is still the table's
pub fn unregister(pid: u64) {
    let ptr = unsafe { PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return; }
    PROC_INDEX.clear(pid);
    job_remove(unsafe { &*ptr });
}

/// Returns true if any online CPU currently has pid as its current task
pub fn pid_running_anywhere(pid: u64) -> bool {
    for c in percpu::iter_online() {
        if c.current_pid.load(Ordering::Relaxed) == pid { return true; }
    }
    false
}

/// External read-only handle into the index. Kept for callers outside the
/// scheduler that need to peek at a process by PID without locking
pub unsafe fn proc_index_raw(pid: u64) -> *mut Process {
    PROC_INDEX.get_raw(pid)
}
