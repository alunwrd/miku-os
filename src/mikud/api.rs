// mikuD public API - register, start, stop, restart, query, mask, reload

extern crate alloc;
use alloc::vec::Vec;

use super::target::Target;
use super::service::*;
use super::daemon::*;
use super::journal;

// registration

pub fn register_service(name: &'static str, entry: fn() -> !, restart: RestartPolicy) {
    let mut svc = Service::empty();
    svc.name = name;
    svc.entry = Some(entry);
    svc.restart = restart;
    svc.target = Target::MultiUser;
    svc.priority = 5;
    svc.restart_delay_ticks = DEFAULT_RESTART_DELAY;
    do_register(svc);
}

pub fn register_service_target(
    name: &'static str,
    description: &'static str,
    entry: fn() -> !,
    restart: RestartPolicy,
    target: Target,
    deps: &'static [&'static str],
) {
    let mut svc = Service::empty();
    svc.name = name;
    svc.description = description;
    svc.entry = Some(entry);
    svc.restart = restart;
    svc.target = target;
    svc.deps = deps;
    svc.priority = 5;
    svc.restart_delay_ticks = DEFAULT_RESTART_DELAY;
    do_register(svc);
}

pub fn register_service_full(
    name: &'static str,
    description: &'static str,
    entry: fn() -> !,
    restart: RestartPolicy,
    target: Target,
    deps: &'static [&'static str],
    priority: u8,
    restart_delay_ticks: u64,
) {
    let mut svc = Service::empty();
    svc.name = name;
    svc.description = description;
    svc.entry = Some(entry);
    svc.restart = restart;
    svc.target = target;
    svc.deps = deps;
    svc.priority = priority;
    svc.restart_delay_ticks = restart_delay_ticks;
    do_register(svc);
}

/// Register an ELF-based service (launched from disk binary)
pub fn register_elf_service(
    name: &'static str,
    description: &'static str,
    exec_path: &'static str,
    restart: RestartPolicy,
    target: Target,
    deps: &'static [&'static str],
) {
    let mut svc = Service::empty();
    svc.name = name;
    svc.description = description;
    svc.exec_start_path = Some(exec_path);
    svc.restart = restart;
    svc.target = target;
    svc.deps = deps;
    svc.priority = 5;
    svc.restart_delay_ticks = DEFAULT_RESTART_DELAY;
    do_register(svc);
}

/// Register with full Service struct for advanced features
pub fn register_service_ext(svc: Service) {
    do_register(svc);
}

fn do_register(svc: Service) {
    let name = svc.name;
    let target = svc.target;
    let restart = svc.restart;
    let ok = SERVICES.lock().add(svc);
    if ok {
        crate::serial_println!("[mikud] registered '{}' target={} restart={}",
            name, target.as_str(), restart.as_str());
    } else {
        crate::serial_println!("[mikud] failed to register '{}'", name);
    }
}

// service control

pub fn start_service(name: &str) -> bool {
    let now = crate::interrupts::get_tick();
    with_service_table(|table| start_service_locked(table, name, now, 0))
}

pub fn stop_service(name: &str) -> bool {
    with_service_table(|table| stop_service_locked(table, name, 0))
}

pub fn force_stop_service(name: &str) -> bool {
    with_service_table(|table| force_stop_service_locked(table, name))
}

pub fn restart_service(name: &str) -> bool {
    let now = crate::interrupts::get_tick();
    with_service_table(|table| {
        let _ = stop_service_locked(table, name, 0);
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].next_restart_tick = 0;
        start_service_locked(table, name, now, 0)
    })
}

pub fn restart_service_delayed(name: &str, delay_ticks: u64) -> bool {
    let now = crate::interrupts::get_tick();
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].next_restart_tick = now + delay_ticks;
        table.services[idx].state = ServiceState::Stopped;
        table.services[idx].pid = 0;
        true
    })
}

