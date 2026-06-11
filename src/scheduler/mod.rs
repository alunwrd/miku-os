// SMP scheduler (CFS)
//
// Each CPU owns its run queue, min_vruntime, current_pid and idle process.
// Cross-CPU wakeups route to the lightest eligible CPU; when the local
// queue is empty we steal from a neighbour
//
// Module layout:
//      proc_index - PID -> *mut Process registry + owning BTreeMap
//      runqueue   - per-CPU min-vruntime list, work-steal, CPU pick
//      core       - schedule_from_isr, naked software_context_switch,
//                    sleeper wake-ups, TOTAL_SWITCHES
//      control    - yield/sleep/block/wakeup/waitpid
//      lifecycle  - spawn family, kill/reap, attribute mutators, queries
//      workers    - kernel work queue and worker pool
//      stats      - ThreadStat, get_stats, debug_dump_all

mod control;
#[path = "core.rs"]
mod core_sched;
mod lifecycle;
mod proc_index;
mod runqueue;
mod stats;
mod workers;

use ::core::ptr::null_mut;
use ::core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::percpu;

// public re-exports

// Hot-path entry point referenced from the timer-ISR stub via #[no_mangle]
pub use core_sched::schedule_from_isr;

pub use control::{block_current, sleep, wakeup, waitpid, yield_now};

pub use lifecycle::{
    add_user_process, find_zombie_child, get_ppid, has_children, init_ap_idle,
    init_main_thread, kill, kill_with_code, process_exists, reap_zombie,
    set_affinity, set_parent, set_priority, spawn, spawn_child_of, spawn_named,
    spawn_named_child, spawn_named_child_of, started, update_process_cr3,
};

pub use proc_index::proc_index_raw;

pub use stats::{debug_dump_all, get_stats, thread_count, ThreadStat};

pub use workers::{init_workers, submit_task, Task};

// tiny inline accessors

#[inline]
pub fn current_pid() -> u64 {
    percpu::current().current_pid.load(Ordering::Relaxed)
}

// Per-process cwd accessors. cwd is stored as a u64 InodeId on the
// Process struct so it can be read/written atomically without taking the
// PROC_TABLE lock from hot paths

#[inline]
pub fn cwd_of(pid: u64) -> u64 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).cwd.load(Ordering::Relaxed) }
}

#[inline]
pub fn current_cwd() -> u64 {
    cwd_of(current_pid())
}

#[inline]
pub fn set_current_cwd(id: u64) {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(current_pid()) };
    if ptr.is_null() { return; }
    unsafe { (*ptr).cwd.store(id, Ordering::Relaxed) };
}

// Per-process VFS identity accessors. Used by fork to copy parent state
// and by `with_vfs` to sync vfs.ctx before invoking syscall closures.
// Idle / pre-init callers (pid 0 with no Process entry) get sensible
// defaults (umask 0o022, root cred)

#[inline]
pub fn umask_of(pid: u64) -> u16 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0o022; }
    unsafe { (*ptr).umask.load(Ordering::Relaxed) }
}

#[inline]
pub fn set_current_umask(mask: u16) {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(current_pid()) };
    if ptr.is_null() { return; }
    unsafe { (*ptr).umask.store(mask, Ordering::Relaxed) };
}

#[inline]
pub fn uid_of(pid: u64) -> u16 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).uid.load(Ordering::Relaxed) }
}

#[inline]
pub fn gid_of(pid: u64) -> u16 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).gid.load(Ordering::Relaxed) }
}

#[inline]
pub fn euid_of(pid: u64) -> u16 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).euid.load(Ordering::Relaxed) }
}

#[inline]
pub fn egid_of(pid: u64) -> u16 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).egid.load(Ordering::Relaxed) }
}

#[inline]
pub fn current_identity() -> (u16, u16, u16, u16, u16) {
    let pid = current_pid();
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return (0o022, 0, 0, 0, 0); }
    let p = unsafe { &*ptr };
    (
        p.umask.load(Ordering::Relaxed),
        p.uid.load(Ordering::Relaxed),
        p.gid.load(Ordering::Relaxed),
        p.euid.load(Ordering::Relaxed),
        p.egid.load(Ordering::Relaxed),
    )
}

#[inline]
pub fn total_switches() -> u64 {
    core_sched::TOTAL_SWITCHES.load(Ordering::Relaxed)
}

// soft-reboot reset 

/// Wipe scheduler state for a clean restart. Caller must already have all
/// other CPUs halted before invoking this
pub fn reinit_scheduler() {
    core_sched::TOTAL_SWITCHES.store(0, Ordering::Relaxed);
    core_sched::SLEEPER_COUNT.store(0, Ordering::Relaxed);
    proc_index::PROC_INDEX.clear_all();

    for i in 0..percpu::MAX_CPUS {
        let c = percpu::get(i);
        c.current_pid .store(0, Ordering::Relaxed);
        c.idle_pid    .store(0, Ordering::Relaxed);
        c.min_vruntime.store(0, Ordering::Relaxed);
        c.run_queue.with(|inner| {
            inner.head = null_mut();
            inner.tail = null_mut();
            inner.len  = 0;
        });
    }

    interrupts::without_interrupts(|| {
        proc_index::PROC_TABLE.lock().clear();
    });
    workers::drain_queue();
}
