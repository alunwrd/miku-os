// Voluntary state transitions: yield, sleep, block, wakeup, waitpid
//
// Each records the current task's wanted state under interrupts-off and calls
// software_context_switch to take the scheduler path the timer ISR uses

// None of these may write 'state' or touch a run queue directly. A task that is
// READY and queued (or SLEEPING/BLOCKED and so wakeable) is visible to every
// other CPU, which may resume it from 'rsp', and 'rsp' is only written once
// software_context_switch has built the frame. Publishing first would let a
// second CPU run the task on the stack the first is still yielding on. So the
// transition is recorded in 'pending_state' and applied by schedule_from_isr
// after the context is saved; see core_sched::apply_pending

use core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::arch::x86_64::percpu;
use crate::process::{STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING};

use super::core_sched::{software_context_switch, TICK_SCALE};
use super::proc_index::PROC_INDEX;
use super::runqueue::{pick_cpu_for, place_on, push_to_cpu, weight};

pub fn yield_now() {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        // The idle task must never enter the run queue: it is the fallback when
        // the queue is empty, and queueing it lets pop_min hand the CPU back to
        // idle while real work is waiting
        if p.state.load(Ordering::Relaxed) == STATE_RUNNING && !p.is_idle {
            let min_vr = cpu.min_vruntime.load(Ordering::Relaxed);
            let cur_vr = p.vruntime.load(Ordering::Relaxed);
            if cur_vr < min_vr { p.vruntime.store(min_vr, Ordering::Relaxed); }
            // Charges the yielding task a virtual time slice. Without this,
            // tasks sharing a vruntime with a fresh joiner deadlock the queue:
            // tie-break is by PID, the smaller-PID incumbent always wins
            // pop_min, and the new task starves
            let w = weight(p.priority.load(Ordering::Relaxed));
            p.vruntime.fetch_add(TICK_SCALE / w, Ordering::Relaxed);
            p.pending_state.store(STATE_READY, Ordering::Relaxed);
        }
    });
    unsafe { software_context_switch() }
}

/// Sleeps until the `ticks`th tick from now: a backstop or a poll that counts
/// in 4 ms steps. `sleep_ns` means a length of time
pub fn sleep(ticks: u64) {
    sleep_until(tick_deadline(ticks));
}

/// Sleep for `ns` nanoseconds, or until woken
pub fn sleep_ns(ns: u64) {
    sleep_until(crate::kcore::time::now_ns().saturating_add(ns));
}

/// Sleep until `kcore::time::now_ns` reaches `deadline`, or until woken. The
/// task goes into its CPU's heap of sleepers at the switch (`sched::timer`)
pub fn sleep_until(deadline: u64) {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.wake_ns.store(deadline, Ordering::Relaxed);
        p.pending_state.store(STATE_SLEEPING, Ordering::Relaxed);
    });
    unsafe { software_context_switch() }
}

/// `sleep_until`, unless `ready()`, asked once the sleep has been asked for,
/// says the wait is over. Returns whether it slept
///
/// A `wakeup` after the request cancels it, and a change before the request is
/// what `ready` sees, so a wake cannot fall between the two. Looking first and
/// asking second, as a `WaitQueue::wait` caller does, leaves that gap to the
/// backstop.
///
/// `ready` runs with the request recorded: it must not yield or sleep, which
/// would overwrite it. A load of an atomic is what it is for
pub fn sleep_until_unless(deadline: u64, ready: impl FnOnce() -> bool) -> bool {
    let me = interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let ptr  = unsafe { PROC_INDEX.get_raw(cpu.current_pid.load(Ordering::Relaxed)) };
        if !ptr.is_null() {
            let p = unsafe { &*ptr };
            p.wake_ns.store(deadline, Ordering::Relaxed);
            p.pending_state.store(STATE_SLEEPING, Ordering::SeqCst);
        }
        ptr
    });
    if me.is_null() {
        return false;
    }
    if ready() {
        // A waker may have cancelled it first; either way it is not asked now
        let _ = unsafe { &*me }.pending_state.compare_exchange(
            STATE_SLEEPING, crate::process::PENDING_NONE, Ordering::AcqRel, Ordering::Relaxed,
        );
        return false;
    }
    unsafe { software_context_switch() }
    true
}

/// The wakes the running task has been sent (`Process::wakes`)
pub fn wake_count() -> u64 {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(percpu::current().current_pid.load(Ordering::Relaxed)) };
        if ptr.is_null() { 0 } else { unsafe { &*ptr }.wakes.load(Ordering::SeqCst) }
    })
}

/// When the `ticks`th tick from now is, on this CPU, as a deadline
pub fn tick_deadline(ticks: u64) -> u64 {
    let hz = (crate::arch::x86_64::apic::timer_hz() as u64).max(1);
    let period = 1_000_000_000 / hz;
    let now = crate::kcore::time::now_ns();
    let next = crate::arch::x86_64::apic::next_tick_ns(percpu::current_index());
    if ticks == 0 {
        now
    } else if next > now {
        next + (ticks - 1) * period
    } else {
        now + ticks * period
    }
}

