// mikuD service types - state machine, restart policy, service table
//
// Full service model with watchdog, notify, conditions,
// masking, dependency types, lifecycle hooks, and resource limits

extern crate alloc;
use alloc::vec::Vec;
use super::target::Target;

pub const MAX_SERVICES: usize = 32;
pub const MAX_DEPENDENCIES: usize = 8;
pub const MAX_CONDITIONS: usize = 4;
pub const MAX_ENV: usize = 8;
pub const DEFAULT_RESTART_DELAY: u64 = 50;
pub const DEFAULT_TIMEOUT_START: u64 = 2500; // 10 sec at 250Hz
pub const DEFAULT_TIMEOUT_STOP: u64 = 2500;
pub const MAX_RESTART_BURST: u32 = 5;
pub const RESTART_BURST_WINDOW: u64 = 2500; // 10 sec

// service state

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    Reloading,
    Stopping,
    Failed,
    Activating, // oneshot: running, not yet finished
    Dead,       // masked or permanently stopped
}

impl ServiceState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Reloading => "reloading",
            Self::Stopping => "stopping",
            Self::Failed => "failed",
            Self::Activating => "activating",
            Self::Dead => "dead",
        }
    }

    pub fn is_active(self) -> bool {
        matches!(self, Self::Running | Self::Reloading | Self::Activating)
    }
}

// service type 

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ServiceType {
    Simple,
    Oneshot,
    Forking,
    Notify,
}

impl ServiceType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Simple => "simple",
            Self::Oneshot => "oneshot",
            Self::Forking => "forking",
            Self::Notify => "notify",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "simple" => Some(Self::Simple),
            "oneshot" => Some(Self::Oneshot),
            "forking" => Some(Self::Forking),
            "notify" => Some(Self::Notify),
            _ => None,
        }
    }
}

// restart policy

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum RestartPolicy {
    Always,
    Never,
    OnFailure,
    OnSuccess,
    OnAbnormal,
}

impl RestartPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Never => "never",
            Self::OnFailure => "on-failure",
            Self::OnSuccess => "on-success",
            Self::OnAbnormal => "on-abnormal",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "always" => Some(Self::Always),
            "never" | "no" => Some(Self::Never),
            "on-failure" => Some(Self::OnFailure),
            "on-success" => Some(Self::OnSuccess),
            "on-abnormal" => Some(Self::OnAbnormal),
            _ => None,
        }
    }

    pub fn should_restart(self, exit_code: u64, was_signal: bool) -> bool {
        match self {
            Self::Always => true,
            Self::Never => false,
            Self::OnFailure => exit_code != 0,
            Self::OnSuccess => exit_code == 0,
            Self::OnAbnormal => was_signal || exit_code != 0,
        }
    }
}

// dependency type

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum DepType {
    Requires,  // hard dep - fail if dep fails
    Wants,     // soft dep - continue if dep fails
    After,     // ordering only - start after dep
    Conflicts, // stop this if dep starts
}

impl DepType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Requires => "requires",
            Self::Wants => "wants",
            Self::After => "after",
            Self::Conflicts => "conflicts",
        }
    }
}

#[derive(Clone, Copy)]
pub struct Dependency {
    pub name: &'static str,
    pub dep_type: DepType,
}

// conditions

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConditionType {
    PathExists,
    ServiceActive,
    TargetActive,
}

#[derive(Clone, Copy)]
pub struct Condition {
    pub cond_type: ConditionType,
    pub arg: &'static str,
    pub negate: bool,
}

impl Condition {
    pub fn check(&self) -> bool {
        let result = match self.cond_type {
            ConditionType::PathExists => {
                // Use VFS stat to check path existence
                crate::vfs::core::with_vfs(|vfs| vfs.stat(0, self.arg).is_ok())
            }
            ConditionType::ServiceActive => {
                super::api::is_service_running(self.arg)
            }
            ConditionType::TargetActive => {
                Target::from_str(self.arg)
                    .map(|t| super::daemon::target_at_least(t))
                    .unwrap_or(false)
            }
        };
        if self.negate { !result } else { result }
    }
}

// environment

#[derive(Clone, Copy)]
pub struct EnvVar {
    pub key: &'static str,
    pub value: &'static str,
}

// service flags

#[derive(Clone, Copy)]
pub struct ServiceFlags {
    pub critical: bool,      // cannot be stopped/masked by user
    pub masked: bool,        // completely prevented from starting
    pub oneshot_done: bool,  // oneshot completed successfully
    pub notify_ready: bool,  // notify-type: service reported ready
    pub remain_after_exit: bool, // keep "active" state after exit (oneshot)
    pub was_signal_kill: bool,   // last death was signal, not clean exit
}

impl ServiceFlags {
    pub const fn default() -> Self {
        Self {
            critical: false,
            masked: false,
            oneshot_done: false,
            notify_ready: false,
            remain_after_exit: false,
            was_signal_kill: false,
        }
    }
}

// the service struct

#[derive(Clone, Copy)]
pub struct Service {
    // identity
    pub name: &'static str,
    pub description: &'static str,
    pub active: bool,

    // type and state
    pub svc_type: ServiceType,
    pub state: ServiceState,
    pub flags: ServiceFlags,

    // process
    pub pid: u64,
    pub entry: Option<fn() -> !>,
    pub exec_start_path: Option<&'static str>, // ELF binary path on disk
    pub on_restart: Option<fn()>, // called before re-entering entry on restart
    pub priority: u8,

