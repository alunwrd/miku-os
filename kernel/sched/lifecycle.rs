// Process lifecycle: spawn, kill, reap, parent/child wiring, attribute mutators
//
// Anything that creates a Process, removes one, or alters a stable attribute
// (priority, affinity, parent, address space) lives here. State transitions
// (sleep/block/wakeup) live in control.rs

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use x86_64::instructions::interrupts;

use crate::arch::x86_64::percpu;
use crate::kcore::irq_lock::IrqMutex;
use crate::process::{Process, CPU_ALL, STATE_DEAD};

use super::control::wakeup;
use super::proc_index::{
    pid_running_anywhere, register_process, PROC_INDEX, PROC_TABLE,
};
use super::runqueue::{pick_cpu_for, push_to_cpu};

// spawn

fn add_process(mut p: Box<Process>) {
    let mask   = p.cpu_mask;
    let target = pick_cpu_for(mask);
    let cpu    = percpu::get(target);
    let min_vr = cpu.min_vruntime.load(Ordering::Relaxed);
    p.vruntime.store(min_vr, Ordering::Relaxed);
    let name = p.name;
    let raw  = register_process(p);
    let pid  = unsafe { (*raw).pid };
    push_to_cpu(cpu, raw);
    crate::trace_println!("[sched] spawn pid={} name={} cpu={}", pid, name, target);

    // Nudge the target CPU so it picks the new task up sooner
    if target != percpu::current_index() {
        super::idle::kick(cpu);
    }
}

pub fn spawn(entry: fn() -> !) -> u64 {
    spawn_named(entry, "kthread", 10)
}