pub fn reload_service(name: &str) -> bool {
    with_service_table(|table| reload_service_locked(table, name))
}

pub fn enable_service(name: &str) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].active = true;
        journal::log(journal::Event::Enabled, table.services[idx].name, 0, 0);
        true
    })
}

pub fn disable_service(name: &str) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        let _ = stop_service_locked(table, name, 0);
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].active = false;
        journal::log(journal::Event::Disabled, table.services[idx].name, 0, 0);
        true
    })
}

// mask/unmask

pub fn mask_service(name: &str) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        if table.services[idx].flags.critical {
            crate::serial_println!("[mikud] cannot mask critical service '{}'", name);
            return false;
        }
        // Stop if running
        if table.services[idx].state.is_active() {
            stop_service_locked(table, name, 0);
        }
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].flags.masked = true;
        table.services[idx].state = ServiceState::Dead;
        journal::log(journal::Event::Masked, table.services[idx].name, 0, 0);
        crate::serial_println!("[mikud] masked '{}'", name);
        true
    })
}

pub fn unmask_service(name: &str) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].flags.masked = false;
        table.services[idx].state = ServiceState::Stopped;
        journal::log(journal::Event::Unmasked, table.services[idx].name, 0, 0);
        crate::serial_println!("[mikud] unmasked '{}'", name);
        true
    })
}

pub fn is_masked(name: &str) -> bool {
    let table = SERVICES.lock();
    table.find_by_name(name)
        .map(|idx| table.services[idx].flags.masked)
        .unwrap_or(false)
}

// watchdog

pub fn watchdog_ping(name: &str) -> bool {
    let now = crate::interrupts::get_tick();
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].watchdog_last_ping = now;
        true
    })
}

/// Called by a service to report readiness
pub fn notify_ready(name: &str) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].flags.notify_ready = true;
        true
    })
}

// queries

pub fn services_ready() -> bool {
    let target = current_target();
    let table = SERVICES.lock();
    table.services.iter().all(|svc| {
        !svc.active || svc.flags.masked || svc.target > target || svc.state == ServiceState::Running
    })
}

pub fn ready_targets() -> bool {
    services_ready()
}

pub fn service_count() -> usize {
    SERVICES.lock().count
}

pub fn active_service_count() -> usize {
    SERVICES.lock().services.iter().filter(|s| s.active).count()
}

pub fn waiting_restart_count() -> usize {
    let table = SERVICES.lock();
    table.services.iter().filter(|s| {
        s.active && s.state == ServiceState::Failed && s.next_restart_tick != 0
    }).count()
}

pub fn service_names() -> Vec<&'static str> {
    let table = SERVICES.lock();
    table.services.iter().filter(|s| s.active).map(|s| s.name).collect()
}

pub fn target_for_service(name: &str) -> Option<Target> {
    let table = SERVICES.lock();
    table.find_by_name(name).map(|idx| table.services[idx].target)
}

pub fn service_state(name: &str) -> Option<ServiceState> {
    let table = SERVICES.lock();
    table.find_by_name(name).map(|idx| table.services[idx].state)
}

pub fn service_pid(name: &str) -> Option<u64> {
    let table = SERVICES.lock();
    table.find_by_name(name).map(|idx| table.services[idx].pid)
}

pub fn service_description(name: &str) -> Option<&'static str> {
    let table = SERVICES.lock();
    table.find_by_name(name).map(|idx| table.services[idx].description)
}

pub fn is_service_active(name: &str) -> bool {
    service_state(name).is_some()
}

pub fn is_service_running(name: &str) -> bool {
    match service_state(name) {
        Some(s) => s.is_active(),
        None => false,
    }
}

pub fn service_restart_delay(name: &str) -> Option<u64> {
    let table = SERVICES.lock();
    table.find_by_name(name).map(|idx| table.services[idx].restart_delay_ticks)
}

pub fn set_service_restart_delay(name: &str, delay_ticks: u64) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].restart_delay_ticks = delay_ticks;
        true
    })
}

