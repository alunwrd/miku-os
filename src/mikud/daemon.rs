// mikuD daemon - main loop, service lifecycle, reconciliation
//
// Handles watchdog, oneshot, notify, graceful shutdown,
// conflict resolution, burst protection, and condition checks

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use spin::Mutex;

use super::target::Target;
use super::service::*;
use super::journal;

pub const MAX_DEP_DEPTH: usize = 8;
const BOOT_WAIT_TICKS: u64 = 5;
const SHUTDOWN_TIMEOUT_TICKS: u64 = 7500; // 30 sec at 250Hz

pub static SERVICES: Mutex<ServiceTable> = Mutex::new(ServiceTable::new());
static MIKUD_RUNNING: AtomicBool = AtomicBool::new(false);
static MIKUD_PID: AtomicU64 = AtomicU64::new(0);
static ACTIVE_TARGET: AtomicU8 = AtomicU8::new(Target::SysInit as u8);
static DEFAULT_TARGET: AtomicU8 = AtomicU8::new(Target::MultiUser as u8);
static BOOT_PROMOTED: AtomicBool = AtomicBool::new(false);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);
static ISOLATING: AtomicBool = AtomicBool::new(false);
// 0 = none, 1 = poweroff requested, 2 = reboot requested
static SHUTDOWN_REQUEST: AtomicU8 = AtomicU8::new(0);

// daemon state accessors

pub fn is_running() -> bool {
    MIKUD_RUNNING.load(Ordering::Relaxed)
}

pub fn activate() {
    MIKUD_RUNNING.store(true, Ordering::Relaxed);
}

pub fn deactivate() {
    MIKUD_RUNNING.store(false, Ordering::Relaxed);
}

pub fn mikud_pid() -> u64 {
    MIKUD_PID.load(Ordering::Relaxed)
}

pub fn set_mikud_pid(pid: u64) {
    MIKUD_PID.store(pid, Ordering::Relaxed);
}

pub fn initialized() -> bool {
    mikud_pid() != 0
}

pub fn is_shutting_down() -> bool {
    SHUTTING_DOWN.load(Ordering::Relaxed)
}

// target management

pub fn current_target() -> Target {
    match ACTIVE_TARGET.load(Ordering::Relaxed) {
        0 => Target::SysInit,
        1 => Target::MultiUser,
        2 => Target::Graphical,
        3 => Target::Rescue,
        _ => Target::MultiUser,
    }
}

pub fn target_name() -> &'static str {
    current_target().as_str()
}

pub fn set_target(target: Target) {
    let old = current_target();
    ACTIVE_TARGET.store(target as u8, Ordering::Relaxed);
    if old != target {
        journal::log(journal::Event::TargetChanged, target.as_str(), 0, old as u64);
    }
    crate::serial_println!("[mikud] target -> {}", target.as_str());
}

pub fn set_target_name(name: &str) -> bool {
    match Target::from_str(name) {
        Some(target) => {
            set_target(target);
            true
        }
        None => false,
    }
}

pub fn default_target() -> Target {
    match DEFAULT_TARGET.load(Ordering::Relaxed) {
        0 => Target::SysInit,
        1 => Target::MultiUser,
        2 => Target::Graphical,
        3 => Target::Rescue,
        _ => Target::MultiUser,
    }
}

pub fn set_default_target(target: Target) {
    DEFAULT_TARGET.store(target as u8, Ordering::Relaxed);
    crate::serial_println!("[mikud] default target -> {}", target.as_str());
}

pub fn target_is(target: Target) -> bool {
    current_target() == target
}

pub fn target_at_least(target: Target) -> bool {
    current_target() >= target
}

pub fn promote_target(target: Target) {
    set_target(target);
    BOOT_PROMOTED.store(true, Ordering::Relaxed);
}

pub fn enable_boot_promotion() {
    BOOT_PROMOTED.store(false, Ordering::Relaxed);
}