pub fn spawn_named(entry: fn() -> !, name: &str, priority: u8) -> u64 {
    let p = Process::new_kernel_named(entry, name, priority);
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn spawn_named_child_of(
    parent_pid: u64,
    entry: fn() -> !,
    name: &str,
    priority: u8,
) -> u64 {
    let mut p = Process::new_kernel_named(entry, name, priority);
    p.ppid.store(parent_pid, Ordering::Relaxed);
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn spawn_child_of(parent_pid: u64, entry: fn() -> !) -> u64 {
    spawn_named_child_of(parent_pid, entry, "kthread", 10)
}

pub fn spawn_named_child(entry: fn() -> !, name: &str, priority: u8) -> u64 {
    let parent = super::current_pid();
    spawn_named_child_of(parent, entry, name, priority)
}

pub fn add_user_process(p: Box<Process>) -> u64 {
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

// idle thread bringup

/// True once 'init_main_thread' has run; drivers consult this before yielding
/// out of an I/O wait loop (pre-scheduler boot code must spin instead)
static SCHED_STARTED: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn started() -> bool {
    SCHED_STARTED.load(Ordering::Acquire)
}

// BSP idle thread registration - must run after percpu is initialised
pub fn init_main_thread() {
    let tick = crate::arch::x86_64::interrupts::get_tick();
    let cr3  = crate::mm::vmm::kernel_cr3();
    let raw  = register_process(Process::new_idle(cr3, tick));
    let bsp  = percpu::get(0);
    bsp.current_pid.store(0, Ordering::Relaxed);
    bsp.idle_pid   .store(0, Ordering::Relaxed);
    SCHED_STARTED.store(true, Ordering::Release);
    crate::trace_println!("[sched] bsp idle ptr={:p}", raw);
}

// Register a per-CPU idle thread for an AP. Called from ap_entry after
// percpu::install_gs_base and gdt::init_cpu
pub fn init_ap_idle(cpu_idx: usize) {
    let tick = crate::arch::x86_64::interrupts::get_tick();
    let cr3  = crate::mm::vmm::kernel_cr3();
    let idle = Process::new_idle_ap(cr3, tick, "idle_ap");
    let pid  = idle.pid;
    let _raw = register_process(idle);
    let cpu  = percpu::get(cpu_idx);
    cpu.current_pid.store(pid, Ordering::Relaxed);
    cpu.idle_pid   .store(pid, Ordering::Relaxed);
    crate::trace_println!("[sched] cpu{} idle pid={}", cpu_idx, pid);
}

// kill / reap

/// Every task in a thread group, leader first.
///
/// A `Vec`, with the lock dropped before it returns: the callers (`set_ctty`,
/// `kill_group`, the reaper) do something to each member that itself takes the
/// table
pub fn group_threads(tgid: u64) -> Vec<u64> {
    let table = PROC_TABLE.lock();
    let mut out: Vec<u64> = Vec::new();
    if table.contains_key(&tgid) { out.push(tgid); }
    for (&pid, p) in table.iter() {
        if pid != tgid && p.tgid() == tgid { out.push(pid); }
    }
    out
}

/// True while any task of `tgid` other than `except` is still on a processor.
///
/// What `exec` waits on. Marking a sibling dead takes it off the run queue but
/// does not stop a CPU already inside it, which keeps executing the old image
/// until its next scheduling point. `exec` frees the old address space by hand,
/// so without this wait it would unmap page tables under a running thread.
///
/// `on_cpu` rather than the state: it says a processor is still on this task's
/// stack, and is cleared on the far side of the stack switch
pub fn group_still_on_cpu(tgid: u64, except: u64) -> bool {
    for t in group_threads(tgid) {
        if t == except { continue; }
        let ptr = unsafe { PROC_INDEX.get_raw(t) };
        if ptr.is_null() { continue; }
        if unsafe { (*ptr).on_cpu.load(Ordering::Acquire) } { return true; }
        if pid_running_anywhere(t) { return true; }
    }
    false
}

/// Ends a whole process: every thread of it, siblings first and the leader
/// last.
///
/// `exit` means the process. The leader goes last so `leader_raw` can still
/// resolve the siblings' credential and descriptors through it.
///
/// Until there are none left. The list is a snapshot, and a thread creating
/// another can register a new one after the snapshot and before its own kill
/// lands. So the group is marked first (`group_exiting`, which `thread_spawn`
/// checks once its thread is in the table, so one of the two always sees the
/// other) and the walk repeats until it finds nobody alive
pub fn kill_group_with_code(tgid: u64, code: u64) {
    kill_group_threads(tgid, tgid, code);
    kill_with_code(tgid, code);
}

/// Every thread of `tgid` but its leader and `spare`, as `kill_group_with_code`
/// does it: what `exit` runs first, sparing itself.
///
/// A task marked dead and still in the kernel never comes back from a switch,
/// so what the exiting task still has to do must not wait once it is dead, and
/// closing its descriptors takes the VFS lock. Killed first, an exit meeting
/// that lock would leave the descriptors open in a zombie, including a pipe's
/// write end, and the parent would read on for ever
pub fn kill_group_threads(tgid: u64, spare: u64, code: u64) {
    let leader = unsafe { PROC_INDEX.get_raw(tgid) };
    if !leader.is_null() {
        unsafe { (*leader).group_exiting.store(true, Ordering::SeqCst) };
    }
    loop {
        let alive: Vec<u64> = group_threads(tgid)
            .into_iter()
            .filter(|&t| t != tgid && t != spare)
            .filter(|&t| {
                let p = unsafe { PROC_INDEX.get_raw(t) };
                !p.is_null() && unsafe { (*p).state.load(Ordering::SeqCst) } != STATE_DEAD
            })
            .collect();
        if alive.is_empty() { break; }
        for t in alive {
            kill_with_code(t, code);
        }
    }
}

/// Whether the group `tgid` is being ended: see `kill_group_with_code`
pub fn group_exiting(tgid: u64) -> bool {
    let leader = unsafe { PROC_INDEX.get_raw(tgid) };
    leader.is_null()
        || unsafe { (*leader).group_exiting.load(Ordering::SeqCst) }
        || unsafe { (*leader).state.load(Ordering::SeqCst) } == STATE_DEAD
}

/// Never go back to ring 3 dead.
///
/// `kill_with_code` marks a task that may be in the middle of a syscall, and
/// the task would learn of it at its next switch, after returning to ring 3. A
/// thread of a process that just called `exit` would see its `poll`, woken by
/// the descriptors going, answer EBADF. The syscall return path calls this; a
/// dead task yields for good
pub fn stop_if_dead() {
    let p = unsafe { PROC_INDEX.get_raw(super::current_pid()) };
    if !p.is_null() && unsafe { (*p).state.load(Ordering::SeqCst) } == STATE_DEAD {
        loop {
            super::yield_now();
        }
    }
}

pub fn kill(pid: u64) { kill_with_code(pid, 0); }

pub fn kill_with_code(pid: u64, code: u64) {
    let (ppid, thread) = interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return (0u64, false); }
        let p = unsafe { &*ptr };
        p.exit_code.store(code, Ordering::Relaxed);
        let prev_state = p.state.swap(STATE_DEAD, Ordering::Relaxed);
        if prev_state == crate::process::STATE_SLEEPING {
            unsafe { super::timer::unlink(ptr) };
        }
        super::runqueue::remove(ptr);
        if prev_state != STATE_DEAD {
            DEAD_LIST.lock().push(pid);
        }
        (p.ppid.load(Ordering::Relaxed), p.is_thread())
    });
    // And one line when a process ends, with the code. Not one per thread,
    // outside `trace`: a program's exit ends every thread it has, and at 115200
    // baud a line is 2.5 ms with interrupts off.
    //
    // A process's exit goes to the log only, for the same reason: every spawn
    // has one. A crash says itself in `[crash]`
    if thread {
        crate::trace_println!("[sched] kill tid={} code={}", pid, code);
    } else {
        crate::kdebug!("[sched] kill pid={} code={}", pid, code);
    }

    // Reparent any children of the dying process to init (PID 1) so they can
    // still be reaped via wait4 once they exit. An orphan's ppid would
    // otherwise point at a stale slot wait4 can never walk to. PID 1 is mikuD,
    // which reaps them. If pid 1 does not exist (early boot) ppid stays;
    // reap_dead still frees dead processes whose state is DEAD, and only the
    // zombie/wait4 path needs a live parent.
    //
    // Not for a thread, which is nobody's parent: `fork` records the process
    // (`sys_fork` takes the caller's tgid) and the kernel's own spawns name
    // mikuD. The walk is of the whole table, so it must not run once per thread
    let init_alive = process_exists(1);
    if init_alive && pid != 1 && !thread {
        let table = PROC_TABLE.lock();
        let orphans: Vec<u64> = table.iter()
            .filter(|(_, c)| c.ppid.load(Ordering::Relaxed) == pid)
            .map(|(&cpid, _)| cpid)
            .collect();
        drop(table);
        for orphan in orphans {
            set_parent(orphan, 1);
            crate::trace_println!("[sched] reparent pid={} -> init (1)", orphan);
        }
        // Already-zombie orphans need init to be told; the journal SIGCHLD
        // delivery is best-effort
        let _ = init_alive;
    }

    if ppid != 0 { wakeup(ppid); }
    // And anyone waiting on this pid who is not its parent
    super::wait::DEATH.wake_all();

    // A session whose leader has died is over: a desktop that quits must not
    // leave a login open, or the greeter would come back over a session owning
    // the seat
    crate::process::session::on_process_exit(pid);

    // A terminal whose foreground has died has no foreground. Left set, the
    // slot names a pid that can be reused, and the next Ctrl-C would signal
    // whoever has that number
    crate::io::tty::on_process_exit(pid);
}