    // dependencies
    pub target: Target,
    pub deps: &'static [&'static str],           // legacy: Requires+After
    pub wants: &'static [&'static str],           // soft deps
    pub conflicts: &'static [&'static str],       // stop if these start

    // restart
    pub restart: RestartPolicy,
    pub restarts: u32,
    pub restart_delay_ticks: u64,
    pub next_restart_tick: u64,
    pub max_restarts: u32,          // 0 = unlimited
    pub burst_count: u32,           // restarts in current burst window
    pub burst_window_start: u64,

    // timing
    pub last_exit_code: u64,
    pub last_start_tick: u64,
    pub last_stop_tick: u64,
    pub timeout_start_ticks: u64,
    pub timeout_stop_ticks: u64,

    // watchdog
    pub watchdog_ticks: u64,        // 0 = disabled
    pub watchdog_last_ping: u64,

    // conditions
    pub conditions: [Option<Condition>; MAX_CONDITIONS],

    // environment
    pub env: [Option<EnvVar>; MAX_ENV],
}

impl Service {
    pub const fn empty() -> Self {
        Self {
            name: "",
            description: "",
            active: false,
            svc_type: ServiceType::Simple,
            state: ServiceState::Stopped,
            flags: ServiceFlags::default(),
            pid: 0,
            entry: None,
            exec_start_path: None,
            on_restart: None,
            priority: 5,
            target: Target::MultiUser,
            deps: &[],
            wants: &[],
            conflicts: &[],
            restart: RestartPolicy::Never,
            restarts: 0,
            restart_delay_ticks: DEFAULT_RESTART_DELAY,
            next_restart_tick: 0,
            max_restarts: 0,
            burst_count: 0,
            burst_window_start: 0,
            last_exit_code: 0,
            last_start_tick: 0,
            last_stop_tick: 0,
            timeout_start_ticks: DEFAULT_TIMEOUT_START,
            timeout_stop_ticks: DEFAULT_TIMEOUT_STOP,
            watchdog_ticks: 0,
            watchdog_last_ping: 0,
            conditions: [None; MAX_CONDITIONS],
            env: [None; MAX_ENV],
        }
    }

    pub fn check_conditions(&self) -> bool {
        for cond in self.conditions.iter().flatten() {
            if !cond.check() {
                return false;
            }
        }
        true
    }

    pub fn in_restart_burst(&self, now: u64) -> bool {
        if self.max_restarts == 0 {
            // check burst rate limit
            if now.saturating_sub(self.burst_window_start) < RESTART_BURST_WINDOW {
                return self.burst_count >= MAX_RESTART_BURST;
            }
        } else {
            return self.restarts >= self.max_restarts;
        }
        false
    }

    pub fn watchdog_expired(&self, now: u64) -> bool {
        self.watchdog_ticks > 0
            && self.state == ServiceState::Running
            && now.saturating_sub(self.watchdog_last_ping) > self.watchdog_ticks
    }
}

// snapshot for queries

pub struct ServiceSnapshot {
    pub name: &'static str,
    pub description: &'static str,
    pub target: &'static str,
    pub state: &'static str,
    pub svc_type: &'static str,
    pub pid: u64,
    pub restarts: u32,
    pub last_exit_code: u64,
    pub next_restart_tick: u64,
    pub deps: &'static [&'static str],
    pub wants: &'static [&'static str],
    pub conflicts: &'static [&'static str],
    pub restart: &'static str,
    pub critical: bool,
    pub masked: bool,
    pub watchdog_ticks: u64,
    pub last_start_tick: u64,
    pub last_stop_tick: u64,
    pub exec_start_path: Option<&'static str>,
}

// service table

pub struct ServiceTable {
    pub services: [Service; MAX_SERVICES],
    pub count: usize,
}

impl ServiceTable {
    pub const fn new() -> Self {
        Self {
            services: [Service::empty(); MAX_SERVICES],
            count: 0,
        }
    }

    pub fn add(&mut self, svc: Service) -> bool {
        if self.count >= MAX_SERVICES {
            return false;
        }
        for slot in self.services.iter_mut() {
            if !slot.active {
                *slot = svc;
                slot.active = true;
                self.count += 1;
                return true;
            }
        }
        false
    }

    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.services.iter().position(|s| s.active && s.name == name)
    }

    pub fn dependents_of(&self, name: &str) -> Vec<&'static str> {
        let mut out = Vec::new();
        for svc in self.services.iter() {
            if !svc.active { continue; }
            if svc.deps.iter().any(|dep| *dep == name) {
                out.push(svc.name);
            }
            if svc.wants.iter().any(|dep| *dep == name) {
                out.push(svc.name);
            }
        }
        out
    }

    pub fn conflicts_of(&self, name: &str) -> Vec<&'static str> {
        let mut out = Vec::new();
        for svc in self.services.iter() {
            if !svc.active { continue; }
            if svc.conflicts.iter().any(|c| *c == name) {
                out.push(svc.name);
            }
        }
        out
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let idx = match self.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        self.services[idx] = Service::empty();
        self.count = self.count.saturating_sub(1);
        true
    }

    pub fn sorted_by_target(&self) -> Vec<usize> {
        let mut indices: Vec<usize> = (0..MAX_SERVICES)
            .filter(|&i| self.services[i].active)
            .collect();
        indices.sort_by_key(|&i| self.services[i].target as u8);
        indices
    }
}
