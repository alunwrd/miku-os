// mikuD - MikuOS init daemon (PID 1)
//
// Supports: targets, dependencies (Requires/Wants/Conflicts), lifecycle hooks,
// watchdog, notify, conditions, masking, graceful shutdown, boot analysis

pub mod target;
pub mod service;
pub mod daemon;
pub mod api;
pub mod journal;
pub mod unit;
pub mod timer;
pub mod socket;

// re-exports
pub use target::Target;
pub use service::{
    ServiceState, RestartPolicy, ServiceType, ServiceSnapshot,
    ServiceFlags, Service, DepType, Condition, ConditionType, EnvVar,
};
pub use daemon::{
    is_running, activate, deactivate, mikud_pid, set_mikud_pid, initialized,
    current_target, target_name, set_target, set_target_name,
    default_target, set_default_target,
    target_is, target_at_least, promote_target,
    enable_boot_promotion, disable_boot_promotion, boot_promoted,
    boot_phase, boot_state, mikud_main,
    is_shutting_down, isolate_target, poweroff, reboot,
};
pub use socket::{
    register_socket, activate_socket, stop_socket, remove_socket,
    list_sockets, socket_for_port, socket_count, SocketType,
};
pub use timer::{
    register_timer, start_timer, stop_timer, remove_timer,
    list_timers, timer_count, TimerType,
};
pub use api::{
    register_service, register_service_target, register_service_full,
    register_service_ext, register_elf_service,
    start_service, stop_service, force_stop_service,
    restart_service, restart_service_delayed,
    reload_service,
    enable_service, disable_service,
    mask_service, unmask_service, is_masked,
    watchdog_ping, notify_ready,
    services_ready, ready_targets, service_count, active_service_count,
    waiting_restart_count, service_names, target_for_service,
    service_state, service_pid, service_description,
    is_service_active, is_service_running,
    service_restart_delay, set_service_restart_delay,
    set_service_target, set_service_priority,
    set_service_watchdog, set_service_critical,
    pending_services, list_services,
    boot_analyze, dependency_tree, reverse_deps,
};