/// A dead task its parent is still entitled to hear about.
///
/// `reap_dead` is called from every spawn path, and must not free a child whose
/// parent has not reached its `wait4`.
///
/// The test is the two fields `find_zombie_child` reads. `PROC_INDEX` rather
/// than `process_exists` for the parent, because this runs with `PROC_TABLE`
/// held. A thread is never a zombie: `new_thread` sets `ppid` to zero and
/// `collected` to true
#[inline]
fn is_uncollected_zombie(p: &Process) -> bool {
    if p.collected.load(Ordering::Relaxed) { return false; }
    let ppid = p.ppid.load(Ordering::Relaxed);
    if ppid == 0 { return false; }
    !unsafe { PROC_INDEX.get_raw(ppid) }.is_null()
}

/// Nothing is standing on this task any more: no processor names it as current,
/// no processor is on its kernel stack, and no run queue holds it. What freeing
/// a `Process` (its kernel stack, and for the last task of a group the page
/// tables) waits for.
///
/// `current_pid` alone is not enough. A switch names the next task as current
/// before the stub leaves the old one's stack (`mov rsp, rax`), and `on_cpu`
/// covers that stretch. A zombie is DEAD from inside its own `exit`, well
/// before it gets off the processor, so `wait4` on another CPU must not free
/// its stack while it is returning through them
#[inline]
/// Read in the order the scheduler hands a task along (queue, then the pick:
/// `in_switch`, set before `on_rq` drops and cleared after `on_cpu` is set,
/// then the processor), so there is no moment when all read clear while a CPU
/// is on its way onto this task's stack
fn quiesced(p: &Process) -> bool {
    !p.on_rq.load(Ordering::SeqCst)
        && !p.in_switch.load(Ordering::SeqCst)
        && !p.on_cpu.load(Ordering::SeqCst)
        && !pid_running_anywhere(p.pid)
        // Nor in a CPU's heap of sleepers, which would point at it freed
        && !super::timer::linked(p)
}

