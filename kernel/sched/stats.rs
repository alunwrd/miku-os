// Read-only views into scheduler state for top-style tools and debugging

extern crate alloc;

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::percpu::MAX_CPUS;

use crate::process::{
    STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING,
};

use super::proc_index::{for_each_task, PROC_INDEX, PROC_TABLE};

#[derive(Debug, Clone)]
pub struct ThreadStat {
    pub pid:            u64,
    /// By value: a snapshot outlives the table lock it was taken under. `state`
    /// next door is `'static`: it comes from `Process::state_name`, which
    /// returns a literal
    pub name:           crate::process::Name,
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

/// How each processor spent its timer ticks: in ring 3, in the kernel, idle
///
/// Counted in the timer interrupt, one tick at a time, so atomics only.
/// `/proc/stat`'s `cpu` lines are made of it, and a CPU meter takes the
/// difference of two reads
static USER: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static SYSTEM: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static IDLE: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];

/// What the tick interrupted
#[derive(Clone, Copy)]
pub enum Spent { User, System, Idle }

/// One tick on `cpu`. Called from the timer interrupt
pub fn account_tick(cpu: usize, spent: Spent) {
    let Some(slot) = (match spent {
        Spent::User => USER.get(cpu),
        Spent::System => SYSTEM.get(cpu),
        Spent::Idle => IDLE.get(cpu),
    }) else { return };
    slot.fetch_add(1, Ordering::Relaxed);
}

/// `(user, system, idle)` ticks on `cpu` since it started
pub fn cpu_ticks(cpu: usize) -> (u64, u64, u64) {
    let get = |a: &[AtomicU64; MAX_CPUS]| a.get(cpu).map_or(0, |v| v.load(Ordering::Relaxed));
    (get(&USER), get(&SYSTEM), get(&IDLE))
}

pub fn thread_count() -> usize {
    PROC_TABLE.lock().len()
}

