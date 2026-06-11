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

use crate::percpu;
use crate::process::{Process, CPU_ALL, STATE_DEAD};

use super::control::wakeup;
use super::proc_index::{
    pid_running_anywhere, register_process, PROC_INDEX, PROC_TABLE,
};
use super::runqueue::{pick_cpu_for, push_to_cpu, remove_from_any_queue};

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
    crate::serial_println!("[sched] spawn pid={} name={} cpu={}", pid, name, target);

    // Nudge the target CPU so it picks the new task up sooner
    if target != percpu::current_index() {
        crate::apic::send_ipi(
            cpu.lapic_id.load(Ordering::Relaxed),
            crate::apic::VEC_IPI_RESCHED,
        );
    }
}

pub fn spawn(entry: fn() -> !) -> u64 {
    spawn_named(entry, "kthread", 10)
}

pub fn spawn_named(entry: fn() -> !, name: &'static str, priority: u8) -> u64 {
    let p = Process::new_kernel_named(entry, name, priority);
    let pid = p.pid;
    reap_dead();
    add_process(p);
    pid
}

pub fn spawn_named_child_of(
    parent_pid: u64,
    entry: fn() -> !,
    name: &'static str,
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

pub fn spawn_named_child(entry: fn() -> !, name: &'static str, priority: u8) -> u64 {
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
    let tick = crate::interrupts::get_tick();
    let cr3  = crate::vmm::kernel_cr3();
    let raw  = register_process(Process::new_idle(cr3, tick));
    let bsp  = percpu::get(0);
    bsp.current_pid.store(0, Ordering::Relaxed);
    bsp.idle_pid   .store(0, Ordering::Relaxed);
    SCHED_STARTED.store(true, Ordering::Release);
    crate::serial_println!("[sched] bsp idle ptr={:p}", raw);
}

// Register a per-CPU idle thread for an AP. Called from ap_entry after percpu::install_gs_base and gdt::init_cpu
pub fn init_ap_idle(cpu_idx: usize) {
    let tick = crate::interrupts::get_tick();
    let cr3  = crate::vmm::kernel_cr3();
    let idle = Process::new_idle_ap(cr3, tick, "idle_ap");
    let pid  = idle.pid;
    let _raw = register_process(idle);
    let cpu  = percpu::get(cpu_idx);
    cpu.current_pid.store(pid, Ordering::Relaxed);
    cpu.idle_pid   .store(pid, Ordering::Relaxed);
    crate::serial_println!("[sched] cpu{} idle pid={}", cpu_idx, pid);
}

// kill / reap

pub fn kill(pid: u64) { kill_with_code(pid, 0); }

pub fn kill_with_code(pid: u64, code: u64) {
    let ppid = interrupts::without_interrupts(|| {
        let ptr = unsafe { PROC_INDEX.get_raw(pid) };
        if ptr.is_null() { return 0u64; }
        let p = unsafe { &*ptr };
        p.exit_code.store(code, Ordering::Relaxed);
        let prev_state = p.state.swap(STATE_DEAD, Ordering::Relaxed);
        if prev_state == crate::process::STATE_SLEEPING {
            super::core_sched::sleeper_dec_saturating();
        }
        remove_from_any_queue(pid);
        crate::serial_println!("[sched] kill pid={} code={}", pid, code);
        p.ppid.load(Ordering::Relaxed)
    });

    // Reparent any children of the dying process to init (PID 1) so
    // they can still be reaped via wait4 once they exit. Without this
    // step, an orphaned child whose parent died is held in PROC_TABLE
    // forever: its ppid points at a stale slot that wait4 can never
    // walk to. PID 1 is mikuD, which loops waitpid as part of its
    // reaper duty. If for some reason pid 1 doesn't exist (early boot)
    // we leave ppid as-is; reap_dead will still free dead processes
    // whose state is DEAD; only the zombie/wait4 path needs a live parent
    let init_alive = process_exists(1);
    if init_alive && pid != 1 {
        let table = PROC_TABLE.lock();
        let orphans: Vec<u64> = table.iter()
            .filter(|(_, c)| c.ppid.load(Ordering::Relaxed) == pid)
            .map(|(&cpid, _)| cpid)
            .collect();
        drop(table);
        for orphan in orphans {
            set_parent(orphan, 1);
            crate::serial_println!("[sched] reparent pid={} -> init (1)", orphan);
        }
        // Already-zombie orphans need init to be told; the journal SIGCHLD
        // delivery is best-effort
        let _ = init_alive;
    }

    if ppid != 0 { wakeup(ppid); }
}

pub fn reap_dead() {
    let dead_pids: Vec<u64> = {
        let table = PROC_TABLE.lock();
        table.iter()
            .filter(|(_, p)| {
                p.state.load(Ordering::Relaxed) == STATE_DEAD
                    && !pid_running_anywhere(p.pid)
                    && !p.on_rq.load(Ordering::Relaxed)
            })
            .map(|(&pid, _)| pid)
            .collect()
    };

    for pid in dead_pids {
        PROC_INDEX.clear(pid);
        core::sync::atomic::compiler_fence(Ordering::SeqCst);

        let mut table = PROC_TABLE.lock();
        let collectable = table.get(&pid).map_or(false, |p| {
            p.state.load(Ordering::Relaxed) == STATE_DEAD
                && !pid_running_anywhere(p.pid)
                && !p.on_rq.load(Ordering::Relaxed)
        });
        if !collectable { continue; }

        if let Some(p) = table.remove(&pid) {
            drop(table);
            free_process_resources(p, pid);
        }
    }
}

pub fn reap_zombie(pid: u64) {
    PROC_INDEX.clear(pid);
    core::sync::atomic::compiler_fence(Ordering::SeqCst);

    let mut table = PROC_TABLE.lock();
    if let Some(p) = table.remove(&pid) {
        drop(table);
        free_process_resources(p, pid);
    }
}

fn free_process_resources(mut p: Box<Process>, pid: u64) {
    // Release any network sockets the process still owns. Done before the
    // VFS fd table teardown; socket fds live in a separate range and are not
    // tracked by the VFS, so they need their own cleanup pass
    crate::net::socket::close_all_for_pid(pid);

    // Drop the process's per-process FD table and release the vnode
    // references it was keeping alive
    let victims = crate::vfs::core::with_vfs(|vfs| vfs.drop_fds(pid));
    if !victims.is_empty() {
        crate::vfs::core::with_vfs(|vfs| {
            for vid in victims {
                let idx = vid as usize;
                if vfs.valid_vnode(idx) {
                    vfs.nodes[idx].dec_ref();
                }
            }
        });
    }

    crate::mmap::vma_cleanup(p.cr3);
    if let Some(phys) = p.user_stack_phys.take() {
        crate::pmm::free_frames(phys, crate::process::USER_STACK_PAGES);
    }
    if p.cr3 != 0 && p.cr3 != crate::vmm::kernel_cr3() {
        let mut aspace = crate::vmm::AddressSpace { cr3: p.cr3 };
        aspace.free_address_space();
    }
    crate::serial_println!("[sched] reaped pid={}", pid);
}

// queries

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

pub fn has_children(parent_pid: u64) -> bool {
    let table = PROC_TABLE.lock();
    table.iter().any(|(_, p)| p.ppid.load(Ordering::Relaxed) == parent_pid)
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