/// Tasks `kill_with_code` has marked dead, by pid, for `reap_dead` to look at
/// instead of the whole table.
///
/// An entry stays while its task is dead and not yet free to go (on its
/// processor still, or a zombie its parent has not collected), and is let go
/// once the task has left the table, or the pid names a task not dead (the
/// number was given out again).
///
/// An `IrqMutex`: a fault in ring 3 kills from the exception handler
static DEAD_LIST: IrqMutex<Vec<u64>> = IrqMutex::new(Vec::new());

pub fn reap_dead() {
    let listed: Vec<u64> = core::mem::take(&mut *DEAD_LIST.lock());
    if listed.is_empty() {
        return;
    }
    let mut ready = Vec::new();
    let mut waiting = Vec::new();
    {
        let table = PROC_TABLE.lock();
        for pid in listed {
            let Some(p) = table.get(&pid) else { continue };
            if p.state.load(Ordering::Relaxed) != STATE_DEAD || p.reaping.load(Ordering::Relaxed) {
                continue;
            }
            if quiesced(p) && !is_uncollected_zombie(p) {
                ready.push(pid);
            } else {
                waiting.push(pid);
            }
        }
    }
    if !waiting.is_empty() {
        DEAD_LIST.lock().extend_from_slice(&waiting);
    }
    for pid in ready {
        reap_one(pid, true);
    }
}

/// Forget every listed death, for a scheduler starting over with an empty
/// table (`reinit_scheduler`)
pub fn forget_dead() {
    DEAD_LIST.lock().clear();
}

pub fn reap_zombie(pid: u64) {
    // Its status has been taken; now waits for it to be off the processor it
    // died on: a few instructions at most, the tail of its own `exit` and one
    // switch. See `quiesced`
    loop {
        let off = PROC_TABLE.lock().get(&pid).map_or(true, |p| quiesced(p));
        if off { break; }
        super::yield_now();
    }
    reap_one(pid, false);
}

/// The dead siblings of an `exec`ing leader, reaped before it frees the old
/// address space, and waited for when another reaper has them.
///
/// Each holds that address space until it leaves the table (its stack is
/// unmapped from it first, `reap_one`), so none left in the table makes the
/// free safe. Bounded like the wait for them to leave the processors: a limp
/// rather than a hang if something is wrong
pub fn reap_group_siblings(tgid: u64) {
    let mut spins = 0u32;
    loop {
        let rest: Vec<u64> = group_threads(tgid).into_iter().filter(|&t| t != tgid).collect();
        if rest.is_empty() { return; }
        for t in rest { reap_one(t, true); }
        spins += 1;
        if spins > 100_000 {
            crate::serial_println!("[exec] pid={} gave up reaping its threads", tgid);
            return;
        }
        super::yield_now();
    }
}

