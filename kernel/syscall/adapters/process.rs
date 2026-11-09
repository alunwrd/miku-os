// Process-management syscalls: exit, sleep, uptime, fork, wait4, kill, exec

extern crate alloc;

use crate::syscall::errno::{err, EACCES, EAGAIN, ECHILD, EFAULT, EINTR, EINVAL, ENOENT, ENOMEM, EPERM, ESRCH};
use crate::syscall::usercopy::{current_cr3, current_pid, read_user_argv, read_user_path, user_ptr_writable};
use crate::mm::vmm::AddressSpace;

const WNOHANG: u64 = miku_abi::wait::WNOHANG as u64;
/// Reports a child that stopped, as well as one that exited
const WUNTRACED: u64 = miku_abi::wait::WUNTRACED as u64;

// What a status word says
//
// POSIX's layout, which `WIFEXITED` and its neighbours in `lib/sdk/miku` take
// apart:
//
//   exited     code << 8
//   signalled  sig                (the low seven bits)
//   stopped    0x7f | (sig << 8)
//
// Shared with `docs/MikuOS_ABI.md`, `lib/runtime/libmiku`, `lib/sdk/miku`, msh, mikudod and
// mikuD's restart rule

/// The exit code a process recorded, as a status word
///
/// Takes the signal from the record: `kill_with_code` stores `128 + sig`, the
/// convention a shell prints, and `exit(148)` stores the same value as death by
/// SIGTSTP
fn status_of_exit(pid: u64, code: u64) -> u64 {
    match crate::sched::exit_signal_of(pid) {
        Some(sig) => sig as u64 & 0x7f,
        None => (code & 0xff) << 8,
    }
}

/// A stop, as a status word
fn status_of_stop(sig: u32) -> u64 {
    0x7f | ((sig as u64) << 8)
}

/// Ticks `wait4` sleeps for when no death wakes it (4 ms each)
///
/// The same 100 ms as `sched::waitpid`: a wakeup can be missed
const WAIT4_BACKSTOP: u64 = 25;

// Identity and capability syscalls.
//
// The POSIX saved-set-id model, with capabilities standing in for the "is root"
// test. A login program or an elevation broker drops to a user irreversibly, or
// switches effective identity and comes back.

use crate::process::cred::{self, Cred, CAP_SETGID, CAP_SETUID, CAP_SYS_TIME};
use crate::fs::vfs::types::MAX_GROUPS;

// 47  umask(mask) -> previous_mask
pub fn sys_umask(mask: u64) -> u64 {
    let new_mask = (mask as u16) & 0o777;
    let pid = current_pid();
    let old = crate::sched::umask_of(pid);
    crate::sched::set_current_umask(new_mask);
    old as u64
}

// 48  getuid() -> uid
pub fn sys_getuid() -> u64 { crate::sched::cred_of(current_pid()).uid as u64 }

// 49  getgid() -> gid
pub fn sys_getgid() -> u64 { crate::sched::cred_of(current_pid()).gid as u64 }

// 50  geteuid() -> euid
pub fn sys_geteuid() -> u64 { crate::sched::cred_of(current_pid()).euid as u64 }

// 51  getegid() -> egid
pub fn sys_getegid() -> u64 { crate::sched::cred_of(current_pid()).egid as u64 }

/// Reads an id argument that arrived from ring 3.
///
/// An id that does not fit is refused, not folded: `setuid(0x10000)` cut to 16
/// bits stores uid 0, which is CVE-2019-14287 reached by arithmetic
#[inline]
fn arg_id(raw: u64) -> Result<u16, u64> {
    cred::checked_id(raw).ok_or_else(|| err(EINVAL))
}

/// `0xFFFF` means "leave this one alone", matching sys_chown's sentinel. Used
/// by setresuid/setresgid, where a caller usually wants to move one of three
#[inline]
fn arg_id_opt(raw: u64) -> Result<Option<u16>, u64> {
    if raw == 0xFFFF || raw == u64::MAX { return Ok(None); }
    arg_id(raw).map(Some)
}

// 52  setuid(uid) -> 0/-errno
//
// With CAP_SETUID: all three ids move, and the capability set is dropped when
// the target is not root, which is how a broker becomes the user for good.
//
// Without it: POSIX's restricted form, effective id only, and only to one of
// the ids already held
pub fn sys_setuid(uid: u64) -> u64 {
    let u = match arg_id(uid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if c.has(CAP_SETUID) {
            c.uid  = u;
            c.euid = u;
            c.suid = u;
            if u != 0 {
                c.drop_all_caps();
            }
        } else if c.may_become_euid(u) {
            c.euid = u;
        } else {
            out = err(EPERM);
        }
    });
    if !ok { return err(ESRCH); }
    out
}

// 53  setgid(gid) -> 0/-errno
pub fn sys_setgid(gid: u64) -> u64 {
    let g = match arg_id(gid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if c.has(CAP_SETGID) {
            c.gid  = g;
            c.egid = g;
            c.sgid = g;
        } else if c.may_become_egid(g) {
            c.egid = g;
        } else {
            out = err(EPERM);
        }
    });
    if !ok { return err(ESRCH); }
    out
}

// 54  seteuid(euid) -> 0/-errno
//
// Effective id only. Reversible: suid keeps the id to come back to, so a
// process may lower itself for one operation and raise itself again without any
// capability
pub fn sys_seteuid(euid: u64) -> u64 {
    let u = match arg_id(euid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if c.has(CAP_SETUID) || c.may_become_euid(u) {
            c.euid = u;
        } else {
            out = err(EPERM);
        }
    });
    if !ok { return err(ESRCH); }
    out
}

// 55  setegid(egid) -> 0/-errno
pub fn sys_setegid(egid: u64) -> u64 {
    let g = match arg_id(egid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if c.has(CAP_SETGID) || c.may_become_egid(g) {
            c.egid = g;
        } else {
            out = err(EPERM);
        }
    });
    if !ok { return err(ESRCH); }
    out
}

// 86  setresuid(ruid, euid, suid) -> 0/-errno
//
// The form an elevation broker wants: all three at once, with no half-built
// identity, failing as a whole. 0xFFFF leaves a field alone.
//
// Without CAP_SETUID each target must be one of the three ids already held, so
// this can rearrange an identity but not invent one
pub fn sys_setresuid(ruid: u64, euid: u64, suid: u64) -> u64 {
    let r = match arg_id_opt(ruid) { Ok(v) => v, Err(e) => return e };
    let e = match arg_id_opt(euid) { Ok(v) => v, Err(e) => return e };
    let s = match arg_id_opt(suid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        let privileged = c.has(CAP_SETUID);
        if !privileged {
            for t in [r, e, s].into_iter().flatten() {
                if !c.may_become_euid(t) {
                    out = err(EPERM);
                    return;
                }
            }
        }
        let new_r = r.unwrap_or(c.uid);
        let new_e = e.unwrap_or(c.euid);
        let new_s = s.unwrap_or(c.suid);
        c.uid  = new_r;
        c.euid = new_e;
        c.suid = new_s;
        // Same reasoning as setuid: a privileged process that has put root
        // behind it in every one of the three ids keeps no authority
        if privileged && new_r != 0 && new_e != 0 && new_s != 0 {
            c.drop_all_caps();
        }
    });
    if !ok { return err(ESRCH); }
    out
}

