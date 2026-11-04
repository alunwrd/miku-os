// mikuD unit file parser: INI-like .service files
//
// Format:
//
// [Unit]
// Description=My service
// After=kbd
// Wants=network
// Conflicts=rescue-shell
// ConditionPathExists=/etc/config
//
// [Service]
// Type=simple
// Restart=always
// RestartSec=200ms
// Priority=5
// WatchdogSec=30
// TimeoutStartSec=10s
// TimeoutStopSec=1min
// RemainAfterExit=true
// Environment=KEY=VALUE
// User=greeter
// Capabilities=may_priv
//
// [Install]
// WantedBy=multi-user

extern crate alloc;
use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use super::target::Target;
use super::service::{RestartPolicy, ServiceType};

#[derive(Clone)]
pub struct UnitFile {
    pub name: String,
    pub description: String,
    // [Unit]
    pub after: Vec<String>,
    pub wants: Vec<String>,
    pub requires: Vec<String>,
    pub conflicts: Vec<String>,
    pub condition_path_exists: Vec<String>,
    pub condition_path_not_exists: Vec<String>,
    pub condition_service_active: Vec<String>,
    // [Service]
    pub service_type: ServiceType,
    pub exec_start: String,
    pub restart: RestartPolicy,
    // The four times below are timer ticks, as the service table counts
    // them; the file says seconds and `parse_duration_ticks` converts
    pub restart_sec: u64,
    pub priority: u8,
    pub watchdog_sec: u64,
    pub timeout_start_sec: u64,
    pub timeout_stop_sec: u64,
    pub remain_after_exit: bool,
    pub critical: bool,
    pub env: Vec<(String, String)>,
    /// The account the service runs as, by name. Resolved against /etc/passwd
    /// when the unit is registered, not here: the parser is handed text and
    /// this file does no I/O
    pub user: String,
    /// The capabilities named by `Capabilities=`, as a mask, or `None` for a
    /// unit that did not say. `None` means the default for this user, `Some(0)`
    /// means none at all, and `Capabilities=` with an empty value means the
    /// second
    pub caps: Option<u64>,
    // [Install]
    pub wanted_by: Target,
}

#[derive(Debug)]
pub enum ParseError {
    EmptyInput,
    UnknownSection,
    InvalidValue,
    MissingField,
}

impl ParseError {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EmptyInput => "empty input",
            Self::UnknownSection => "unknown section",
            Self::InvalidValue => "invalid value",
            Self::MissingField => "missing required field",
        }
    }
}

impl UnitFile {
    pub fn new() -> Self {
        Self {
            name: String::new(),
            description: String::new(),
            after: Vec::new(),
            wants: Vec::new(),
            requires: Vec::new(),
            conflicts: Vec::new(),
            condition_path_exists: Vec::new(),
            condition_path_not_exists: Vec::new(),
            condition_service_active: Vec::new(),
            service_type: ServiceType::Simple,
            exec_start: String::new(),
            restart: RestartPolicy::Never,
            restart_sec: 50,
            priority: 5,
            watchdog_sec: 0,
            timeout_start_sec: 2500,
            timeout_stop_sec: 2500,
            remain_after_exit: false,
            critical: false,
            env: Vec::new(),
            user: String::new(),
            caps: None,
            wanted_by: Target::MultiUser,
        }
    }

