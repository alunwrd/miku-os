// Sleeping tasks, by when they wake (`dev/kernel-core.md`, "Time")
//
// Every CPU has a min-heap of the tasks that went to sleep on it, keyed by
// `Process::wake_ns`, and its timer interrupt is armed for the soonest of them
// or the next tick, whichever comes first (`apic::arm_timer`). The heap's slot
// for a task lives in the task (`sched::slot_heap`), so taking one out early (a
// wake, a kill) is O(log n).
//
// The rules that keep a freed task out of a heap:
//   - a task goes in from `apply_pending`, on its own CPU, before its state
//     says
//     SLEEPING, so whoever wins SLEEPING -> anything finds it in;
//   - it comes out on expiry, in `wakeup`, and in `kill_with_code`;
//   - `lifecycle::quiesced` counts a task in a heap as still in use.

use core::sync::atomic::Ordering;

use crate::arch::x86_64::percpu::MAX_CPUS;
use crate::kcore::irq_lock::IrqMutex;
use crate::process::Process;

use super::slot_heap::SlotHeap;

/// `timer_cpu` of a task that is in no heap
pub const NONE: u32 = super::slot_heap::NO_CPU;

static HEAPS: [IrqMutex<SlotHeap>; MAX_CPUS] =
    [const { IrqMutex::new(SlotHeap::new(|p| &p.timer_slot)) }; MAX_CPUS];

/// Into CPU `cpu`'s heap at its `wake_ns`. Out of whichever heap it was in
/// first, so that a task is never in two. True when it is now the soonest
///
/// # Safety
/// `p` is a live task
pub unsafe fn link(p: *mut Process, cpu: usize) -> bool {
    unsafe { unlink(p) };
    let mut h = HEAPS[cpu].lock();
    unsafe {
        h.push(p, (*p).wake_ns.load(Ordering::Relaxed), 0);
        (*p).timer_cpu.store(cpu as u32, Ordering::Release);
        (*p).timer_slot.load(Ordering::Relaxed) == 0
    }
}

/// Out of whichever heap `p` is in, if any
///
/// # Safety
/// `p` is a live task
pub unsafe fn unlink(p: *mut Process) {
    loop {
        let c = unsafe { (*p).timer_cpu.load(Ordering::Acquire) };
        if c == NONE { return; }
        let mut h = HEAPS[c as usize].lock();
        // Moved to another heap between the look and the lock
        if unsafe { (*p).timer_cpu.load(Ordering::Relaxed) } != c { continue; }
        unsafe {
            let i = (*p).timer_slot.load(Ordering::Relaxed) as usize;
            h.remove(i);
            (*p).timer_cpu.store(NONE, Ordering::Release);
        }
        return;
    }
}

/// Whether `p` is in a heap, for `lifecycle::quiesced`
pub fn linked(p: &Process) -> bool {
    p.timer_cpu.load(Ordering::Acquire) != NONE
}

/// The soonest deadline on CPU `cpu`, if anyone sleeps there
pub fn soonest(cpu: usize) -> Option<u64> {
    HEAPS[cpu].lock().first().map(|(_, k)| k)
}

/// Every task on CPU `cpu` whose deadline is at or before `now`, out of the
/// heap and into `out`. Returns how many. Whatever did not fit stays, and
/// `soonest` arms the timer for it at once
pub fn expire(cpu: usize, now: u64, out: &mut [*mut Process]) -> usize {
    let mut h = HEAPS[cpu].lock();
    let mut n = 0;
    while n < out.len() {
        let Some((p, k)) = h.first() else { break };
        if k > now { break; }
        unsafe {
            h.remove(0);
            (*p).timer_cpu.store(NONE, Ordering::Release);
        }
        out[n] = p;
        n += 1;
    }
    n
}

/// How many tasks sleep on each CPU, for `sched` reports
pub fn sleepers(cpu: usize) -> usize {
    HEAPS[cpu].lock().len()
}

/// Every heap emptied, for a scheduler starting over (`reinit_scheduler`):
/// the tasks they pointed at are being thrown away with the table
pub fn clear_all() {
    for h in HEAPS.iter() {
        h.lock().clear();
    }
}
