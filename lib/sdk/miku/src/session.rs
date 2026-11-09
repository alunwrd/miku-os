use super::cred::MAX_GROUPS;

/// Open a session, having already proved the password
pub use miku_abi::kproc::SESSION_OPEN as OPEN;
/// End one
pub use miku_abi::kproc::SESSION_CLOSE as CLOSE;
/// Copy out the open sessions
pub use miku_abi::kproc::SESSION_LIST as LIST;
/// Which session a process is in, or 0 for none
pub use miku_abi::kproc::SESSION_CURRENT as CURRENT;

/// The in-kernel shell gets the identity; nothing starts
pub use miku_abi::kproc::SESSION_KIND_CONSOLE as CONSOLE;
/// Starts the /etc/passwd session program as the user, owning the seat
pub use miku_abi::kproc::SESSION_KIND_GRAPHICAL as GRAPHICAL;

/// The only seat
pub use miku_abi::kproc::SEAT0;

pub use miku_abi::kproc::SessionRequest as Request;

pub use miku_abi::kproc::SessionInfo as Info;

/// Publishes a session for a user whose password was already checked.
/// `leader` is the session process, 0 for console. Returns the id. Performs
/// no authentication; requires `CAP_SETUID`
pub fn open(uid: u16, gid: u16, groups: &[u16], kind: u8, seat: u8, leader: u64)
    -> Result<u32, i64>
{
    let n = groups.len().min(MAX_GROUPS);
    let mut req = Request {
        uid, gid, kind, seat, ngroups: n as u8, _pad: 0,
        groups: [0; MAX_GROUPS],
    };
    req.groups[..n].copy_from_slice(&groups[..n]);
    let r = unsafe {
        super::miku_session(OPEN, leader,
                            &req as *const Request as u64,
                            core::mem::size_of::<Request>() as u64)
    };
    if r < 0 { Err(r) } else { Ok(r as u32) }
}

/// End a session. `0` means the caller's own
pub fn close(id: u32) -> Result<(), i64> {
    let r = unsafe { super::miku_session(CLOSE, id as u64, 0, 0) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Fill `out` with the open sessions, oldest first. Answers how many
pub fn list(out: &mut [Info]) -> usize {
    let r = unsafe {
        super::miku_session(LIST, 0, out.as_mut_ptr() as u64, out.len() as u64)
    };
    if r < 0 { 0 } else { r as usize }
}

/// The caller's own session, or 0 if it leads none
pub fn current() -> u32 {
    current_of(0)
}

/// Session of another process, or 0. Found by the kernel through its parent
/// chain
pub fn current_of(pid: u64) -> u32 {
    let r = unsafe { super::miku_session(CURRENT, pid, 0, 0) };
    if r < 0 { 0 } else { r as u32 }
}