pub fn get_stats() -> Vec<ThreadStat> {
    let now   = crate::arch::x86_64::interrupts::get_tick();
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

/// Tasks whose sleep should have ended a second ago and has not, said once
/// each: asleep with nothing left to wake it. Every sleep has a deadline
/// (`wait.rs`, "The backstop"), so one long past is a task missing from its
/// CPU's heap of sleepers, or a heap whose timer nobody arms. Read without
/// locks, from kswapd's loop; a task mid-transition is looked at again next
/// time.
///
/// With `schedwatch` on the boot line it also says which tasks sat READY or
/// RUNNING for a second on no processor, where every processor is, and the
/// whole table every five seconds. Not by default: a task queued for a second
/// behind a CPU busy in the kernel is slow, not lost
pub fn report_overdue_sleepers() {
    const GRACE_NS: u64 = 1_000_000_000;
    static REPORTED: AtomicU64 = AtomicU64::new(0);
    let now = crate::kcore::time::now_ns();
    let mut said = false;
    let watch = crate::arch::x86_64::grub::cmdline_flag("schedwatch");
    for_each_task(|p| {
        let i = p.pid as usize;
        let state = p.state.load(Ordering::Relaxed);
        if (state == STATE_RUNNING || state == STATE_READY) && !p.is_idle {
            if watch {
                said |= report_if_lost(i, p, state);
            }
            return;
        }
        if state != STATE_SLEEPING { return; }
        let wake = p.wake_ns.load(Ordering::Relaxed);
        if wake.saturating_add(GRACE_NS) > now { return; }
        // One report per stuck sleep: the pid and its deadline together
        let tag = (i as u64) << 44 ^ wake;
        if REPORTED.swap(tag, Ordering::Relaxed) == tag { return; }
        said = true;
        crate::kwarn!(
            "[sched] pid {} ({}) asleep {} ms past its deadline: heap cpu {} slot {}, pending {}, on_cpu {}, in_switch {}",
            i, p.name.as_str(), (now - wake) / 1_000_000,
            p.timer_cpu.load(Ordering::Relaxed) as i32, p.timer_slot.load(Ordering::Relaxed),
            p.pending_state.load(Ordering::Relaxed), p.on_cpu.load(Ordering::Relaxed),
            p.in_switch.load(Ordering::Relaxed),
        );
        let c = p.timer_cpu.load(Ordering::Relaxed);
        if c != super::timer::NONE {
            crate::kwarn!("[sched]   cpu {} has {} sleepers, soonest {:?} ns from now",
                c, super::timer::sleepers(c as usize),
                super::timer::soonest(c as usize).map(|s| s as i64 - now as i64));
        }
    });
    // `schedwatch` on the boot line: the whole table every few seconds, for
    // a hang that none of the above recognises
    static LAST_DUMP: AtomicU64 = AtomicU64::new(0);
    if watch
        && now.saturating_sub(LAST_DUMP.load(Ordering::Relaxed)) >= 5_000_000_000
    {
        LAST_DUMP.store(now, Ordering::Relaxed);
        said = true;
        for_each_task(|p| {
            if p.is_idle { return; }
            let i = p.pid;
            let st = p.state.load(Ordering::Relaxed);
            let wake = p.wake_ns.load(Ordering::Relaxed);
            crate::kwarn!("[sched] watch pid {} ({}) state {} pending {} wake {} ms heap {} on_cpu {} switch {} sleeps {}",
                i, p.name.as_str(), st, p.pending_state.load(Ordering::Relaxed),
                if st == STATE_SLEEPING { (wake as i64 - now as i64) / 1_000_000 } else { 0 },
                p.timer_cpu.load(Ordering::Relaxed) as i32, p.on_cpu.load(Ordering::Relaxed),
                p.in_switch.load(Ordering::Relaxed), p.sleep_count.load(Ordering::Relaxed));
        });
    }

    // Where every processor is
    if said {
        let total = crate::arch::x86_64::percpu::cpu_count().min(MAX_CPUS);
        for c in 0..total {
            let cpu = crate::arch::x86_64::percpu::get(c);
            let cur = cpu.current_pid.load(Ordering::Relaxed);
            let ptr = unsafe { PROC_INDEX.get_raw(cur) };
            let name = if ptr.is_null() { "?" } else { unsafe { (*ptr).name.as_str() } };
            crate::kwarn!("[sched]   cpu {}: running {} ({}){}, {} queued, {} sleeping, {} switches",
                c, cur, name, if cur == cpu.idle_pid.load(Ordering::Relaxed) { " idle" } else { "" },
                cpu.run_queue.with(|q| q.len()), super::timer::sleepers(c),
                cpu.total_switches.load(Ordering::Relaxed));
        }
    }
}

/// A task READY or RUNNING at two looks a second apart, and at neither on a
/// processor: queued and never picked, or out of every queue with nothing to
/// put it back
fn report_if_lost(pid: usize, p: &crate::process::Process, state: u8) -> bool {
    use core::sync::atomic::AtomicUsize;
    static SEEN: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
    let total = crate::arch::x86_64::percpu::cpu_count().min(MAX_CPUS);
    let current = (0..total).any(|c| crate::arch::x86_64::percpu::get(c).current_pid.load(Ordering::Relaxed) == pid as u64);
    let slot = &SEEN[pid % SEEN.len()];
    if current || p.on_cpu.load(Ordering::Relaxed) {
        let _ = slot.compare_exchange(pid, 0, Ordering::Relaxed, Ordering::Relaxed);
        return false;
    }
    // Twice in a row, then once more only after it was seen to move
    if slot.swap(pid, Ordering::Relaxed) != pid { return false; }
    if slot.swap(usize::MAX, Ordering::Relaxed) == usize::MAX { return false; }
    crate::kwarn!(
        "[sched] pid {} ({}) {} for a second on no processor: pending {}, in_switch {}, heap cpu {}",
        pid, p.name.as_str(), if state == STATE_READY { "READY" } else { "RUNNING" },
        p.pending_state.load(Ordering::Relaxed), p.in_switch.load(Ordering::Relaxed),
        p.timer_cpu.load(Ordering::Relaxed) as i32,
    );
    true
}

pub fn debug_dump_all() {
    for_each_task(|p| {
        let state = p.state.load(Ordering::Relaxed);
        let state_str = match state {
            STATE_READY    => "READY",
            STATE_RUNNING  => "RUN",
            STATE_SLEEPING => "SLEEP",
            STATE_BLOCKED  => "BLOCK",
            STATE_DEAD     => "DEAD",
            _              => "?",
        };
        crate::serial_println!("[debug] pid={} state={}", p.pid, state_str);
    });
}
