// mikuD daemon: main loop, service lifecycle, reconciliation
//
// Handles watchdog, oneshot, notify, graceful shutdown, conflict resolution,
// burst protection, and condition checks

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

    // Leaving the graphical target puts the screen back on the console.
    // `reconcile_target` stops the compositor service, a thread that watches;
    // the frame loop it started is a different thread, and the console it
    // blanked stays blanked otherwise. The fallback out of a failing greeter
    // needs this
    if old == Target::Graphical && target < Target::Graphical
        && crate::mayland::render::running()
    {
        crate::serial_println!("[mikud] leaving the graphical target - stopping the compositor");
        crate::mayland::render::stop();
    }
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
    if crate::kcore::boot_state::is_done() {
        target_name()
    } else {
        "booting"
    }
}

pub fn boot_state() -> (&'static str, bool) {
    (boot_phase(), crate::kcore::boot_state::is_done())
}

// isolate

pub fn isolate_target(target: Target) {
    ISOLATING.store(true, Ordering::Relaxed);
    set_target(target);
    journal::log(journal::Event::TargetChanged, target.as_str(), 0, 0);

    // stop all services not in the new target before clearing ISOLATING
    let now = crate::arch::x86_64::interrupts::get_tick();
    reconcile_target(now);

    ISOLATING.store(false, Ordering::Relaxed);
}

// graceful shutdown

