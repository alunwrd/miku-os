// SMP scheduler (CFS)
//
// Each CPU owns its run queue, min_vruntime, current_pid and idle process.
// Cross-CPU wakeups route to the lightest eligible CPU; when the local queue is
// empty it steals from a neighbour
//
// Module layout:
//      proc_index  PID -> *mut Process registry + owning BTreeMap
//      runqueue    per-CPU min-vruntime heap, work-steal, CPU pick
//      slot_heap   the min-heap of tasks the run queues and timers are
//      core        schedule_from_isr, naked software_context_switch,
//                  sleeper wake-ups, TOTAL_SWITCHES
//      control     yield/sleep/block/wakeup/waitpid
//      lifecycle   spawn family, kill/reap, attribute mutators, queries
//      workers     kernel work queue and worker pool
//      wait        WaitQueue: who is waiting for what, so a wait is not a spin
//      stats       ThreadStat, get_stats, debug_dump_all

mod control;
#[path = "core.rs"]
mod core_sched;
pub mod idle;
mod lifecycle;
mod proc_index;
mod runqueue;
pub mod selftest;
pub mod slot_heap;
pub mod stats;
pub mod futex;
pub mod timer;
pub mod wait;
mod workers;

use ::core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::arch::x86_64::percpu;

// public re-exports

// Hot-path entry point referenced from the timer-ISR stub via #[no_mangle]
pub use core_sched::{expire_sleepers, schedule_from_isr, schedule_from_timer, SwitchOut};

pub use control::{block_current, cont, is_stopped, sleep, sleep_ns, sleep_until, sleep_until_unless, stop_current, tick_deadline, wake_count, wakeup, waitpid, yield_now};
pub use lifecycle::{clear_stop_report, exit_signal_of, find_stopped_child, kill_by_signal};

pub use wait::WaitQueue;

pub use lifecycle::{
    add_user_process, find_zombie_child, find_zombie_child_excluding, get_ppid, reap_group_siblings,
    group_exiting, group_still_on_cpu, group_threads, has_children, init_ap_idle, kill_group_threads, kill_group_with_code,
    init_main_thread, kill, kill_with_code, process_alive, process_exists, reap_zombie,
    set_affinity, set_parent, set_priority, spawn, spawn_child_of, spawn_named, stop_if_dead,
    spawn_named_child, spawn_named_child_of, started, update_process_cr3,
};

pub use proc_index::{alloc_pid, find_task, for_each_task, free_pid, proc_index_raw, MAX_PROCS};

pub use stats::{cpu_ticks, debug_dump_all, get_stats, thread_count, ThreadStat};

pub use workers::{default_worker_count, init_workers, submit_task, Task};

// tiny inline accessors

#[inline]
pub fn current_pid() -> u64 {
    percpu::current().current_pid.load(Ordering::Relaxed)
}

/// Whether this CPU is running its idle task, which may yield but has no queue
/// to sleep off and come back from
#[inline]
pub fn current_is_idle() -> bool {
    let cpu = percpu::current();
    cpu.current_pid.load(Ordering::Relaxed) == cpu.idle_pid.load(Ordering::Relaxed)
}

/// Whether the running task has been marked dead and is still in the kernel
/// finishing something, which it must do without leaving the processor: a dead
/// task that switches away is not resumed (`kill_group_threads`)
#[inline]
pub fn current_is_dead() -> bool {
    let p = unsafe { proc_index::PROC_INDEX.get_raw(current_pid()) };
    !p.is_null() && unsafe { (*p).state.load(Ordering::Relaxed) } == crate::process::STATE_DEAD
}

// Threads. These three functions tell the rest of the kernel which one it is
// asking about.
//
// Per-task state is read from the task, process-wide state from the leader. A
// thread has its own kernel stack, registers, `FS_BASE` and FPU area; it shares
// the descriptor table, the working directory, the credential, the terminal and
// the job. Every accessor below reaching through `leader_raw` is on the second
// list, and those using `PROC_INDEX.get_raw` directly are on the first.