// 87  setresgid(rgid, egid, sgid) -> 0/-errno
pub fn sys_setresgid(rgid: u64, egid: u64, sgid: u64) -> u64 {
    let r = match arg_id_opt(rgid) { Ok(v) => v, Err(e) => return e };
    let e = match arg_id_opt(egid) { Ok(v) => v, Err(e) => return e };
    let s = match arg_id_opt(sgid) { Ok(v) => v, Err(e) => return e };
    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if !c.has(CAP_SETGID) {
            for t in [r, e, s].into_iter().flatten() {
                if !c.may_become_egid(t) {
                    out = err(EPERM);
                    return;
                }
            }
        }
        c.gid  = r.unwrap_or(c.gid);
        c.egid = e.unwrap_or(c.egid);
        c.sgid = s.unwrap_or(c.sgid);
    });
    if !ok { return err(ESRCH); }
    out
}

// 88  setgroups(list, count) -> 0/-errno
//
// Needs CAP_SETGID: supplementary groups grant access as a primary group does,
// and a process could otherwise add itself to `wheel`
pub fn sys_setgroups(list_ptr: u64, count: u64) -> u64 {
    if count as usize > MAX_GROUPS { return err(EINVAL); }
    if !crate::sched::current_has_cap(CAP_SETGID) { return err(EPERM); }

    let n = count as usize;
    let mut buf = [0u16; MAX_GROUPS];
    if n > 0 {
        let cr3  = current_cr3();
        let size = (n * 2) as u64;
        let _ua = match crate::syscall::usercopy::user_ptr_mapped(cr3, list_ptr, size) {
            Some(g) => g,
            None    => return err(EFAULT),
        };
        for i in 0..n {
            // read_unaligned: the array came from ring 3 and need not be
            // aligned to its element type
            let raw = unsafe { (list_ptr as *const u16).add(i).read_unaligned() };
            if raw > cred::ID_MAX { return err(EINVAL); }
            buf[i] = raw;
        }
    }

    if !crate::sched::update_current_cred(|c| c.set_groups(&buf[..n])) {
        return err(ESRCH);
    }
    0
}

// 89  getgroups(list, max) -> count / -errno
//
// max == 0 asks how many there are without writing anything, which is how a
// caller sizes its buffer
pub fn sys_getgroups(list_ptr: u64, max: u64) -> u64 {
    let c = crate::sched::cred_of(current_pid());
    let n = (c.ngroups as usize).min(MAX_GROUPS);
    if max == 0 { return n as u64; }
    if (max as usize) < n { return err(EINVAL); }

    let cr3  = current_cr3();
    let size = (n * 2) as u64;
    if n > 0 {
        let _ua = match user_ptr_writable(cr3, list_ptr, size) {
            Some(g) => g,
            None    => return err(EFAULT),
        };
        for i in 0..n {
            unsafe { (list_ptr as *mut u16).add(i).write_unaligned(c.groups[i]) };
        }
    }
    n as u64
}

// 90  capget(buf) -> 0/-errno
//
// Writes u64[3]: permitted, effective, bounding
pub fn sys_capget(buf: u64) -> u64 {
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf, 24) {
        Some(g) => g,
        None    => return err(EFAULT),
    };
    let c = crate::sched::cred_of(current_pid());
    unsafe {
        let p = buf as *mut u64;
        p.add(0).write_unaligned(c.permitted.0);
        p.add(1).write_unaligned(c.effective.0);
        p.add(2).write_unaligned(c.bounding.0);
    }
    0
}

// 91  capset(permitted, effective, bounding) -> 0/-errno
//
// Only narrows: each requested set must be a subset of the one held, so a
// privileged service sheds authority it does not need.
//
// The bounding set survives exec and fork, so a process can confine its whole
// subtree before starting anything
pub fn sys_capset(permitted: u64, effective: u64, bounding: u64) -> u64 {
    use crate::process::cred::CapSet;
    let want_p = CapSet(permitted).sanitized();
    let want_e = CapSet(effective).sanitized();
    let want_b = CapSet(bounding).sanitized();

    let mut out = 0u64;
    let ok = crate::sched::update_current_cred(|c| {
        if !want_p.subset_of(c.permitted)
            || !want_b.subset_of(c.bounding)
            || !want_e.subset_of(want_p)
        {
            out = err(EPERM);
            return;
        }
        c.permitted = want_p;
        c.effective = want_e;
        c.bounding  = want_b;
        // A capability outside the new ceiling cannot stay held
        c.permitted = c.permitted.intersect(c.bounding);
        c.effective = c.effective.intersect(c.bounding);
    });
    if !ok { return err(ESRCH); }
    out
}

// 92  session(op, arg, ptr, len) -> id / count / 0 / -errno
//
// Gives the console's shell the identity of the person now at it.
//
// # The console
//
// The command interpreter is in ring 0, because its commands reach kernel
// internals with no syscall equivalent. Every VFS operation goes through
// `with_vfs`, which calls `sync_ctx_from_current` and takes the credential from
// the calling thread's PCB, so the shell's commands are checked against whoever
// the shell thread is.
//
// # Why the caller is not the target
//
// `/bin/login` runs in ring 3, because verifying an Argon2id hash wants 19 MiB
// of heap. Dropping its own privilege would change nothing about the console,
// so it authenticates, calls this, and exits.
//
// # What it grants
//
// The user's uid, gid and groups, and the capabilities in
// `session::Session::cred`: CAP_MAY_PRIV, without which starting the desktop
// fails at the handshake because maywm, mayterm and aosh each bind a privileged
// global, and CAP_SYS_BOOT. The window manager may command the compositor and
// may not read /etc/shadow.
//
// # Four operations
//
// A graphical login needs facts a credential on a thread cannot hold (which
// seat, which process leads the session, what to take away when it ends), so
// the session is an object (kernel/process/session.rs) and this is the door to
// it.
//
// OPEN and CLOSE require CAP_SETUID, so no program can declare who is logged
// in. LIST and CURRENT require nothing: they are `who`.

/// Open a session. `arg` is the leader pid (0 for a console session)
pub use miku_abi::kproc::SESSION_OPEN;
/// End one. `arg` is the id, or 0 for the caller's own
pub use miku_abi::kproc::SESSION_CLOSE;
/// Copy out the open sessions. `ptr`/`len` are an array of `SessionInfo`
pub use miku_abi::kproc::SESSION_LIST;
/// The session a process is in. `arg` is a pid, or 0 for the caller. 0 back
/// means "in no session"
pub use miku_abi::kproc::SESSION_CURRENT;

pub use miku_abi::kproc::SessionRequest;

pub use miku_abi::kproc::SessionInfo;

