//! The requests and records of `tty`(99), `session`(92), `power`(93), and
//! `procinfo`(103)

use super::cred::MAX_GROUPS;

// `tty`(99). 0 and 2 are reserved: a pty comes from `open("/dev/ptmx")`, and
// closing the master descriptor is the hangup
/// The caller's controlling terminal becomes `arg`
pub const TTY_ATTACH: u64 = 1;
/// Which terminal the caller is on. 0 is the console
pub const TTY_SELF: u64 = 3;
/// `tcsetpgrp`: `arg` is a process group, 0 the caller's own
pub const TTY_SETFG: u64 = 4;
/// `tcgetpgrp`
pub const TTY_GETFG: u64 = 5;
/// Blanks the caller's terminal and puts the pen at the top left
pub const TTY_CLEAR: u64 = 6;
/// Which terminal another process is on. `arg` is its pid
pub const TTY_OF_PID: u64 = 7;
/// The console's `TtyId`
pub const TTY_SYSTEM: u32 = 0x0100;

// `session`(92)
/// Open a session. `arg` is the leader pid (0 for a console session)
pub const SESSION_OPEN: u64 = 0;
/// End one. `arg` is the id, or 0 for the caller's own
pub const SESSION_CLOSE: u64 = 1;
/// Copy out the open sessions as an array of `SessionInfo`
pub const SESSION_LIST: u64 = 2;
/// The session a process is in. `arg` is a pid, or 0 for the caller. 0 back
/// means no session
pub const SESSION_CURRENT: u64 = 3;

pub const SESSION_KIND_CONSOLE: u8 = 0;
pub const SESSION_KIND_GRAPHICAL: u8 = 1;
pub const SEAT0: u8 = 0;

/// What `SESSION_OPEN` reads
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SessionRequest {
    pub uid: u16,
    pub gid: u16,
    pub kind: u8,
    pub seat: u8,
    pub ngroups: u8,
    pub _pad: u8,
    pub groups: [u16; MAX_GROUPS],
}

/// One row of `SESSION_LIST`
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SessionInfo {
    pub id: u32,
    pub uid: u16,
    pub gid: u16,
    pub kind: u8,
    pub seat: u8,
    pub _pad: [u8; 2],
    pub leader: u64,
    pub opened_tick: u64,
}

// `power`(93)
pub const POWER_OFF: u64 = 0;
pub const POWER_REBOOT: u64 = 1;

/// One record of `procinfo`(103)
#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct ProcInfo {
    pub pid: u64,
    pub ppid: u64,
    pub pgid: u64,
    /// CPU ticks since start
    pub cpu_time: u64,
    pub uid: u16,
    /// R running, S sleeping, B blocked, D dead, Z zombie
    pub state: u8,
    pub priority: u8,
    /// Tenths of a percent over the last window
    pub cpu_pct_x10: u32,
    /// Terminal as a raw `TtyId`; 0 for none
    pub tty: u32,
    pub name: [u8; 24],
    pub _pad: [u8; 4],
}

impl ProcInfo {
    pub fn name(&self) -> &str {
        let end = self.name.iter().position(|&b| b == 0).unwrap_or(self.name.len());
        match core::str::from_utf8(&self.name[..end]) {
            Ok(s) => s,
            Err(_) => "?",
        }
    }
}

const _: () = assert!(core::mem::size_of::<ProcInfo>() == 72);
const _: () = assert!(core::mem::size_of::<SessionInfo>() == 32);
