// Who is logged in, as a thing the kernel knows
//
// A credential on a thread cannot answer:
//
//   - who is at this machine? `who` needs a list, and a graphical session's
//     identity lives on a process the console knows nothing about
//   - who owns the screen and the keyboard? One login may, and the greeter owns
//     them while nobody is logged in. That is a property of a seat
//   - when does it end? Logging out has to reach the processes the login
//     started
//
// So a session is an object: an identity, a seat, and the process that leads
// it. Eight of them, in a fixed array in the style of mikuD's service table; a
// login failing because the table is full fails loudly rather than allocating
// under a lock on the login path.
//
// # The seat
//
// One seat, numbered 0, and named rather than assumed. A seat is the screen and
// the keyboard in front of a person. Naming it makes a second one a change and
// not a rewrite, and costs one field.
//
// # The rule that makes a graphical login safe
//
// `CAP_MAY_PRIV` follows the seat. The greeter holds it as the pre-session
// program: it must draw over everything and take the keyboard. When a session
// opens on that seat the greeter loses it (permitted, effective and bounding)
// and the session's leader gets it.
//
// Taking a capability off a running process is not a new mechanism:
// `mikud::apply_unit_identity` writes another process's credential from PID 1,
// at the same point in a program's life.

use spin::Mutex;

use crate::fs::vfs::types::MAX_GROUPS;
use crate::process::cred::{CapSet, Cred, CAP_MAY_PRIV, CAP_SYS_BOOT};

/// How many logins may be open at once.
///
/// Small: every entry is an identity somebody proved, and a machine out of them
/// has eight people on it or a program opening sessions in a loop
pub const MAX_SESSIONS: usize = 8;

/// The only seat this machine has
pub const SEAT0: u8 = 0;

/// What a session is for
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Kind {
    /// The in-kernel shell wears the identity. There is no leader process:
    /// the shell is a kernel thread and was there before the login
    #[default]
    Console,
    /// A desktop. `leader` is the session program from /etc/passwd, started by
    /// mikudod as the user, and it owns the seat
    Graphical,
}

impl Kind {
    pub fn as_str(self) -> &'static str {
        match self { Kind::Console => "console", Kind::Graphical => "graphical" }
    }
    pub fn from_u8(v: u8) -> Option<Kind> {
        match v { 0 => Some(Kind::Console), 1 => Some(Kind::Graphical), _ => None }
    }
    pub fn as_u8(self) -> u8 {
        match self { Kind::Console => 0, Kind::Graphical => 1 }
    }
}

/// One login
///
/// `Default` is derived so a caller can hand `list` a stack array; the zero
/// value is not a session, and `id == 0` says so
#[derive(Clone, Copy, Default)]
pub struct Session {
    /// Never zero, and never reused while the machine is up: `0` is how a
    /// caller says "mine", and a recycled id would be ambiguous when a process
    /// asks about a session that ended
    pub id: u32,
    pub uid: u16,
    pub gid: u16,
    pub groups: [u16; MAX_GROUPS],
    pub ngroups: u8,
    pub kind: Kind,
    pub seat: u8,
    /// The process the session is, or 0 for a console session
    pub leader: u64,
    pub opened_tick: u64,
}

impl Session {
    /// The credential this session's processes run with.
    ///
    /// Beyond what is below, a session asks mikudo. Root is the exception,
    /// because `logout` hands the console back to root, and a root console that
    /// could not mount a disk could not fix the machine
    pub fn cred(&self) -> Cred {
        if self.uid == 0 {
            return Cred::root();
        }
        Cred {
            uid: self.uid, euid: self.uid, suid: self.uid,
            gid: self.gid, egid: self.gid, sgid: self.gid,
            groups: self.groups,
            ngroups: self.ngroups,
            // Two capabilities and no others. `CAP_MAY_PRIV` is the screen: a
            // session drives the display it was opened on. `CAP_SYS_BOOT` is
            // the power button: whoever sits at this seat can hold the button
            // on the case anyway, and logind grants the same to an active local
            // session.
            //
            // Not CAP_SYS_ADMIN: mounting a disk, partitioning one and starting
            // services stay behind mikudo, which asks for a password and
            // records that it was asked
            permitted: CapSet::empty().with(CAP_MAY_PRIV).with(CAP_SYS_BOOT),
            effective: CapSet::empty().with(CAP_MAY_PRIV).with(CAP_SYS_BOOT),
            bounding:  CapSet::empty().with(CAP_MAY_PRIV).with(CAP_SYS_BOOT),
            umask: 0o022,
        }
    }
}

static TABLE: Mutex<[Option<Session>; MAX_SESSIONS]> = Mutex::new([None; MAX_SESSIONS]);

/// The next id to hand out. Monotonic: see `Session::id`
static NEXT_ID: Mutex<u32> = Mutex::new(1);

/// Which process holds the seat's `CAP_MAY_PRIV` when no session does.
///
/// The greeter: mikuD starts it with that capability from its unit, and it
/// keeps it until somebody logs in. Recorded so opening a session can take it
/// away (see `grant_seat`)
static SEAT_HOLDER: Mutex<[u64; 1]> = Mutex::new([0]);