/// Stops the machine. `action` is 0 for off, 1 for reboot
///
/// `CAP_SYS_BOOT` and nothing else. A session holds it (see
/// `process::session::Session::cred`), so a desktop can offer the menu; an
/// ordinary program started from that session does not, because `spawn::run`
/// drops what it does not need.
///
/// Both paths go through mikuD when it is running, which stops the services in
/// order and syncs, as `poweroff` at the console does. Nothing returns on
/// success
pub fn sys_power(action: u64) -> u64 {
    use crate::process::cred::CAP_SYS_BOOT;
    if !crate::sched::current_has_cap(CAP_SYS_BOOT) {
        return err(EPERM);
    }
    let reboot = match action {
        0 => false,
        1 => true,
        _ => return err(EINVAL),
    };
    crate::serial_println!(
        "[power] pid {} asked to {}", crate::sched::current_pid(),
        if reboot { "reboot" } else { "power off" });
    if crate::mikud::is_running() {
        if reboot { crate::mikud::reboot() } else { crate::mikud::poweroff() }
    } else if reboot {
        crate::kcore::power::reboot();
    } else {
        crate::kcore::power::shutdown();
    }
    0
}

pub fn sys_session(op: u64, arg: u64, ptr: u64, len: u64) -> u64 {
    use crate::process::session::{self, Kind};

    match op {
        SESSION_OPEN => {
            if !crate::sched::current_has_cap(CAP_SETUID) {
                return err(EPERM);
            }
            let size = core::mem::size_of::<SessionRequest>() as u64;
            if len != size {
                return err(EINVAL);
            }
            let cr3 = current_cr3();
            let _ua = match crate::syscall::usercopy::user_ptr_mapped(cr3, ptr, size) {
                Some(gd) => gd,
                None => return err(EFAULT),
            };
            let req = unsafe { (ptr as *const SessionRequest).read_unaligned() };

            // Every id crossing this boundary is range-checked rather than
            // truncated. CVE-2019-14287 was `sudo -u#-1`: the id parsed to
            // (uid_t)-1, setuid returned success, and nothing changed
            if req.uid > cred::ID_MAX || req.gid > cred::ID_MAX {
                return err(EINVAL);
            }
            let n = req.ngroups as usize;
            if n > MAX_GROUPS {
                return err(EINVAL);
            }
            for g in &req.groups[..n] {
                if *g > cred::ID_MAX {
                    return err(EINVAL);
                }
            }
            let kind = match Kind::from_u8(req.kind) {
                Some(k) => k,
                None => return err(EINVAL),
            };

            match session::open(req.uid, req.gid, &req.groups[..n], kind, req.seat, arg) {
                Ok(id) => {
                    // The prompt's name, resolved here rather than in prompt():
                    // a filesystem read on the prompt path deadlocks, since
                    // reprompt() runs from the console's print path, and some
                    // of that printing is inside a with_vfs closure whose lock
                    // is not reentrant
                    //
                    // Both kinds, because `session::open` sets the console for
                    // both: the desktop shows the one console in a window
                    let mut namebuf = [0u8; 33];
                    let name = passwd_name(req.uid, &mut namebuf)
                        .unwrap_or(if req.uid == 0 { "root" } else { "?" });
                    crate::shell::set_session_name(name, req.uid);
                    id as u64
                }
                Err(e) => {
                    crate::serial_println!("[session] open refused: {}", e.as_str());
                    err(EINVAL)
                }
            }
        }

        SESSION_CLOSE => {
            if !crate::sched::current_has_cap(CAP_SETUID) {
                return err(EPERM);
            }
            let id = if arg == 0 {
                match session::of_leader(current_pid()) {
                    Some(s) => s.id,
                    None => return err(ESRCH),
                }
            } else {
                arg as u32
            };
            if session::close(id) { 0 } else { err(ESRCH) }
        }

        SESSION_LIST => {
            let max = len as usize;
            if max == 0 {
                return 0;
            }
            let bytes = (max * core::mem::size_of::<SessionInfo>()) as u64;
            // Held for the whole copy: the guard is what keeps the pages
            // present and writable while the loop below is writing into them
            let _ua = match user_ptr_writable(current_cr3(), ptr, bytes) {
                Some(gd) => gd,
                None => return err(EFAULT),
            };
            let mut buf = [crate::process::session::Session {
                id: 0, uid: 0, gid: 0, groups: [0; MAX_GROUPS], ngroups: 0,
                kind: Kind::Console, seat: 0, leader: 0, opened_tick: 0,
            }; crate::process::session::MAX_SESSIONS];
            let n = session::list(&mut buf).min(max);
            for (i, s) in buf[..n].iter().enumerate() {
                let info = SessionInfo {
                    id: s.id, uid: s.uid, gid: s.gid,
                    kind: s.kind.as_u8(), seat: s.seat, _pad: [0; 2],
                    leader: s.leader, opened_tick: s.opened_tick,
                };
                unsafe { (ptr as *mut SessionInfo).add(i).write_unaligned(info) };
            }
            n as u64
        }

        // `arg` is the pid to ask about, or 0 for the caller. mikudod asks
        // about another process on every request: a token it remembers is keyed
        // to the login the client is in, and mikudod is PID 1's child.
        // SESSION_LIST hands out every leader pid anyway
        SESSION_CURRENT => {
            let pid = if arg == 0 { current_pid() } else { arg };
            match session::of_process(pid) {
                Some(s) => s.id as u64,
                None => 0,
            }
        }

        _ => err(EINVAL),
    }
}

/// The fields of the account line for a uid, from /etc/passwd: name, password,
/// uid, gid, description, home, program.
///
/// Called on ordinary syscall and shell paths with no VFS lock held.
/// Hand-rolled rather than `mikuauth::db`: the kernel needs a field or two of
/// one line
fn passwd_fields(uid: u16) -> Option<alloc::vec::Vec<alloc::string::String>> {
    let text = crate::fs::vfs_read::read_file("/etc/passwd")?;
    let text = core::str::from_utf8(&text).ok()?;
    for line in text.lines() {
        if line.starts_with('#') { continue; }
        let f: alloc::vec::Vec<&str> = line.split(':').collect();
        if f.len() < 3 || f[0].is_empty() { continue; }
        if f[2].parse::<u16>().ok() != Some(uid) { continue; }
        return Some(f.iter().map(|s| alloc::string::String::from(*s)).collect());
    }
    None
}

/// The login name for a uid. Used by `sys_session`
pub fn passwd_name<'a>(uid: u16, out: &'a mut [u8; 33]) -> Option<&'a str> {
    let f = passwd_fields(uid)?;
    let name = f[0].as_bytes();
    let n = name.len().min(out.len());
    out[..n].copy_from_slice(&name[..n]);
    core::str::from_utf8(&out[..n]).ok()
}

/// A uid's login name and home directory, for the environment of a program
/// the kernel's shell starts
pub fn passwd_name_home(uid: u16) -> Option<(alloc::string::String, alloc::string::String)> {
    let mut f = passwd_fields(uid)?;
    let home = if f.len() > 5 && !f[5].is_empty() { f.swap_remove(5) } else { "/".into() };
    Some((f.swap_remove(0), home))
}

/// The uid and gid of a named account, from /etc/passwd
///
/// The other direction of `passwd_name`: mikuD resolves a unit's `User=` before
/// the service has run an instruction, long before ring 3 could be asked
pub fn passwd_ids(name: &str) -> Option<(u16, u16)> {
    if name.is_empty() {
        return None;
    }
    let text = crate::fs::vfs_read::read_file("/etc/passwd")?;
    let text = core::str::from_utf8(&text).ok()?;
    for line in text.lines() {
        if line.starts_with('#') { continue; }
        let mut f = line.split(':');
        if f.next()? != name { continue; }
        let _pw = f.next();
        let uid: u16 = match f.next()?.parse() { Ok(v) => v, Err(_) => continue };
        let gid: u16 = match f.next()?.parse() { Ok(v) => v, Err(_) => continue };
        // The range check the syscall makes. A passwd file with 65535 in it
        // would otherwise become chown's "leave alone" sentinel
        if uid > cred::ID_MAX || gid > cred::ID_MAX {
            return None;
        }
        return Some((uid, gid));
    }
    None
}