/// The thread group of `pid` - its own pid unless it is a thread.
#[inline]
pub fn tgid_of(pid: u64) -> u64 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return pid; }
    unsafe { (*ptr).tgid() }
}

/// The parent of `pid` as the index has it, or `None` when `pid` is not in
/// it. Ask with a thread group's leader (`tgid_of`) for a process's parent
#[inline]
pub fn ppid_of(pid: u64) -> Option<u64> {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return None; }
    Some(unsafe { (*ptr).ppid.load(Ordering::Relaxed) })
}

/// The thread group of the running task. What the VFS keys descriptors on.
#[inline]
pub fn current_tgid() -> u64 {
    tgid_of(current_pid())
}

/// The `Process` holding this task's process-wide state.
///
/// Falls back to the task itself when the leader has gone from the index: a
/// thread outliving its leader, for the instant between the leader being
/// cleared and the group being torn down. Its own (stale but real) fields beat
/// a null.
#[inline]
unsafe fn leader_raw(pid: u64) -> *mut crate::process::Process {
    let ptr = proc_index::PROC_INDEX.get_raw(pid);
    if ptr.is_null() { return ptr; }
    let tg = (*ptr).tgid();
    if tg == pid { return ptr; }
    let lead = proc_index::PROC_INDEX.get_raw(tg);
    if lead.is_null() { ptr } else { lead }
}

/// True when `pid` names a thread rather than a whole process. `ps`, `wait4`
/// and the reaper each need to know
pub fn is_thread(pid: u64) -> bool {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return false; }
    unsafe { (*ptr).is_thread() }
}

// Per-process cwd accessors. cwd is a u64 InodeId on the Process struct, read
// and written atomically without taking PROC_TABLE from hot paths

#[inline]
/// Bounds of the kernel stack the running thread is on, for the backtrace
/// walk. None before the scheduler exists, which is the one case where the
/// walk has to bound itself
pub fn current_kernel_stack_bounds() -> Option<(u64, u64)> {
    kernel_stack_bounds(current_pid())
}

/// What a process is called, for reports that want to say more than a pid, by
/// value: `process::crash::report_user` holds it across a symbol-table load
/// from the VFS
pub fn name_of(pid: u64) -> Option<crate::process::Name> {
    let ptr = unsafe { proc_index_raw(pid) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { (*ptr).name })
}

/// Renames a process, which `exec` does to the one it replaces
///
/// Through the raw index like its neighbours: the field is a plain inline
/// buffer written by the one thread inside its own `exec`, and readers see the
/// old or new bytes of a 24-byte copy, never a dangling reference
pub fn set_name(pid: u64, name: crate::process::Name) {
    let ptr = unsafe { proc_index_raw(pid) };
    if ptr.is_null() {
        return;
    }
    unsafe { (*ptr).name = name };
}

pub fn kernel_stack_bounds(pid: u64) -> Option<(u64, u64)> {
    let ptr = unsafe { proc_index_raw(pid) };
    if ptr.is_null() {
        return None;
    }
    Some(unsafe { (*ptr).stack.bounds() })
}

pub fn cwd_of(pid: u64) -> u64 {
    // Process-wide: `chdir` in one thread moves the whole program
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).cwd.load(Ordering::Relaxed) }
}


#[inline]
pub fn current_cwd() -> u64 {
    cwd_of(current_pid())
}

/// A task's userspace signal entry point, or 0 if it has none
///
/// Beside `cwd_of` and `ctty_of`: inherited, since `fork` makes a copy of the
/// process
pub fn sig_entry_of(pid: u64) -> u64 {
    // Process-wide: a handler is installed once, by whichever thread got there
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).sig_entry.load(Ordering::Relaxed) }
}

/// What a task wants done about each signal, packed two bits per signal
///
/// Inherited, like `sig_entry_of`
pub fn sig_disp_of(pid: u64) -> u64 {
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).sig_disp.load(Ordering::Relaxed) }
}

/// Which signals' handlers asked for the alternate stack (`SA_ONSTACK`), one
/// bit per signal. Process-wide, like the disposition it rides beside
pub fn sig_onstack_of(pid: u64) -> u32 {
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).sig_onstack.load(Ordering::Relaxed) }
}

