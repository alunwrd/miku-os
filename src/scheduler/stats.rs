// Read-only views into scheduler state for top-style tools and debugging

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::process::{
    pid_range, STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING,
};

use super::proc_index::{MAX_PROCS, PROC_INDEX, PROC_TABLE};

#[derive(Debug, Clone)]
pub struct ThreadStat {
    pub pid:            u64,
    pub name:           &'static str,
    pub state:          &'static str,
    pub priority:       u8,
    pub cpu_mask:       u64,
    pub cpu_time:       u64,
    pub vruntime:       u64,
    pub preempt_count:  u64,
    pub sleep_count:    u64,
    pub switch_in:      u64,
    pub cpu_pct_x10:    u32,
    pub uptime_ticks:   u64,
    pub is_idle:        bool,
    pub stack_alloc_kb: usize,
    pub stack_used_kb:  usize,
}

pub fn thread_count() -> usize {
    PROC_TABLE.lock().len()
}

pub fn get_stats() -> Vec<ThreadStat> {
    let now   = crate::interrupts::get_tick();
    let table = PROC_TABLE.lock();
    let mut out = Vec::with_capacity(table.len());
    for (&pid, p) in table.iter() {
        out.push(ThreadStat {
            pid,
            name:           p.name,
            state:          p.state_name(),
            priority:       p.priority.load(Ordering::Relaxed),
            cpu_mask:       p.cpu_mask,
            cpu_time:       p.cpu_time.load(Ordering::Relaxed),
            vruntime:       p.vruntime.load(Ordering::Relaxed),
            preempt_count:  p.preempt_count.load(Ordering::Relaxed),
            sleep_count:    p.sleep_count.load(Ordering::Relaxed),
            switch_in:      p.switch_in_count.load(Ordering::Relaxed),
            cpu_pct_x10:    p.cpu_percent_window(now),
            uptime_ticks:   p.uptime_ticks(now),
            is_idle:        p.is_idle,
            stack_alloc_kb: p.stack.len() / 1024,
            stack_used_kb:  p.stack_used_bytes() / 1024,
        });
    }
    out.sort_by_key(|s| s.pid);
    out
}

pub fn debug_dump_all() {
    let max = pid_range().min(MAX_PROCS as u64) as usize;
    let arr = unsafe { &*PROC_INDEX.0.get() };
    for i in 0..max {
        let ptr = arr[i];
        if ptr.is_null() { continue; }
        let p = unsafe { &*ptr };
        let state = p.state.load(Ordering::Relaxed);
        let state_str = match state {
            STATE_READY    => "READY",
            STATE_RUNNING  => "RUN",
            STATE_SLEEPING => "SLEEP",
            STATE_BLOCKED  => "BLOCK",
            STATE_DEAD     => "DEAD",
            _              => "?",
        };
        crate::serial_println!("[debug] pid={} state={}", i, state_str);
    }
}