pub fn set_service_target(name: &str, target: Target) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].target = target;
        true
    })
}

pub fn set_service_priority(name: &str, priority: u8) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].priority = priority.clamp(1, 20);
        true
    })
}

pub fn set_service_watchdog(name: &str, ticks: u64) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].watchdog_ticks = ticks;
        true
    })
}

pub fn set_service_critical(name: &str, critical: bool) -> bool {
    with_service_table(|table| {
        let idx = match table.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        table.services[idx].flags.critical = critical;
        true
    })
}

pub fn pending_services() -> Vec<&'static str> {
    let table = SERVICES.lock();
    table.services.iter()
        .filter(|s| s.active && !s.state.is_active() && s.state != ServiceState::Dead)
        .map(|s| s.name)
        .collect()
}

pub fn list_services() -> Vec<ServiceSnapshot> {
    let table = SERVICES.lock();
    let mut out = Vec::new();

    for svc in table.services.iter() {
        if !svc.active {
            continue;
        }

        out.push(ServiceSnapshot {
            name: svc.name,
            description: svc.description,
            target: svc.target.as_str(),
            state: svc.state.as_str(),
            svc_type: svc.svc_type.as_str(),
            pid: svc.pid,
            restarts: svc.restarts,
            last_exit_code: svc.last_exit_code,
            next_restart_tick: svc.next_restart_tick,
            deps: svc.deps,
            wants: svc.wants,
            conflicts: svc.conflicts,
            restart: svc.restart.as_str(),
            critical: svc.flags.critical,
            masked: svc.flags.masked,
            watchdog_ticks: svc.watchdog_ticks,
            last_start_tick: svc.last_start_tick,
            last_stop_tick: svc.last_stop_tick,
            exec_start_path: svc.exec_start_path,
        });
    }

    out
}

// boot analysis

pub struct BootTiming {
    pub name: &'static str,
    pub start_tick: u64,
    pub duration_ticks: u64,
    pub target: &'static str,
}

pub fn boot_analyze() -> Vec<BootTiming> {
    let table = SERVICES.lock();
    let mut out = Vec::new();

    for svc in table.services.iter() {
        if !svc.active || svc.last_start_tick == 0 {
            continue;
        }
        let end = if svc.state == ServiceState::Running {
            svc.last_start_tick // instant for simple services
        } else if svc.last_stop_tick > svc.last_start_tick {
            svc.last_stop_tick
        } else {
            svc.last_start_tick
        };
        // For services that started during boot, calculate their activation time
        let duration = if svc.restarts == 0 && svc.state == ServiceState::Running {
            // First start, still running - measure from registration to running
            0 // instant start for kernel services
        } else {
            end.saturating_sub(svc.last_start_tick)
        };

        out.push(BootTiming {
            name: svc.name,
            start_tick: svc.last_start_tick,
            duration_ticks: duration,
            target: svc.target.as_str(),
        });
    }

    out.sort_by_key(|t| t.start_tick);
    out
}

// dependency tree

pub fn dependency_tree(name: &str) -> Vec<(&'static str, u8)> {
    let table = SERVICES.lock();
    let mut out = Vec::new();
    build_dep_tree(&table, name, 0, &mut out);
    out
}

fn build_dep_tree(table: &ServiceTable, name: &str, depth: u8, out: &mut Vec<(&'static str, u8)>) {
    if depth > 8 {
        return;
    }
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return,
    };
    out.push((table.services[idx].name, depth));

    for dep in table.services[idx].deps.iter() {
        build_dep_tree(table, dep, depth + 1, out);
    }
    for want in table.services[idx].wants.iter() {
        build_dep_tree(table, want, depth + 1, out);
    }
}

// reverse deps (what depends on this service)

pub fn reverse_deps(name: &str) -> Vec<&'static str> {
    let table = SERVICES.lock();
    table.dependents_of(name)
}