pub fn block_current(cause: &'static str) {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.blocked_cause.store(cause.as_ptr() as *mut u8, Ordering::Relaxed);
        p.pending_state.store(STATE_BLOCKED, Ordering::Relaxed);
    });
    unsafe { software_context_switch() }
}

/// Takes the caller off the CPU until somebody continues it
///
/// What SIGSTOP and SIGTSTP do. Through `pending_state` like `sleep` and
/// `block_current`: writing `state` from the task itself races the switch
/// putting it away, and on two CPUs that is one stack used twice.
///
/// Only the caller: see `request_stop`, which asks and lets the target do it at
/// its next scheduling point
pub fn stop_current() {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.stop_pending.store(true, Ordering::Release);
    });
    unsafe { software_context_switch() }
}

/// Puts a stopped process back on a run queue
///
/// What SIGCONT does, and the only way out of `STATE_STOPPED`: `wakeup`
/// refuses, because `send_signal` wakes its target after every signal.
///
/// A process not stopped is left alone: SIGCONT to something running is a no-op
pub fn cont(pid: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };

        // It may have asked to stop and not reached the switch. A continue
        // arriving before the stop is applied cancels it, and the task keeps
        // running
        if p.stop_pending.swap(false, Ordering::AcqRel) {
            return;
        }

        if p.state.compare_exchange(
            crate::process::STATE_STOPPED, STATE_READY,
            Ordering::AcqRel, Ordering::Relaxed,
        ).is_err() {
            return;
        }

        // Onto a queue with a vruntime that does not let a long stop turn into
        // a long turn at the front, the same rule as `wakeup`
        let target = pick_cpu_for(p.cpu_mask);
        let cpu    = percpu::get(target);
        place_on(cpu, p);
        push_to_cpu(cpu, ptr);

        if target != percpu::current_index() {
            super::idle::kick(cpu);
        }
    });
}

/// Is this process stopped?
pub fn is_stopped(pid: u64) -> bool {
    let ptr = unsafe { PROC_INDEX.get_raw(pid) };
    if ptr.is_null() { return false; }
    unsafe { (*ptr).state.load(Ordering::Relaxed) == crate::process::STATE_STOPPED }
}

/// Whether it did anything: false for a task neither asleep nor asking to be,
/// or one that is gone. `WaitQueue::wake_one` needs this to try the next waiter
pub fn wakeup(pid: u64) -> bool {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return false; }
        let p = unsafe { &*ptr };
        // Counted before anything is decided: a task still looking at its
        // condition, asking nothing yet, sees this and does not sleep
        // (`wait::join`)
        p.wakes.fetch_add(1, Ordering::SeqCst);

        // The target may have asked to sleep or block but not reached the
        // switch applying it. It is still RUNNING, so the state check below
        // would ignore this wakeup and the task would sleep just after, never
        // woken. Cancelling the request means the task keeps running. If the
        // switch got there first the CAS fails and the path below applies
        for want in [STATE_SLEEPING, STATE_BLOCKED] {
            if p.pending_state.compare_exchange(
                want, crate::process::PENDING_NONE,
                Ordering::AcqRel, Ordering::Relaxed,
            ).is_ok() {
                return true;
            }
        }

        // A stopped process is not woken here: `send_signal` calls this after
        // every signal it sends. The way out is `cont`
        let state = p.state.load(Ordering::Relaxed);
        if state != STATE_SLEEPING && state != STATE_BLOCKED { return false; }

        // CAS to avoid racing the timer's expiry (`expire_sleepers`): whichever
        // lands SLEEPING -> READY first queues the task
        let won = p.state.compare_exchange(
            state, STATE_READY,
            Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok();
        if !won { return false; }
        // Out of its timer heap before it is on a queue: once queued, it can
        // run and sleep again, and this must not take that sleep's entry out
        if state == STATE_SLEEPING { unsafe { super::timer::unlink(ptr) }; }

        // Placed only once the task is this wake's to queue: a wake losing the
        // CAS to the timer must not write the vruntime of a task already queued
        let target = pick_cpu_for(p.cpu_mask);
        let cpu    = percpu::get(target);
        place_on(cpu, p);

        push_to_cpu(cpu, ptr);

        if target != percpu::current_index() {
            super::idle::kick(cpu);
        }
        true
    })
}

/// Blocks the caller until pid reaches STATE_DEAD or disappears from the index.
/// Not from an ISR
pub fn waitpid(pid: u64) {
    let alive = || {
        interrupts::without_interrupts(|| {
            let ptr = unsafe { PROC_INDEX.get_raw(pid) };
            if ptr.is_null() { return false; }
            let state = unsafe { &*ptr }.state.load(Ordering::Relaxed);
            state != STATE_DEAD
        })
    };
    while alive() {
        // Off the run queue until somebody dies. `kill_with_code` wakes the
        // parent by pid and everything on this queue, so a waiter that is not
        // the parent (the shell waiting on a program it did not fork) is woken
        // too. Looked at again once the sleep is asked for, so a death between
        // is not left to the backstop
        let deadline = crate::kcore::time::now_ns().saturating_add(DEATH_BACKSTOP_NS);
        super::wait::DEATH.wait_unless(deadline, || !alive());
    }
}

/// How long a `waitpid` sleeps when no death wakes it
const DEATH_BACKSTOP_NS: u64 = 100_000_000;