// 0  exit(code) -> never returns to caller
pub fn sys_exit(code: u64) -> u64 {
    let pid  = current_pid();
    // The process, not the task. `exit` means this program is over, and every
    // program in /bin assumes so. `thread_exit`(107) ends only a thread
    let tgid = crate::sched::tgid_of(pid);
    crate::trace_println!("[syscall] exit pid={} tgid={} code={}", pid, tgid, code);
    // The threads first, then the descriptors, as Linux orders `exit_group`:
    // otherwise a `poll` woken by the close answers EBADF into a program meant
    // to be over (`sched::stop_if_dead` for the ones already running).
    //
    // All but this task and the leader: closing the descriptors may wait for
    // the VFS lock, and a dead task that waits is never resumed
    // (`sched::kill_group_threads`). The leader stays as the process a parent's
    // `wait4` would otherwise find dead and reap from under this
    crate::sched::kill_group_threads(tgid, pid, code);
    // Descriptors go now, not at reap time: a zombie holding a pipe's write end
    // keeps every reader blocked, and the parent that would reap it may be
    // waiting on that reader. Keyed on the process
    crate::fs::vfs::core::release_fds_on_exit(tgid);
    // And now the rest, this task with them: nothing after this line waits
    crate::sched::kill_group_with_code(tgid, code);
    crate::process::signal::send_sigchld(tgid);
    crate::sched::yield_now();
    0
}

// 16  sleep(ticks) -> 0
pub fn sys_sleep(ticks: u64) -> u64 {
    if ticks == 0 {
        crate::sched::yield_now();
        return 0;
    }
    let clamped = ticks.min(100_000);
    crate::sched::sleep(clamped);
    0
}

// 122  sleep_ns(ns) -> 0
//
// What `nanosleep` means: this long, to the timer's precision (a few
// microseconds), not up to the next 4 ms tick
pub fn sys_sleep_ns(ns: u64) -> u64 {
    if ns == 0 {
        crate::sched::yield_now();
        return 0;
    }
    // A day: past that a program is asleep by mistake, and the deadline
    // arithmetic stays far from overflow
    crate::sched::sleep_ns(ns.min(86_400_000_000_000));
    0
}

// 17  uptime() -> ticks
pub fn sys_uptime() -> u64 {
    crate::arch::x86_64::interrupts::get_tick()
}

// 70  clock_gettime(clock_id, timespec_ptr) -> 0
//
// Two clocks:
//   0 = CLOCK_REALTIME   seconds since the Unix epoch (RTC, refined by NTP)
//   1 = CLOCK_MONOTONIC  seconds since boot, never jumps when NTP corrects
//
// CLOCK_MONOTONIC is the TSC to the nanosecond, the reading and arithmetic of
// the vDSO. CLOCK_REALTIME is anchored to a tick (4 ms at 250 Hz); tv_nsec is
// filled from the tick remainder, and the vDSO's realtime is the precise one
pub const CLOCK_REALTIME: u64 = miku_abi::time::CLOCK_REALTIME as u64;
pub const CLOCK_MONOTONIC: u64 = miku_abi::time::CLOCK_MONOTONIC as u64;

pub fn sys_clock_gettime(clock_id: u64, ts_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    // struct timespec { i64 tv_sec; i64 tv_nsec; }
    const TIMESPEC_SIZE: u64 = 16;
    let _ua = match crate::syscall::usercopy::user_ptr_writable(cr3, ts_ptr, TIMESPEC_SIZE) {
        Some(g) => g,
        None => return err(EFAULT),
    };

    let (secs, nanos) = match clock_id {
        CLOCK_REALTIME => {
            if !crate::kcore::clock::is_set() {
                return err(EINVAL);
            }
            (crate::kcore::clock::now_unix(), crate::kcore::clock::now_nanos_frac())
        }
        // The TSC, as the vDSO reads it: this is its fallback, and the two must
        // agree
        CLOCK_MONOTONIC => {
            let ns = crate::kcore::time::now_ns();
            (ns / 1_000_000_000, ns % 1_000_000_000)
        }
        _ => return err(EINVAL),
    };

    unsafe {
        let p = ts_ptr as *mut i64;
        p.write_unaligned(secs as i64);
        p.add(1).write_unaligned(nanos as i64);
    }
    0
}

// 79  clock_settime(clock_id, timespec_ptr) -> 0
//
// Root-only, because the wall clock is the machine's: certificate validity,
// file timestamps and every log line are read against it. CLOCK_MONOTONIC
// cannot be set.
//
// Nanoseconds are accepted and ignored: the epoch is anchored to a tick
pub fn sys_clock_settime(clock_id: u64, ts_ptr: u64) -> u64 {
    if clock_id != CLOCK_REALTIME {
        return err(EINVAL);
    }
    if !crate::sched::current_has_cap(CAP_SYS_TIME) {
        return err(EPERM);
    }
    let cr3 = current_cr3();
    const TIMESPEC_SIZE: u64 = 16;
    let _ua = match crate::syscall::usercopy::user_ptr_mapped(cr3, ts_ptr, TIMESPEC_SIZE) {
        Some(g) => g,
        None => return err(EFAULT),
    };
    let secs = unsafe { (ts_ptr as *const i64).read_unaligned() };
    if secs < 0 {
        return err(EINVAL);
    }
    crate::fs::procfs::set_wall_clock(secs as u64);
    0
}

// 78  getrandom(buf, len) -> bytes written
//
// Userspace's PRNG is a xorshift seeded from uptime and pid, useless for a key.
//
// Backed by kcore::random: RDRAND where the CPU has it and a TSC mix where it
// does not. Never blocks and never returns short: there is no entropy pool to
// drain
pub const GETRANDOM_MAX: u64 = 1024;

pub fn sys_getrandom(buf: u64, len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    if len > GETRANDOM_MAX {
        return err(EINVAL);
    }
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf, len) {
        Some(g) => g,
        None => return err(EFAULT),
    };

    let mut written: u64 = 0;
    while written < len {
        let word = crate::kcore::random::random_u64().to_ne_bytes();
        let take = core::cmp::min(8, (len - written) as usize);
        unsafe {
            core::ptr::copy_nonoverlapping(
                word.as_ptr(),
                (buf + written) as *mut u8,
                take,
            );
        }
        written += take as u64;
    }
    written
}

