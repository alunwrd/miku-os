// Voluntary state transitions: yield, sleep, block, wakeup, waitpid
//
// All four functions follow the same recipe - flip the current task's
// state under interrupts-off, then call into software_context_switch to
// trigger the same scheduler path the timer ISR uses

use core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::percpu;
use crate::process::{STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING};

use super::core_sched::{
    sleeper_dec_saturating, sleeper_inc, software_context_switch, TICK_SCALE,
};
use super::proc_index::PROC_INDEX;
use super::runqueue::{pick_cpu_for, rq_push_sorted, weight};

pub fn yield_now() {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        if p.state.load(Ordering::Relaxed) == STATE_RUNNING {
            let min_vr = cpu.min_vruntime.load(Ordering::Relaxed);
            let cur_vr = p.vruntime.load(Ordering::Relaxed);
            if cur_vr < min_vr { p.vruntime.store(min_vr, Ordering::Relaxed); }
            // Charge the yielding task a virtual time slice. Without this,
            // tasks that share a vruntime with a fresh joiner deadlock the
            // queue: tie-break is by PID, the smaller-PID incumbent always
            // wins pop_min, and the new task starves. The timer ISR would
            // normally do this accounting on preemption, but our timer is
            // an extern "x86-interrupt" stub that cannot drive the
            // scheduler - yield_now is the only entry point we have
            let w = weight(p.priority.load(Ordering::Relaxed));
            p.vruntime.fetch_add(TICK_SCALE / w, Ordering::Relaxed);
            p.state.store(STATE_READY, Ordering::Relaxed);
            cpu.run_queue.with(|inner| unsafe { rq_push_sorted(inner, ptr) });
        }
    });
    unsafe { software_context_switch() }
}

pub fn sleep(ticks: u64) {
    let wake_tick = crate::interrupts::get_tick() + ticks;
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.sleep_until.store(wake_tick, Ordering::Relaxed);
        let prev = p.state.swap(STATE_SLEEPING, Ordering::Relaxed);
        if prev != STATE_SLEEPING { sleeper_inc(); }
    });
    unsafe { software_context_switch() }
}

pub fn block_current(cause: &'static str) {
    interrupts::without_interrupts(|| {
        let cpu  = percpu::current();
        let curr = cpu.current_pid.load(Ordering::Relaxed);
        let ptr  = unsafe { PROC_INDEX.get_raw(curr) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.blocked_cause.store(cause.as_ptr() as *mut u8, Ordering::Relaxed);
        p.state.store(STATE_BLOCKED, Ordering::Relaxed);
    });
    unsafe { software_context_switch() }
}

pub fn wakeup(pid: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        let p     = unsafe { &*ptr };
        let state = p.state.load(Ordering::Relaxed);
        if state != STATE_SLEEPING && state != STATE_BLOCKED { return; }

        let target = pick_cpu_for(p.cpu_mask);
        let cpu    = percpu::get(target);
        let min_vr = cpu.min_vruntime.load(Ordering::Relaxed);
        let vr     = p.vruntime.load(Ordering::Relaxed).max(min_vr);
        p.vruntime.store(vr, Ordering::Relaxed);
        // CAS to avoid racing wake_sleepers_isr; whichever lands the
        // SLEEPING->READY transition first owns the SLEEPER_COUNT dec
        let won = p.state.compare_exchange(
            state, STATE_READY,
            Ordering::AcqRel, Ordering::Relaxed,
        ).is_ok();
        if !won { return; }
        if state == STATE_SLEEPING { sleeper_dec_saturating(); }

        cpu.run_queue.with(|inner| unsafe { rq_push_sorted(inner, ptr) });

        if target != percpu::current_index() {
            crate::apic::send_ipi(
                cpu.lapic_id.load(Ordering::Relaxed),
                crate::apic::VEC_IPI_RESCHED,
            );
        }
    });
}

/// Block the caller until pid reaches STATE_DEAD or disappears from the
/// index. Polls via yield_now, so it must not be called from an ISR
pub fn waitpid(pid: u64) {
    loop {
        let alive = interrupts::without_interrupts(|| {
            let ptr = unsafe { PROC_INDEX.get_raw(pid) };
            if ptr.is_null() { return false; }
            let state = unsafe { &*ptr }.state.load(Ordering::Relaxed);
            state != STATE_DEAD
        });
        if !alive { break; }
        yield_now();
    }
}
