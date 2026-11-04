// Per-CPU run queue primitives and CPU selection
//
// Each CPU owns a RunQueueInner, a min-heap by vruntime with ties by pid
// (`sched::slot_heap`, `dev/kernel-core.md` "The run queue"), guarded by its
// per-CPU spinlock. The functions here are the only code touching a queue, and
// they keep `Process::rq_cpu` true: a task names the queue it is in, under that
// queue's lock

use core::sync::atomic::Ordering;

use crate::arch::x86_64::percpu::{self, Cpu, RunQueueInner};
use crate::process::{Process, CPU_ALL};

use super::slot_heap::NO_CPU;

// CFS-style nice→weight table for priorities 1..=20
static PRIO_WEIGHT: [u64; 20] = [
    88761, 71755, 56483, 46273, 36291,
    29154, 23254, 18705, 14949, 11916,
     9548,  7620,  6100,  4904,  3906,
     3121,  2501,  1991,  1586,  1277,
];

#[inline]
pub fn weight(priority: u8) -> u64 {
    PRIO_WEIGHT[priority.clamp(1, 20) as usize - 1]
}

/// How far past a processor's `min_vruntime` a task arriving there may sit:
/// four ticks of a task at the default priority (10)
const PLACE_AHEAD: u64 = 4 * super::core_sched::TICK_SCALE / PRIO_WEIGHT[9];

/// A task's vruntime as it joins `cpu`'s queue from anywhere but that queue (a
/// wake, an expired sleep, a continue, a steal): into
/// `[min, min + PLACE_AHEAD]` of that processor.
///
/// Every processor counts vruntime on a scale of its own, `min_vruntime` being
/// where its tasks have got to. A task carrying its number from one to the next
/// could queue behind a runner it is "ahead" of by seconds, or keep a lead over
/// the new processor. Linux renormalises on migration; one window for every
/// arrival does the same for both directions, at the cost of a sleeper's credit
/// or a hog's debt past four ticks, as in CFS
pub fn place_on(cpu: &Cpu, p: &Process) {
    let min = cpu.min_vruntime.load(Ordering::Relaxed);
    let vr = p.vruntime.load(Ordering::Relaxed).clamp(min, min.saturating_add(PLACE_AHEAD));
    p.vruntime.store(vr, Ordering::Relaxed);
}

//         queue ops on a held RunQueueInner

/// `p` onto `inner`, the queue of CPU `cpu`, at its vruntime
///
/// Idempotent: re-pushing a task already queued is a no-op. The idle task is
/// refused: the scheduler hands it out through the per-CPU `idle_pid` and never
/// from a queue
unsafe fn insert(inner: &mut RunQueueInner, cpu: u32, p: *mut Process) {
    if (*p).is_idle || (*p).on_rq.load(Ordering::Relaxed) {
        return;
    }
    (*p).on_rq.store(true, Ordering::Relaxed);
    inner.heap.push(p, (*p).vruntime.load(Ordering::Relaxed), (*p).pid);
    (*p).rq_cpu.store(cpu, Ordering::Release);
}

/// The lowest-vruntime task, off `inner`
unsafe fn take_min(inner: &mut RunQueueInner) -> Option<*mut Process> {
    let p = inner.heap.pop()?;
    // Held from here until the picker has it on a CPU or lets it go: see
    // `Process::in_switch`. Before `on_rq` drops, so there is no moment with
    // neither set
    (*p).in_switch.store(true, Ordering::SeqCst);
    (*p).rq_cpu.store(NO_CPU, Ordering::Release);
    (*p).on_rq.store(false, Ordering::SeqCst);
    Some(p)
}

// locked CPU-level helpers

/// `p` onto `cpu`'s queue
pub fn push_to_cpu(cpu: &Cpu, p: *mut Process) {
    let idx = cpu.cpu_index;
    cpu.run_queue.with(|inner| unsafe { insert(inner, idx, p) });
}

/// The lowest-vruntime task on `cpu`'s queue, taken off it and marked
/// `in_switch`, for the picker to claim or let go
pub unsafe fn pop_min(cpu: &Cpu) -> Option<*mut Process> {
    cpu.run_queue.with(|inner| take_min(inner))
}

/// The vruntime the next pick on `cpu` would run at, as it was queued
pub fn peek_min_vr(cpu: &Cpu) -> Option<u64> {
    cpu.run_queue.with(|inner| inner.heap.first().map(|(_, vr)| vr))
}

/// Whether anyone waits on `cpu`'s queue. Idle never does
pub fn has_waiting(cpu: &Cpu) -> bool {
    cpu.run_queue.with(|inner| inner.len() > 0)
}

/// Steal a runnable task from any other CPU's queue. try_with avoids
/// blocking on a peer whose ISR currently holds the lock
pub unsafe fn try_steal_task(my_idx: usize) -> Option<*mut Process> {
    let total = percpu::cpu_count().min(percpu::MAX_CPUS);
    for i in 0..total {
        if i == my_idx { continue; }
        let peer = percpu::get(i);
        if peer.online.load(Ordering::Relaxed) == 0 { continue; }
        let stolen = peer.run_queue.try_with(|inner| take_min(inner)).flatten();
        if let Some(p) = stolen {
            if (*p).cpu_mask & (1u64 << my_idx) != 0 {
                place_on(percpu::get(my_idx), &*p);
                return Some(p);
            }
            // Affinity forbids running here: put it back on its origin
            push_to_cpu(peer, p);
            (*p).in_switch.store(false, Ordering::SeqCst);
        }
    }
    None
}

/// `p` off whichever CPU's queue holds it, if any: straight to the queue
/// its `rq_cpu` names, and again if it moved before that queue's lock was
/// taken
pub fn remove(p: *mut Process) {
    loop {
        let c = unsafe { (*p).rq_cpu.load(Ordering::Acquire) };
        if c == NO_CPU { return; }
        let moved = percpu::get(c as usize).run_queue.with(|inner| unsafe {
            if (*p).rq_cpu.load(Ordering::Relaxed) != c { return true; }
            let i = (*p).rq_slot.load(Ordering::Relaxed) as usize;
            if inner.heap.holds(i, p) {
                inner.heap.remove(i);
            } else {
                // Cannot be: the two are written together under this lock
                crate::kerror!("[sched] pid {} names cpu {} slot {} and is not there",
                    (*p).pid, c, i);
            }
            (*p).rq_cpu.store(NO_CPU, Ordering::Release);
            (*p).on_rq.store(false, Ordering::Relaxed);
            false
        });
        if !moved { return; }
    }
}

/// Pick the lightest CPU permitted by mask, or fall back to the current
/// CPU during early boot when nothing else is online yet
pub fn pick_cpu_for(mask: u64) -> usize {
    let m = if mask == 0 { CPU_ALL } else { mask };
    let idx = percpu::pick_lightest(m);
    if percpu::get(idx).online.load(Ordering::Relaxed) == 0 {
        percpu::current_index()
    } else {
        idx
    }
}