pub fn initiate_shutdown() {
    if SHUTTING_DOWN.swap(true, Ordering::Relaxed) {
        return; // already shutting down
    }
    let start = crate::arch::x86_64::interrupts::get_tick();
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
            if crate::arch::x86_64::interrupts::get_tick().saturating_sub(start) > SHUTDOWN_TIMEOUT_TICKS {
                crate::serial_println!("[mikud] shutdown timeout, force-killing remaining services");
                force_kill_all_services();
                break;
            }
            let _ = stop_service_locked_wrapper(name);
        }
    }
    for &(name, critical) in &stop_order {
        if critical {
            if crate::arch::x86_64::interrupts::get_tick().saturating_sub(start) > SHUTDOWN_TIMEOUT_TICKS {
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
                crate::sched::kill(svc.pid);
                if let Some((child_pid, code)) = crate::sched::find_zombie_child(mikud_pid(), svc.pid) {
                    crate::sched::reap_zombie(child_pid);
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

/// Requests asynchronous poweroff: the daemon loop handles it
pub fn poweroff() {
    SHUTDOWN_REQUEST.store(1, Ordering::Relaxed);
}

/// Requests asynchronous reboot: the daemon loop handles it
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
    if crate::kcore::boot_state::is_done() && !BOOT_PROMOTED.swap(true, Ordering::Relaxed) {
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
        // Traced rather than printed: a condition is usually false for a while
        // (the greeter's for as long as somebody is logged in), and reconcile
        // comes round every tick, so full volume would be one serial write per
        // tick per waiting service
        crate::trace_println!("[mikud] '{}' conditions not met, skipping", name);
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

    // Stop conflicting services
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

    // Start soft dependencies (Wants): they may fail
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

    // How to spawn: kernel fn() entry or ELF binary from disk
    let pid = if let Some(entry) = table.services[idx].entry {
        crate::sched::spawn_named_child_of(mikud_pid(), entry, name_ref, priority)
    } else if let Some(exec_path) = table.services[idx].exec_start_path {
        match crate::process::exec::exec(exec_path, &[], crate::process::exec::DEFAULT_ENV) {
            Ok(pid) => {
                // Re-parent the process under mikuD
                crate::sched::set_parent(pid, mikud_pid());
                // And give it the identity and authority its unit asks for,
                // usually less than root. A service left as root because that
                // is what exec() produces would have every capability for no
                // reason
                apply_unit_identity(
                    pid,
                    table.services[idx].user,
                    table.services[idx].caps,
                    name_ref,
                );
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

    // A unit granted `CAP_MAY_PRIV` drives the screen before anybody has logged
    // in: the greeter, and nothing else in what ships. `session::open` takes
    // the capability off the seat's previous holder, so who holds it is
    // recorded here
    if table.services[idx].caps
        .map(|m| crate::process::cred::CapSet(m).has(crate::process::cred::CAP_MAY_PRIV))
        .unwrap_or(false)
    {
        crate::process::session::hold_seat(crate::process::session::SEAT0, pid);
    }

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
        crate::process::signal::send_signal(pid, crate::process::signal::SIGTERM);

        // Wait briefly for graceful exit
        let timeout = table.services[idx].timeout_stop_ticks.min(250); // max 1 sec wait in stop
        let start = crate::arch::x86_64::interrupts::get_tick();
        loop {
            if let Some((child_pid, code)) = crate::sched::find_zombie_child(mikud_pid(), pid) {
                crate::sched::reap_zombie(child_pid);
                table.services[idx].last_exit_code = code;
                break;
            }
            if crate::arch::x86_64::interrupts::get_tick() - start > timeout {
                // Force kill after timeout
                crate::sched::kill(pid);
                if let Some((child_pid, code)) = crate::sched::find_zombie_child(mikud_pid(), pid) {
                    crate::sched::reap_zombie(child_pid);
                    table.services[idx].last_exit_code = code;
                }
                table.services[idx].flags.was_signal_kill = true;
                break;
            }
            // yield briefly
            crate::sched::yield_now();
        }
    }

    table.services[idx].pid = 0;
    table.services[idx].state = ServiceState::Stopped;
    table.services[idx].last_stop_tick = crate::arch::x86_64::interrupts::get_tick();
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
    crate::process::signal::send_signal(pid, 1); // SIGHUP = 1
    table.services[idx].state = ServiceState::Running;
    journal::log(journal::Event::Reloaded, table.services[idx].name, pid, 0);
    crate::serial_println!("[mikud] reloaded '{}'", name);
    true
}

// dead service observation

fn observe_dead_services(now: u64) -> Vec<&'static str> {
    let mut restart_list = Vec::new();
    // Set inside the closure below, acted on outside it: changing the target
    // reconciles every service, which needs the service table's lock
    let mut fall_back = false;

    with_service_table(|table| {
        for svc in table.services.iter_mut() {
            if !svc.active || svc.pid == 0 {
                continue;
            }

            // Only check running/activating/starting services
            if !matches!(svc.state, ServiceState::Running | ServiceState::Activating | ServiceState::Starting) {
                continue;
            }

            // A stopped service is neither dead nor running:
            // `find_zombie_child` below answers `None` for it, so without this
            // `kill -STOP` would wedge it with no restart and no journal line.
            //
            // Supervision leaves a deliberately stopped service stopped, and
            // says so once
            if svc.pid != 0 && crate::sched::is_stopped(svc.pid) {
                if !svc.flags.noticed_stopped {
                    svc.flags.noticed_stopped = true;
                    crate::serial_println!(
                        "[mikud] '{}' pid={} has stopped - not restarting a service                          somebody suspended", svc.name, svc.pid);
                    journal::log(journal::Event::Stopped, svc.name, svc.pid, 0);
                }
                continue;
            }
            if svc.pid != 0 && svc.flags.noticed_stopped {
                // Continued: watched again, and the next stop is reported too
                svc.flags.noticed_stopped = false;
                crate::serial_println!("[mikud] '{}' pid={} is running again",
                                       svc.name, svc.pid);
            }

            if let Some((child_pid, exit_code)) = crate::sched::find_zombie_child(mikud_pid(), svc.pid) {
                crate::sched::reap_zombie(child_pid);
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

                // Update burst tracking.
                //
                // Only failures count: the limit stops a service crashing on
                // startup from being restarted for ever. A clean exit restarted
                // by policy is not that: the greeter exits with 0 every time
                // somebody logs in
                let crashed = exit_code != 0 || svc.flags.was_signal_kill;
                if now - svc.burst_window_start >= RESTART_BURST_WINDOW {
                    svc.burst_count = 0;
                    svc.burst_window_start = now;
                }
                if crashed {
                    svc.burst_count = svc.burst_count.saturating_add(1);
                }
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
                    // A service only the graphical target wants, which cannot
                    // be kept running: this machine is not a graphical machine.
                    // Without the fallback there is a black screen with a
                    // working console behind it
                    if svc.target == Target::Graphical && current_target() == Target::Graphical {
                        fall_back = true;
                    }
                } else {
                    svc.state = ServiceState::Stopped;
                    svc.next_restart_tick = 0;
                }
            }
        }
    });

    if fall_back {
        crate::serial_println!(
            "[mikud] the graphical target cannot be kept up - falling back to multi-user");
        journal::log(journal::Event::TargetChanged, Target::MultiUser.as_str(), 0, 0);
        set_target(Target::MultiUser);
        set_default_target(Target::MultiUser);
    }

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
                    crate::sched::kill(pid);
                    if let Some((child_pid, code)) = crate::sched::find_zombie_child(mikud_pid(), pid) {
                        crate::sched::reap_zombie(child_pid);
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
                    crate::sched::kill(pid);
                    if let Some((child_pid, code)) = crate::sched::find_zombie_child(mikud_pid(), pid) {
                        crate::sched::reap_zombie(child_pid);
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
    let pid = crate::sched::current_pid();
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
                crate::kcore::power::reboot();
            } else {
                crate::serial_println!("[mikud] poweroff");
                crate::kcore::power::shutdown();
            }
        }

        ensure_boot_target();
        let now = crate::arch::x86_64::interrupts::get_tick();

        if !is_shutting_down() {
            reconcile_target(now);
            check_watchdogs(now);
            check_notify_services(now);
            super::timer::tick_timers(now);
            reap_orphans();
        }

        crate::sched::sleep(BOOT_WAIT_TICKS);
    }
}


/// Collects the zombies nobody else will
///
/// When a process dies its children are reparented to PID 1, which is this.
/// Without this every orphan that exited would sit in PROC_TABLE for the rest
/// of the boot.
///
/// A session leader is orphaned on purpose: `mikudod` double-forks so the
/// desktop it starts does not have the machine's one authority as its parent.
///
/// Registered services are skipped and left to `reconcile_target`, which reads
/// the exit code to decide whether to restart them
fn reap_orphans() {
    // The services' own pids, gathered before anything is marked collected: see
    // find_zombie_child_excluding for why the order matters
    let mut mine = [0u64; super::service::MAX_SERVICES];
    let mut n = 0;
    {
        let table = SERVICES.lock();
        for svc in table.services.iter().filter(|s| s.active && s.pid != 0) {
            mine[n] = svc.pid;
            n += 1;
        }
    }
    // Bounded: this runs every loop, and a machine producing more than a few
    // zombies per tick will not be fixed by reaping faster
    for _ in 0..4 {
        let (pid, code) = match crate::sched::find_zombie_child_excluding(mikud_pid(), &mine[..n]) {
            Some(z) => z,
            None => return,
        };
        crate::sched::reap_zombie(pid);
        crate::serial_println!("[mikud] reaped orphan pid={} code={}", pid, code);
    }
}

/// Puts a freshly exec'd service into the identity its unit describes.
///
/// Done here, from PID 1, rather than by the service dropping privilege itself:
/// the process has run none of its own code yet (`exec` has built the image and
/// the scheduler has not picked it up), so it is never running as root.
///
/// The capability set is narrowed through the bounding set as well as the
/// permitted one, so nothing the service starts can regain what was taken.
///
/// Public because the console's `login` command starts the same program the
/// greeter is, for the same reason
pub fn apply_unit_identity(
    pid: u64,
    user: Option<(u16, u16)>,
    caps: Option<u64>,
    name: &'static str,
) {
    use crate::process::cred::CapSet;
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() {
        return;
    }
    let mut cred = unsafe { (*ptr).cred.write() };

    if let Some(mask) = caps {
        let want = CapSet(mask).sanitized();
        cred.permitted = cred.permitted.intersect(want);
        cred.effective = cred.effective.intersect(want);
        cred.bounding  = cred.bounding.intersect(want);
    }

    if let Some((uid, gid)) = user {
        cred.uid = uid;
        cred.euid = uid;
        cred.suid = uid;
        cred.gid = gid;
        cred.egid = gid;
        cred.sgid = gid;
        cred.groups = [0; crate::fs::vfs::types::MAX_GROUPS];
        cred.ngroups = 0;
        // A service told to run as a user keeps only what its unit named. If
        // the unit named nothing, it keeps nothing: "User=" without
        // "Capabilities=" means an ordinary unprivileged program
        if uid != 0 && caps.is_none() {
            cred.drop_all_caps();
            cred.bounding = crate::process::cred::CAP_NONE;
        }
    }

    // Said with the guard released. A serial line runs with interrupts off and
    // spins on the port for milliseconds, and anybody reading this process's
    // credential (the syscall path, on every call checking one) would wait
    // through it
    let said = (user.is_some() || caps.is_some()).then_some((cred.uid, cred.effective.0));
    drop(cred);
    if let Some((uid, caps)) = said {
        crate::serial_println!("[mikud] {} runs as uid {} with caps {:#x}", name, uid, caps);
    }
}