// 43  fork() -> child_pid (parent) / 0 (child)
pub fn sys_fork() -> u64 {
    let cr3 = current_cr3();
    let pid = current_pid();

    // Cannot fork kernel threads
    if cr3 == crate::mm::vmm::kernel_cr3() {
        return err(EPERM);
    }

    // The syscall_handler prologue pushes (in order from the kernel stack top):
    // rcx, r11, rbp, rbx, r12, r13, r14, r15, r10, r9, r8. The kernel stack top
    // lives at gs:[0x10] (percpu::Cpu::kernel_rsp) and the saved user RSP at
    // gs:[0x18] (percpu::Cpu::user_rsp). See 'syscall_handler' in
    // syscall/mod.rs and 'Cpu' in arch/x86_64/percpu.rs: those three stay in
    // lock-step
    let kernel_stack_top: u64;
    let user_rsp: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) kernel_stack_top);
        core::arch::asm!("mov {}, gs:[0x18]", out(reg) user_rsp);
    }
    let user_rip    = unsafe { *((kernel_stack_top - 8)  as *const u64) }; // saved rcx
    let user_rflags = unsafe { *((kernel_stack_top - 16) as *const u64) }; // saved r11

    let saved = crate::process::SavedSyscallRegs {
        rbp: unsafe { *((kernel_stack_top - 24) as *const u64) },
        rbx: unsafe { *((kernel_stack_top - 32) as *const u64) },
        r12: unsafe { *((kernel_stack_top - 40) as *const u64) },
        r13: unsafe { *((kernel_stack_top - 48) as *const u64) },
        r14: unsafe { *((kernel_stack_top - 56) as *const u64) },
        r15: unsafe { *((kernel_stack_top - 64) as *const u64) },
        r10: unsafe { *((kernel_stack_top - 72) as *const u64) },
        r9:  unsafe { *((kernel_stack_top - 80) as *const u64) },
        r8:  unsafe { *((kernel_stack_top - 88) as *const u64) },
        rdi: unsafe { *((kernel_stack_top - 96) as *const u64) },
        rsi: unsafe { *((kernel_stack_top - 104) as *const u64) },
        rdx: unsafe { *((kernel_stack_top - 112) as *const u64) },
    };

    // Clone address space with COW
    let parent_aspace = AddressSpace::from_raw(cr3);
    let child_aspace = match parent_aspace.clone_cow() {
        Some(a) => a,
        None => {
            let _ = parent_aspace.into_raw();
            return err(ENOMEM);
        }
    };
    let _ = parent_aspace.into_raw();

    let child_cr3 = child_aspace.into_raw();

    // A pool's window was left out of the child's page tables, and goes out of
    // its VMA table with them (`mm::vmm::PTE_LENT`)
    let windows = crate::mayland::server::shm::windows_in(cr3);
    crate::mm::mmap::vma_clone(cr3, child_cr3, &windows);
    let parent_brk = crate::mm::mmap::sys_brk(cr3, 0);

    // The parent of the child is the process, not the calling task: a child
    // whose ppid named a thread would be unreapable once that thread ended
    let parent = crate::sched::tgid_of(pid);
    let child = match crate::process::Process::new_fork(
        parent,
        child_cr3,
        None, // user_stack_phys is COW-shared, not separately tracked
        parent_brk,
        user_rip,
        user_rsp,
        user_rflags,
        &saved,
    ) {
        Some(c) => c,
        None => {
            // Out of pids. The address space is built and ours to undo, or a
            // failed fork costs a whole page table each time
            crate::mm::mmap::vma_cleanup(child_cr3);
            let mut a = AddressSpace::from_raw(child_cr3);
            a.free_address_space();
            return err(EAGAIN);
        }
    };
    let child_pid = child.pid;

    // Child inherits the signal dispatch entry (same code mapping); the
    // pending set starts empty
    unsafe {
        let pptr = crate::sched::proc_index_raw(parent);
        if !pptr.is_null() {
            child.sig_entry.store(
                (*pptr).sig_entry.load(core::sync::atomic::Ordering::Relaxed),
                core::sync::atomic::Ordering::Relaxed,
            );
        }
    }

    // Per-process FD table: clone parent's so the child inherits the
    // same descriptors at the same numeric slots
    crate::fs::vfs::core::with_vfs(|vfs| vfs.fork_fds(parent, child_pid));

    crate::trace_println!("[fork] parent={} child={} cr3={:#x}", pid, child_pid, child_cr3);

    crate::sched::add_user_process(child);
    child_pid
}

// 44  wait4(pid, status_ptr, options) -> child_pid / -errno
pub fn sys_wait4(target_pid: u64, status_ptr: u64, options: u64) -> u64 {
    // Children belong to the process: `fork` records the caller's thread group
    // as the parent, so any thread can wait. Signals are the thread's own, and
    // so is the check below that one interrupts the wait
    let me = current_pid();
    let my_pid = crate::sched::tgid_of(me);
    let cr3 = current_cr3();
    let death = [&crate::sched::wait::DEATH];
    let mut joined = None;

    loop {
        // A stopped child first, only when asked for: a shell not passing
        // WUNTRACED would take the status for an exit. Before the zombie check
        // because a stop is news now, and a zombie will still be a zombie next
        // time
        if options & WUNTRACED != 0 {
            if let Some((child_pid, sig)) = crate::sched::find_stopped_child(my_pid, target_pid) {
                if status_ptr != 0 {
                    if let Some(_ua) = user_ptr_writable(cr3, status_ptr, 8) {
                        unsafe {
                            (status_ptr as *mut u64).write_unaligned(status_of_stop(sig));
                        }
                    }
                }
                // Not reaped: it is still running, holds its address space, and
                // expects to be continued
                return child_pid;
            }
        }

        let found = crate::sched::find_zombie_child(my_pid, target_pid);
        match found {
            Some((child_pid, exit_code)) => {
                if status_ptr != 0 {
                    if let Some(_ua) = user_ptr_writable(cr3, status_ptr, 8) {
                        unsafe {
                            // write_unaligned: status_ptr is user-supplied and
                            // need not be aligned
                            (status_ptr as *mut u64).write_unaligned(status_of_exit(child_pid, exit_code));
                        }
                    } else {
                        crate::serial_println!(
                            "[wait4] status_ptr={:#x} not writable (cr3={:#x})",
                            status_ptr, cr3
                        );
                    }
                }
                crate::sched::reap_zombie(child_pid);
                return child_pid;
            }
            None => {
                if !crate::sched::has_children(my_pid) {
                    return err(ECHILD);
                }
                if options & WNOHANG != 0 {
                    return 0; // non-blocking, no zombie yet
                }
                // A deliverable signal interrupts the wait (POSIX EINTR);
                // the dispatch tail injects the handler on the way out
                if crate::process::signal::has_deliverable(me) {
                    return err(EINTR);
                }
                // Off the run queue until a child dies, on the queue
                // `sched::waitpid` uses and with the same backstop.
                //
                // A child exiting on another CPU between `find_zombie_child`
                // answering None above and the state being recorded would wake
                // a parent still RUNNING, and nothing else looks at it: joined
                // first and looked at once more (`wait::join`), and the
                // backstop makes a missed wakeup a late return rather than a
                // hang
                match joined.take() {
                    None => joined = Some(crate::sched::wait::join(&death)),
                    Some(j) => { j.sleep(crate::sched::tick_deadline(WAIT4_BACKSTOP)); }
                }
            }
        }
    }
}

