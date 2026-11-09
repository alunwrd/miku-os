//! Process groups and signals. A shell puts each command in its own group and
//! gives it the terminal, so one Ctrl-C ends the job

/// Leave this signal at whatever the system does about it
pub use miku_abi::ksignal::SIG_DISP_DEFAULT as SIG_DEFAULT;
/// Ignores the signal, unlike no handler, where a fatal default kills
pub use miku_abi::ksignal::SIG_DISP_IGNORE as SIG_IGNORE;
/// Deliver it to this process's handler
pub use miku_abi::ksignal::SIG_DISP_CATCH as SIG_CATCH;

/// Puts a process in a group. `pid` 0 is the caller, `pgid` 0 makes a new
/// group led by the target. Called on both sides of a fork to close the
/// window
pub fn setpgid(pid: u64, pgid: u64) -> Result<(), i64> {
    let r = unsafe { super::miku_setpgid(pid, pgid) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Reaps an exited child without blocking (`WNOHANG`). `Some(pid)`, or
/// `None` when nothing is waiting
pub fn try_wait_any() -> Option<u64> {
    let mut status: i64 = 0;
    let r = unsafe { super::miku_try_wait(u64::MAX, &mut status as *mut i64) };
    if r > 0 { Some(r as u64) } else { None }
}

// Decoding a POSIX wait status

/// Did it exit of its own accord?
pub fn exited(status: i64) -> bool { (status & 0x7f) == 0 }

/// Exit code; only when `exited`
pub fn exit_code(status: i64) -> i64 { (status >> 8) & 0xff }

/// Killed by a signal
pub fn signalled(status: i64) -> bool {
    let low = status & 0x7f;
    low != 0 && low != 0x7f
}

/// Killing signal; only when `signalled`
pub fn term_sig(status: i64) -> i64 { status & 0x7f }

/// Stopped, not ended. Only reported with `WUNTRACED`; the child is alive
/// and not reaped
pub fn stopped(status: i64) -> bool { (status & 0xff) == 0x7f }

/// Stopping signal; only when `stopped`
pub fn stop_sig(status: i64) -> i64 { (status >> 8) & 0xff }

/// Shell-style status: the code, or 128+signal
pub fn wait_status_code(status: i64) -> i64 {
    if signalled(status) { 128 + term_sig(status) } else { exit_code(status) }
}

/// Waits for one child, optionally reporting a stop
pub fn wait_job(pid: u64, untraced: bool) -> Option<(u64, i64)> {
    let mut status: i64 = 0;
    let opts = if untraced { 2 } else { 0 };
    let r = unsafe { super::miku_wait4(pid, &mut status as *mut i64, opts) };
    if r <= 0 { None } else { Some((r as u64, status)) }
}

/// Which job a process is in. `pid` 0 is the caller
pub fn getpgid(pid: u64) -> u64 {
    let r = unsafe { super::miku_getpgid(pid) };
    if r < 0 { 0 } else { r as u64 }
}

/// Continues a stopped process
pub const SIGCONT: u64 = miku_abi::signal::SIGCONT as u64;
/// Suspends; sent by Ctrl-Z through the terminal
pub const SIGTSTP: u64 = miku_abi::signal::SIGTSTP as u64;
/// Suspend, and this one cannot be caught or ignored
pub const SIGSTOP: u64 = miku_abi::signal::SIGSTOP as u64;

/// Signals every process in a group, each permission-checked
pub fn killpg(pgid: u64, sig: u64) -> Result<(), i64> {
    let r = unsafe { super::miku_killpg(pgid, sig) };
    if r < 0 { Err(r) } else { Ok(()) }
}

pub use miku_abi::kproc::ProcInfo;

/// Every process, up to `out.len()`: pids, names, parents, jobs, CPU.
/// Nothing about memory
pub fn procinfo(out: &mut [ProcInfo]) -> usize {
    if out.is_empty() { return 0; }
    let n = unsafe {
        super::miku_procinfo(out.as_mut_ptr() as *mut u8, out.len() as u64)
    };
    if n < 0 { 0 } else { (n as usize).min(out.len()) }
}

/// Path `exec` was given, asked per pid. `None` for kernel threads
pub fn path_of(pid: u64) -> Option<alloc::string::String> {
    let mut buf = [0u8; 256];
    let n = unsafe {
        super::miku_proc_path(pid, buf.as_mut_ptr(), buf.len() as u64)
    };
    if n <= 0 { return None; }
    let n = (n as usize).min(buf.len());
    core::str::from_utf8(&buf[..n]).ok().map(alloc::string::String::from)
}

/// Sets a signal's disposition, for ignoring one outright
pub fn sigdisp(sig: u32, disp: u64) -> Result<(), i64> {
    let r = unsafe { super::miku_sigdisp(sig as u64, disp) };
    if r < 0 { Err(r) } else { Ok(()) }
}