    pub fn parse(input: &str) -> Result<Self, ParseError> {
        if input.is_empty() {
            return Err(ParseError::EmptyInput);
        }

        let mut unit = Self::new();
        let mut section = Section::None;

        for line in input.lines() {
            let line = line.trim();

            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }

            if line.starts_with('[') && line.ends_with(']') {
                section = match &line[1..line.len() - 1] {
                    "Unit" => Section::Unit,
                    "Service" => Section::Service,
                    "Install" => Section::Install,
                    _ => return Err(ParseError::UnknownSection),
                };
                continue;
            }

            let (key, value) = match line.split_once('=') {
                Some((k, v)) => (k.trim(), v.trim()),
                None => continue,
            };

            match section {
                Section::Unit => match key {
                    "Description" => unit.description = String::from(value),
                    "After" => parse_list(value, &mut unit.after),
                    "Wants" => parse_list(value, &mut unit.wants),
                    "Requires" => parse_list(value, &mut unit.requires),
                    "Conflicts" => parse_list(value, &mut unit.conflicts),
                    "ConditionPathExists" => {
                        if value.starts_with('!') {
                            unit.condition_path_not_exists.push(String::from(&value[1..]));
                        } else {
                            unit.condition_path_exists.push(String::from(value));
                        }
                    }
                    "ConditionServiceActive" => {
                        unit.condition_service_active.push(String::from(value));
                    }
                    _ => {}
                },
                Section::Service => match key {
                    "Type" => {
                        unit.service_type = ServiceType::from_str(value)
                            .ok_or(ParseError::InvalidValue)?;
                    }
                    "ExecStart" => unit.exec_start = String::from(value),
                    "Restart" => {
                        unit.restart = RestartPolicy::from_str(value)
                            .ok_or(ParseError::InvalidValue)?;
                    }
                    "RestartSec" => {
                        unit.restart_sec = parse_duration_ticks(value).ok_or(ParseError::InvalidValue)?;
                    }
                    "Priority" => {
                        unit.priority = parse_u64(value).ok_or(ParseError::InvalidValue)? as u8;
                    }
                    "WatchdogSec" => {
                        unit.watchdog_sec = parse_duration_ticks(value).ok_or(ParseError::InvalidValue)?;
                    }
                    "TimeoutStartSec" => {
                        unit.timeout_start_sec = parse_duration_ticks(value).ok_or(ParseError::InvalidValue)?;
                    }
                    "TimeoutStopSec" => {
                        unit.timeout_stop_sec = parse_duration_ticks(value).ok_or(ParseError::InvalidValue)?;
                    }
                    "RemainAfterExit" => {
                        unit.remain_after_exit = parse_bool(value);
                    }
                    "Critical" => {
                        unit.critical = parse_bool(value);
                    }
                    "User" => unit.user = String::from(value),
                    // Space- or comma-separated names, spelled as `id` prints
                    // them. An unknown name is refused rather than ignored
                    "Capabilities" => {
                        let mut mask = 0u64;
                        for word in value.split(|c: char| c.is_whitespace() || c == ',') {
                            let word = word.trim();
                            if word.is_empty() { continue; }
                            match crate::process::cred::CapSet::by_name(word) {
                                Some(bit) => mask |= 1u64 << bit,
                                None => return Err(ParseError::InvalidValue),
                            }
                        }
                        unit.caps = Some(mask);
                    }
                    "Environment" => {
                        if let Some((k, v)) = value.split_once('=') {
                            unit.env.push((String::from(k), String::from(v)));
                        }
                    }
                    _ => {}
                },
                Section::Install => match key {
                    "WantedBy" => {
                        unit.wanted_by = Target::from_str(value)
                            .ok_or(ParseError::InvalidValue)?;
                    }
                    _ => {}
                },
                Section::None => {}
            }
        }

        Ok(unit)
    }

    pub fn validate(&self) -> Result<(), ParseError> {
        if self.name.is_empty() && self.exec_start.is_empty() {
            return Err(ParseError::MissingField);
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum Section {
    None,
    Unit,
    Service,
    Install,
}

fn parse_list(value: &str, out: &mut Vec<String>) {
    for item in value.split_whitespace() {
        out.push(String::from(item));
    }
}

/// Timer ticks per second, for turning a unit file's times into the ticks
/// the service table counts in. 250 until the LAPIC timer is calibrated
fn ticks_per_sec() -> u64 {
    match crate::arch::x86_64::apic::timer_hz() {
        0 => 250,
        hz => hz as u64,
    }
}

/// A `*Sec=` value, in ticks. A bare number is seconds, as in systemd; `ms`,
/// `s` and `min` may follow it. `0` stays 0, which means "off" for
/// `WatchdogSec`. A non-zero time shorter than one tick rounds up to one tick,
/// so it never turns into "off"
fn parse_duration_ticks(value: &str) -> Option<u64> {
    let v = value.trim();
    let (num, ms_per_unit) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1u64)
    } else if let Some(n) = v.strip_suffix("min") {
        (n, 60_000)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1_000)
    } else {
        (v, 1_000)
    };
    let ms = parse_u64(num.trim())?.checked_mul(ms_per_unit)?;
    if ms == 0 { return Some(0); }
    let ticks = ms.checked_mul(ticks_per_sec())? / 1_000;
    Some(ticks.max(1))
}