// 45  kill(pid, sig) -> 0 / -errno
//
// POSIX's rule: you may signal a process you own, matching its real or saved
// id, so a process that lowered itself is still yours; or you may hold CAP_KILL
// and signal anything
fn may_signal(target_pid: u64) -> Result<(), u64> {
    let me = crate::sched::cred_of(current_pid());
    if me.has(crate::process::cred::CAP_KILL) {
        return Ok(());
    }
    // cred_checked, not cred_of: a pid with no process must not answer "root"
    let target = match crate::sched::cred_checked(target_pid) {
        Some(c) => c,
        None    => return Err(err(ESRCH)),
    };
    if me.euid == target.uid || me.euid == target.suid {
        Ok(())
    } else {
        Err(err(EPERM))
    }
}

pub fn sys_kill(target_pid: u64, sig: u64) -> u64 {
    if target_pid == 0 { return err(EINVAL); }
    if sig > 64 { return err(EINVAL); }

    // A negative pid is a process group: `kill(-pgid)` ends a whole pipeline.
    // Each member is checked separately, so a group holding one process the
    // caller may not signal is no way to signal it
    if (target_pid as i64) < 0 {
        let pgid = (target_pid as i64).unsigned_abs();
        return kill_group(pgid, sig);
    }

    // The existence probe answers before the permission check, but only with
    // ESRCH or EPERM - never with "yes" to somebody who may not signal it
    if sig == 0 {
        return match may_signal(target_pid) {
            Ok(())  => if crate::sched::process_exists(target_pid) { 0 } else { err(ESRCH) },
            Err(e)  => e,
        };
    }

    if let Err(e) = may_signal(target_pid) {
        return e;
    }

    match sig {
        // SIGKILL, and nothing else: a target may have an opinion on SIGTERM,
        // as mikuD's "SIGTERM, then SIGKILL after a timeout" stop sequence
        // needs
        9 => {
            crate::sched::kill_by_signal(target_pid, 9);
            crate::process::signal::send_sigchld(target_pid);
            0
        }
        _ => {
            crate::process::signal::send_signal(target_pid, sig as u32);
            0
        }
    }
}

/// `kill(-pgid, sig)`: every member of a job, each permission-checked
///
/// ESRCH when the group is empty: "nobody was there" and "you may not" stay
/// different answers
fn kill_group(pgid: u64, sig: u64) -> u64 {
    if pgid == 0 { return err(EINVAL); }
    let mut buf = [0u64; 32];
    let mut from = 0usize;
    let mut found = 0usize;
    let mut denied = 0usize;
    loop {
        let (n, next) = crate::sched::group_members(pgid, from, &mut buf);
        for &pid in &buf[..n] {
            found += 1;
            if may_signal(pid).is_err() { denied += 1; continue; }
            if sig == 0 { continue; }
            if sig == 9 {
                crate::sched::kill_by_signal(pid, 9);
                crate::process::signal::send_sigchld(pid);
            } else {
                crate::process::signal::send_signal(pid, sig as u32);
            }
        }
        if next >= crate::sched::MAX_PROCS { break; }
        from = next;
    }
    if found == 0 { return err(ESRCH); }
    if denied == found { return err(EPERM); }
    0
}

// 101  setpgid(pid, pgid) -> 0 / -errno
//
// Puts a process in a job. `pid` 0 means the caller; `pgid` 0 means a new group
// led by the target, the form a shell uses on both sides of a fork so neither
// ordering leaves the child in the wrong group.
//
// Who may: yourself, or a child of yours. A terminal's keys belong to a group,
// so moving a process into somebody's group puts it in reach of their Ctrl-C,
// and moving one out takes it out of reach of the shell that started it. POSIX
// also requires the target to be in the caller's session; the equivalent here
// is the terminal, so a join at a different one is refused
pub fn sys_setpgid(pid: u64, pgid: u64) -> u64 {
    let me = current_pid();
    let target = if pid == 0 { me } else { pid };
    if !crate::sched::process_exists(target) {
        return err(ESRCH);
    }
    if target != me && crate::sched::get_ppid(target) != me {
        return err(EPERM);
    }
    let new_pgid = if pgid == 0 { target } else { pgid };

    // Joining an existing group: it has to exist, at the same terminal. Leading
    // a new one is always allowed
    if new_pgid != target {
        if !crate::sched::process_exists(new_pgid) {
            return err(EPERM);
        }
        if crate::sched::pgid_of(new_pgid) != new_pgid {
            // The named pid is not its group's leader, so this is a group id
            // that means nothing
            return err(EPERM);
        }
        if crate::sched::ctty_of(new_pgid) != crate::sched::ctty_of(target) {
            return err(EPERM);
        }
    }

    if !crate::sched::set_pgid(target, new_pgid) {
        return err(ESRCH);
    }
    0
}

// 102  getpgid(pid) -> pgid / -errno
pub fn sys_getpgid(pid: u64) -> u64 {
    let target = if pid == 0 { current_pid() } else { pid };
    if !crate::sched::process_exists(target) {
        return err(ESRCH);
    }
    crate::sched::pgid_of(target)
}

// 46  exec(path_ptr, path_len, argv_ptr, argc) -> never on success
//
// argv_ptr is an array of 'argc' u64 user pointers, each pointing to a
// NUL-terminated string. argv[0] conventionally repeats the program
// name. If userspace passes argc=0 or argv_ptr=0 we fall back to a
// single-entry argv of [path] so the loader always has a valid argv[0]
pub fn sys_exec(path_ptr: u64, path_len: u64, argv_ptr: u64, argc: u64) -> u64 {
    let path = match read_user_path(path_ptr, path_len) {
        Ok(p)  => p,
        Err(e) => return e,
    };

    // Copies argv strings into kernel memory before swapping address spaces:
    // user pages disappear once the new cr3 is loaded
    let user_args = match read_user_argv(argv_ptr, argc) {
        Ok(v)  => v,
        Err(e) => return e,
    };

    let envs: alloc::vec::Vec<alloc::string::String> = crate::process::exec::DEFAULT_ENV
        .iter()
        .map(|s| alloc::string::String::from(*s))
        .collect();
    do_exec(path, user_args, envs)
}

/// execve(args_ptr): the six parameters travel through a u64[6] struct in
/// user memory (same trick as mmap_file) because the syscall ABI carries
/// only four registers:
///   [path_ptr, path_len, argv_ptr, argc, envp_ptr, envc]
/// envp uses the same layout as argv: an array of pointers to
/// NUL-terminated "KEY=value" strings
pub fn sys_execve(args_ptr: u64) -> u64 {
    let cr3 = current_cr3();
    if args_ptr == 0 {
        return err(EFAULT);
    }
    let _ua = match crate::syscall::usercopy::user_ptr_mapped(cr3, args_ptr, 48) {
        Some(g) => g,
        None    => return err(EFAULT),
    };
    let mut a = [0u64; 6];
    unsafe {
        core::ptr::copy_nonoverlapping(args_ptr as *const u64, a.as_mut_ptr(), 6);
    }

    let path = match read_user_path(a[0], a[1]) {
        Ok(p)  => p,
        Err(e) => return e,
    };
    let user_args = match read_user_argv(a[2], a[3]) {
        Ok(v)  => v,
        Err(e) => return e,
    };
    let user_envs = match read_user_argv(a[4], a[5]) {
        Ok(v)  => v,
        Err(e) => return e,
    };
    do_exec(path, user_args, user_envs)
}

