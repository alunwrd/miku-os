//! sys_miku: signals, environment, events, timers, dates

// date/time components
#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuDateTime {
    pub year: i32,
    pub month: u8,    // 1-12
    pub day: u8,      // 1-31
    pub hour: u8,     // 0-23
    pub minute: u8,   // 0-59
    pub second: u8,   // 0-59
    pub weekday: u8,  // 0=Sunday, 6=Saturday
    pub yearday: u16, // 0-365
}

pub type EnvCallback = extern "C" fn(*const u8, *const u8, *mut u8);

pub type EventHandler = extern "C" fn(u32, *mut u8, *mut u8);

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuSigaction {
    pub handler: Option<SignalHandler>,
    pub flags: u32,
    pub mask: u32, // signals to block during handler execution
}

pub type SignalHandler = extern "C" fn(u32);

#[repr(C)]
pub struct MikuStopwatch {
    pub start_ms: u64,
    pub paused_elapsed: u64,
    pub running: bool,
}

#[repr(C)]
pub struct MikuTimer {
    pub deadline_ms: u64,
    pub duration_ms: u64,
    pub repeat: bool,
    pub expired: bool,
}
