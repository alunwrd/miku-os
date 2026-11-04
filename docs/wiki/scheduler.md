# Scheduler, SMP, locks and time

The MikuOS scheduler (`kernel/sched/`) is a CFS-style scheduler with one run queue per CPU. Each queue is a min-heap ordered by virtual runtime, and tasks are placed when they arrive and stolen when a CPU's queue runs dry. A task is either a process or a thread; both are scheduled the same way. Task state changes only by compare-and-swap. Ring 3 and the idle task are preempted by the 250 Hz tick, but a task in ring 0 is never preempted (`PREEMPT_NONE`). Sleeping tasks wait in per-CPU deadline heaps, and the LAPIC timer is armed for the next tick or the next deadline, whichever comes first. Waiting is done on wait queues whose discipline prevents lost wakes. The clock is the TSC, calibrated against the PIT. This page covers all of that, plus SMP bring-up, idle polling, the kernel's lock types, and the tools for diagnosing a stall.

## Contents

- [Code map](#code-map)
- [Tasks](#tasks)
- [Task states and the CAS discipline](#task-states-and-the-cas-discipline)
- [Run queues](#run-queues)
- [vruntime, priorities and placement](#vruntime-priorities-and-placement)
- [The scheduling decision](#the-scheduling-decision)
- [Preemption policy](#preemption-policy)
- [The timer](#the-timer)
- [SMP bring-up and per-CPU data](#smp-bring-up-and-per-cpu-data)
- [Idle and polling idle](#idle-and-polling-idle)
- [Sleeping and waking](#sleeping-and-waking)
- [Wait queues and lost wakes](#wait-queues-and-lost-wakes)
- [Time and clocks](#time-and-clocks)
- [Kernel locks](#kernel-locks)
- [Lock statistics](#lock-statistics)
- [Kernel threads and the work queue](#kernel-threads-and-the-work-queue)
- [Boot flags](#boot-flags)
- [Diagnosing a stall](#diagnosing-a-stall)
- [Tests](#tests)
- [Measured numbers](#measured-numbers)
- [Limits and what is not done](#limits-and-what-is-not-done)

---

## Code map

| File | What it holds |
|:--|:--|
| [`kernel/sched/mod.rs`](../../kernel/sched/mod.rs) | Public API, thread/process accessors (`tgid_of`, `current_tgid`) |
| [`kernel/sched/core.rs`](../../kernel/sched/core.rs) | `schedule`, `schedule_from_timer`, `schedule_from_isr`, `apply_pending`, `expire_sleepers`, the naked `software_context_switch` |
| [`kernel/sched/control.rs`](../../kernel/sched/control.rs) | `yield_now`, `sleep*`, `block_current`, `stop_current`, `cont`, `wakeup`, `waitpid` |
| [`kernel/sched/runqueue.rs`](../../kernel/sched/runqueue.rs) | Queue operations, `place_on`, priority weights, work stealing, CPU choice |
| [`kernel/sched/slot_heap.rs`](../../kernel/sched/slot_heap.rs) | `SlotHeap`, the min-heap used by run queues and timers |
| [`kernel/sched/timer.rs`](../../kernel/sched/timer.rs) | Per-CPU heaps of sleepers |
| [`kernel/sched/idle.rs`](../../kernel/sched/idle.rs) | The idle loop, polling idle, `kick` |
| [`kernel/sched/wait.rs`](../../kernel/sched/wait.rs) | `WaitQueue`, `wait_any`, `join`, keyed queues |
| [`kernel/sched/lifecycle.rs`](../../kernel/sched/lifecycle.rs) | Spawning, kill, reaping, groups |
| [`kernel/sched/proc_index.rs`](../../kernel/sched/proc_index.rs) | `PROC_INDEX`, `PROC_TABLE`, the pid bitmap, job index |
| [`kernel/sched/stats.rs`](../../kernel/sched/stats.rs) | CPU tick accounting, overdue-sleeper report, `schedwatch` |
| [`kernel/sched/workers.rs`](../../kernel/sched/workers.rs) | Kernel work queue and worker pool |
| [`kernel/sched/futex.rs`](../../kernel/sched/futex.rs) | `futex` (see [processes.md](processes.md)) |
| [`kernel/kcore/sched_lock.rs`](../../kernel/kcore/sched_lock.rs) | `SchedMutex`, `SchedRwLock`, `LockStats` |
| [`kernel/kcore/irq_lock.rs`](../../kernel/kcore/irq_lock.rs) | `IrqMutex` |
| [`kernel/kcore/isr_event.rs`](../../kernel/kcore/isr_event.rs) | `IsrEvent`: an event an interrupt handler announces and threads sleep on |
| [`kernel/kcore/time.rs`](../../kernel/kcore/time.rs) | TSC calibration, `now_ns`, `udelay`, PIT one-shot |
| [`kernel/kcore/clock.rs`](../../kernel/kcore/clock.rs) | Wall clock (RTC, NTP), time zone |
| [`kernel/arch/x86_64/apic.rs`](../../kernel/arch/x86_64/apic.rs) | LAPIC, timer modes, `arm_timer`, IPIs |
| [`kernel/arch/x86_64/interrupts.rs`](../../kernel/arch/x86_64/interrupts.rs) | Timer and reschedule ISR entries, `TICK` |
| [`kernel/arch/x86_64/smp.rs`](../../kernel/arch/x86_64/smp.rs) | AP trampoline and bring-up |
| [`kernel/arch/x86_64/percpu.rs`](../../kernel/arch/x86_64/percpu.rs) | `Cpu`, the per-CPU block, `RunQueue`, `pick_lightest` |

Design history: [`dev/kernel-core.md`](../../dev/kernel-core.md), sections "Time", "Waiting", "The run queue and the process table", "Bottlenecks round" and "Second round".

---

## Tasks

A task is a `Process` (`kernel/process/mod.rs`). A thread is a task whose `tgid` is its group leader's pid. The rule for accessors: **per-task state is read from the task, process-wide state from the leader.** A thread has its own kernel stack, registers, `FS_BASE` and FPU area; it shares the descriptor table, working directory, credential, terminal and job. Processes, threads and signals are described in [processes.md](processes.md).

Scheduler-relevant fields of a task:

| Field | Meaning |
|:--|:--|
| `state` | `READY` (0), `RUNNING` (1), `SLEEPING` (2), `BLOCKED` (3), `DEAD` (4), `STOPPED` (5) |
| `pending_state` | A transition the task has asked for and the switch has not yet applied (`PENDING_NONE` = 255) |
| `stop_pending` | A stop requested from another CPU |
| `vruntime` | Virtual runtime |
| `priority` | 1 (highest) to 20 (lowest) |
| `cpu_mask` | Allowed CPUs; `CPU_ALL` by default |
| `rq_cpu`, `rq_slot` | Which CPU's run queue holds the task, and its slot in that heap |
| `timer_cpu`, `timer_slot`, `wake_ns` | Which CPU's sleeper heap holds it, its slot, and its deadline |
| `on_rq`, `in_switch`, `on_cpu` | Whether it is queued; popped and being claimed; still on some CPU's stack |
| `wakes` | Count of wakes sent to it (for `wait::join`) |
| `rsp`, `saved_user_rsp`, `saved_fs_base`, `fpu` | Saved context |
| `cpu_time`, `switch_in_count`, `preempt_count`, `sleep_count` | Accounting |

### The process table

| Structure | Purpose |
|:--|:--|
| `PROC_INDEX` | A static array of `MAX_PROCS` (32,768) task pointers indexed by pid: lock-free lookups from any context, including interrupt handlers. Updated with interrupts off |
| `PROC_TABLE` | `Mutex<BTreeMap<pid, Box<Process>>>`: owns the tasks |
| Pid bitmap | One atomic bit per pid, set from `alloc_pid` to `free_pid`. It is also the list of live tasks: `for_each_task` walks the set bits without a lock (512 words), and is safe in a fault handler |
| `PGROUPS` | Ordered set of `(pgid, pid)` for processes (not threads): job control and `kill(-pgid)` |
| `DEAD_LIST` | Tasks killed but not yet freed; the reaper looks here instead of walking the table |

Pids are taken from a cursor that rotates over the whole space and skips live pids, so a pid is reused as late as possible. A task is freed only when it is *quiesced*: off every run queue, not `in_switch`, not `on_cpu`, not current anywhere, and in no timer heap (`lifecycle::quiesced`).

`MAX_PROCS` is 32,768, Linux's default `pid_max`. What limits the number of live tasks in practice is memory for kernel stacks (128 KiB per program task; see [memory.md](memory.md)).

---

## Task states and the CAS discipline

Every change of `state` away from `RUNNING` is a compare-and-swap from `RUNNING` (`leave_running`). If it fails, the task was killed meanwhile, nothing is changed, and it goes onto no queue. `DEAD` is the one state nobody may write over.

- **A task never writes its own state or touches a run queue directly.** `yield_now`, `sleep`, `block_current` and `stop_current` record the wanted transition in `pending_state` and call `software_context_switch`. The scheduler applies it (`apply_pending`) only after the task's `rsp` is saved, so no other CPU can resume the task on a stack its own CPU is still using.
- **Taking a task off a queue is claiming it**: `READY -> RUNNING` by CAS in the pick loop. A task that fails the claim was killed after it was queued, and is dropped. Only the idle task is marked `RUNNING` with a plain store.
- **A task on another CPU's stack is passed over, not waited for** (up to four such tasks per pick). Two CPUs each picking the task the other just queued would otherwise wait for each other for ever.
- **`on_cpu` is released after the stack switch.** The switch stub clears the outgoing task's `on_cpu` only after `mov rsp, rax`, so another CPU never resumes a task whose stack is still in use.
- **A stop has its own flag** (`stop_pending`), checked first in `apply_pending`. A stop written into `pending_state` was overwritten by the target's next `yield_now`.
- **`STOPPED` is left only by `cont`** (SIGCONT). `wakeup` refuses to move a stopped task, because every signal sent wakes its target.
- **A dead task never comes back.** `kill_with_code` marks a task `DEAD` wherever it is. A task inside the kernel at that moment runs on to its next switch and is then dropped. So a dead task must finish what it is doing **without waiting**: `SchedMutex` spins rather than sleeps for a dead task, and `exit` closes its descriptors before it kills itself (see [processes.md](processes.md)). The syscall return path calls `stop_if_dead`, so a dead task never goes back to ring 3.

History: until 2026-09-25 the scheduler moved states with plain stores after a check, and a kill on another CPU between the check and the store resurrected killed threads (one boot in three hung after `stdtokio` exited).

---

## Run queues

Each CPU's `RunQueue` (`percpu.rs`) is a `SlotHeap`: a binary min-heap of `{ key, tie, task }` entries, keyed by vruntime with the pid as tie-breaker.

- **The key is copied into the entry** when the task goes in, so writing a queued task's vruntime cannot break the heap.
- **The task records its own slot** (`rq_slot`) and queue (`rq_cpu`), so taking a task out early (`kill`, a stop) goes straight to the right queue and is O(log n). `remove` re-checks under the queue's lock that the task is still there, and looks again if it moved.
- **The idle task is never queued**; `push` refuses it. The pick is the first entry, and "anyone waiting" is the length.
- **The array grows by doubling** under the queue lock with interrupts off, starting at 256 entries. This is safe because the kernel heap uses `IrqMutex` and neither it nor the frame allocator calls into the scheduler.
- **The lock** is a test-and-set spin that always disables interrupts (`RunQueue::with`), because the timer interrupt walks the queue.
- **`queued`** is an atomic copy of the length written on every unlock. Polling idle CPUs and `pick_lightest` read it without the lock.

### Choosing a CPU

`pick_cpu_for(mask)` returns `percpu::pick_lightest(mask)`: among online CPUs in the mask, the one with the lowest `2 * queued + busy` (busy = running something other than idle). During early boot, when the chosen CPU is not online, it falls back to the current CPU.

### Work stealing

When a CPU's queue is empty at a pick, `try_steal_task` walks the other online CPUs and takes the minimum of the first queue it can lock without waiting (`try_with`). A stolen task whose affinity forbids this CPU is put back. A stolen task is placed on the new CPU (`place_on`).

---

## vruntime, priorities and placement

**Charging.** Each tick charges the running task `TICK_SCALE / weight(priority)` of vruntime (`TICK_SCALE = 1,000,000`). The weights are CFS's nice-to-weight table:

| Priority | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|:--|:--|:--|:--|:--|:--|:--|:--|:--|:--|:--|
| Weight | 88761 | 71755 | 56483 | 46273 | 36291 | 29154 | 23254 | 18705 | 14949 | 11916 |

| Priority | 11 | 12 | 13 | 14 | 15 | 16 | 17 | 18 | 19 | 20 |
|:--|:--|:--|:--|:--|:--|:--|:--|:--|:--|:--|
| Weight | 9548 | 7620 | 6100 | 4904 | 3906 | 3121 | 2501 | 1991 | 1586 | 1277 |

Program tasks (`exec`, threads, `fork` children) start at priority 10; idle tasks are 20; kernel threads get theirs at spawn (`mikud` runs at 1). The kernel-shell command `nice <pid> <1-20>` changes a task's priority and `affinity <pid> <hex mask>` its CPU mask; there is no system call for either.

**`min_vruntime`.** Each CPU keeps the running maximum of the vruntime of tasks it switched in. It is a scale of its own; CPUs do not share one.

**Placement (`runqueue::place_on`).** Every task that joins a CPU's queue from anywhere but that queue (a wake, an expired sleep, a `SIGCONT`, a steal) has its vruntime clamped into `[min_vruntime, min_vruntime + PLACE_AHEAD]` of that CPU, where `PLACE_AHEAD` is four ticks at priority 10. Without this a task woken from a CPU further along queued behind a runner it was seconds "ahead" of (14 s measured under `stdproc`, 2026-10-23), and a thread stolen from a CPU further behind had the new CPU to itself. **Any new path that puts a task on another CPU's queue must call `place_on`.**

`wakeup` places the task only after winning the CAS that makes it the wake's to queue; placing before the CAS let a wake that lost to the timer write the vruntime of a task already in a heap.

---

## The scheduling decision

`schedule(old_rsp, in_kernel, charge)` in [`core.rs`](../../kernel/sched/core.rs) is reached three ways:

| Entry | When | `charge` |
|:--|:--|:--|
| `schedule_from_isr` | `software_context_switch`: every yield and sleep | yes |
| `schedule_from_timer(.., is_tick = true)` | The 250 Hz tick | yes (also counts the tick as user, system or idle in `/proc/stat`) |
| `schedule_from_timer(.., is_tick = false)` | A sleeper's deadline, or the reschedule IPI | no |

Steps:

1. Save `rsp`; mark the current task `on_cpu`.
2. Unless the task is held (caught in ring 0 by the timer, see below), apply its pending transition.
3. If it is still `RUNNING`:
   - charged: add a tick's vruntime and CPU time;
   - uncharged: compare as if it were a tick *behind*, so two tasks level with each other do not trade the CPU on every wake;
   - if the queue's minimum is lower, put the current task back on the queue (`preempt_count` + 1) and switch.
   - The idle task switches whenever anything is queued.
4. Pick: pop the minimum, or steal; pass over tasks still on another CPU; claim by CAS; else idle.
5. Switch: wait for the next task's `on_cpu` to clear, save and restore the per-CPU user `rsp` slot, `FS_BASE` (written only if it differs; `WRMSR` serialises) and the FPU/SSE state (`fxsave`/`fxrstor`, unconditionally), set `rsp0` in the TSS, update `min_vruntime`, store `loaded_cr3` and load CR3 if it differs, and return the new `rsp` plus the address of the old task's `on_cpu` flag for the stub to clear.

The FPU state is part of every task since 2026-09-22; before that two ring-3 programs doing float work shared the XMM registers.

---

## Preemption policy

**Ring 0 is not preempted.** A task the timer catches in the kernel keeps its CPU until it sleeps or yields: Linux's `PREEMPT_NONE`. The tick is still accounted and sleepers are still woken, but the task is not switched away, its pending transition is not applied, and a stop or kill reaches it at its next scheduling point. Ring 3 and the idle task are preempted as before.

Why: the kernel's spinlocks do not hold preemption off. A holder that the tick took away kept its lock while descheduled, and every CPU that came next spun on it with interrupts off until none was left to run the holder. `PROC_TABLE`, a wait queue's list and the TLB requester flag each stopped a four-CPU machine that way under a fork storm. In place since 2026-09-26; seven of seven `-smp 2` desktop boots were clean on 2026-10-14.

Consequences for kernel code:

- A long loop in the kernel must yield or sleep, or it holds its CPU (and only the tick's accounting runs).
- `SchedMutex` waiters spin 256 times and then sleep; because holders are never preempted, the spin usually succeeds.

---

## The timer

Each CPU's LAPIC timer runs in one of three modes, chosen at `apic::init_timer` and printed as `[apic] timer: ...`:

| Mode | When | Behaviour |
|:--|:--|:--|
| TSC-deadline | TSC calibrated and the CPU has TSC-deadline (`CPUID.01H:ECX[24]`) | Armed through `IA32_TSC_DEADLINE` for the next tick or the soonest sleeper |
| One-shot | TSC calibrated, no TSC-deadline | The LAPIC count is set for the same deadline |
| Periodic | TSC not calibrated, or `tickonly` on the boot line | 250 Hz, every interrupt is a tick, sleeps round to ticks |

`TIMER_HZ_DEFAULT` is 250, so a tick is 4 ms. After every timer interrupt, and whenever a sleeper sooner than either deadline is added on a CPU, `arm_timer` re-arms it.

**Tick or deadline.** `apic::timer_due` decides whether an interrupt is the tick: it is if it lands within 50 us (`TICK_SLACK_NS`) of the CPU's `NEXT_TICK_NS`. A tick missed while interrupts were off is gone; the next is a period from now, not a burst.

**The timer interrupt** (`timer_isr_entry`):

1. Decide tick or deadline; on the boot CPU a tick advances the global `TICK`.
2. EOI.
3. Once a second (every 250th tick, boot CPU only), ask kswapd to refill the emergency pool if it is low. Nothing on this path does I/O or takes a lock that thread code holds with interrupts on.
4. Expire this CPU's due sleepers onto run queues (`expire_sleepers`, up to 16 per interrupt).
5. Re-arm the timer.
6. `schedule_from_timer`.

**The reschedule IPI** (`VEC_IPI_RESCHED`, `resched_isr_entry`) goes through the same path, uncharged: another CPU queued work here. It used to only end a `hlt`, so a busy CPU left a woken task waiting up to 4 ms for its next tick.

The periodic tick is still the preemption quantum; there is no tickless idle.

**What ring 3 sees.** A tick is 4 ms. `sleep` (16) and `uptime` (17) count ticks, `poll` (76) takes milliseconds and rounds to ticks. `sleep_ns` (122) and `poll_ns` (123) take nanoseconds and are precise; `futex` (110) waits for its `timespec`. libmiku's `TICK_MS` is 4, matching the kernel (it said 10 until 2026-09-22, which made every ring-3 sleep 40 % of what it asked). libc's `nanosleep`, `usleep` and `clock_nanosleep` use `sleep_ns`, and `poll` uses `poll_ns`. See [MikuOS_ABI.md](../MikuOS_ABI.md).

---

## SMP bring-up and per-CPU data

**Bring-up** (`smp::start_aps`, after ACPI, the BSP's LAPIC, the timer and the main thread exist):

1. Read the processors from the ACPI MADT.
2. Copy the AP trampoline (its own ELF section) to physical `0x8000`, with a parameter block at offset `0xF00`.
3. Send INIT to every AP, wait `INIT_TO_SIPI_MS` once, then the SIPI handshake one AP at a time; each AP gets a 64 KiB boot stack (`AP_STACK_PAGES = 16`).
4. Wait up to 100 ms for every AP to mark itself online, then print `[smp] bringup complete: N/M cpus online`.

The trampoline goes from real mode to long mode and sets `EFER.NXE` (an AP without it triple-faulted on its first NX page, found 2026-10-27). Each AP then (`ap_entry`): installs its GS base, loads its GDT/TSS and the shared IDT, enables SSE, turns on the CPU protections (WP, SMEP, SMAP are per-core registers), sets `TSC_AUX` to its index (the vDSO's `getcpu` reads it with `RDTSCP`), programs the syscall MSRs, sets the PAT, brings its LAPIC online, registers its idle task, starts its timer with the BSP's calibration, and enters `sched::idle::run`.

Up to 64 CPUs (`percpu::MAX_CPUS`).

**Per-CPU data** is a `Cpu` block reached through `GS` (`gs:[0]` is the CPU index):

| Field | Purpose |
|:--|:--|
| `cpu_index`, `lapic_id`, `online` | Identity |
| `kernel_rsp`, `user_rsp` | Syscall entry stack and the saved user stack (swapped per task) |
| `current_pid`, `idle_pid`, `current_tty` | What runs here |
| `run_queue`, `min_vruntime`, `ticks`, `total_switches` | Scheduling |
| `tss_ptr`, `df_stack`, `pf_stack`, `sysc_stack` | TSS and IST stacks (double fault and page fault have their own) |
| `loaded_cr3` | The CR3 this CPU has loaded, stored before every `mov cr3` (TLB shootdown, [memory.md](memory.md)) |
| `polling`, `poll_ns`, `idle_left_ns` | Polling idle |

**IPIs:**

| Vector | Use |
|:--|:--|
| `0xE0` `VEC_IPI_RESCHED` | Work was queued for this CPU |
| `0xE1` `VEC_IPI_TLB` | TLB shootdown |
| `0xE2` `VEC_IPI_HALT` | Stop this CPU (panic) |
| `0x70`-`0x7F` | MSI vectors for devices (`VEC_MSI_BASE`, 16 of them) |

---

## Idle and polling idle

Every CPU's idle task runs `sched::idle::run`: yield, then `idle_once`, for ever.

- **Polling.** An idle CPU first watches its own run queue's `queued` count for a window (`poll_ns`), with `pause` between looks and `polling` set in its `Cpu`.
- **No IPI to a polling CPU.** `idle::kick` (called after a task is queued for another CPU) fences, reads `polling`, and sends the reschedule IPI only when the target is not polling. The idle CPU clears `polling` with a full fence before it looks at the queue a last time, so either it sees the task or the waker sends the IPI.
- **Halt.** After the window it disables interrupts, checks the queue once more, and executes `sti; hlt` as one step, so an IPI sent after the check ends the halt.
- **Adaptive window** (as Linux's `haltpoll` governor for guests): a CPU woken after its window but within 200 us (`POLL_MAX_NS`) doubles the window, starting from 50 us (`POLL_START_NS`); one that slept longer than 200 us halves it, and below 50 us it drops to zero. An idle desktop wakes for the tick and halts at once; a pipeline that wakes every few microseconds keeps its CPUs polling.
- **`nopoll`** on the boot line keeps the window at zero: every idle CPU halts at once and every cross-CPU wake is an IPI.

Why: under KVM, leaving `hlt` for an IPI is a VM exit on each side, about 4 us per wake. A pipe round trip between two threads went from 9.7 us to 2.1-2.6 us (2026-10-27). MWAIT idle is not implemented.

---

## Sleeping and waking

### Sleeping

| Function | Meaning |
|:--|:--|
| `sleep(ticks)` | Until the `ticks`th tick from now (a backstop, or a tick-based poll) |
| `sleep_ns(ns)` | For `ns` nanoseconds or until woken |
| `sleep_until(deadline)` | Until `kcore::time::now_ns()` reaches `deadline` or until woken |
| `sleep_until_unless(deadline, ready)` | Ask to sleep, then call `ready()`; if it returns true, cancel the sleep. Whether it slept |
| `block_current(cause)` | Off the CPU with no deadline until `wakeup` |
| `yield_now()` | Back onto the queue (refused for idle) |

A sleeping task has a deadline in nanoseconds (`wake_ns`) and sits in its CPU's min-heap of sleepers (`sched::timer`). The heap slot is stored in the task, so taking it out early is O(log n). It is a heap and not a sorted list because most sleepers are waits with a 100 ms backstop, and a 1 ms sleep inserted into a list would walk past all of them with interrupts off.

**`apply_sleep`**, the protocol `wakeup` relies on, in order:

1. Into this CPU's sleeper heap (before the state says `SLEEPING`, so whoever wins `SLEEPING -> READY` finds it there).
2. `RUNNING -> SLEEPING` by CAS (if that fails, the task was killed: take it out of the heap).
3. Only then take the request down: `pending_state` `SLEEPING -> NONE` by CAS. If a waker cancelled it first, put the task back to `RUNNING` and take it out of the heap: the waker returned believing the task runs on, so it does.

A task leaves a sleeper heap in exactly three places: expiry, `wakeup`, and `kill`.

### `wakeup(pid)`

With interrupts off:

1. Count the wake (`wakes += 1`), whatever state the task is in.
2. If the task has asked to sleep or block and not yet switched, cancel the request by CAS on `pending_state` and return true: the task keeps running.
3. Otherwise, if it is `SLEEPING` or `BLOCKED`, move it to `READY` by CAS (racing the timer's expiry; whoever wins queues it). A stopped task is not woken.
4. Take it out of its sleeper heap before queueing it, so this wake cannot remove the entry of a later sleep.
5. Pick a CPU, `place_on`, push, and `kick` the CPU if it is not this one.

`wakeup` returns whether it did anything, so `WaitQueue::wake_one` can skip a dead or already-awake waiter. It is safe from an interrupt handler.

### `waitpid(pid)` (kernel)

Sleeps on the `DEATH` queue with `wait_unless` until the pid is dead or gone, with a 100 ms backstop. `kill_with_code` wakes the parent and everything on `DEATH`. The system call `wait4` is described in [processes.md](processes.md).

---

## Wait queues and lost wakes

A `WaitQueue` (`sched/wait.rs`) is an `IrqMutex<Vec<pid>>` of waiters. Wakes are hints, never promises: a waiter always looks at its condition again.

| Call | Use |
|:--|:--|
| `wait(backstop_ticks)` / `wait_until(deadline)` | Join, sleep, leave. The caller must have looked at its condition just before; a wake in between is lost to the backstop |
| `wait_unless(deadline, ready)` | Join, ask to sleep, then ask `ready()`. Lost-wake free. `ready` must not sleep: an atomic or a spinlocked value |
| `wait_any(queues, deadline)` / `wait_any_unless(queues, deadline, ready)` | The same on several queues at once (`poll`) |
| `join(queues)` -> `Joined::sleep(deadline)` | For a look that may sleep or is long (a `poll` scan, a terminal under its locks): join first, look, then sleep unless a wake was counted since `join` |
| `wake_all()` | Take the whole list, drop the lock, wake each |
| `wake_one()` | Wake the oldest waiter that was really asleep (a lock's handoff) |

**The rule for wakers:** collect the pids under the lock that protects what changed, drop that lock, and only then wake. A woken task must not run straight into a lock its waker still holds.

**The backstop.** Every wait has a deadline even when something is supposed to wake it. The wake sites make a wait end promptly; the backstop only decides what a missed wake costs: latency, not a thread that never runs again.

**Lost-wake rules.** A wait that looks first and asks to sleep second has a gap: a waker that changes the condition in between finds the waiter `RUNNING` with no request, and the sleep that follows lasts the backstop. Two ways close it:

- `wait_unless` / `wait_any_unless` / `sleep_until_unless`: the condition is asked again with the sleep already requested, so a wake either cancels the request or came before the look.
- `wait::join`: the waiter joins, looks, and sleeps only if `Process::wakes` has not moved since the join.

Since 2026-10-28 no check-first wait remains: futex, pipes, `poll`, `wait4`, terminal input, pty master reads, local socket send/receive/connect/accept, TCP, `sched::waitpid` use `wait_unless`/`wait_any_unless`; an inotify read and a file-lock wait use `join` (their look consumes the event or takes the lock). A 5-7 us average regression in a ping-pong test is the signature of one 100 ms backstop per run.

**Queues.**

| Queue | Woken by |
|:--|:--|
| `DEATH` | A task dying (`kill_with_code`) |
| `PIPE` | Pipe writes and the last writer closing |
| `POLL` | Events that have no key: a process exiting, a socket closing, network receive |
| `keyed(key)` | 256 shared queues, chosen by hashing a key (`wait::key`): a kind in the top byte and the object's number below |

Keys (`wait::key`): `pipe(vnode)`, `socket(slot)`, `mayland(client)`, `tty_input(id)`, `pty_output(id)`, `net_tcp(handle)`, `net_listen(handle)`, `net_udp(handle)`, `net_icmp(handle)`. `wake_key(key)` wakes the queue that key hashes to. Two objects sharing a queue cost each other's waiters one more look, which a wake already allows. `poll` waits on the queues of exactly the descriptors it was given plus `POLL`.

**`IsrEvent`** (`kcore/isr_event.rs`) is the interrupt-handler variant: a sequence number and a few atomic waiter slots, no allocation. The GPU driver's message queue, vertical blank, event thread and fences use it. A waiter publishes its pid, looks again, then sleeps; the handler bumps the number and wakes the slots.

---

## Time and clocks

### The monotonic clock

`kcore::time::now_ns()` is nanoseconds since TSC calibration. Every deadline in the kernel and the vDSO's `CLOCK_MONOTONIC` use it, with the same arithmetic, so the system call and the vDSO never disagree.

- **Calibration** (`calibrate_early`, very early in boot): the TSC is counted over a 10 ms one-shot on PIT channel 2 (gate through port `0x61`), which needs no interrupts and no LAPIC. A result outside 800 MHz-6 GHz is rejected. It prints `[timing] TSC <MHz> MHz (PIT ch2, 10 ms window)`. The LAPIC timer is calibrated over the same PIT window.
- **Fallback.** Where the PIT does not respond or calibration fails, `precise()` is false, `now_ns` is ticks times 4 ms, and the timer stays periodic.
- **HPET** is only reported by the firmware probe; nothing uses it as a clock.
- `udelay(us)` / `mdelay(ms)` busy-wait on the TSC (or port `0x80` reads before calibration). `since_boot_ms`/`since_boot_us` timestamp boot steps and log lines.

### The wall clock

`kcore::clock` keeps an epoch anchor (Unix seconds plus the tick at which it was taken); reading adds the elapsed ticks. Sources:

- the CMOS RTC at boot (`clock::init_from_rtc`);
- `clock_settime(CLOCK_REALTIME)` (79), which needs `CAP_SYS_TIME`. The ring-3 program `ntpdate [server] [-q]` uses it (default server 216.239.35.0, time.google.com); see [network.md](network.md).

The clock is UTC. A time-zone offset in minutes (set by the installer, `clock::set_tz_offset`) is used only to format local dates.

### clock_gettime

`clock_gettime` (70) supports `CLOCK_REALTIME` (0; `EINVAL` until a source has set the clock) and `CLOCK_MONOTONIC` (1, the TSC to the nanosecond). The system call's realtime is anchored to a tick (4 ms steps, with nanoseconds from the tick remainder); the vDSO's realtime is precise. Programs normally read both through the vDSO without a system call: `std::time::Instant::now` costs 11 ns (2026-10-27). How the vDSO and its data page work is in [processes.md](processes.md).

---

## Kernel locks

| Lock | Where | Behaviour | Use for |
|:--|:--|:--|:--|
| `spin::Mutex` | `spin` crate | Spins, interrupts unchanged | Short sections taken only by threads, never from an interrupt handler or the timer path |
| `IrqMutex` | `kcore/irq_lock.rs` | Disables interrupts for the whole section, restores the previous state; nests | Anything reachable from an interrupt handler: the frame allocator, the swap reverse map, heap caches and magazines, sleeper heaps, wait-queue lists, the wall clock anchor |
| `RunQueue` lock | `percpu.rs` | Test-and-set spin; `with` always disables interrupts | Run queues only |
| `SchedMutex` | `kcore/sched_lock.rs` | Spins 256 times, then sleeps on its own `WaitQueue`; 20 ms backstop | Locks held across disk I/O or long work |
| `SchedRwLock` | `kcore/sched_lock.rs` | Many readers or one writer; writers first; spins then sleeps | Mostly-read data: the VFS tree, the file cache |
| `COW_LOCK` | `mm/vmm.rs` | Machine-wide spin with interrupts off that answers TLB shootdowns while waiting | Copy-on-write page-table changes ([memory.md](memory.md)) |

### SchedMutex details

- **No lost wake.** A waiter counts itself in `waiting`, joins the queue and asks to sleep (`wait_unless`) before reading `locked` a last time; unlock clears `locked` and then wakes one waiter only if `waiting` is non-zero. All SeqCst.
- **Who never sleeps on it:** code before the scheduler starts (spins), the idle task (yields), and a task already marked dead (spins).
- **Never take one from an interrupt handler.** Yielding from an ISR is not possible.
- `lock()` is `#[track_caller]`, so lock statistics name the caller, not the wrapper.
- `SchedMutex::named(name, ..)` gives it a name in `/proc/lockstat`; `new` names it `?`.

The named sleeping locks in the kernel:

| Name | Type | Protects |
|:--|:--|:--|
| `vfs` | `SchedRwLock` | The VFS (`VFS_LOCK`); see [vfs.md](vfs.md) |
| `filecache` | `SchedRwLock` | The disk-file page cache |
| `fd` (64 shards), `fd-retired` | `SchedMutex` | Descriptor tables; retired tables waiting out their grace period |
| `unlinked`, `wb-room` | `SchedMutex` | Pending deletions; free space per mount for write-back |
| `ext`, `btrfs`, `memfs` | `SchedMutex` | Each filesystem's mount slots and state; see [filesystems.md](filesystems.md) |
| `gsp-rm`, `disp-mem`, `kms` | `SchedMutex` | GPU resource manager, display memory, display (see [Nvidia_gb206.md](../Nvidia_gb206.md)) |

### Rules

- **Nothing on the timer-interrupt path takes a plain `spin::Mutex` that thread code holds with interrupts on.** The tick landing on the holder's CPU would spin for ever. ISR-reachable state is atomic or behind `IrqMutex` (`PID_ALLOC` broke this and hung fork storms until 2026-09-26; the pid high-water mark is an atomic now).
- **Never shoot down the TLB while holding a lock another CPU may spin on with interrupts off.**
- **Wake after unlocking.**
- **A dead task must not wait.**
- **Allocation is safe with interrupts off** (heap locks are `IrqMutex`), but nothing allocates under the frame allocator's lock.

### Lock orders

| Order (outer to inner) | Source |
|:--|:--|
| `VFS_LOCK` -> a filesystem's slot lock (`ext`, `btrfs`, `memfs`) -> `FILE_CACHE` -> a descriptor shard | [vfs.md](vfs.md), [`dev/vfs-locking.md`](../../dev/vfs-locking.md) |
| Leaves (nothing taken under them): `unlinked`, `wb-room`, `fd-retired`, a descriptor shard | same |
| Mayland: `SERVER` -> `POOLS` | [mayland.md](mayland.md) |
| Heap: a CPU's magazines -> a class's cache -> frames; vmap -> a cache -> frames | [memory.md](memory.md) |

---

## Lock statistics

Every `SchedMutex` and `SchedRwLock` counts, always, how often it was taken, how often a taker had to wait, and how often a taker slept. With `lockstat` on the boot line it also times each wait and each hold (a histogram of under 1 us, 10 us, 100 us, 1 ms, and 1 ms or more) and records the longest hold with the place that took it. A hold of 1 ms or more is logged with its place (`kwarn!`), at most ten a second. Locks register in a fixed table of 128 the first time they are taken, and `/proc/lockstat` prints one line per name with shards merged. Without `lockstat` the cost is one atomic add and one relaxed load per lock; with it, two clock reads. Spinlocks, `IrqMutex` and run-queue locks are not covered. The file format is in [diagnostics.md](diagnostics.md).

---

## Kernel threads and the work queue

Kernel threads are tasks with no user address space, spawned with `sched::spawn_named(entry, name, priority)` and given 512 KiB stacks. `mikud` itself runs at priority 1 and starts the services registered in `main.rs` (`kbd`, `mikudod`, `shell`, `netd`, `usbd`, `bdflush`, `kswapd`, `compositor`, `greeter`); the in-kernel ones run as its child threads, `kbd` at priority 2 and `netd`, `bdflush` and `kswapd` at 1 (see [system.md](system.md)). `run=` on the boot line adds `bootline` (see [boot.md](boot.md)); the compositor runs as the `mayland` kernel thread.

`sched::submit_task(closure)` queues work for a pool of kernel worker threads (`workers.rs`). The pool is sized from the ACPI processor count, between 4 and 32 workers. Workers sleep briefly when the queue is empty.

---

## Boot flags

The full table is in [boot.md](boot.md). Flags that change scheduling, waiting or time:

| Flag | Effect |
|:--|:--|
| `tickonly` | Keep the periodic 250 Hz timer: no TSC-deadline or one-shot arming, sleeps round to ticks |
| `nopoll` | No polling idle: every idle CPU halts at once and every cross-CPU wake is an IPI |
| `schedwatch` | kswapd's once-a-second check also reports tasks `READY`/`RUNNING` on no CPU, a line per CPU, and the whole task table every five seconds |
| `lockstat` | Time sleeping-lock waits and holds; log holds of 1 ms or more |
| `trace` | Per-item boot lines and debug lines go to the serial port too (115200 baud with interrupts off: slow) |
| `tlbalways` | A TLB shootdown on every page-table change ([memory.md](memory.md)) |
| `vfsmutex`, `walklocked` | VFS locking fallbacks ([vfs.md](vfs.md)) |

---

## Diagnosing a stall

| Tool | What it tells you |
|:--|:--|
| kswapd's overdue-sleeper report | Every second: any task asleep more than a second past its deadline (`[sched] ...`). Should never print; when it does, a task is missing from its heap or a heap's timer is not armed |
| `schedwatch` | Starvation (`READY` on no CPU at two looks a second apart), each CPU's current task, queue length, sleepers and switch count, and the whole table every 5 s. A **frozen switch count** is starvation; a task asleep on a 100 ms backstop for ever is a wait whose condition never comes. `READY` reports for tasks that wake every few ms are noise |
| `/proc/lockstat` with `lockstat` | Which sleeping lock waits, sleeps or holds longest, and where |
| `/proc/stat` | Per-CPU user/system/idle ticks and context switches |
| `top` (kernel shell), `/proc/<pid>/` | Per-task state and CPU use ([commands.md](commands.md), [diagnostics.md](diagnostics.md)) |
| `dmesg` | The kernel log, including `kwarn!` lines from the above |
| QEMU monitor | `info registers -a`, symbolised with `addr2line -e iso/boot/kernel.elf`, shows which lock each CPU spins on; this found the `PID_ALLOC` and Mayland `SERVER`/`POOLS` deadlocks ([testing.md](testing.md)) |

Signatures:

- **Exactly 20 ms worst-case latency**: a `SchedMutex` waiter that slept to its backstop (a lost wake).
- **Exactly 100 ms**: a pipe, `poll`, futex or `waitpid` backstop.
- **A small average regression in a ping-pong test** (a few microseconds over thousands of round trips): one backstop per run.
- **Every CPU spinning with interrupts off**: a lock held by a task that cannot run (taken in an ISR path, or held across a shootdown).

---

## Tests

- **`ipc` suite** (`ipc test` in the kernel shell, or `run=ipc_test` on the GRUB line) includes the scheduler's cases from [`kernel/sched/selftest.rs`](../../kernel/sched/selftest.rs):
  - `sched/heap`: 300 entries with a third removed from the middle;
  - `pid/alloc`: 130 pids across two bitmap words, all distinct and in range;
  - `pid/live-walk`: the bitmap walk against the table;
  - `job/index`: the running task moved into a job of its own and back;
  - keyed-queue wake cases, local sockets and pipes.
- **Fork storm** (`target/sigtest/storm`, `SMP=`): many processes forking and exiting at once; the main regression test for scheduler and lock bugs (see [testing.md](testing.md)).
- **`stdthreads --swarm <threads> <yields>`**: thousands of threads parked on a `Condvar`, released with one `notify_all`, yielding, then exiting.
- **`stdbench`**: pipe and futex round trips, spawn, `Instant::now` (see [testing.md](testing.md)).

---

## Measured numbers

QEMU/KVM, 4 vCPUs. Linux numbers are the dev box on bare metal, so not like for like. Source: [`dev/kernel-core.md`](../../dev/kernel-core.md).

**Wakes and sleeps:**

| | Before | After | Linux | Date |
|:--|:--|:--|:--|:--|
| 1 ms `nanosleep`, average / worst | 1.2 / 3.3 ms | 1.012 / 1.04 ms | | 2026-10-23 |
| Pipe round trip, two threads | 9.7 us | 2.1-2.6 us (`nopoll`: 9.4) | 7.0 us | 2026-10-27 |
| Pipe round trip, two processes | 10.8 us | 2.2-2.6 us | 6.2 us | 2026-10-27 |
| Futex round trip (`Mutex` + `Condvar`) | 1.3 us | 1.1-1.5 us | 6.2 us | 2026-10-27 |
| `Instant::now` | 150 ns | 11 ns | 20 ns | 2026-10-27 |
| Process spawn + wait (a `std` program) | 9.0 ms | 0.27 ms | 0.63 ms | 2026-10-27 |

**Many tasks (2026-10-23):**

| | Before | After |
|:--|:--|:--|
| Exit of a program with 2000 threads | 624 ms | 0-2 ms |
| Reaping its 2000 threads | 106 ms | 11 ms |
| Adding a task with 2000 in the table | 18 us | 4 us |
| 8 x 3000 `fork` + exit + `wait4` (`stdproc --churn`) | 22-23 s | 8 s |
| Tasks alive at once | 4096 at most | 4300 checked (`-m 4G`) |

2000 threads start in 144-224 ms and 4300 in 807 ms; 2000 threads woken at once and yielding 200 times each take 43-47 ms, 8.5-9.5 million yields a second across four CPUs.

---

## Limits and what is not done

- **No kernel preemption** by design (`PREEMPT_NONE`); a long kernel loop must yield.
- **No tickless idle**; the 250 Hz tick keeps running on every CPU.
- **No MWAIT idle**: KVM hides `MONITOR` from guests without `-overcommit cpu-pm=on`, so it could not be tested, and it matters only on hardware.
- **No priority or affinity system calls**; only the kernel-shell `nice` and `affinity` commands.
- **No real-time scheduling classes.**
- **Some walks of `PROC_TABLE` are O(live tasks) under its lock**: `group_threads` (exit, `exec`, `set_ctty`), the orphan reparent when a process dies, `wait4`'s search for a zombie child, and the reaper's check whether a group has anyone left. None showed up at 2000 tasks.
- **`CLOCK_REALTIME` through the system call moves in 4 ms steps** (the vDSO's is precise). Only `CLOCK_REALTIME` and `CLOCK_MONOTONIC` exist.
- **Lock statistics do not cover spinlocks, `IrqMutex` or run queues**; a long hold on Mayland's `SERVER` spinlock shows as a long frame, not in `/proc/lockstat`.
- Verification: the scheduler, timer, polling idle and lock changes of 2026-10-21..28 are QEMU-verified on both CPU models (qemu64 with a one-shot LAPIC, host with TSC-deadline); a boot on the real card machine was still pending at the last note.

Related pages: [memory.md](memory.md), [processes.md](processes.md), [vfs.md](vfs.md), [diagnostics.md](diagnostics.md), [boot.md](boot.md).