/// Frees one dead task, as exactly one reaper.
///
/// Three steps, in this order:
///
/// 1. Claim it (`Process::reaping`). Every spawn runs `reap_dead`, on every
///    processor, and each settles what it claims and nothing else.
/// 2. Unmap the stack it asked the kernel to own, while it is still in the
///    table. A task in the table keeps its group's address space alive, so the
///    unmap cannot meet the teardown. Only in the group's live address space:
///    after `exec` the siblings name the old one, which `exec` frees itself.
/// 3. Remove it, and ask whether it was the last of its group under the same
///    lock: two threads removed by two reapers at once must not both count zero
///    and both free the address space.
///
/// `check_dead` is off for `reap_zombie`, whose caller has already decided
fn reap_one(pid: u64, check_dead: bool) {
    let (tgid, cr3, base, len) = {
        let table = PROC_TABLE.lock();
        let Some(p) = table.get(&pid) else { return };
        if check_dead
            && !(p.state.load(Ordering::Relaxed) == STATE_DEAD
                && quiesced(p)
                && !is_uncollected_zombie(p))
        {
            return;
        }
        if p.reaping
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let tgid = p.tgid();
        let live = table.iter().any(|(&q, t)| q != pid && t.tgid() == tgid && t.cr3 == p.cr3);
        let (base, len) = if live {
            (p.owned_stack.load(Ordering::Relaxed), p.owned_stack_len.load(Ordering::Relaxed))
        } else {
            (0, 0)
        };
        (tgid, p.cr3, base, len)
    };

    super::proc_index::unregister(pid);
    core::sync::atomic::compiler_fence(Ordering::SeqCst);

    // See `Process::owned_stack`: the task is off every processor, and the
    // address space still belongs to its siblings
    if base != 0 && len != 0 {
        crate::mm::mmap::sys_munmap(cr3, base, len);
    }

    let (p, last) = {
        let mut table = PROC_TABLE.lock();
        let Some(p) = table.remove(&pid) else { return };
        let last = !table.values().any(|q| q.tgid() == tgid);
        (p, last)
    };
    free_process_resources(p, pid, last);
}

