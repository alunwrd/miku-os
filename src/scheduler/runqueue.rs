// Per-CPU run queue primitives and CPU selection
//
// Each CPU owns a RunQueueInner (singly-linked min-vruntime list) guarded
// by its per-CPU spinlock. The functions here are the only code permitted
// to touch a queue's links - keeping the unsafe surface area small

use core::ptr::null_mut;
use core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::percpu::{self, Cpu, RunQueueInner};
use crate::process::{Process, CPU_ALL};

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

//         low-level list ops on a held RunQueueInner

/// Insert 'p' into the queue sorted by vruntime, breaking ties by PID
///
/// Idempotent: re-pushing a process already linked into this queue is a
/// no-op. That guard exists because earlier crashes traced back to a path
/// where the BSP idle was picked via the None-branch fallback (which does
/// not dequeue), and a subsequent yield_now would push it again - making
/// the list circular and looping the scheduler forever
pub unsafe fn rq_push_sorted(inner: &mut RunQueueInner, p: *mut Process) {
    if (*p).on_rq.load(Ordering::Relaxed) {
        return;
    }
    let p_vr  = (*p).vruntime.load(Ordering::Relaxed);
    let p_pid = (*p).pid;
    (*p).rq_next.store(null_mut(), Ordering::Relaxed);
    (*p).on_rq.store(true, Ordering::Relaxed);

    // Empty list - O(1)
    if inner.head.is_null() {
        inner.head = p;
        inner.tail = p;
        inner.len  = 1;
        return;
    }

    // Insert-before-head - O(1)
    let h_vr  = (*inner.head).vruntime.load(Ordering::Relaxed);
    let h_pid = (*inner.head).pid;
    if p_vr < h_vr || (p_vr == h_vr && p_pid < h_pid) {
        (*p).rq_next.store(inner.head, Ordering::Relaxed);
        inner.head = p;
        inner.len += 1;
        return;
    }

    // Append-after-tail - O(1). This is the hot path for yielding tasks
    // whose vruntime has just grown past everyone else's
    if !inner.tail.is_null() {
        let t_vr  = (*inner.tail).vruntime.load(Ordering::Relaxed);
        let t_pid = (*inner.tail).pid;
        if p_vr > t_vr || (p_vr == t_vr && p_pid > t_pid) {
            (*inner.tail).rq_next.store(p, Ordering::Relaxed);
            inner.tail = p;
            inner.len += 1;
            return;
        }
    }

    // Middle insertion - walk forward
    let mut curr = inner.head;
    loop {
        let next = (*curr).rq_next.load(Ordering::Relaxed);
        if next.is_null() {
            (*curr).rq_next.store(p, Ordering::Relaxed);
            inner.tail = p;
            break;
        }
        let nv   = (*next).vruntime.load(Ordering::Relaxed);
        let npid = (*next).pid;
        if p_vr < nv || (p_vr == nv && p_pid < npid) {
            (*p).rq_next.store(next, Ordering::Relaxed);
            (*curr).rq_next.store(p, Ordering::Relaxed);
            break;
        }
        curr = next;
    }
    inner.len += 1;
}

/// Pop and return the lowest-vruntime non-idle task, or None if only idle
/// tasks remain (idle is never returned by this function - it lives in the
/// queue but the scheduler hands it out only via the per-CPU 'idle_pid')
pub unsafe fn rq_pop_min(inner: &mut RunQueueInner) -> Option<*mut Process> {
    if inner.head.is_null() { return None; }

    let mut prev: *mut Process = null_mut();
    let mut curr = inner.head;

    while !curr.is_null() {
        if !(*curr).is_idle {
            let next = (*curr).rq_next.load(Ordering::Relaxed);
            if prev.is_null() { inner.head = next; }
            else { (*prev).rq_next.store(next, Ordering::Relaxed); }
            if inner.tail == curr { inner.tail = prev; }
            (*curr).rq_next.store(null_mut(), Ordering::Relaxed);
            (*curr).on_rq.store(false, Ordering::Relaxed);
            inner.len = inner.len.saturating_sub(1);
            return Some(curr);
        }
        prev = curr;
        curr = (*curr).rq_next.load(Ordering::Relaxed);
    }
    None
}

pub unsafe fn rq_peek_min_vr(inner: &RunQueueInner) -> Option<u64> {
    if inner.head.is_null() { return None; }
    Some((*inner.head).vruntime.load(Ordering::Relaxed))
}

pub unsafe fn rq_has_non_idle(inner: &RunQueueInner) -> bool {
    let mut curr = inner.head;
    while !curr.is_null() {
        if !(*curr).is_idle { return true; }
        curr = (*curr).rq_next.load(Ordering::Relaxed);
    }
    false
}

pub unsafe fn rq_remove(inner: &mut RunQueueInner, pid: u64) -> bool {
    if inner.head.is_null() { return false; }

    if (*inner.head).pid == pid {
        let p = inner.head;
        inner.head = (*p).rq_next.load(Ordering::Relaxed);
        if inner.tail == p { inner.tail = null_mut(); }
        (*p).rq_next.store(null_mut(), Ordering::Relaxed);
        (*p).on_rq.store(false, Ordering::Relaxed);
        inner.len = inner.len.saturating_sub(1);
        return true;
    }

    let mut curr = inner.head;
    loop {
        let next = (*curr).rq_next.load(Ordering::Relaxed);
        if next.is_null() { return false; }
        if (*next).pid == pid {
            let after = (*next).rq_next.load(Ordering::Relaxed);
            (*curr).rq_next.store(after, Ordering::Relaxed);
            if inner.tail == next { inner.tail = curr; }
            (*next).rq_next.store(null_mut(), Ordering::Relaxed);
            (*next).on_rq.store(false, Ordering::Relaxed);
            inner.len = inner.len.saturating_sub(1);
            return true;
        }
        curr = next;
    }
}

// locked CPU-level helpers

pub fn push_to_cpu(cpu: &Cpu, p: *mut Process) {
    interrupts::without_interrupts(|| {
        cpu.run_queue.with(|inner| unsafe { rq_push_sorted(inner, p) });
    });
}

/// Steal a runnable task from any other CPU's queue. try_with avoids
/// blocking on a peer whose ISR currently holds the lock
pub unsafe fn try_steal_task(my_idx: usize) -> Option<*mut Process> {
    let total = percpu::cpu_count().min(percpu::MAX_CPUS);
    for i in 0..total {
        if i == my_idx { continue; }
        let peer = percpu::get(i);
        if peer.online.load(Ordering::Relaxed) == 0 { continue; }
        let stolen = peer.run_queue.try_with(|inner| rq_pop_min(inner)).flatten();
        if let Some(p) = stolen {
            if (*p).cpu_mask & (1u64 << my_idx) != 0 {
                return Some(p);
            }
            // Affinity forbids running here - put it back on its origin
            peer.run_queue.with(|inner| rq_push_sorted(inner, p));
        }
    }
    None
}

/// Remove pid from whichever CPU's queue currently holds it
pub fn remove_from_any_queue(pid: u64) {
    let total = percpu::cpu_count().min(percpu::MAX_CPUS);
    for i in 0..total {
        let removed = percpu::get(i)
            .run_queue
            .with(|inner| unsafe { rq_remove(inner, pid) });
        if removed { return; }
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