pub fn disable_boot_promotion() {
    BOOT_PROMOTED.store(true, Ordering::Relaxed);
}

pub fn boot_promoted() -> bool {
    BOOT_PROMOTED.load(Ordering::Relaxed)
}

pub fn boot_phase() -> &'static str {
    if crate::boot::is_done() {
        target_name()
    } else {
        "booting"
    }
}

pub fn boot_state() -> (&'static str, bool) {
    (boot_phase(), crate::boot::is_done())
}

// isolate

pub fn isolate_target(target: Target) {
    ISOLATING.store(true, Ordering::Relaxed);
    set_target(target);
    journal::log(journal::Event::TargetChanged, target.as_str(), 0, 0);

    // stop all services not in the new target before clearing ISOLATING
    let now = crate::interrupts::get_tick();
    reconcile_target(now);

    ISOLATING.store(false, Ordering::Relaxed);
}

// graceful shutdown

pub fn initiate_shutdown() {
    if SHUTTING_DOWN.swap(true, Ordering::Relaxed) {
        return; // already shutting down
    }
    let start = crate::interrupts::get_tick();
    crate::serial_println!("[mikud] initiating graceful shutdown");
    journal::log(journal::Event::Shutdown, "mikud", mikud_pid(), 0);

    // Stop all services in reverse priority order (non-critical first)
    let mut stop_order: Vec<(&'static str, bool)> = Vec::new();
    {
        let table = SERVICES.lock();
        let sorted = table.sorted_by_target();
        for &idx in sorted.iter().rev() {
            let svc = &table.services[idx];
            if svc.state.is_active() {
                stop_order.push((svc.name, svc.flags.critical));
            }
        }
    }

    // Stop non-critical first, then critical
    for &(name, critical) in &stop_order {
        if !critical {
            if crate::interrupts::get_tick().saturating_sub(start) > SHUTDOWN_TIMEOUT_TICKS {
                crate::serial_println!("[mikud] shutdown timeout, force-killing remaining services");
                force_kill_all_services();
                break;
            }
            let _ = stop_service_locked_wrapper(name);
        }
    }
    for &(name, critical) in &stop_order {
        if critical {
            if crate::interrupts::get_tick().saturating_sub(start) > SHUTDOWN_TIMEOUT_TICKS {
                crate::serial_println!("[mikud] shutdown timeout, force-killing remaining services");
                force_kill_all_services();
                break;
            }
            let _ = stop_service_locked_wrapper(name);
        }
    }

    crate::serial_println!("[mikud] all services stopped");
    journal::log(journal::Event::Shutdown, "mikud", mikud_pid(), 1);
}

fn force_kill_all_services() {
    with_service_table(|table| {
        for svc in table.services.iter_mut() {
            if svc.active && svc.pid != 0 && svc.state.is_active() {
                crate::scheduler::kill(svc.pid);
                if let Some((child_pid, code)) = crate::scheduler::find_zombie_child(mikud_pid(), svc.pid) {
                    crate::scheduler::reap_zombie(child_pid);
                    svc.last_exit_code = code;
                }
                svc.pid = 0;
                svc.state = ServiceState::Stopped;
                svc.flags.was_signal_kill = true;
            }
        }
    });
}

fn stop_service_locked_wrapper(name: &str) -> bool {
    with_service_table(|table| stop_service_locked(table, name, 0))
}

/// Request asynchronous poweroff - daemon loop will handle it
pub fn poweroff() {
    SHUTDOWN_REQUEST.store(1, Ordering::Relaxed);
}

/// Request asynchronous reboot - daemon loop will handle it
pub fn reboot() {
    SHUTDOWN_REQUEST.store(2, Ordering::Relaxed);
}

fn pending_shutdown() -> u8 {
    SHUTDOWN_REQUEST.load(Ordering::Relaxed)
}

// helpers

pub fn with_service_table<F, R>(f: F) -> R
where
    F: FnOnce(&mut ServiceTable) -> R,
{
    let mut table = SERVICES.lock();
    f(&mut table)
}

fn ensure_boot_target() {
    if crate::boot::is_done() && !BOOT_PROMOTED.swap(true, Ordering::Relaxed) {
        set_target(default_target());
    }
}

// service lifecycle

pub fn start_service_locked(table: &mut ServiceTable, name: &str, now: u64, depth: usize) -> bool {
    if depth >= MAX_DEP_DEPTH {
        crate::serial_println!("[mikud] dependency depth exceeded for '{}'", name);
        return false;
    }

    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };

    // Masked check
    if table.services[idx].flags.masked {
        crate::serial_println!("[mikud] '{}' is masked, refusing start", name);
        return false;
    }

    // Oneshot that already completed with remain_after_exit
    if table.services[idx].svc_type == ServiceType::Oneshot
        && table.services[idx].flags.oneshot_done
        && table.services[idx].flags.remain_after_exit
    {
        return true;
    }

    let target = current_target();
    {
        let svc = &table.services[idx];
        if !svc.active || svc.target > target || now < svc.next_restart_tick {
            return false;
        }
        if svc.state == ServiceState::Running || svc.state == ServiceState::Activating {
            return true;
        }
        if svc.state == ServiceState::Starting {
            return false;
        }
        if svc.state == ServiceState::Dead {
            return false;
        }
    }

    // Condition checks
    if !table.services[idx].check_conditions() {
        crate::serial_println!("[mikud] '{}' conditions not met, skipping", name);
        table.services[idx].state = ServiceState::Stopped;
        return false;
    }

    // Burst protection
    if table.services[idx].in_restart_burst(now) {
        crate::serial_println!("[mikud] '{}' hit restart limit, entering failed state", name);
        table.services[idx].state = ServiceState::Failed;
        journal::log(journal::Event::BurstLimit, table.services[idx].name, 0, table.services[idx].restarts as u64);
        return false;
    }

    table.services[idx].state = ServiceState::Starting;

    // Resolve conflicts - stop conflicting services
    let conflicts = table.services[idx].conflicts;
    for conflict_name in conflicts.iter() {
        if let Some(cidx) = table.find_by_name(conflict_name) {
            if table.services[cidx].state.is_active() {
                crate::serial_println!("[mikud] stopping conflict '{}' for '{}'", conflict_name, name);
                stop_service_locked(table, conflict_name, depth + 1);
            }
        }
    }

    // Start hard dependencies (Requires + After)
    let deps = table.services[idx].deps;
    for dep in deps.iter() {
        if !start_service_locked(table, dep, now, depth + 1) {
            table.services[idx].state = ServiceState::Failed;
            table.services[idx].next_restart_tick = now + table.services[idx].restart_delay_ticks;
            journal::log(journal::Event::DepFailed, table.services[idx].name, 0, 0);
            return false;
        }
    }

    // Start soft dependencies (Wants) - don't fail if they fail
    let wants = table.services[idx].wants;
    for want in wants.iter() {
        let _ = start_service_locked(table, want, now, depth + 1);
    }

    // Call on_restart callback if this is a restart (restarts > 0)
    if table.services[idx].restarts > 0 {
        if let Some(callback) = table.services[idx].on_restart {
            callback();
        }
    }

    let name_ref = table.services[idx].name;
    let priority = table.services[idx].priority;

    // Determine how to spawn: kernel fn() entry or ELF binary from disk
    let pid = if let Some(entry) = table.services[idx].entry {
        crate::scheduler::spawn_named_child_of(mikud_pid(), entry, name_ref, priority)
    } else if let Some(exec_path) = table.services[idx].exec_start_path {
        match crate::exec_elf::exec(exec_path, &[]) {
            Ok(pid) => {
                // Re-parent the process under mikuD
                crate::scheduler::set_parent(pid, mikud_pid());
                pid
            }
            Err(e) => {
                crate::serial_println!("[mikud] exec '{}' failed: {}", exec_path, e.as_str());
                table.services[idx].state = ServiceState::Failed;
                journal::log(journal::Event::ExecFailed, name_ref, 0, 0);
                return false;
            }
        }
    } else {
        // No entry point and no exec path
        table.services[idx].state = ServiceState::Failed;
        return false;
    };

    table.services[idx].pid = pid;
    table.services[idx].last_start_tick = now;
    table.services[idx].next_restart_tick = 0;
    table.services[idx].watchdog_last_ping = now;
    table.services[idx].flags.was_signal_kill = false;

    // State depends on service type
    match table.services[idx].svc_type {
        ServiceType::Simple | ServiceType::Forking => {
            table.services[idx].state = ServiceState::Running;
        }
        ServiceType::Oneshot => {
            table.services[idx].state = ServiceState::Activating;
        }
        ServiceType::Notify => {
            // Stay in Starting until notify_ready
            table.services[idx].state = ServiceState::Starting;
            table.services[idx].flags.notify_ready = false;
        }
    }

    journal::log(journal::Event::Started, name_ref, pid, 0);
    crate::serial_println!("[mikud] started '{}' pid={} target={} type={}",
        name_ref, pid, table.services[idx].target.as_str(), table.services[idx].svc_type.as_str());
    true
}