/// A tick count as a unit file would write it, for `sv show`: whole seconds
/// when it is a whole number of them, milliseconds otherwise
pub fn ticks_as_duration(ticks: u64) -> alloc::string::String {
    let ms = ticks.saturating_mul(1_000) / ticks_per_sec();
    if ms % 1_000 == 0 {
        alloc::format!("{}", ms / 1_000)
    } else {
        alloc::format!("{}ms", ms)
    }
}

fn parse_u64(s: &str) -> Option<u64> {
    let mut result: u64 = 0;
    for b in s.bytes() {
        if b < b'0' || b > b'9' {
            return None;
        }
        result = result.checked_mul(10)?.checked_add((b - b'0') as u64)?;
    }
    Some(result)
}

fn parse_bool(s: &str) -> bool {
    matches!(s, "true" | "yes" | "1" | "on")
}

// unit directory scanning and loading //

pub const UNIT_DIR: &str = "/etc/mikud";

/// Load a single .service file from VFS path
pub fn load_unit_file(path: &str) {
    let data = match crate::fs::vfs_read::read_file(path) {
        Some(d) => d,
        None => {
            crate::serial_println!("[mikud] unit: cannot read '{}'", path);
            crate::println!("  error: cannot read '{}'", path);
            return;
        }
    };

    let text = match core::str::from_utf8(&data) {
        Ok(s) => s,
        Err(_) => {
            crate::serial_println!("[mikud] unit: '{}' is not valid UTF-8", path);
            crate::println!("  error: '{}' is not valid UTF-8", path);
            return;
        }
    };

    let mut unit = match UnitFile::parse(text) {
        Ok(u) => u,
        Err(e) => {
            crate::serial_println!("[mikud] unit: parse error in '{}': {}", path, e.as_str());
            crate::println!("  error: parse error in '{}': {}", path, e.as_str());
            return;
        }
    };

    // Derive name from filename if not set
    if unit.name.is_empty() {
        let filename = path.rsplit('/').next().unwrap_or(path);
        let name = filename.strip_suffix(".service").unwrap_or(filename);
        unit.name = String::from(name);
    }

    crate::serial_println!("[mikud] loaded unit '{}' from '{}'", unit.name, path);
    crate::println!("  loaded '{}' ({})", unit.name, unit.description);

    // Registers with mikuD. A disk-loaded service has no Rust fn() entry point;
    // its ELF path is stored below
    register_unit(&unit);
}

/// Scan /etc/mikud/ directory for .service files and load them
pub fn scan_unit_dir() {
    crate::serial_println!("[mikud] scanning {}", UNIT_DIR);
    crate::println!("  scanning {}...", UNIT_DIR);

    let entries = crate::fs::vfs::core::with_vfs(|vfs| {
        use crate::fs::vfs::types::DirEntry;
        let dir_id = match vfs.resolve_path(0, UNIT_DIR) {
            Ok(id) => id,
            Err(_) => {
                crate::println!("  {} not found", UNIT_DIR);
                return Vec::new();
            }
        };
        let mut dir_entries = [DirEntry::empty(); 32];
        let count = vfs.readdir(dir_id, &mut dir_entries).unwrap_or(0);
        let mut names = Vec::new();
        for i in 0..count {
            let name = dir_entries[i].get_name();
            if name.ends_with(".service") {
                names.push(String::from(name));
            }
        }
        names
    });

    if entries.is_empty() {
        crate::println!("  no .service files found");
        return;
    }

    let mut loaded = 0;
    for name in &entries {
        let path = alloc::format!("{}/{}", UNIT_DIR, name);
        load_unit_file(&path);
        loaded += 1;
    }
    crate::println!("  loaded {} unit(s)", loaded);
}

