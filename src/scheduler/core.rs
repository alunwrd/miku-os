//                          The CFS scheduler core
//
//       schedule_from_isr        - one tick of the scheduler. Invoked from
//                                  the timer ISR and from
//                                  software_context_switch. The function
//                                  is #[no_mangle] extern "C" because the
//                                  ISR entry stub branches to it by symbol
//      software_context_switch   - naked yield path: builds the same iret
//                                  frame the timer ISR pushes, then jumps
//                                  into schedule_from_isr and iretq's into
//                                  whichever task was selected
//     wake_sleepers_isr`        - promote sleepers whose deadline passed
//                                  Driven only from the BSP timer ISR so
//                                  nobody pushes the same task twice

use core::sync::atomic::{AtomicU64, Ordering};

use crate::percpu;
use crate::process::{
    pid_range, STATE_DEAD, STATE_READY, STATE_RUNNING, STATE_SLEEPING,
};

use super::proc_index::{MAX_PROCS, PROC_INDEX};
use super::runqueue::{
    pick_cpu_for, rq_has_non_idle, rq_peek_min_vr, rq_pop_min, rq_push_sorted,
    try_steal_task, weight,
};

const CPU_WINDOW_TICKS: u64 = 250;
pub(super) const TICK_SCALE: u64 = 1_000_000;

pub static TOTAL_SWITCHES: AtomicU64 = AtomicU64::new(0);

#[inline(always)]
unsafe fn wake_sleepers_isr(tick: u64) {
    let max = pid_range().min(MAX_PROCS as u64) as usize;
    let arr = &*PROC_INDEX.0.get();

    for i in 0..max {
        let ptr = arr[i];
        if ptr.is_null() { continue; }
        let p = &*ptr;
        if p.state.load(Ordering::Relaxed) != STATE_SLEEPING { continue; }
        if tick < p.sleep_until.load(Ordering::Relaxed) { continue; }

        if p.state.compare_exchange(
            STATE_SLEEPING, STATE_READY,
            Ordering::AcqRel, Ordering::Relaxed,
        ).is_err() { continue; }

        let target = pick_cpu_for(p.cpu_mask);
        let cpu    = percpu::get(target);
        let min_vr = cpu.min_vruntime.load(Ordering::Relaxed);
        let vr     = p.vruntime.load(Ordering::Relaxed).max(min_vr);
        p.vruntime.store(vr, Ordering::Relaxed);
        cpu.run_queue.with(|inner| rq_push_sorted(inner, ptr));
    }
}

