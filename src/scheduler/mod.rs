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
    spawn_named_child, spawn_named_child_of, update_process_cr3,
};

pub use proc_index::proc_index_raw;

pub use stats::{debug_dump_all, get_stats, thread_count, ThreadStat};

pub use workers::{init_workers, submit_task, Task};

// tiny inline accessors

#[inline]
pub fn current_pid() -> u64 {
    percpu::current().current_pid.load(Ordering::Relaxed)
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
    proc_index::PROC_INDEX.clear_all();

    for i in 0..percpu::MAX_CPUS {
        let c = percpu::get(i);
        c.current_pid .store(0, Ordering::Relaxed);
        c.idle_pid    .store(0, Ordering::Relaxed);
        c.min_vruntime.store(0, Ordering::Relaxed);
        c.run_queue.with(|inner| {
            inner.head = null_mut();
            inner.len  = 0;
        });
    }

    interrupts::without_interrupts(|| {
        proc_index::PROC_TABLE.lock().clear();
    });
    workers::drain_queue();
}