fn do_exec(
    path: alloc::string::String,
    user_args: alloc::vec::Vec<alloc::string::String>,
    user_envs: alloc::vec::Vec<alloc::string::String>,
) -> u64 {
    let cr3 = current_cr3();
    let pid = current_pid();

    // A thread may not exec.
    //
    // POSIX says the calling thread becomes the new program and the others are
    // destroyed, keeping the process's pid, so the surviving task has to take a
    // pid not its own, and every table here is keyed on pid: the run queue, the
    // index, the descriptor table, the image record. Nothing in this tree or
    // `std` does it: `Command` forks first. Refused by name
    if crate::sched::is_thread(pid) {
        crate::serial_println!("[exec] refused: pid={} is a thread, not a process", pid);
        return err(EPERM);
    }
    // The leader's siblings belong to the image being replaced. They go before
    // the address space does, and this call does not return until they are off
    // the processors: marking a task dead does not stop a CPU already inside
    // it, and `exec` frees the old address space itself
    for t in crate::sched::group_threads(pid) {
        if t != pid { crate::sched::kill_with_code(t, 0); }
    }
    // Bounded: each sibling leaves at its next scheduling point, at most one
    // tick away, and yielding lets that tick happen on a single-processor
    // machine. The cap is a limp rather than a hang if something above is wrong
    let mut spins = 0u32;
    while crate::sched::group_still_on_cpu(pid, pid) {
        crate::sched::yield_now();
        spins += 1;
        if spins > 100_000 {
            crate::serial_println!("[exec] pid={} gave up waiting for its threads", pid);
            break;
        }
    }
    // And out of the table: each holds the old address space until it leaves
    // it, and a reaper must not unmap a sibling's stack from page tables this
    // call has freed
    crate::sched::reap_group_siblings(pid);


    // Permission before bytes: read_file answers with the contents whoever
    // asks, so the execute bit and noexec are checked here
    if let Err(e) = crate::fs::vfs::core::may_exec_path(&path) {
        crate::trace_println!("[exec] {}: {}", path, e.as_str());
        return match e {
            crate::fs::vfs::types::VfsError::PermissionDenied => err(EACCES),
            crate::fs::vfs::types::VfsError::IsDirectory      => err(EACCES),
            _ => err(ENOENT),
        };
    }

    let prog = match crate::process::exec::Program::open(&path) {
        Ok(p)  => p,
        Err(_) => return err(ENOENT),
    };

    // Build &[&str] view: argv[0]=path when userspace didn't supply one
    let arg_refs_in: alloc::vec::Vec<&str> = if user_args.is_empty() {
        alloc::vec![path.as_str()]
    } else {
        user_args.iter().map(|s| s.as_str()).collect()
    };

    // #! scripts: swap in the interpreter binary and its adjusted argv
    let (prog, argv) =
        match crate::process::exec::resolve_shebang(&path, prog, &arg_refs_in) {
            Ok(r) => r,
            Err(e) => {
                crate::serial_println!("[exec] {}: {}", path, e.as_str());
                return err(ENOENT);
            }
        };
    let arg_refs: alloc::vec::Vec<&str> = argv.iter().map(|s| s.as_str()).collect();
    let env_refs: alloc::vec::Vec<&str> = user_envs.iter().map(|s| s.as_str()).collect();

    let new_aspace = match AddressSpace::new_user() {
        Some(a) => a,
        None    => return err(ENOMEM),
    };

    // The credential this image will run with. `after_exec()` below only
    // shrinks capability sets and does not touch uid/euid/gid/egid, so the ids
    // are the same either side of it
    let exec_cred = crate::sched::cred_of(pid);
    let image = crate::process::elf_loader::load(
        &prog, &new_aspace, &path, &arg_refs, &env_refs, Some(&crate::process::exec::read_interp),
        exec_cred,
    );
    // Its descriptor goes before the close-on-exec sweep below can see it,
    // and long before the new image could
    drop(prog);
    let image = match image {
        Ok(img) => img,
        Err(e) => {
            crate::serial_println!("[exec] ELF load failed: {}", e.as_str());
            // new_aspace dropped -> freed
            return err(ENOENT);
        }
    };

    let new_cr3 = new_aspace.into_raw();

    // POSIX: descriptors marked O_CLOEXEC are closed when the new image starts,
    // and only then. Closing before the permission check, the read and the ELF
    // load would let each failure return to a program with those descriptors
    // gone, which breaks a child reporting a failed exec down a close-on-exec
    // pipe. From `into_raw` above this function cannot fail
    let cloexec_victims = crate::fs::vfs::core::with_vfs(|vfs| vfs.close_cloexec_fds());
    if !cloexec_victims.is_empty() {
        // The pipes' keys before the drops, which may take an end with them
        let pipe_keys: alloc::vec::Vec<u64> = crate::fs::vfs::core::with_vfs(|vfs| {
            let keys = cloexec_victims.iter().map(|&v| v as usize)
                .filter(|&v| crate::fs::vfs::pipe::is_end(v))
                .map(crate::fs::vfs::pipe::wait_key)
                .collect();
            for vid in cloexec_victims {
                vfs.drop_ref(vid as usize);
            }
            keys
        });
        // The parent reading the other end of an exec-status pipe (every
        // `Command::spawn`) has its end of file now
        for key in pipe_keys {
            crate::sched::wait::wake_key(key);
        }
    }
    // Sockets are not in that table; they are in the socket layer's, keyed on
    // the owning pid, and an exec keeps the pid
    crate::net::socket::close_cloexec_for_pid(pid);

    // Same pid, different image: the record is replaced, not added to
    crate::process::exec::register_image(pid, &path, &image);

    // And the process is called what it now is. Here because from `into_raw`
    // above this function can no longer fail.
    //
    // From the path rather than `argv[0]`, which is whatever the caller passed.
    // It also gets a `#!` script right: after `resolve_shebang`, `arg_refs[0]`
    // is the interpreter, while `path` is the script that was run
    crate::sched::set_name(pid, crate::process::Name::from_path(&path));

    crate::mm::mmap::vma_cleanup(cr3);
    crate::mm::mmap::vma_set_brk(new_cr3, image.brk);

    crate::sched::update_process_cr3(pid, new_cr3);

    // The credential transition.
    //
    // Nothing here reads the file's mode bits. FileMode::setuid() and setgid()
    // exist, ls renders them, and the loader does not consult them.
    // after_exec() intersects the permitted set with the bounding set and makes
    // that effective: the sets can stay the same or shrink.
    //
    // So an unprivileged caller cannot use exec as a step in an attack:
    // whatever it runs, it runs as itself. Elevation happens in a process mikuD
    // started, by that process choosing to become somebody smaller
    unsafe {
        let pptr = crate::sched::proc_index_raw(pid);
        if !pptr.is_null() {
            let mut guard = (*pptr).cred.write();
            *guard = guard.after_exec();
        }
    }

    // Fresh image: the old sig_entry/pending signals belong to code that
    // no longer exists in this address space
    unsafe {
        let pptr = crate::sched::proc_index_raw(pid);
        if !pptr.is_null() {
            (*pptr).sig_entry.store(0, core::sync::atomic::Ordering::Relaxed);
            let disp = (*pptr).sig_disp.load(core::sync::atomic::Ordering::Relaxed);
            (*pptr).sig_disp.store(
                crate::process::signal::disp_after_exec(disp),
                core::sync::atomic::Ordering::Relaxed,
            );
            (*pptr).pending_sig.store(0, core::sync::atomic::Ordering::Relaxed);
            (*pptr).sig_in_delivery.store(false, core::sync::atomic::Ordering::Relaxed);
            // The alternate stack was memory in the image that just went, and
            // `SA_ONSTACK` described handlers that went with it
            (*pptr).sig_alt_sp.store(0, core::sync::atomic::Ordering::Relaxed);
            (*pptr).sig_alt_size.store(0, core::sync::atomic::Ordering::Relaxed);
            (*pptr).sig_onstack.store(0, core::sync::atomic::Ordering::Relaxed);
            // And the arithmetic state: a rounding mode or an unmasked SSE
            // exception from the previous image would make the new program's
            // results depend on what this pid was running. `exec::exec_as`
            // builds a new `Process`; this path replaces one in place
            (*pptr).fpu.reset();
        }
    }

    // Switch to the new address space before freeing the old one so an
    // interrupt firing between free and switch cannot use-after-free
    unsafe {
        // Said before it is done: see `mm::tlb::cr3_loaded_elsewhere`
        crate::arch::x86_64::percpu::current()
            .loaded_cr3.store(new_cr3, core::sync::atomic::Ordering::SeqCst);
        core::arch::asm!("mov cr3, {}", in(reg) new_cr3, options(nostack, preserves_flags));
    }

    if cr3 != 0 && cr3 != crate::mm::vmm::kernel_cr3() {
        // The pools outlive the image; their windows in it do not
        crate::mayland::server::shm::unmap_windows_in(cr3);
        let mut old = AddressSpace::from_raw(cr3);
        old.free_address_space();
    }

    if image.tls_base != 0 {
        // try_new: tls_base derives from the loaded ELF and must stay canonical
        // user-half. Skipped if the loader produced an unusable value, rather
        // than wrmsr-#GP in the kernel
        if let Ok(va) = x86_64::VirtAddr::try_new(image.tls_base) {
            if image.tls_base <= crate::syscall::usercopy::USER_MAX {
                x86_64::registers::model_specific::FsBase::write(va);
            }
        }
    }

    // Patches sysret-state to return to the new entry and stack. Per-cpu
    // layout: gs:[0x10]=kernel_rsp (saved-reg stack top, see Cpu in
    // arch/x86_64/percpu.rs and the syscall prologue in syscall/mod.rs),
    // gs:[0x18]=user_rsp (where the prologue stashed userspace RSP)
    let kernel_stack_top: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) kernel_stack_top);
        // saved RCX (= user RIP) is at [top-8]
        *((kernel_stack_top - 8) as *mut u64) = image.entry;
        // RDI, RSI and RDX, which the syscall exit restores: the new image
        // starts with nothing of the old one's in them. RDX is `rtld_fini`
        // at a SysV entry point, and zero is "none"
        for off in [96u64, 104, 112] {
            *((kernel_stack_top - off) as *mut u64) = 0;
        }
        core::arch::asm!("mov gs:[0x18], {}", in(reg) image.stack_top, options(nostack, preserves_flags));
    }

    // The other half of `[exec] spawned`: a process that replaced its image in
    // place. The log only
    crate::kdebug!(
        "[exec] pid={} replaced with '{}': entry={:#x} sp={:#x}",
        pid, path, image.entry, image.stack_top
    );

    0
}