fn register_unit(unit: &UnitFile) {
    use super::service::*;

    let mut svc = Service::empty();

    // Leak the name to get &'static str (necessary for kernel service table)
    let name_leaked: &'static str = leak_string(&unit.name);
    let desc_leaked: &'static str = if unit.description.is_empty() {
        ""
    } else {
        leak_string(&unit.description)
    };

    svc.name = name_leaked;
    svc.description = desc_leaked;
    svc.svc_type = unit.service_type;
    svc.restart = unit.restart;
    svc.restart_delay_ticks = unit.restart_sec;
    svc.priority = unit.priority;
    svc.target = unit.wanted_by;
    svc.watchdog_ticks = unit.watchdog_sec;
    svc.timeout_start_ticks = unit.timeout_start_sec;
    svc.timeout_stop_ticks = unit.timeout_stop_sec;
    svc.flags.remain_after_exit = unit.remain_after_exit;
    svc.flags.critical = unit.critical;

    // The identity, resolved now against the account files, for
    // `apply_unit_identity`.
    //
    // A `User=` naming an account that does not exist is refused rather than
    // run as root: a missing account is a reason not to start a service
    if !unit.user.is_empty() {
        match crate::syscall::adapters::process::passwd_ids(&unit.user) {
            Some(ids) => svc.user = Some(ids),
            None => {
                crate::serial_println!(
                    "[mikud] unit '{}' wants User={} and there is no such account - not registering",
                    unit.name, unit.user);
                crate::println!("  error: '{}' names no account '{}'", unit.name, unit.user);
                return;
            }
        }
    }
    svc.caps = unit.caps;

    // Leak dependency lists to get &'static [&'static str]
    // Requires + After combined into deps (hard deps)
    if !unit.requires.is_empty() || !unit.after.is_empty() {
        let mut all_deps: Vec<&'static str> = Vec::new();
        for d in unit.requires.iter().chain(unit.after.iter()) {
            all_deps.push(leak_string(d));
        }
        all_deps.sort();
        all_deps.dedup();
        svc.deps = leak_str_slice(all_deps);
    }
    if !unit.wants.is_empty() {
        let leaked: Vec<&'static str> = unit.wants.iter().map(|s| leak_string(s)).collect();
        svc.wants = leak_str_slice(leaked);
    }
    if !unit.conflicts.is_empty() {
        let leaked: Vec<&'static str> = unit.conflicts.iter().map(|s| leak_string(s)).collect();
        svc.conflicts = leak_str_slice(leaked);
    }

    // ELF-based services: store the exec path for launch at start time
    if !unit.exec_start.is_empty() {
        svc.exec_start_path = Some(leak_string(&unit.exec_start));
    }

    // Set conditions - combine all condition types into the fixed-size array
    let mut cond_idx = 0;
    for path in unit.condition_path_exists.iter() {
        if cond_idx >= MAX_CONDITIONS { break; }
        svc.conditions[cond_idx] = Some(super::service::Condition {
            cond_type: super::service::ConditionType::PathExists,
            arg: leak_string(path),
            negate: false,
        });
        cond_idx += 1;
    }
    for path in unit.condition_path_not_exists.iter() {
        if cond_idx >= MAX_CONDITIONS { break; }
        svc.conditions[cond_idx] = Some(super::service::Condition {
            cond_type: super::service::ConditionType::PathExists,
            arg: leak_string(path),
            negate: true,
        });
        cond_idx += 1;
    }
    for svc_name in unit.condition_service_active.iter() {
        if cond_idx >= MAX_CONDITIONS { break; }
        svc.conditions[cond_idx] = Some(super::service::Condition {
            cond_type: super::service::ConditionType::ServiceActive,
            arg: leak_string(svc_name),
            negate: false,
        });
        cond_idx += 1;
    }

    // Environment
    for (i, (k, v)) in unit.env.iter().enumerate() {
        if i >= MAX_ENV { break; }
        svc.env[i] = Some(super::service::EnvVar {
            key: leak_string(k),
            value: leak_string(v),
        });
    }

    super::api::register_service_ext(svc);
}

/// Leaks a String to get &'static str, for service names in the kernel table
fn leak_string(s: &str) -> &'static str {
    let boxed = String::from(s).into_boxed_str();
    Box::leak(boxed)
}

/// Leak a Vec<&'static str> to get &'static [&'static str]
fn leak_str_slice(v: Vec<&'static str>) -> &'static [&'static str] {
    let boxed = v.into_boxed_slice();
    Box::leak(boxed)
}

/// Helper for UnitFile: show parsed info
impl UnitFile {
    pub fn summary(&self) -> String {
        alloc::format!("[{}] type={} restart={} target={} exec={}",
            self.name,
            self.service_type.as_str(),
            self.restart.as_str(),
            self.wanted_by.as_str(),
            if self.exec_start.is_empty() { "(none)" } else { &self.exec_start },
        )
    }
}