pub fn stop_service_locked(table: &mut ServiceTable, name: &str, depth: usize) -> bool {
    if depth >= MAX_DEP_DEPTH {
        crate::serial_println!("[mikud] dependency depth exceeded while stopping '{}'", name);
        return false;
    }

    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };

    // Critical service protection (unless shutting down)
    if table.services[idx].flags.critical && !is_shutting_down() {
        crate::serial_println!("[mikud] '{}' is critical, refusing stop (use --force or shutdown)", name);
        return false;
    }

    // Stop dependents first
    let dependents = table.dependents_of(name);
    for dep in dependents {
        if dep != name {
            stop_service_locked(table, dep, depth + 1);
        }
    }

    let pid = table.services[idx].pid;
    table.services[idx].state = ServiceState::Stopping;

    if pid != 0 {
        // Send SIGTERM first for graceful stop
        crate::signal::send_signal(pid, crate::signal::SIGTERM);

        // Wait briefly for graceful exit
        let timeout = table.services[idx].timeout_stop_ticks.min(250); // max 1 sec wait in stop
        let start = crate::interrupts::get_tick();
        loop {
            if let Some((child_pid, code)) = crate::scheduler::find_zombie_child(mikud_pid(), pid) {
                crate::scheduler::reap_zombie(child_pid);
                table.services[idx].last_exit_code = code;
                break;
            }
            if crate::interrupts::get_tick() - start > timeout {
                // Force kill after timeout
                crate::scheduler::kill(pid);
                if let Some((child_pid, code)) = crate::scheduler::find_zombie_child(mikud_pid(), pid) {
                    crate::scheduler::reap_zombie(child_pid);
                    table.services[idx].last_exit_code = code;
                }
                table.services[idx].flags.was_signal_kill = true;
                break;
            }
            // yield briefly
            crate::scheduler::yield_now();
        }
    }

    table.services[idx].pid = 0;
    table.services[idx].state = ServiceState::Stopped;
    table.services[idx].last_stop_tick = crate::interrupts::get_tick();
    table.services[idx].next_restart_tick = 0;
    table.services[idx].flags.notify_ready = false;
    journal::log(journal::Event::Stopped, table.services[idx].name, pid, 0);
    crate::serial_println!("[mikud] stopped '{}'", table.services[idx].name);
    true
}