/// Why a session could not be opened
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenError {
    /// MAX_SESSIONS are already open
    Full,
    /// A graphical session named a leader that is not a process
    NoLeader,
    /// No such seat on this machine
    NoSeat,
}

impl OpenError {
    pub fn as_str(self) -> &'static str {
        match self {
            OpenError::Full => "there are already as many sessions open as this machine allows",
            OpenError::NoLeader => "the session leader is not a running process",
            OpenError::NoSeat => "there is no such seat",
        }
    }
}

/// Records a session that fresh credentials have been checked for, and puts its
/// identity where it belongs.
///
/// The caller (syscall 92, and so mikudod) has verified the password. This only
/// publishes the result.
pub fn open(
    uid: u16, gid: u16, groups: &[u16], kind: Kind, seat: u8, leader: u64,
) -> Result<u32, OpenError> {
    if seat != SEAT0 {
        return Err(OpenError::NoSeat);
    }
    if kind == Kind::Graphical {
        if leader == 0 || unsafe { crate::sched::proc_index_raw(leader) }.is_null() {
            return Err(OpenError::NoLeader);
        }
    }

    let mut gbuf = [0u16; MAX_GROUPS];
    let n = groups.len().min(MAX_GROUPS);
    gbuf[..n].copy_from_slice(&groups[..n]);

    let mut table = TABLE.lock();

    // A second session of the same kind on the same seat replaces the first:
    // logging in at the console twice is one console, and a graphical session
    // arriving while another is open is the greeter used again
    for slot in table.iter_mut() {
        if matches!(slot, Some(s) if s.seat == seat && s.kind == kind) {
            *slot = None;
        }
    }

    let idx = match table.iter().position(|s| s.is_none()) {
        Some(i) => i,
        None => return Err(OpenError::Full),
    };

    let id = {
        let mut next = NEXT_ID.lock();
        let id = *next;
        *next = next.wrapping_add(1).max(1);
        id
    };

    let session = Session {
        id, uid, gid, groups: gbuf, ngroups: n as u8,
        kind, seat, leader,
        opened_tick: crate::arch::x86_64::interrupts::get_tick(),
    };
    table[idx] = Some(session);
    drop(table);

    // Puts the identity where the session's work happens
    //
    // The shell thread is the console session. Every VFS operation takes its
    // credential from the calling thread's PCB, so this write makes `cat` and
    // `ls` honest.
    //
    // Written for a graphical session too: `/bin/mayterm` shows the one console
    // in a window, and every builtin in it would otherwise run as root
    if let Some(pid) = crate::mikud::service_pid("shell").filter(|p| *p != 0) {
        write_cred(pid, session.cred());
    }
    if kind == Kind::Graphical {
        write_cred(leader, session.cred());
    }

    grant_seat(seat, if kind == Kind::Graphical { leader } else { 0 });

    crate::serial_println!(
        "[session] {} session {} opened on seat {} for uid {} ({} groups)",
        kind.as_str(), id, seat, uid, n
    );
    Ok(id)
}

/// Ends a session.
///
/// The console goes back to root, which is what physical access at a ring-0
/// console means. A graphical session's leader is killed: its children are the
/// desktop, which must not outlive the login
pub fn close(id: u32) -> bool {
    let mut table = TABLE.lock();
    let idx = match table.iter().position(|s| matches!(s, Some(s) if s.id == id)) {
        Some(i) => i,
        None => return false,
    };
    let session = table[idx].take().unwrap();
    drop(table);

    // Whichever kind it was, the console goes back to root: `open` set it
    // either way
    if let Some(pid) = crate::mikud::service_pid("shell").filter(|p| *p != 0) {
        write_cred(pid, Cred::root());
    }
    crate::shell::set_session_name("root", 0);
    if session.kind == Kind::Graphical && session.leader != 0 {
        crate::sched::kill(session.leader);
    }

    // Nobody owns the seat now. The greeter takes it back by being restarted,
    // and until then nothing holds CAP_MAY_PRIV, where the compositor's lock
    // keeps the screen shut
    if !seat_has_session(session.seat) {
        grant_seat(session.seat, 0);
    }

    crate::serial_println!("[session] session {} on seat {} closed", id, session.seat);
    true
}

/// Whether any session is open on a seat
pub fn seat_has_session(seat: u8) -> bool {
    TABLE.lock().iter().any(|s| matches!(s, Some(s) if s.seat == seat))
}

/// The session a process belongs to, by leader pid
pub fn of_leader(pid: u64) -> Option<Session> {
    TABLE.lock().iter().flatten().find(|s| s.leader == pid).copied()
}