fn free_process_resources(mut p: Box<Process>, pid: u64, last: bool) {
    // How deep its kernel stack went, before the stack goes with the `Box`
    // (`mm::kstack::account`)
    crate::mm::kstack::account(&p.stack, pid, &p.name);
    // Whose resources are these?
    //
    // A thread owns its kernel stack and nothing else. Everything below
    // (sockets, shm pools, GPU fences, the descriptor table, the VMA table, the
    // address space) belongs to the process. The `Box` going out of scope at
    // the end of this function is a thread's whole teardown.
    //
    // The group is torn down by whichever of its tasks is reaped last, not by
    // the leader: a leader may be reaped before its threads.
    //
    // `last` is what `reap_one` saw in `PROC_TABLE` as it removed this task,
    // under the same lock, so exactly one task of a group is told. A
    // single-threaded process is last on its first pass
    let tgid = p.tgid();
    if !last {
        crate::trace_println!("[sched] thread pid={} of tgid={} gone", pid, tgid);
        // The number goes back even for a thread: it is out of the table and
        // out of the index, and nothing can name it any more
        super::free_pid(pid);
        return;
    }

    // Releases any network sockets the process still owns. Before the VFS fd
    // table teardown; socket fds live in a separate range not tracked by the
    // VFS
    crate::net::socket::close_all_for_pid(tgid);

    // And its record locks, which are the process's and not any descriptor's
    // (see `fs::vfs::lock`): a database another process waits on must not stay
    // locked by a dead one
    crate::fs::vfs::lock::release_owner(tgid);

    // Same for the display protocol: shared-memory pools are named by a handle,
    // so the VFS teardown never reaches them
    crate::mayland::server::shm::destroy_all_for(tgid);

    // And what it had queued on the GPU. The pools above are gone, so there is
    // nothing left to wait for, and an entry per dead pid would accumulate
    crate::syscall::adapters::gpu::forget_process(tgid);

    // Drop the process's per-process FD table and release the vnode
    // references it was keeping alive
    let victims = crate::fs::vfs::core::with_vfs(|vfs| vfs.drop_fds(tgid));
    // This path and not only `release_fds_on_exit`: a process that called
    // `exit` goes through that one, and one a signal killed comes only here. A
    // pty master among these is counted by its `dec_ref`, and a terminal whose
    // last master died with this process is hung up by `with_vfs` once the lock
    // is released
    if !victims.is_empty() {
        crate::fs::vfs::core::with_vfs(|vfs| {
            for vid in victims {
                let idx = vid as usize;
                vfs.drop_ref(idx);
            }
        });
    }

    crate::mm::mmap::vma_cleanup(p.cr3);
    // The image record exists to explain a faulting address in this process,
    // and no process is left to fault
    crate::process::image::forget(tgid);
    if let Some(phys) = p.user_stack_phys.take() {
        crate::mm::pmm::free_frames(phys, crate::process::USER_STACK_PAGES);
    }
    if p.cr3 != 0 && p.cr3 != crate::mm::vmm::kernel_cr3() {
        // Anything queued on a futex in this address space is queued on memory
        // about to stop existing. The entries are pids, and a pid is reused
        // eventually, so `wake` could find somebody else's thread
        crate::sched::futex::forget_address_space(p.cr3);
        let mut aspace = crate::mm::vmm::AddressSpace { cr3: p.cr3 };
        aspace.free_address_space();
    }
    // Last, and only here. Anywhere earlier and the number could be handed to a
    // new task while a signal, a `wait4` or a run queue still meant the old one
    super::free_pid(pid);
    crate::trace_println!("[sched] reaped pid={}", pid);
}

// queries

/// Ends a process because a signal said to, recording which one
///
/// The pair of `kill_with_code`: its code is the shell convention `128 + sig`,
/// and `exit(148)` produces the same number, so the signal is kept beside the
/// code
pub fn kill_by_signal(pid: u64, sig: u32) {
    // The whole process: a SIGKILL leaving siblings running would leave a
    // program that cannot be killed
    let tgid = crate::sched::tgid_of(pid);
    {
        let table = PROC_TABLE.lock();
        if let Some(p) = table.get(&tgid) {
            p.killed_by_sig.store(sig, Ordering::Relaxed);
        }
    }
    kill_group_with_code(tgid, 128 + sig as u64);
}

/// Which signal ended this process, or `None` if it exited by itself
///
/// Asked between `find_zombie_child` and `reap_zombie`, while the record is
/// there
pub fn exit_signal_of(pid: u64) -> Option<u32> {
    let table = PROC_TABLE.lock();
    let sig = table.get(&pid)?.killed_by_sig.load(Ordering::Relaxed);
    if sig == 0 { None } else { Some(sig) }
}

/// A child that has stopped and has not been reported yet
///
/// Separate from `find_zombie_child`, which marks the child collected so its
/// caller reaps it: a stopped process is still running.
///
/// The flag is cleared by `cont`'s caller, so a job stopped, continued and
/// stopped again is reported both times
pub fn find_stopped_child(parent_pid: u64, target_pid: u64) -> Option<(u64, u32)> {
    let table = PROC_TABLE.lock();
    for (&pid, p) in table.iter() {
        if p.ppid.load(Ordering::Relaxed) != parent_pid { continue; }
        if target_pid != u64::MAX && pid != target_pid { continue; }
        if p.state.load(Ordering::Relaxed) == crate::process::STATE_STOPPED
            && !p.stop_reported.load(Ordering::Relaxed)
        {
            p.stop_reported.store(true, Ordering::Relaxed);
            return Some((pid, p.stop_sig.load(Ordering::Relaxed)));
        }
    }
    None
}