/// Force-stop even critical services
pub fn force_stop_service_locked(table: &mut ServiceTable, name: &str) -> bool {
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };
    let was_critical = table.services[idx].flags.critical;
    table.services[idx].flags.critical = false;
    let result = stop_service_locked(table, name, 0);
    if let Some(idx) = table.find_by_name(name) {
        table.services[idx].flags.critical = was_critical;
    }
    result
}

/// Reload service (SIGHUP)
pub fn reload_service_locked(table: &mut ServiceTable, name: &str) -> bool {
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };

    if table.services[idx].state != ServiceState::Running {
        return false;
    }

    let pid = table.services[idx].pid;
    if pid == 0 {
        return false;
    }

    table.services[idx].state = ServiceState::Reloading;
    crate::signal::send_signal(pid, 1); // SIGHUP = 1
    table.services[idx].state = ServiceState::Running;
    journal::log(journal::Event::Reloaded, table.services[idx].name, pid, 0);
    crate::serial_println!("[mikud] reloaded '{}'", name);
    true
}

// dead service observation

fn observe_dead_services(now: u64) -> Vec<&'static str> {
    let mut restart_list = Vec::new();

    with_service_table(|table| {
        for svc in table.services.iter_mut() {
            if !svc.active || svc.pid == 0 {
                continue;
            }

            // Only check running/activating/starting services
            if !matches!(svc.state, ServiceState::Running | ServiceState::Activating | ServiceState::Starting) {
                continue;
            }

            if let Some((child_pid, exit_code)) = crate::scheduler::find_zombie_child(mikud_pid(), svc.pid) {
                crate::scheduler::reap_zombie(child_pid);
                crate::serial_println!("[mikud] '{}' pid={} exited code={}", svc.name, child_pid, exit_code);
                svc.last_exit_code = exit_code;
                svc.pid = 0;

                // Oneshot handling
                if svc.svc_type == ServiceType::Oneshot {
                    if exit_code == 0 {
                        svc.flags.oneshot_done = true;
                        if svc.flags.remain_after_exit {
                            svc.state = ServiceState::Running; // "active (exited)"
                        } else {
                            svc.state = ServiceState::Stopped;
                        }
                        journal::log(journal::Event::Exited, svc.name, child_pid, exit_code);
                        continue;
                    }
                    // oneshot failed
                }

                let should_restart = svc.restart.should_restart(exit_code, svc.flags.was_signal_kill);

                // Update burst tracking
                if now - svc.burst_window_start >= RESTART_BURST_WINDOW {
                    svc.burst_count = 0;
                    svc.burst_window_start = now;
                }
                svc.burst_count = svc.burst_count.saturating_add(1);
                svc.restarts = svc.restarts.saturating_add(1);

                journal::log(journal::Event::Exited, svc.name, child_pid, exit_code);

                if should_restart && !svc.in_restart_burst(now) {
                    svc.state = ServiceState::Failed;
                    svc.next_restart_tick = now + svc.restart_delay_ticks;
                    restart_list.push(svc.name);
                } else if svc.in_restart_burst(now) {
                    svc.state = ServiceState::Failed;
                    svc.next_restart_tick = 0; // don't auto-restart
                    journal::log(journal::Event::BurstLimit, svc.name, 0, svc.restarts as u64);
                    crate::serial_println!("[mikud] '{}' hit restart burst limit", svc.name);
                } else {
                    svc.state = ServiceState::Stopped;
                    svc.next_restart_tick = 0;
                }
            }
        }
    });

    restart_list
}

