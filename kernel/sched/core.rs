// The CFS scheduler core
//
//   schedule_from_isr        one tick of the scheduler. Invoked from the timer ISR
//                            and from software_context_switch. #[no_mangle]
//                            extern "C" because the ISR entry stub branches to it
//                            by symbol
//   software_context_switch  naked yield path: builds the iret frame the timer ISR
//                            pushes, jumps into schedule_from_isr and iretq's into
//                            the task selected
//   expire_sleepers          sleepers whose deadline has passed, onto run queues.
//                            Each CPU's timer interrupt does its own
//                            (`sched::timer`)

use core::sync::atomic::{AtomicU64, Ordering};

use crate::arch::x86_64::percpu;
use crate::process::{
    PENDING_NONE, STATE_BLOCKED, STATE_DEAD, STATE_READY, STATE_RUNNING,
    STATE_SLEEPING,
};

use super::proc_index::PROC_INDEX;
use super::runqueue::{
    has_waiting, peek_min_vr, pick_cpu_for, pop_min, push_to_cpu, try_steal_task, weight,
};

const CPU_WINDOW_TICKS: u64 = 250;
pub(super) const TICK_SCALE: u64 = 1_000_000;

pub static TOTAL_SWITCHES: AtomicU64 = AtomicU64::new(0);

/// Wakes the tasks on `cpu` whose sleep has run out, on the clock `now`. From
/// that CPU's timer interrupt, which then arms itself for the next one
/// (`apic::arm_timer`)
///
/// Out of the heap first (`timer::expire`), then SLEEPING -> READY by CAS, then
/// onto a queue, so nobody can run the task and put it to sleep again while it
/// is still in a heap. A task losing the CAS was woken or killed meanwhile, and
/// is left to whoever did that
pub fn expire_sleepers(cpu: usize, now: u64) {
    let mut due = [core::ptr::null_mut(); 16];
    let n = super::timer::expire(cpu, now, &mut due);
    for &ptr in &due[..n] {
        let p = unsafe { &*ptr };
        if p.state.compare_exchange(
            STATE_SLEEPING, STATE_READY,
            Ordering::AcqRel, Ordering::Relaxed,
        ).is_err() { continue; }

        let target = pick_cpu_for(p.cpu_mask);
        let tcpu   = percpu::get(target);
        super::runqueue::place_on(tcpu, p);
        push_to_cpu(tcpu, ptr);
        if target != cpu {
            super::idle::kick(tcpu);
        }
    }
}

/// What the entry stubs need back from the scheduler: the stack to resume on,
/// and (when switched away) the address of the outgoing task's 'on_cpu' flag.
/// The stub clears that flag only after 'mov rsp, rax', once this CPU has left
/// the old stack (see Process::on_cpu). Two words, so SysV returns it in
/// rax:rdx and the stubs can use it without touching memory
#[repr(C)]
pub struct SwitchOut {
    pub new_rsp: u64,
    /// &Process.on_cpu of the task we switched away from, or 0
    pub release_on_cpu: u64,
}

impl SwitchOut {
    #[inline]
    fn stay(rsp: u64) -> Self {
        SwitchOut { new_rsp: rsp, release_on_cpu: 0 }
    }
}

