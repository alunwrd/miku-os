// mikuD timer units - periodic service activation (like cron/systemd timers)
//
// Timers trigger service starts at regular intervals or after a delay.
// Each timer is associated with a service name and fires on schedule.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

use super::journal;

pub const MAX_TIMERS: usize = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TimerType {
    Interval,   // fire every N ticks
    Oneshot,    // fire once after N ticks, then disable
    Realtime,   // fire every N ticks aligned to boot time
}

impl TimerType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Interval => "interval",
            Self::Oneshot => "oneshot",
            Self::Realtime => "realtime",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "interval" | "periodic" => Some(Self::Interval),
            "oneshot" | "once" => Some(Self::Oneshot),
            "realtime" | "calendar" => Some(Self::Realtime),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct Timer {
    pub name: &'static str,
    pub service: &'static str,      // service to activate
    pub timer_type: TimerType,
    pub interval_ticks: u64,         // fire interval
    pub last_fire_tick: u64,
    pub next_fire_tick: u64,
    pub fire_count: u32,
    pub active: bool,
    pub persistent: bool,            // fire immediately if missed window
}

impl Timer {
    pub const fn empty() -> Self {
        Self {
            name: "",
            service: "",
            timer_type: TimerType::Interval,
            interval_ticks: 0,
            last_fire_tick: 0,
            next_fire_tick: 0,
            fire_count: 0,
            active: false,
            persistent: false,
        }
    }

    pub fn should_fire(&self, now: u64) -> bool {
        self.active && self.next_fire_tick > 0 && now >= self.next_fire_tick
    }
}

pub struct TimerTable {
    pub timers: [Timer; MAX_TIMERS],
    pub count: usize,
}

impl TimerTable {
    pub const fn new() -> Self {
        Self {
            timers: [Timer::empty(); MAX_TIMERS],
            count: 0,
        }
    }

    pub fn add(&mut self, timer: Timer) -> bool {
        if self.count >= MAX_TIMERS {
            return false;
        }
        for slot in self.timers.iter_mut() {
            if !slot.active {
                *slot = timer;
                slot.active = true;
                self.count += 1;
                return true;
            }
        }
        false
    }

    pub fn find_by_name(&self, name: &str) -> Option<usize> {
        self.timers.iter().position(|t| t.active && t.name == name)
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let idx = match self.find_by_name(name) {
            Some(i) => i,
            None => return false,
        };
        self.timers[idx] = Timer::empty();
        self.count = self.count.saturating_sub(1);
        true
    }
}

static TIMERS: Mutex<TimerTable> = Mutex::new(TimerTable::new());

// public API //

pub fn register_timer(
    name: &'static str,
    service: &'static str,
    timer_type: TimerType,
    interval_ticks: u64,
    persistent: bool,
) -> bool {
    let now = crate::interrupts::get_tick();
    let mut timer = Timer::empty();
    timer.name = name;
    timer.service = service;
    timer.timer_type = timer_type;
    timer.interval_ticks = interval_ticks;
    timer.next_fire_tick = now + interval_ticks;
    timer.persistent = persistent;

    let ok = TIMERS.lock().add(timer);
    if ok {
        crate::serial_println!("[mikud] timer '{}' -> '{}' every {} ticks",
            name, service, interval_ticks);
    }
    ok
}

pub fn start_timer(name: &str) -> bool {
    let now = crate::interrupts::get_tick();
    let mut table = TIMERS.lock();
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };
    table.timers[idx].active = true;
    table.timers[idx].next_fire_tick = now + table.timers[idx].interval_ticks;
    true
}

pub fn stop_timer(name: &str) -> bool {
    let mut table = TIMERS.lock();
    let idx = match table.find_by_name(name) {
        Some(i) => i,
        None => return false,
    };
    table.timers[idx].active = false;
    true
}

pub fn remove_timer(name: &str) -> bool {
    TIMERS.lock().remove(name)
}

pub fn list_timers() -> Vec<TimerSnapshot> {
    let table = TIMERS.lock();
    let mut out = Vec::new();
    for t in table.timers.iter() {
        if !t.active { continue; }
        out.push(TimerSnapshot {
            name: t.name,
            service: t.service,
            timer_type: t.timer_type.as_str(),
            interval_ticks: t.interval_ticks,
            next_fire_tick: t.next_fire_tick,
            last_fire_tick: t.last_fire_tick,
            fire_count: t.fire_count,
            active: t.active,
        });
    }
    out
}

pub fn timer_count() -> usize {
    TIMERS.lock().count
}

pub struct TimerSnapshot {
    pub name: &'static str,
    pub service: &'static str,
    pub timer_type: &'static str,
    pub interval_ticks: u64,
    pub next_fire_tick: u64,
    pub last_fire_tick: u64,
    pub fire_count: u32,
    pub active: bool,
}

// called from daemon main loop //

pub fn tick_timers(now: u64) {
    let mut to_fire: Vec<(&'static str, &'static str)> = Vec::new();

    {
        let mut table = TIMERS.lock();
        for timer in table.timers.iter_mut() {
            if !timer.should_fire(now) {
                continue;
            }

            to_fire.push((timer.name, timer.service));
            timer.last_fire_tick = now;
            timer.fire_count = timer.fire_count.saturating_add(1);

            match timer.timer_type {
                TimerType::Interval | TimerType::Realtime => {
                    timer.next_fire_tick = now + timer.interval_ticks;
                }
                TimerType::Oneshot => {
                    timer.active = false;
                    timer.next_fire_tick = 0;
                }
            }
        }
    }

    // Fire outside the lock to avoid deadlock with service table
    for (timer_name, service_name) in to_fire {
        crate::serial_println!("[mikud] timer '{}' fired -> starting '{}'", timer_name, service_name);
        journal::log(journal::Event::TimerFired, timer_name, 0, 0);
        super::api::start_service(service_name);
    }
}