/// Forget that a stop was reported, so the next one is reported too
pub fn clear_stop_report(pid: u64) {
    let table = PROC_TABLE.lock();
    if let Some(p) = table.get(&pid) {
        p.stop_reported.store(false, Ordering::Relaxed);
    }
}

pub fn find_zombie_child(parent_pid: u64, target_pid: u64) -> Option<(u64, u64)> {
    let table = PROC_TABLE.lock();
    for (&pid, p) in table.iter() {
        if p.ppid.load(Ordering::Relaxed) != parent_pid { continue; }
        if target_pid != u64::MAX && pid != target_pid { continue; }
        if p.state.load(Ordering::Relaxed) == STATE_DEAD
            && !p.collected.load(Ordering::Relaxed)
        {
            let code = p.exit_code.load(Ordering::Relaxed);
            p.collected.store(true, Ordering::Relaxed);
            return Some((pid, code));
        }
    }
    None
}

/// The first zombie child of `parent_pid` that is not in `exclude`
///
/// For a reaper that must leave some of its children alone: mikuD is PID 1 and
/// so the parent of every orphan, but its own services' exit codes belong to
/// the code deciding whether to restart them. The exclusion happens before the
/// mark: marking one collected and putting it back would race with that code
pub fn find_zombie_child_excluding(parent_pid: u64, exclude: &[u64]) -> Option<(u64, u64)> {
    let table = PROC_TABLE.lock();
    for (&pid, p) in table.iter() {
        if p.ppid.load(Ordering::Relaxed) != parent_pid { continue; }
        if exclude.contains(&pid) { continue; }
        if p.state.load(Ordering::Relaxed) == STATE_DEAD
            && !p.collected.load(Ordering::Relaxed)
        {
            let code = p.exit_code.load(Ordering::Relaxed);
            p.collected.store(true, Ordering::Relaxed);
            return Some((pid, code));
        }
    }
    None
}

pub fn has_children(parent_pid: u64) -> bool {
    let table = PROC_TABLE.lock();
    table.iter().any(|(_, p)| p.ppid.load(Ordering::Relaxed) == parent_pid)
}

/// Whether a process is still running, as against gone or waiting to be reaped
///
/// `process_exists` is the wrong question about a resource held on this
/// process's behalf: a killed process keeps its slot in the index until its
/// parent reaps it, and the compositor's keyboard lock is held until
/// `disconnect`, at reaping. Dead but still indexed is gone
pub fn process_alive(pid: u64) -> bool {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return false; }
        unsafe { &*ptr }.state.load(Ordering::Relaxed) != STATE_DEAD
    })
}

pub fn process_exists(pid: u64) -> bool {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        !ptr.is_null()
    })
}

pub fn get_ppid(pid: u64) -> u64 {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return 0; }
        unsafe { &*ptr }.ppid.load(Ordering::Relaxed)
    })
}

// attribute mutators

pub fn set_parent(pid: u64, new_parent: u64) {
    let table = PROC_TABLE.lock();
    if let Some(p) = table.get(&pid) {
        p.ppid.store(new_parent, Ordering::Relaxed);
    }
}

pub fn set_affinity(pid: u64, mask: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { (*ptr).cpu_mask = if mask == 0 { CPU_ALL } else { mask } };
        crate::serial_println!("[sched] pid={} affinity={:#018x}", pid, mask);
    });
}

pub fn set_priority(pid: u64, priority: u8) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { &*ptr }.priority.store(priority.clamp(1, 20), Ordering::Relaxed);
        crate::serial_println!("[sched] pid={} priority={}", pid, priority);
    });
}

pub fn update_process_cr3(pid: u64, new_cr3: u64) {
    interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return; }
        unsafe { (*ptr).cr3 = new_cr3; }
    });
}