/// RUNNING -> `to`, only if the task is still RUNNING. False when not (killed
/// by another CPU since last looked at), and nothing is changed: DEAD is the
/// one state nobody may write over
#[inline]
fn leave_running(p: &crate::process::Process, to: u8) -> bool {
    p.state
        .compare_exchange(STATE_RUNNING, to, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
}

/// A requested sleep, made, or not if a wake cancelled it first.
///
/// The order is the protocol `wakeup` relies on: into the heap, RUNNING ->
/// SLEEPING, and only then the request taken down. A wake before the request is
/// taken cancels it (`wakeup`'s first CAS) and says it woke the task; this sees
/// the request gone and puts the task back to RUNNING. A wake after finds
/// SLEEPING and does the ordinary thing. Returns whether the task gave up the
/// CPU
unsafe fn apply_sleep(curr: &crate::process::Process, curr_ptr: *mut crate::process::Process,
                      cpu: &percpu::Cpu) -> bool {
    // Into this CPU's heap of sleepers before the state says so: a `wakeup`
    // that wins SLEEPING -> READY always finds it there to take out
    // (`sched::timer`). This CPU's heap, with interrupts off: its expiry
    // cannot run in the middle of this
    let cpu_idx = cpu.cpu_index as usize;
    let soonest = super::timer::link(curr_ptr, cpu_idx);
    if !leave_running(curr, STATE_SLEEPING) {
        // Killed since it asked
        super::timer::unlink(curr_ptr);
        curr.pending_state.store(PENDING_NONE, Ordering::Relaxed);
        return true;
    }
    if curr.pending_state
        .compare_exchange(STATE_SLEEPING, PENDING_NONE, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        curr.sleep_count.fetch_add(1, Ordering::Relaxed);
        if soonest {
            crate::arch::x86_64::apic::arm_timer(cpu_idx);
        }
        return true;
    }
    // Cancelled between the look and the state: the waker has returned
    // believing the task runs on, so it does
    if curr.state
        .compare_exchange(STATE_SLEEPING, STATE_RUNNING, Ordering::AcqRel, Ordering::Relaxed)
        .is_ok()
    {
        super::timer::unlink(curr_ptr);
        return false;
    }
    // A second wake got there too and queued it (or a kill marked it dead): off
    // the CPU the ordinary way
    true
}

/// Applies the transition a task recorded in 'pending_state' before it called
/// into the switch. Runs after 'rsp' has been stored: only now is the saved
/// context good enough for another CPU to resume from. Returns true if the task
/// gave up the CPU
unsafe fn apply_pending(curr: &crate::process::Process, curr_ptr: *mut crate::process::Process,
                        cpu: &percpu::Cpu) -> bool {
    // A stop beats whatever the task itself wanted, and is checked first. It
    // lives in a flag of its own because every voluntary transition writes
    // `pending_state`: `yield_now` stores READY, so a stop from another
    // processor would be overwritten
    if curr.stop_pending.swap(false, Ordering::AcqRel)
        && leave_running(curr, crate::process::STATE_STOPPED)
    {
        curr.pending_state.store(PENDING_NONE, Ordering::Relaxed);
        return true;
    }

    // A sleep is read and not taken: the request stays visible until the state
    // says SLEEPING (`apply_sleep`). Otherwise there is a moment when the task
    // is neither asleep nor asking to be, and a `wakeup` from another CPU finds
    // RUNNING and nothing to cancel, and the sleeper sleeps to its backstop
    let pending = match curr.pending_state.load(Ordering::Acquire) {
        STATE_SLEEPING => STATE_SLEEPING,
        _ => curr.pending_state.swap(PENDING_NONE, Ordering::AcqRel),
    };
    if pending == PENDING_NONE {
        return false;
    }
    // A task killed between recording the request and getting here must stay
    // dead. Every arm below moves the state from RUNNING by CAS: a check alone
    // leaves a window where a kill on another CPU writes DEAD and the arm
    // writes over it
    match pending {
        STATE_READY => {
            if leave_running(curr, STATE_READY) {
                push_to_cpu(cpu, curr_ptr);
            }
        }
        STATE_SLEEPING => return apply_sleep(curr, curr_ptr, cpu),
        STATE_BLOCKED => {
            leave_running(curr, STATE_BLOCKED);
        }
        // Stopped. Off the queue and off the timer, like BLOCKED, and the only
        // way back is `sched::cont`.
        //
        // The `_` below drops an unrecognised request, leaving the task marked
        // RUNNING with nothing to push it back onto a queue, so a new pending
        // value needs its own arm.
        //
        // (From RUNNING, so never from a sleep: the task is in no timer heap
        // for this arm to take it out of)
        crate::process::STATE_STOPPED => {
            leave_running(curr, crate::process::STATE_STOPPED);
        }
        _ => {}
    }
    true
}


/// The voluntary way in: `yield_now` and every sleep, through
/// `software_context_switch`, with what the task wants in `pending_state`
#[no_mangle]
pub unsafe extern "C" fn schedule_from_isr(old_rsp: u64) -> SwitchOut {
    schedule(old_rsp, false, true)
}

/// The timer's way in, and the reschedule IPI's. `in_kernel` is whether the
/// interrupt caught ring 0; `is_tick` whether it was the 250 Hz tick, or only a
/// sleeper's deadline or another CPU's nudge, which nobody is charged for.
///
/// Ring 0 is not preempted: a task the timer catches in the kernel keeps its
/// processor until it sleeps or yields, as Linux's `PREEMPT_NONE`. The kernel's
/// locks are plain spinlocks that do not hold preemption off, so a holder taken
/// away would keep its lock while descheduled, and each next processor would
/// spin on it with interrupts off until none was left to run the holder. A
/// program in ring 3 is preempted, and so is idle; a stop or a kill reaches a
/// task in the kernel at its next scheduling point
pub unsafe fn schedule_from_timer(old_rsp: u64, in_kernel: bool, is_tick: bool) -> SwitchOut {
    if !is_tick {
        return schedule(old_rsp, in_kernel, false);
    }
    // Here and not in `schedule`, which a yield reaches too: this is the one
    // call that is a tick, and `/proc/stat` counts ticks
    let cpu = percpu::current();
    let curr = PROC_INDEX.get_raw(cpu.current_pid.load(Ordering::Relaxed));
    let spent = if curr.is_null() || (*curr).is_idle {
        super::stats::Spent::Idle
    } else if in_kernel {
        super::stats::Spent::System
    } else {
        super::stats::Spent::User
    };
    super::stats::account_tick(cpu.cpu_index as usize, spent);
    schedule(old_rsp, in_kernel, true)
}

/// `charge`: a tick's worth of vruntime and CPU time goes to the running task.
/// Without it (a deadline, a nudge) the running task is only compared with what
/// is queued, and gives way to a task behind it by more than a tick, usually
/// one a wake has just put there
unsafe fn schedule(old_rsp: u64, in_kernel: bool, charge: bool) -> SwitchOut {
    let tick   = crate::arch::x86_64::interrupts::get_tick();
    let cpu    = percpu::current();
    let my_idx = cpu.cpu_index as usize;

    let curr_pid = cpu.current_pid.load(Ordering::Relaxed);
    let curr_ptr = PROC_INDEX.get_raw(curr_pid);

    let mut need_switch = false;
    // Caught in the kernel by the tick: accounted, never switched away
    let hold = in_kernel && !curr_ptr.is_null() && !(*curr_ptr).is_idle;

    if !curr_ptr.is_null() {
        let curr = &*curr_ptr;
        curr.rsp.store(old_rsp, Ordering::Relaxed);
        // Whoever is current on this CPU owns its stack. Asserting it here
        // rather than only on switch-in covers the bootstrap tasks, which
        // start running without ever being picked by this function
        curr.on_cpu.store(true, Ordering::Relaxed);

        // A request is the task's own, and it makes it on the way into
        // `software_context_switch`; the tick leaves it there for that
        if !hold && apply_pending(curr, curr_ptr, cpu) {
            need_switch = true;
        }
    }

    if !curr_ptr.is_null() && !need_switch {
        let curr = &*curr_ptr;

        match curr.state.load(Ordering::Relaxed) {
            STATE_RUNNING if !curr.is_idle => {
                let w      = weight(curr.priority.load(Ordering::Relaxed));
                let dv     = TICK_SCALE / w;
                let new_vr = if charge {
                    curr.cpu_time.fetch_add(1, Ordering::Relaxed);
                    curr.window_cpu_ticks.fetch_add(1, Ordering::Relaxed);
                    let ws = curr.window_start.load(Ordering::Relaxed);
                    if tick.saturating_sub(ws) >= CPU_WINDOW_TICKS {
                        curr.window_cpu_ticks.store(1, Ordering::Relaxed);
                        curr.window_start.store(tick, Ordering::Relaxed);
                    }
                    curr.vruntime.fetch_add(dv, Ordering::Relaxed) + dv
                } else {
                    // Uncharged, the queue has to be a tick behind to win, or
                    // two tasks level with each other would trade the CPU on
                    // every wake
                    curr.vruntime.load(Ordering::Relaxed).saturating_sub(dv)
                };

                let peek = if hold { None } else { peek_min_vr(cpu) };
                if let Some(next_vr) = peek {
                    if new_vr > next_vr {
                        // Killed since the state was read: off the CPU, but
                        // onto no queue
                        if leave_running(curr, STATE_READY) {
                            curr.preempt_count.fetch_add(1, Ordering::Relaxed);
                            push_to_cpu(cpu, curr_ptr);
                        }
                        need_switch = true;
                    }
                }
            }
            STATE_RUNNING => {
                // Idle running: switch if anyone else is waiting
                if has_waiting(cpu) {
                    curr.state.store(STATE_READY, Ordering::Relaxed);
                    need_switch = true;
                }
            }
            STATE_SLEEPING => {
                curr.sleep_count.fetch_add(1, Ordering::Relaxed);
                need_switch = !hold;
            }
            STATE_DEAD | _ => { need_switch = !hold; }
        }
    } else {
        need_switch = true;
    }

    if !need_switch { return SwitchOut::stay(old_rsp); }

    // Taking a task off a queue is claiming it: READY -> RUNNING by CAS, here
    // and not by a plain store at the switch below. A kill on another CPU takes
    // a task off every queue and marks it DEAD; one this CPU has just popped is
    // on no queue, and a store would write RUNNING over DEAD. A task failing
    // the claim was killed after it was queued, and is left.
    //
    // A task still on another processor's stack is passed over, not waited for.
    // That processor queued it on its way out and releases it once its own
    // switch is done, and if it is picking our current task, queued the same
    // way, each would wait for the other for ever
    let mut deferred: [*mut crate::process::Process; 4] = [core::ptr::null_mut(); 4];
    let mut n_deferred = 0usize;
    let next_ptr = loop {
        let mut cand = pop_min(cpu);
        if cand.is_none() {
            cand = try_steal_task(my_idx);
        }
        if let Some(p) = cand {
            if p != curr_ptr && !(*p).is_idle && (*p).on_cpu.load(Ordering::Acquire)
                && n_deferred < deferred.len()
            {
                deferred[n_deferred] = p;
                n_deferred += 1;
                continue;
            }
        }
        match cand {
            Some(p) => {
                let claimed = (*p).is_idle
                    || (*p).state
                        .compare_exchange(STATE_READY, STATE_RUNNING, Ordering::AcqRel, Ordering::Relaxed)
                        .is_ok();
                if claimed {
                    break p;
                }
                // Killed while queued: let go, and the reaper may have it
                (*p).in_switch.store(false, Ordering::SeqCst);
            }
            None => {
                let idle_pid = cpu.idle_pid.load(Ordering::Relaxed);
                let idle_ptr = PROC_INDEX.get_raw(idle_pid);
                // Null: stay where we are - decided after the passed-over
                // tasks are back on a queue
                if idle_ptr.is_null() { break core::ptr::null_mut(); }
                // Stay only when idle is what is running already. Idle's state
                // can say RUNNING while another task is current, and a task
                // that just died or went to sleep must not be resumed. With no
                // current task on record (early boot) the state is all there is
                let idle_is_current = idle_ptr == curr_ptr
                    || (curr_ptr.is_null() && (*idle_ptr).state.load(Ordering::Relaxed) == STATE_RUNNING);
                if idle_is_current {
                    break core::ptr::null_mut();
                }
                break idle_ptr;
            }
        }
    };

    // Back where they came from, for the next pick here or anywhere
    for &p in &deferred[..n_deferred] {
        push_to_cpu(cpu, p);
        (*p).in_switch.store(false, Ordering::SeqCst);
    }
    if next_ptr.is_null() {
        return SwitchOut::stay(old_rsp);
    }

    // We may have popped the task we just queued, and claimed it, so it is
    // RUNNING again. Resuming it is what iretq would do, and spinning on our
    // own on_cpu flag below would never terminate
    if next_ptr == curr_ptr {
        (*next_ptr).in_switch.store(false, Ordering::SeqCst);
        return SwitchOut::stay(old_rsp);
    }

    // The task may still be on its previous CPU, which has saved its context
    // but not left its stack; resuming it now would put two CPUs on one stack.
    // The owner clears this a few instructions after returning from here, so
    // the wait is short. This is not the two-way wait the pick above passes
    // over: a task on another CPU when we looked was never claimed, so this is
    // only the instant between that look and the owner's release
    while (*next_ptr).on_cpu.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }

    let next     = &*next_ptr;
    if !curr_ptr.is_null() && (*curr_ptr).is_idle {
        super::idle::left(cpu);
    }
    let old_cr3  = if !curr_ptr.is_null() { (*curr_ptr).cr3 } else { 0 };
    let new_cr3  = next.cr3;
    let new_rsp0 = next.stack_top();
    let new_rsp  = next.rsp.load(Ordering::Relaxed);
    let new_pid  = next.pid;

    // The percpu user_rsp slot (gs:[0x18], written by the syscall entry) is
    // per-CPU, not per-process. Swapped alongside kernel_rsp so a process
    // blocked inside a syscall returns to its user stack even after other
    // processes ran syscalls on this CPU
    if !curr_ptr.is_null() {
        (*curr_ptr).saved_user_rsp.store(
            cpu.user_rsp.load(Ordering::Relaxed),
            Ordering::Relaxed,
        );
    }
    cpu.user_rsp.store(
        next.saved_user_rsp.load(Ordering::Relaxed),
        Ordering::Relaxed,
    );

    // IA32_FS_BASE is the same kind of state: one MSR standing in for something
    // each task has. Without this a process that set a TLS base would read the
    // thread-locals of whatever ran in between
    //
    // Read once and written only when it differs: the common case is two tasks
    // leaving it zero, and a WRMSR changing nothing still serialises
    {
        use x86_64::registers::model_specific::FsBase;
        use x86_64::VirtAddr;
        let cur_fs = FsBase::read().as_u64();
        if !curr_ptr.is_null() {
            (*curr_ptr).saved_fs_base.store(cur_fs, Ordering::Relaxed);
        }
        let new_fs = next.saved_fs_base.load(Ordering::Relaxed);
        if new_fs != cur_fs {
            // try_new, not new: `new` panics, and a panic inside the context
            // switch cannot report why. Every value here came from this RDMSR
            // or a syscall that checked it, so the failing arm is unreachable.
            // Zero is what a kernel thread runs with
            let va = VirtAddr::try_new(new_fs).unwrap_or(VirtAddr::zero());
            FsBase::write(va);
        }
    }

    // The x87/SSE register file, one-per-core standing in for one-per-task, and
    // the one whose absence gives a wrong answer rather than a fault. Saved
    // from the CPU into the outgoing task and restored from the incoming one,
    // here rather than in the naked stub because FXSAVE needs an address and
    // the stub has no task pointer.
    //
    // The save happens while `curr` owns this CPU, and the restore after the
    // spin on `next.on_cpu` above has established no other CPU is running
    // `next`. Moving either side of that fence would let two CPUs write one
    // area.
    //
    // Unconditional, including kernel threads and idle: they cannot dirty the
    // FPU (the kernel is built `+soft-float`), but the invariant "the area
    // holds what this task last had" stays unconditional. See
    // `arch::x86_64::fpu`
    unsafe {
        if !curr_ptr.is_null() { (*curr_ptr).fpu.save(); }
        next.fpu.restore();
    }

    // Claimed above; idle is the one task never claimed, and it is marked here
    if next.is_idle {
        next.state.store(STATE_RUNNING, Ordering::Relaxed);
    }
    next.on_cpu.store(true, Ordering::SeqCst);
    next.in_switch.store(false, Ordering::SeqCst);
    next.switch_in_count.fetch_add(1, Ordering::Relaxed);
    next.last_run_tick.store(tick, Ordering::Relaxed);

    let cur_min = cpu.min_vruntime.load(Ordering::Relaxed);
    let new_min = cur_min.max(next.vruntime.load(Ordering::Relaxed));
    cpu.min_vruntime.store(new_min, Ordering::Relaxed);
    cpu.total_switches.fetch_add(1, Ordering::Relaxed);
    TOTAL_SWITCHES.fetch_add(1, Ordering::Relaxed);
    cpu.current_pid.store(new_pid, Ordering::Relaxed);
    // And which terminal that task prints to. Swapped here rather than looked
    // up per print, because `print!` reads it from inside interrupt handlers
    cpu.current_tty.store(next.ctty.load(Ordering::Relaxed), Ordering::Relaxed);

    crate::arch::x86_64::gdt::set_kernel_stack(new_rsp0);

    if old_cr3 != new_cr3 && new_cr3 != 0 {
        // Said before it is done: see `mm::tlb::cr3_loaded_elsewhere`
        cpu.loaded_cr3.store(new_cr3, Ordering::SeqCst);
        core::arch::asm!(
            "mov cr3, {}",
            in(reg) new_cr3,
            options(nostack, preserves_flags)
        );
    }

    // The outgoing task stays off-limits to other CPUs until the stub has
    // done 'mov rsp, rax'; handing back the flag address is what lets the
    // release happen on the far side of the stack switch
    let release = if curr_ptr.is_null() {
        0
    } else {
        core::ptr::addr_of!((*curr_ptr).on_cpu) as u64
    };
    SwitchOut { new_rsp, release_on_cpu: release }
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
        // rax = stack to resume on, rdx = &prev.on_cpu (0 if we stayed put).
        // Released only here: until rsp has moved this runs on the outgoing
        // task's stack, and another CPU resuming it would push through this
        // frame. rdx is dead: the pops below reload it from the incoming frame
        "mov rsp, rax",
        "test rdx, rdx",
        "jz 2f",
        "mov byte ptr [rdx], 0",
        "2:",
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