// watchdog

fn check_watchdogs(now: u64) {
    let mut expired: Vec<&'static str> = Vec::new();

    {
        let table = SERVICES.lock();
        for svc in table.services.iter() {
            if svc.active && svc.watchdog_expired(now) {
                expired.push(svc.name);
            }
        }
    }

    for name in expired {
        crate::serial_println!("[mikud] watchdog timeout for '{}'", name);
        journal::log(journal::Event::WatchdogTimeout, name, 0, 0);
        // Force restart
        with_service_table(|table| {
            if let Some(idx) = table.find_by_name(name) {
                let pid = table.services[idx].pid;
                if pid != 0 {
                    crate::scheduler::kill(pid);
                    if let Some((child_pid, code)) = crate::scheduler::find_zombie_child(mikud_pid(), pid) {
                        crate::scheduler::reap_zombie(child_pid);
                        table.services[idx].last_exit_code = code;
                    }
                }
                table.services[idx].pid = 0;
                table.services[idx].state = ServiceState::Failed;
                table.services[idx].next_restart_tick = now + table.services[idx].restart_delay_ticks;
                table.services[idx].flags.was_signal_kill = true;
                table.services[idx].restarts = table.services[idx].restarts.saturating_add(1);
            }
        });
    }
}

// notify handling