// 103  procinfo(buf_ptr, max) -> count / -errno
//
// What `ps` is made of. /proc here is a fixed set of vnodes built at boot, so
// this is a call with a fixed-size record per process:
//
//   pid u64, ppid u64, pgid u64, cpu_time u64,
//   uid u16, state u8, priority u8, cpu_pct_x10 u32,
//   tty u32, name [u8; 24], and four bytes of padding
//
// = 72 bytes, laid out so nothing straddles its own alignment.
//
// Everybody may call it: process names and pids are what `ps` shows on every
// Unix. Nothing about a process's memory is here: no addresses, no brk, no
// stack pointer
pub const PROCINFO_SIZE: u64 = core::mem::size_of::<miku_abi::kproc::ProcInfo>() as u64;

// 105  proc_path(pid, buf, len) -> bytes written | -errno
//
// The program a process is running, as the path `exec` was given.
//
// `procinfo` carries a 24-byte name, right for the default `ps` column. This is
// the path, one pid at a time, as `ps -f` uses it. `process::image` holds it
// per pid.
//
// Same audience as `procinfo`. Still nothing about its memory
pub fn sys_proc_path(pid: u64, buf_ptr: u64, len: u64) -> u64 {
    if len == 0 { return 0; }
    let len = len.min(4096);
    let cr3 = current_cr3();
    let _ua = match user_ptr_writable(cr3, buf_ptr, len) {
        Some(g) => g,
        None    => return err(EFAULT),
    };
    // Pid 0 is the caller, as `std::env::current_exe` asks. The image is
    // recorded under the process, so a thread asking gets its leader's
    let pid = if pid == 0 { crate::sched::current_tgid() } else { pid };
    // A kernel thread has no image, and neither has a pid nobody holds. Told
    // apart from an error so `ps -f` can print a blank
    let Some(path) = crate::process::image::with(pid, |i| i.path.clone()) else {
        return 0;
    };
    let bytes = path.as_bytes();
    let n = (bytes.len() as u64).min(len) as usize;
    unsafe {
        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr as *mut u8, n);
    }
    n as u64
}

pub fn sys_procinfo(buf_ptr: u64, max: u64) -> u64 {
    if max == 0 { return 0; }
    let max = max.min(512);
    let cr3 = current_cr3();
    let total = max.saturating_mul(PROCINFO_SIZE);
    let _ua = match user_ptr_writable(cr3, buf_ptr, total) {
        Some(g) => g,
        None    => return err(EFAULT),
    };

    let stats = crate::sched::get_stats();
    let mut n = 0u64;
    for s in stats.iter() {
        if n >= max { break; }
        // Processes, not tasks: a thread has the same uid, terminal, job and
        // name. Linux answers the other question with `ps -L`
        if crate::sched::is_thread(s.pid) { continue; }
        let uid = crate::sched::cred_checked(s.pid).map(|c| c.uid).unwrap_or(0);
        let pgid = crate::sched::pgid_of(s.pid);
        let ppid = crate::sched::get_ppid(s.pid);
        // One letter, as `ps` has always spelled a state: R S B D Z
        let state = s.state.as_bytes().first().copied().unwrap_or(b'?');
        let mut rec = miku_abi::kproc::ProcInfo {
            pid: s.pid,
            ppid,
            pgid,
            cpu_time: s.cpu_time,
            uid,
            state,
            priority: s.priority,
            cpu_pct_x10: s.cpu_pct_x10,
            // Which terminal it is at, so `ps` can answer the question its TTY
            // column has always answered. A raw `TtyId`; 0 is "no terminal",
            // which is what a daemon has
            tty: crate::sched::ctty_of(s.pid),
            name: [0; 24],
            _pad: [0; 4],
        };
        let name = s.name.as_bytes();
        let len = name.len().min(rec.name.len() - 1);
        rec.name[..len].copy_from_slice(&name[..len]);
        unsafe { (buf_ptr as *mut miku_abi::kproc::ProcInfo).add(n as usize).write_unaligned(rec) };
        n += 1;
    }
    n
}