/// The `Process` holding `pid`'s process-wide state, for a writer of a field
/// with no setter of its own: `signal` writes the disposition table and the
/// `SA_ONSTACK` mask through it. Readers want the `*_of` functions above
pub unsafe fn leader_ptr(pid: u64) -> *mut crate::process::Process {
    leader_raw(pid)
}

/// The job a task belongs to. 0 only for the idle task, which is in none
pub fn pgid_of(pid: u64) -> u64 {
    // Process-wide: a job is made of processes, not of threads
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).pgid.load(Ordering::Relaxed) }
}

/// Moves a task into a job. False when there is no such task
///
/// No policy here: who may move whom into what is `sys_setpgid`'s question
pub fn set_pgid(pid: u64, pgid: u64) -> bool {
    // The leader's field, and the job index with it
    proc_index::set_job(tgid_of(pid), pgid)
}

/// Members of a job, from `start` onwards, into a caller-provided buffer
///
/// Answers with how many were written and where to carry on from, so a job
/// larger than the buffer is walked in passes.
///
///   - No allocation: this is on the path from a Ctrl-C to a signal, reached
///     from the keyboard thread.
///   - It collects rather than acts. The caller signals after this returns,
///     because a signal kills, and a process dying underneath a walk leaves it
///     reading a freed slot.
///
/// From the job index (`proc_index::job_members`), in pid order: `start` is a
/// pid, and a member that died between two passes moves nobody else out of the
/// walk. Processes, not threads: a signal goes to a process.
///
/// Dead tasks are skipped: a zombie is still in the index
pub fn group_members(pgid: u64, start: usize, out: &mut [u64]) -> (usize, usize) {
    if pgid == 0 || out.is_empty() { return (0, MAX_PROCS); }
    let mut n = 0usize;
    let next = proc_index::job_members(pgid, start as u64, |pid| {
        if n == out.len() { return false; }
        if alive(pid) {
            out[n] = pid;
            n += 1;
        }
        true
    });
    (n, next.map_or(MAX_PROCS, |pid| pid as usize))
}

/// Is there a live task in this job other than `except`?
///
/// Asked when a process dies: the terminal's foreground is a job, vacant only
/// once the last member has gone
pub fn group_alive(pgid: u64, except: u64) -> bool {
    if pgid == 0 { return false; }
    let mut found = false;
    proc_index::job_members(pgid, 0, |pid| {
        found = pid != except && alive(pid);
        !found
    });
    found
}

/// In the index and not dead, read without the table lock, as the job walks
/// above hold the job index's
fn alive(pid: u64) -> bool {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    !ptr.is_null() && unsafe { (*ptr).state.load(Ordering::Relaxed) } != crate::process::STATE_DEAD
}

/// The terminal a task's output goes to, as a raw `TtyId`. 0 = the console
pub fn ctty_of(pid: u64) -> u32 {
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).ctty.load(Ordering::Relaxed) }
}

/// Puts a task on a terminal. Takes effect for its next print; the running
/// task's own is the percpu copy below, which the context switch keeps
pub fn set_ctty(pid: u64, tty: u32) {
    // Every task in the group, not just the leader. The other process-wide
    // fields are read through `leader_raw`, but the context switch reads
    // `next.ctty` directly to set the percpu terminal, and a leader lookup
    // would put a table walk in the switch path. So the field is per-task and
    // kept in step here, where it changes once in a program's life.
    let tg = tgid_of(pid);
    for t in group_threads(tg) {
        let ptr = unsafe { proc_index::PROC_INDEX.get_raw(t) };
        if !ptr.is_null() {
            unsafe { (*ptr).ctty.store(tty, Ordering::Relaxed) };
        }
    }
    if tgid_of(current_pid()) == tg {
        crate::arch::x86_64::percpu::current()
            .current_tty.store(tty, Ordering::Relaxed);
    }
}

/// Where the running task's output goes, read without a table lookup
///
/// `print!` calls this, on the path of every line the kernel writes, some from
/// inside an interrupt. The percpu slot is swapped by the context switch beside
/// `current_pid`
#[inline]
pub fn current_tty() -> u32 {
    crate::arch::x86_64::percpu::current()
        .current_tty.load(Ordering::Relaxed)
}