fn check_notify_services(now: u64) {
    with_service_table(|table| {
        for svc in table.services.iter_mut() {
            if !svc.active || svc.svc_type != ServiceType::Notify {
                continue;
            }
            if svc.state == ServiceState::Starting && svc.flags.notify_ready {
                svc.state = ServiceState::Running;
                crate::serial_println!("[mikud] '{}' reported ready", svc.name);
                journal::log(journal::Event::Ready, svc.name, svc.pid, 0);
            }
            // Timeout check for notify services stuck in Starting
            if svc.state == ServiceState::Starting
                && now - svc.last_start_tick > svc.timeout_start_ticks
            {
                crate::serial_println!("[mikud] '{}' start timeout exceeded", svc.name);
                journal::log(journal::Event::Timeout, svc.name, svc.pid, 0);
                let pid = svc.pid;
                if pid != 0 {
                    crate::scheduler::kill(pid);
                    if let Some((child_pid, code)) = crate::scheduler::find_zombie_child(mikud_pid(), pid) {
                        crate::scheduler::reap_zombie(child_pid);
                        svc.last_exit_code = code;
                    }
                }
                svc.pid = 0;
                svc.state = ServiceState::Failed;
                svc.next_restart_tick = now + svc.restart_delay_ticks;
            }
        }
    });
}

// reconciliation

fn reconcile_target(now: u64) {
    if is_shutting_down() {
        return;
    }

    let target = current_target();
    let mut to_start = Vec::new();
    let mut to_stop = Vec::new();

    with_service_table(|table| {
        for svc in table.services.iter() {
            if !svc.active || svc.flags.masked {
                continue;
            }

            if svc.target <= target {
                match svc.state {
                    ServiceState::Stopped | ServiceState::Failed => {
                        if (now >= svc.next_restart_tick && svc.next_restart_tick != 0) || svc.state == ServiceState::Stopped {
                            to_start.push(svc.name);
                        }
                    }
                    _ => {}
                }
            } else if svc.state.is_active() {
                to_stop.push(svc.name);
            }
        }
    });

    for name in to_stop {
        let _ = super::api::stop_service(name);
    }

    let mut restart_list = observe_dead_services(now);
    to_start.append(&mut restart_list);

    // Dedup
    to_start.sort();
    to_start.dedup();

    with_service_table(|table| {
        for name in to_start {
            let _ = start_service_locked(table, name, now, 0);
        }
    });
}

// main loop

pub fn mikud_main() -> ! {
    let pid = crate::scheduler::current_pid();
    set_mikud_pid(pid);
    activate();
    journal::log(journal::Event::Started, "mikud", pid, 0);
    crate::serial_println!("[mikud] init daemon started pid={}", pid);

    x86_64::instructions::interrupts::enable();

    loop {
        // Check for async shutdown/reboot request
        let req = pending_shutdown();
        if req != 0 {
            crate::serial_println!("[mikud] processing shutdown request (mode={})", req);
            initiate_shutdown();
            if req == 2 {
                crate::serial_println!("[mikud] reboot");
                crate::power::reboot();
            } else {
                crate::serial_println!("[mikud] poweroff");
                crate::power::shutdown();
            }
        }

        ensure_boot_target();
        let now = crate::interrupts::get_tick();

        if !is_shutting_down() {
            reconcile_target(now);
            check_watchdogs(now);
            check_notify_services(now);
            super::timer::tick_timers(now);
        }

        crate::scheduler::sleep(BOOT_WAIT_TICKS);
    }
}