#[no_mangle]
pub unsafe extern "C" fn schedule_from_isr(old_rsp: u64) -> u64 {
    let tick   = crate::interrupts::get_tick();
    let cpu    = percpu::current();
    let my_idx = cpu.cpu_index as usize;

    let curr_pid = cpu.current_pid.load(Ordering::Relaxed);
    let curr_ptr = PROC_INDEX.get_raw(curr_pid);

    let mut need_switch = false;

    if !curr_ptr.is_null() {
        let curr = &*curr_ptr;
        curr.rsp.store(old_rsp, Ordering::Relaxed);

        match curr.state.load(Ordering::Relaxed) {
            STATE_RUNNING if !curr.is_idle => {
                let w      = weight(curr.priority.load(Ordering::Relaxed));
                let dv     = TICK_SCALE / w;
                let new_vr = curr.vruntime.fetch_add(dv, Ordering::Relaxed) + dv;
                curr.cpu_time.fetch_add(1, Ordering::Relaxed);
                curr.window_cpu_ticks.fetch_add(1, Ordering::Relaxed);

                let ws = curr.window_start.load(Ordering::Relaxed);
                if tick.saturating_sub(ws) >= CPU_WINDOW_TICKS {
                    curr.window_cpu_ticks.store(1, Ordering::Relaxed);
                    curr.window_start.store(tick, Ordering::Relaxed);
                }

                let peek = cpu.run_queue.with(|inner| rq_peek_min_vr(inner));
                if let Some(next_vr) = peek {
                    if new_vr > next_vr {
                        curr.state.store(STATE_READY, Ordering::Relaxed);
                        curr.preempt_count.fetch_add(1, Ordering::Relaxed);
                        cpu.run_queue.with(|inner| rq_push_sorted(inner, curr_ptr));
                        need_switch = true;
                    }
                }
            }
            STATE_RUNNING => {
                // Idle running - switch if anyone else is waiting
                let any = cpu.run_queue.with(|inner| rq_has_non_idle(inner));
                if any {
                    curr.state.store(STATE_READY, Ordering::Relaxed);
                    need_switch = true;
                }
            }
            STATE_SLEEPING => {
                curr.sleep_count.fetch_add(1, Ordering::Relaxed);
                need_switch = true;
            }
            STATE_DEAD | _ => { need_switch = true; }
        }
    } else {
        need_switch = true;
    }

    // Only the BSP drives sleeper wake-ups so the same task isn't pushed
    // twice across CPUs
    if my_idx == 0 { wake_sleepers_isr(tick); }

    if !need_switch { return old_rsp; }

    let mut next_ptr = cpu.run_queue.with(|inner| rq_pop_min(inner));
    if next_ptr.is_none() {
        next_ptr = try_steal_task(my_idx);
    }

    let next_ptr = match next_ptr {
        Some(p) => p,
        None => {
            let idle_pid = cpu.idle_pid.load(Ordering::Relaxed);
            let idle_ptr = PROC_INDEX.get_raw(idle_pid);
            if idle_ptr.is_null() { return old_rsp; }
            let idle = &*idle_ptr;
            if idle.state.load(Ordering::Relaxed) == STATE_RUNNING {
                return old_rsp;
            }
            idle_ptr
        }
    };

    let next     = &*next_ptr;
    let old_cr3  = if !curr_ptr.is_null() { (*curr_ptr).cr3 } else { 0 };
    let new_cr3  = next.cr3;
    let new_rsp0 = next.stack_top();
    let new_rsp  = next.rsp.load(Ordering::Relaxed);
    let new_pid  = next.pid;

    next.state.store(STATE_RUNNING, Ordering::Relaxed);
    next.switch_in_count.fetch_add(1, Ordering::Relaxed);
    next.last_run_tick.store(tick, Ordering::Relaxed);

    let cur_min = cpu.min_vruntime.load(Ordering::Relaxed);
    let new_min = cur_min.max(next.vruntime.load(Ordering::Relaxed));
    cpu.min_vruntime.store(new_min, Ordering::Relaxed);
    cpu.total_switches.fetch_add(1, Ordering::Relaxed);
    TOTAL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    cpu.current_pid.store(new_pid, Ordering::Relaxed);

    crate::gdt::set_kernel_stack(new_rsp0);

    if old_cr3 != new_cr3 && new_cr3 != 0 {
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) new_cr3,
            options(nostack, preserves_flags)
        );
    }

    new_rsp
}

// Software yield entry. Builds the same 15-GPR + iret frame as the timer
// ISR's entry stub so schedule_from_isr is frame-format-agnostic. The
// and rsp, -16 before the Rust call enforces SysV alignment; the returned
// new_rsp replaces rsp entirely, so no post-call restore is needed
#[unsafe(naked)]
pub(super) unsafe extern "C" fn software_context_switch() {
    core::arch::naked_asm!(
        "cli",
        "mov rax, rsp",
        "push 0x10",
        "push rax",
        "pushfq",
        "or qword ptr [rsp], 0x200",
        "push 0x08",
        "lea rax, [rip + 1f]",
        "push rax",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rbp",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push 0",
        "mov rdi, rsp",
        "and rsp, -16",
        "call {sched}",
        "mov rsp, rax",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "iretq",
        "1:",
        "ret",
        sched = sym schedule_from_isr,
    )
}