#[inline]
pub fn set_current_cwd(id: u64) {
    let ptr = unsafe { leader_raw(current_pid()) };
    if ptr.is_null() { return; }
    unsafe { (*ptr).cwd.store(id, Ordering::Relaxed) };
}

// Per-process identity. Used by fork to copy the parent's credential, by
// 'with_vfs' to sync vfs.ctx before a syscall closure runs, and by every
// privileged call site.
//
// Two answers exist for "who is pid N":
//
//   cred_of(pid)      root when there is no such process. Correct for a process
//                     asking about itself, and for pre-init callers with no PCB.
//   cred_checked(pid) None when there is no such process. For code deciding
//                     something about another pid: a pid that has exited would
//                     otherwise be promoted to root on its way out.

#[inline]
pub fn cred_of(pid: u64) -> crate::process::cred::Cred {
    // Process-wide: a thread holding a different identity from its siblings
    // would be a privilege boundary inside one address space, which is no
    // boundary
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return crate::process::cred::Cred::root(); }
    *unsafe { (*ptr).cred.read() }
}

/// The same, for a caller that must not be told root when the answer is
/// "there is no such process"
#[inline]
pub fn cred_checked(pid: u64) -> Option<crate::process::cred::Cred> {
    let ptr = unsafe { leader_raw(pid) };
    if ptr.is_null() { return None; }
    Some(*unsafe { (*ptr).cred.read() })
}

#[inline]
pub fn current_cred() -> crate::process::cred::Cred {
    cred_of(current_pid())
}

/// Applies a change to the calling process's credential.
///
/// The mutation runs with the write lock held, so a reader sees the whole
/// change or none of it. Returns false when the caller has no PCB
pub fn update_current_cred<F: FnOnce(&mut crate::process::cred::Cred)>(f: F) -> bool {
    let ptr = unsafe { leader_raw(current_pid()) };
    if ptr.is_null() { return false; }
    let mut guard = unsafe { (*ptr).cred.write() };
    f(&mut guard);
    true
}

/// True when the calling process holds `cap` effectively. The form every
/// privileged syscall uses
#[inline]
pub fn current_has_cap(cap: u8) -> bool {
    current_cred().has(cap)
}

#[inline]
pub fn umask_of(pid: u64) -> u16 { cred_of(pid).umask }

#[inline]
pub fn set_current_umask(mask: u16) {
    update_current_cred(|c| c.umask = mask);
}

#[inline]
pub fn uid_of(pid: u64) -> u16 { cred_of(pid).uid }

#[inline]
pub fn gid_of(pid: u64) -> u16 { cred_of(pid).gid }

#[inline]
pub fn euid_of(pid: u64) -> u16 { cred_of(pid).euid }

#[inline]
pub fn euid_checked(pid: u64) -> Option<u16> { cred_checked(pid).map(|c| c.euid) }

#[inline]
pub fn egid_of(pid: u64) -> u16 { cred_of(pid).egid }

/// When this process started, in ticks.
///
/// Paired with a pid it makes an identity that survives pid reuse: two
/// processes sharing a pid cannot also share the tick they began on.
/// Local-socket peer credentials record both
#[inline]
pub fn start_tick_of(pid: u64) -> u64 {
    let ptr = unsafe { proc_index::PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return 0; }
    unsafe { (*ptr).wall_start_tick }
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
    timer::clear_all();
    lifecycle::forget_dead();
    proc_index::PROC_INDEX.clear_all();
    proc_index::reset_pids();

    for i in 0..percpu::MAX_CPUS {
        let c = percpu::get(i);
        c.current_pid .store(0, Ordering::Relaxed);
        c.idle_pid    .store(0, Ordering::Relaxed);
        c.min_vruntime.store(0, Ordering::Relaxed);
        c.run_queue.with(|inner| inner.heap.clear());
    }

    interrupts::without_interrupts(|| {
        proc_index::PROC_TABLE.lock().clear();
    });
    workers::drain_queue();
}