/// The session a process is in, not the one it leads
///
/// A program in a graphical session is a descendant of the leader, and one
/// started from the console a descendant of the shell thread, so the answer is
/// found by walking the parent chain. Nothing carries a session id on the PCB,
/// since nothing would keep it right across a `set_parent`.
///
/// The walk is bounded, since this runs inside a syscall
pub fn of_process(pid: u64) -> Option<Session> {
    let shell = crate::mikud::service_pid("shell").filter(|p| *p != 0);
    let mut cur = pid;
    for _ in 0..MAX_PARENT_WALK {
        if cur == 0 {
            return None;
        }
        if let Some(s) = of_leader(cur) {
            return Some(s);
        }
        // The console session has no leader process: the shell thread wears the
        // identity, so reaching it is reaching the login.
        //
        // On a graphical boot there is no Console session (the login was the
        // greeter), yet the console carries that session's identity, so a
        // program started from the terminal belongs to it. The fallback is
        // whoever owns the seat, then nobody
        if Some(cur) == shell {
            let table = TABLE.lock();
            if let Some(s) = table.iter().flatten().find(|s| s.kind == Kind::Console) {
                return Some(*s);
            }
            return table.iter().flatten().find(|s| s.seat == SEAT0).copied();
        }
        if cur == 1 {
            return None;
        }
        cur = crate::sched::get_ppid(cur);
    }
    None
}

/// How far up a parent chain `of_process` is willing to look
const MAX_PARENT_WALK: usize = 64;

/// One session by id
pub fn by_id(id: u32) -> Option<Session> {
    TABLE.lock().iter().flatten().find(|s| s.id == id).copied()
}

/// Every open session, oldest first
pub fn list(out: &mut [Session]) -> usize {
    let table = TABLE.lock();
    let mut n = 0;
    for s in table.iter().flatten() {
        if n == out.len() { break; }
        out[n] = *s;
        n += 1;
    }
    drop(table);
    out[..n].sort_unstable_by_key(|s| s.opened_tick);
    n
}

/// A process exited; if it was leading a session, that session is over
///
/// Called from the scheduler's exit path. A desktop that quits (log out in the
/// shell, or maywm died) has to end the login, or the greeter would come back
/// over a session still owning the seat.
///
/// Only a leader closes a session: a program crashing inside a session is a
/// crashed program
pub fn on_process_exit(pid: u64) {
    let id = {
        let table = TABLE.lock();
        table.iter().flatten().find(|s| s.leader == pid).map(|s| s.id)
    };
    if let Some(id) = id {
        crate::serial_println!("[session] leader pid {} exited; closing session {}", pid, id);
        close(id);
    }
}

/// The console sessions only, for `logout`
pub fn list_console(out: &mut [Session]) -> usize {
    let table = TABLE.lock();
    let mut n = 0;
    for s in table.iter().flatten().filter(|s| s.kind == Kind::Console) {
        if n == out.len() { break; }
        out[n] = *s;
        n += 1;
    }
    n
}

/// Whoever holds a seat's display authority, session or greeter
pub fn seat_holder(seat: u8) -> u64 {
    if seat as usize >= SEAT_HOLDER.lock().len() { return 0; }
    SEAT_HOLDER.lock()[seat as usize]
}

/// Moves a seat's `CAP_MAY_PRIV` to `pid`, taking it from whoever had it
///
/// `pid == 0` means nobody holds it, the state between a session ending and the
/// greeter coming back. The capability is removed from the bounding set as
/// well, so it cannot be put back.
///
/// A pid that has exited is skipped: the greeter usually exits on its own once
/// the session it asked for exists
pub fn grant_seat(seat: u8, pid: u64) {
    if seat as usize >= SEAT_HOLDER.lock().len() {
        return;
    }
    let previous = {
        let mut holder = SEAT_HOLDER.lock();
        let previous = holder[seat as usize];
        holder[seat as usize] = pid;
        previous
    };

    if previous != 0 && previous != pid {
        let ptr = unsafe { crate::sched::proc_index_raw(previous) };
        if !ptr.is_null() {
            {
                let mut cred = unsafe { (*ptr).cred.write() };
                cred.permitted = cred.permitted.without(CAP_MAY_PRIV);
                cred.effective = cred.effective.without(CAP_MAY_PRIV);
                cred.bounding  = cred.bounding.without(CAP_MAY_PRIV);
            }
            // Outside the guard. A serial line runs with interrupts off and
            // spins on the port for milliseconds, and every syscall that
            // checks a credential reads this lock
            crate::serial_println!(
                "[session] seat {} taken from pid {} - it no longer holds may_priv", seat, previous
            );
        }
    }
}

/// Gives a process the seat's display authority without opening a session
///
/// The greeter only: it runs before there is anybody to be, and cannot draw
/// without this. mikuD calls it after starting the service: the unit's
/// `Capabilities=` grants the capability, and this records who has it so the
/// next login can take it away
pub fn hold_seat(seat: u8, pid: u64) {
    if seat_has_session(seat) {
        crate::serial_println!(
            "[session] refusing to give seat {} to pid {}: a session owns it", seat, pid
        );
        return;
    }
    grant_seat(seat, pid);
}

/// Writes a credential onto another process
///
/// The one unsafe act in this file, the same mikuD performs when applying a
/// unit's `User=`. A null index means the process is gone, the ordinary case on
/// the close path
fn write_cred(pid: u64, cred: Cred) {
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() {
        return;
    }
    unsafe { *(*ptr).cred.write() = cred };
}
