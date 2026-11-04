#![no_std]
#![no_main]

//! mikudod: the elevation broker
//!
//! The only program that changes one identity into another; nothing on MikuOS
//! is setuid.
//!
//! Started by mikuD with its own environment. Input arrives as a
//! length-prefixed socket message decoded into fixed storage, and the kernel
//! names its sender. `/bin/mikudo` holds all hostile input and has no
//! authority.
//!
//! * No environment inheritance: the child's environment is built here.
//! * No caller argv[0]: written here as the resolved path. An empty argv is
//!   refused.
//! * No timestamp file: the persist token is in memory, keyed to the terminal,
//!   on the monotonic clock.
//! * No lookup between check and exec: the command resolves once, and the rule
//!   matches that path.
//! * The hash stays here: only this process opens /etc/shadow. The password
//!   arrives over the socket.
//!
//! Request flow:
//! 1. accept and get `peercred`, taken at connect time
//! 2. read and decode into fixed storage
//! 3. read /etc/passwd, /etc/group, and /etc/mikudo.conf, checking the last
//!    one's owner and mode
//! 4. decide; no match denies, and a parse error denies everything
//! 5. check the password unless the rule says nopass or a persist token is
//!    valid
//! 6. fork; the child drops to the target identity, checking every return, and
//!    execs the resolved path with argv and environment built here
//! 7. journal the result and deciding policy line

extern crate alloc;

pub use miku;

use alloc::string::{String, ToString};
use alloc::vec::Vec;
use alloc::format;

use mikuauth::conf::{Action, Config, Request};
use mikuauth::db;
use mikuauth::hash::{self, Verdict};
use mikuauth::proto::{self, flags, frame_len, power_action, session_kind, Op, Reader, Reply, Writer};
use mikuauth::session::{Identity, Refusal};

// Configuration

/// Persist token lifetime in ms, on the monotonic clock, so setting the wall
/// clock cannot extend it
const PERSIST_MS: u64 = 5 * 60 * 1000;

/// Password attempts before the connection drops
const MAX_TRIES: u32 = 3;

/// Password prompt timeout in ms
const PROMPT_TIMEOUT_MS: u64 = 60_000;

/// Search path when no path is given; never from the caller
const SAFE_PATH: &str = "/sbin:/bin";

/// Prefix for non-program commands in a rule:
/// `permit :wheel cmd kernel:nvidia`. Cannot collide, since `resolve_command`
/// answers with absolute paths
const KERNEL_PREFIX: &str = "kernel:";

/// The largest any of the three account files may be
const MAX_DB_BYTES: usize = 256 * 1024;

// Entry

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    // Refuses to run without authority, logging once at boot
    if miku::cred::geteuid() != 0 {
        klog("mikudod: not running as root - refusing to start");
        miku::exit(1);
    }

    // World-connectable: anyone may ask, and the decision is made here
    let listener = match open_socket() {
        Some(fd) => fd,
        None => miku::exit(1),
    };
    klog("mikudod: listening on /run/mikudo.sock");

    let mut persist: Vec<Token> = Vec::new();

    loop {
        let conn = unsafe { miku::miku_accept(listener, core::ptr::null_mut(), core::ptr::null_mut()) };
        if conn < 0 {
            // Keeps serving after a failed accept
            miku::sleep_ms(10);
            continue;
        }
        serve(conn, &mut persist);
        unsafe { miku::miku_close(conn) };
    }
}

fn open_socket() -> Option<i64> {
    let path = c(mikuauth::SOCKET_PATH);
    // Removes a stale node, which would make bind fail
    unsafe { miku::miku_unlink(path.as_ptr()) };

    let fd = unsafe { miku::miku_socket_unix() };
    if fd < 0 {
        klog("mikudod: cannot make a socket");
        return None;
    }
    if unsafe { miku::miku_bind_unix(fd, path.as_ptr()) } < 0 {
        klog("mikudod: cannot bind /run/mikudo.sock");
        return None;
    }
    if unsafe { miku::miku_listen(fd, 8) } < 0 {
        klog("mikudod: cannot listen");
        return None;
    }
    Some(fd)
}

// One connection

/// Remembered authentication, keyed by caller identity and session. `pid` and
/// `start_tick` together, since pids are recycled
struct Token {
    uid: u16,
    /// Login session that remembered it
    session: u32,
    expires_ms: u64,
}

fn serve(conn: i64, persist: &mut Vec<Token>) {
    // Caller from the kernel; the message has no such field
    let peer = match miku::cred::peercred(conn) {
        Ok(p) => p,
        Err(_) => {
            klog("mikudod: a connection with no peer credentials - refused");
            return;
        }
    };

    let mut raw = [0u8; proto::MAX_MESSAGE];
    let body = match read_message(conn, &mut raw) {
        Some(b) => b,
        None => return,
    };

    let mut r = Reader::new(body);
    if r.u8() != Some(proto::VERSION) {
        send_denied(conn, "mikudo: this client and this daemon disagree about the protocol");
        return;
    }
    let op = match r.u8().and_then(Op::from_u8) {
        Some(o) => o,
        None => {
            send_denied(conn, "mikudo: unknown request");
            return;
        }
    };
    let req_flags = match r.u16() {
        Some(f) => f,
        None => return,
    };

    // Caller identity by kernel-given uid. A uid with no account is refused
    let passwd = read_db(mikuauth::PASSWD_PATH);
    let group = read_db(mikuauth::GROUP_PATH);
    let caller = match db::find_uid(&passwd, peer.uid) {
        Some(p) => p,
        None => {
            audit(&format!("uid {} has no account - refused", peer.uid));
            send_denied(conn, "mikudo: you have no account on this machine");
            return;
        }
    };
    let caller_name = caller.name.to_string();

    let mut gid_buf = [0u16; miku::cred::MAX_GROUPS];
    let ngroups = db::groups_of(&group, &caller_name, caller.gid, &mut gid_buf);
    let gids = &gid_buf[..ngroups];
    let gnames: Vec<&str> = db::Lines::new(&group)
        .filter_map(|(_, l)| db::Group::parse(l).ok())
        .filter(|g| gids.contains(&g.gid) || g.has_member(&caller_name))
        .map(|g| g.name)
        .collect();

    match op {
        Op::ClearPersist => {
            // Forgets tokens in every session of this user, unlike remembering,
            // which is per session
            persist.retain(|t| t.uid != peer.uid);
            audit(&format!("{caller_name} cleared their remembered authentication"));
            send(conn, Reply::End, "");
        }
        Op::List => do_list(conn, &caller_name, peer.uid, &gnames, gids),
        Op::Check | Op::Run => {
            do_run(conn, op, req_flags, &mut r, &peer, &caller_name, caller.gid,
                   gids, &gnames, &passwd, &group, persist)
        }
        Op::Passwd => do_passwd(conn, &mut r, &peer, &caller_name, &passwd),
        Op::OpenSession => do_open_session(conn, &mut r, &caller_name, &passwd, &group, &gnames),
        Op::CloseSession => do_close_session(conn, &mut r, &caller_name, &peer, &gnames),
        Op::Sessions => do_sessions(conn, &passwd),
        Op::Power => do_power(conn, &mut r, &caller_name, &passwd, &group),
    }
}

/// Request timeout in ms
const REQUEST_TIMEOUT_MS: u64 = 5_000;

/// Reads one framed message. The length is checked before the buffer is
/// touched, and the body goes into compile-time-sized storage. The deadline
/// stops a silent client from blocking the single-threaded daemon
fn read_message<'a>(conn: i64, raw: &'a mut [u8]) -> Option<&'a [u8]> {
    read_message_within(conn, raw, REQUEST_TIMEOUT_MS)
}

/// As above with a caller-given deadline: short for requests, long for typed
/// passwords
fn read_message_within<'a>(conn: i64, raw: &'a mut [u8], timeout_ms: u64) -> Option<&'a [u8]> {
    let deadline = miku::time::monotonic_ms() + timeout_ms;
    let mut prefix = [0u8; 4];
    if !read_exact(conn, &mut prefix, deadline) {
        return None;
    }
    let n = frame_len(prefix)?;
    if n > raw.len() {
        return None;
    }
    if !read_exact(conn, &mut raw[..n], deadline) {
        return None;
    }
    Some(&raw[..n])
}

/// Fills `buf` or gives up at `deadline`, using `poll`, since `recv` has no
/// timeout
fn read_exact(conn: i64, buf: &mut [u8], deadline_ms: u64) -> bool {
    let mut got = 0usize;
    while got < buf.len() {
        let now = miku::time::monotonic_ms();
        if now >= deadline_ms {
            return false;
        }
        let mut pfd = miku::MikuPollFd { fd: conn as i32, events: miku::POLLIN, revents: 0 };
        let ready = unsafe { miku::miku_poll(&mut pfd, 1, (deadline_ms - now) as i64) };
        if ready <= 0 {
            return false;
        }
        let n = unsafe { miku::miku_recv(conn, buf[got..].as_mut_ptr(), buf.len() - got) };
        if n <= 0 {
            return false;
        }
        got += n as usize;
    }
    true
}

// Run

#[allow(clippy::too_many_arguments)]
fn do_run(
    conn: i64,
    op: Op,
    req_flags: u16,
    r: &mut Reader<'_>,
    peer: &miku::cred::PeerCred,
    caller_name: &str,
    _caller_gid: u16,
    gids: &[u16],
    gnames: &[&str],
    passwd: &str,
    group: &str,
    persist: &mut Vec<Token>,
) {
    let target_name = match r.str() {
        Some(t) if !t.is_empty() => t,
        _ => "root",
    };
    let cwd = r.str().unwrap_or("/");
    let term = r.str().unwrap_or("");

    let argc = match r.u16() {
        Some(n) if (n as usize) <= proto::MAX_ARGS => n as usize,
        _ => {
            send_denied(conn, "mikudo: too many arguments");
            return;
        }
    };

    // An empty argv is not a request (CVE-2021-4034)
    if argc == 0 && req_flags & flags::SHELL == 0 {
        send_denied(conn, "mikudo: no command given");
        return;
    }

    let mut args: Vec<String> = Vec::new();
    for _ in 0..argc {
        match r.str() {
            Some(a) => args.push(a.to_string()),
            None => {
                send_denied(conn, "mikudo: malformed request");
                return;
            }
        }
    }

    // The target must be a real account
    let target = match db::find_user(passwd, target_name) {
        Some(t) => t,
        None => {
            audit(&format!("{caller_name} asked to become '{target_name}', which is not an account"));
            send_denied(conn, "mikudo: no such user");
            return;
        }
    };

    // Resolves the command once; the rule, journal, and exec use this string
    let (cmd_path, exec_args) = if req_flags & flags::SHELL != 0 {
        // The shell, not field seven of /etc/passwd, which is the session
        // program
        (db::DEFAULT_SHELL.to_string(), Vec::new())
    } else {
        let resolved = match resolve_command(&args[0]) {
            Some(p) => p,
            // Kernel commands, named `kernel:<name>`, which cannot be a path.
            // The kernel decides whether the name exists
            None => format!("{KERNEL_PREFIX}{}", args[0]),
        };
        (resolved, args[1..].to_vec())
    };

    let arg_refs: Vec<&str> = exec_args.iter().map(|s| s.as_str()).collect();

    // A policy file that does not parse denies everything
    let conf = match load_config() {
        Ok(c) => c,
        Err(why) => {
            audit(&format!("REFUSING EVERYTHING: {why}"));
            send_denied(conn, &format!("mikudo: {why}"));
            return;
        }
    };

    let decision = conf.decide(&Request {
        user: caller_name,
        uid: peer.uid,
        gid: gids.first().copied().unwrap_or(0),
        groups: gids,
        group_names: gnames,
        target: target_name,
        cmd: &cmd_path,
        args: &arg_refs,
    });

    if decision.action != Action::Permit {
        // Refusals are always logged; a policy can disable only success records
        audit(&format!(
            "DENIED {caller_name} -> {target_name}: {cmd_path} ({})",
            if decision.line == 0 { "no rule matches".to_string() }
            else { format!("denied by line {}", decision.line) }
        ));
        send_denied(conn, "mikudo: you are not permitted to do that");
        return;
    }

    if op == Op::Check {
        send(conn, Reply::Would, &format!(
            "permitted by /etc/mikudo.conf line {}", decision.line));
        return;
    }

    // No descriptors pass: fds 0, 1, and 2 route to the task's controlling
    // terminal, so the child joins the caller's terminal in `spawn` before
    // dropping privilege. The password comes over the socket
    // (Reply::NeedPassword)
    let now = miku::time::monotonic_ms();
    // Token keyed to the login session
    let session = miku::session::current_of(peer.pid);
    let remembered = decision.options.persist
        && persist.iter().any(|t| t.uid == peer.uid && t.session == session
                                  && t.expires_ms > now);

    if !decision.options.nopass && !remembered {
        if req_flags & flags::NON_INTERACTIVE != 0 {
            send_denied(conn, "mikudo: a password is required and -n was given");
            return;
        }
        // `allow_empty` is true only for a console login
        match authenticate_over_socket(conn, caller_name, passwd, group, false) {
            Ok(Some(_)) => {}
            Ok(None) => {
                audit(&format!("{caller_name} failed to authenticate ({MAX_TRIES} attempts)"));
                send_denied(conn, "mikudo: authentication failed");
                return;
            }
            Err(why) => {
                send_denied(conn, &format!("mikudo: {why}"));
                return;
            }
        }
        if decision.options.persist {
            persist.retain(|t| (t.uid != peer.uid || t.session != session) && t.expires_ms > now);
            persist.push(Token { uid: peer.uid, session, expires_ms: now + PERSIST_MS });
        }
    }

    if !decision.options.nolog {
        audit(&format!(
            "{caller_name} ran {cmd_path} as {target_name} (line {})",
            decision.line
        ));
    }

    let status = spawn(&cmd_path, &exec_args, &target, term, cwd, &decision.options, peer.pid);
    send_status(conn, status);
}

// Sessions

/// Group allowed to open sessions, containing `greeter` and `login`. A group so
/// `/etc/group` can change it
const SESSION_GROUP: &str = "session";

/// Verifies identity and opens a session. The program showing the login screen
/// stays unprivileged; only this process checks the password
fn do_open_session(
    conn: i64, r: &mut Reader<'_>, caller_name: &str,
    passwd: &str, group: &str, gnames: &[&str],
) {
    let user = match r.str() {
        Some(u) if !u.is_empty() => u,
        _ => { send_denied(conn, "mikudo: no user named"); return; }
    };
    let kind = r.u8().unwrap_or(session_kind::CONSOLE);
    let seat = r.u8().unwrap_or(0);
    if !session_kind::valid(kind) {
        send_denied(conn, "mikudo: no such kind of session");
        return;
    }

    if !gnames.iter().any(|g| *g == SESSION_GROUP) {
        audit(&format!(
            "{caller_name} tried to open a {} session for '{user}' and is not in {SESSION_GROUP}",
            session_kind::name(kind)));
        send_denied(conn, "mikudo: you may not open sessions on this machine");
        return;
    }

    // `allow_empty` only on the console
    let (identity, shipped) = match authenticate_over_socket(
        conn, user, passwd, group, kind == session_kind::CONSOLE) {
        Ok(Some(pair)) => pair,
        Ok(None) => {
            audit(&format!("failed {} login for '{user}' ({MAX_TRIES} attempts)",
                           session_kind::name(kind)));
            send_denied(conn, "incorrect");
            return;
        }
        Err(why) => {
            audit(&format!("login refused for '{user}': {why}"));
            send_denied(conn, &format!("login: {why}"));
            return;
        }
    };

    // A console session starts nothing; the kernel puts the identity on the
    // shell thread
    let leader = if kind == session_kind::GRAPHICAL {
        match start_session_leader(&identity) {
            Ok(pid) => pid,
            Err(why) => {
                audit(&format!("could not start {}'s session: {why}", identity.name));
                send_denied(conn, &format!("login: {why}"));
                return;
            }
        }
    } else {
        0
    };

    // Warns about the shipped password on every login until it changes; the
    // client decides where to show it
    if shipped {
        audit(&format!("{} logged in with the shipped password", identity.name));
        send(conn, Reply::Line,
             "This account still has the password it shipped with, which is \
              written down in the documentation and is therefore not a secret. \
              Change it with `passwd`.");
    }

    match miku::session::open(identity.uid, identity.gid, identity.group_slice(),
                              kind, seat, leader as u64) {
        Ok(id) => {
            audit(&format!("{} logged in ({} session {}, seat {})",
                           identity.name, session_kind::name(kind), id, seat));
            // The id as a number, as `Reply::Exited` carries an i64
            send_status(conn, id as i64);
        }
        Err(e) => {
            // Kills a leader with no session, since nothing would close it
            if leader != 0 {
                unsafe { miku::miku_kill(leader as u64, 9) };
            }
            audit(&format!("the kernel refused {}'s session ({e})", identity.name));
            send(conn, Reply::Error, "login: the kernel refused the session");
        }
    }
}

/// Creates the user's home if missing, like `pam_mkhomedir`, since `/home` is
/// on tmpfs. Parents are root 0755, the home is the user's 0700. An existing
/// one is untouched. Filled from `/etc/skel` (`builder/src/pkg.rs`)
fn make_home(identity: &Identity<'_>) {
    let home = identity.home;
    if home.is_empty() || home == "/" || unsafe { miku::miku_isdir(c(home).as_ptr()) } {
        return;
    }
    let p = c(home);
    let made = unsafe { miku::miku_mkdir_p(p.as_ptr(), 0o755) } >= 0
        && unsafe { miku::miku_chown(p.as_ptr(), identity.uid as u32, identity.gid as u32) } >= 0
        && unsafe { miku::miku_chmod(p.as_ptr(), 0o700) } >= 0;
    if made {
        audit(&format!("made {}'s home, {home}", identity.name));
    } else {
        audit(&format!("could not make {}'s home, {home}", identity.name));
        return;
    }
    let mut failed = 0;
    let copied = copy_skel(SKEL, home, identity.uid as u32, identity.gid as u32, 0, &mut failed);
    if copied > 0 || failed > 0 {
        audit(&format!("{copied} files from {SKEL} in {}'s home, {failed} not", identity.name));
    }
}

/// What a new home starts with
const SKEL: &str = "/etc/skel";

/// `copy_skel` depth limit
const SKEL_DEPTH: usize = 16;

/// Copies `from` to `to` owned by `uid`:`gid`, files 0644 and directories 0755.
/// Symlinks and others are skipped. Returns the file count; `failed` counts
/// errors
fn copy_skel(from: &str, to: &str, uid: u32, gid: u32, depth: usize, failed: &mut usize) -> usize {
    if depth >= SKEL_DEPTH {
        return 0;
    }
    let mut copied = 0;
    for (name, kind) in miku::read_dir(from) {
        let (src, dst) = (format!("{from}/{name}"), format!("{to}/{name}"));
        let d = c(&dst);
        match kind {
            miku::KIND_DIRECTORY => {
                let ok = unsafe { miku::miku_mkdir(d.as_ptr(), 0o755) } >= 0
                    && unsafe { miku::miku_chown(d.as_ptr(), uid, gid) } >= 0;
                if ok {
                    copied += copy_skel(&src, &dst, uid, gid, depth + 1, failed);
                } else {
                    *failed += 1;
                }
            }
            miku::KIND_REGULAR => {
                if copy_file(&src, &d) && unsafe { miku::miku_chown(d.as_ptr(), uid, gid) } >= 0 {
                    copied += 1;
                } else {
                    *failed += 1;
                }
            }
            _ => {}
        }
    }
    copied
}

/// Copies `src` to a new 0644 file at `dst` in pieces
fn copy_file(src: &str, dst: &[u8]) -> bool {
    let input = unsafe { miku::miku_open_cstr(c(src).as_ptr()) };
    if input < 0 {
        return false;
    }
    let output = unsafe { miku::miku_create(dst.as_ptr(), 0o644) };
    if output < 0 {
        unsafe { miku::miku_close(input) };
        return false;
    }
    let mut buf = [0u8; 16 * 1024];
    let mut ok = true;
    loop {
        let n = unsafe { miku::miku_read_fd(input, buf.as_mut_ptr(), buf.len()) };
        if n <= 0 {
            ok = n == 0;
            break;
        }
        let mut done = 0usize;
        while done < n as usize {
            let w = unsafe { miku::miku_write_fd(output, buf[done..].as_ptr(), n as usize - done) };
            if w <= 0 {
                ok = false;
                break;
            }
            done += w as usize;
        }
        if !ok {
            break;
        }
    }
    unsafe {
        miku::miku_close(input);
        miku::miku_close(output);
    }
    ok
}

/// Starts the session program as the user: field seven of /etc/passwd
/// (`/bin/maysession` on a desktop).
///
/// Forks twice so the leader is orphaned and reaped by PID 1, not this daemon.
/// The pid returns through a pipe, since exit status has only eight bits
fn start_session_leader(identity: &Identity<'_>) -> Result<i64, String> {
    make_home(identity);
    let program = if identity.program.is_empty() {
        db::DEFAULT_SESSION
    } else {
        identity.program
    };
    if !unsafe { miku::miku_isfile(c(program).as_ptr()) } {
        return Err(format!("{program} is not there to start"));
    }

    let mut fds: [i64; 2] = [-1, -1];
    if unsafe { miku::miku_pipe(fds.as_mut_ptr()) } < 0 {
        return Err("no pipe to learn the session's pid through".to_string());
    }
    let (rd, wr) = (fds[0], fds[1]);

    let middle = unsafe { miku::miku_fork() };
    if middle < 0 {
        miku::close(rd);
        miku::close(wr);
        return Err("the kernel refused to fork".to_string());
    }
    if middle == 0 {
        // middle
        miku::close(rd);
        let leader = unsafe { miku::miku_fork() };
        if leader == 0 {
            // the session
            miku::close(wr);
            become_user(identity);
            let dir = if identity.home.is_empty() { "/" } else { identity.home };
            let dirp = c(dir);
            if unsafe { miku::miku_chdir(dirp.as_ptr()) } < 0 {
                let rootp = c("/");
                unsafe { miku::miku_chdir(rootp.as_ptr()) };
            }
            let env = session_env(identity);
            exec_child(program, &[], &env);
            fail_child("login: cannot start the session program");
        }
        // Reports the pid even on failure; -1 unblocks the parent
        let word = (leader as i64).to_le_bytes();
        miku::write(wr as u64, &word);
        miku::close(wr);
        miku::exit(0);
    }

    // parent
    miku::close(wr);
    let mut word = [0u8; 8];
    let got = miku::read(rd, &mut word);
    miku::close(rd);

    // Collects the middle process
    let mut status: i64 = 0;
    unsafe { miku::miku_waitpid(middle as u64, &mut status) };

    if got != word.len() as i64 {
        return Err("the session did not report a pid".to_string());
    }
    let leader = i64::from_le_bytes(word);
    if leader <= 0 {
        return Err("the kernel refused to fork the session".to_string());
    }
    Ok(leader)
}

/// Drops to a user and verifies the result. Order: groups, gid, uid.
/// `session::open` may race and apply the credential first, leaving no
/// `CAP_SETGID`, so an EPERM from a step is not a failure. The final identity
/// check decides, including groups
fn become_user(identity: &Identity<'_>) {
    let mut why = "";
    if miku::cred::setgroups(identity.group_slice()).is_err() {
        why = "login: cannot set the groups";
    }
    if miku::cred::setresgid(Some(identity.gid), Some(identity.gid), Some(identity.gid)).is_err() {
        why = "login: cannot become the group";
    }
    if miku::cred::setresuid(Some(identity.uid), Some(identity.uid), Some(identity.uid)).is_err() {
        why = "login: cannot become the user";
    }

    if miku::cred::getuid() != identity.uid
        || miku::cred::geteuid() != identity.uid
        || miku::cred::getgid() != identity.gid
        || !has_groups(identity)
    {
        // Reports the failed step, if any
        if !why.is_empty() {
            fail_child(why);
        }
        fail_child("login: the identity change did not take - refusing to start a session");
    }
}

/// Whether this process has exactly the identity's groups, in any order
fn has_groups(identity: &Identity<'_>) -> bool {
    let want = identity.group_slice();
    let mut have = [0u16; miku::cred::MAX_GROUPS];
    let n = match miku::cred::getgroups(&mut have) {
        Ok(n) => n,
        Err(_) => return false,
    };
    if n != want.len() { return false; }
    want.iter().all(|g| have[..n].contains(g))
}

/// Session environment, built rather than inherited
fn session_env(identity: &Identity<'_>) -> Vec<String> {
    let home = if identity.home.is_empty() { "/" } else { identity.home };
    alloc::vec![
        format!("PATH={SAFE_PATH}"),
        format!("HOME={home}"),
        format!("USER={}", identity.name),
        format!("LOGNAME={}", identity.name),
        format!("SHELL={}", db::DEFAULT_SHELL),
    ]
}

/// Ends a session. Your own needs nothing; another's needs the group that
/// opening one needs
fn do_close_session(
    conn: i64, r: &mut Reader<'_>, caller_name: &str,
    peer: &miku::cred::PeerCred, gnames: &[&str],
) {
    let id = r.u32().unwrap_or(0);

    let mut buf = [miku::session::Info::default(); MAX_LISTED_SESSIONS];
    let n = miku::session::list(&mut buf);
    let target = buf[..n].iter().find(|s| s.id == id).copied();

    let mine = matches!(target, Some(t) if t.uid == peer.uid);
    if !mine && !gnames.iter().any(|g| *g == SESSION_GROUP) {
        audit(&format!("{caller_name} tried to close session {id}"));
        send_denied(conn, "mikudo: that is not your session");
        return;
    }

    match miku::session::close(id) {
        Ok(()) => {
            audit(&format!("{caller_name} closed session {id}"));
            send(conn, Reply::End, "");
        }
        Err(_) => send_denied(conn, "mikudo: there is no such session"),
    }
}

/// Power off or reboot. No password when nobody owns seat 0 (the login screen's
/// power button); otherwise a password, so a program in a session cannot do it
/// alone. Journaled first
fn do_power(conn: i64, r: &mut Reader<'_>, caller_name: &str, passwd: &str, group: &str) {
    let action = r.u8().unwrap_or(0xFF);
    if !power_action::valid(action) {
        send_denied(conn, "mikudo: no such power action");
        return;
    }

    let mut buf = [miku::session::Info::default(); MAX_LISTED_SESSIONS];
    let n = miku::session::list(&mut buf);
    let seat_taken = buf[..n].iter().any(|s| s.seat == miku::session::SEAT0);

    if seat_taken {
        match authenticate_over_socket(conn, caller_name, passwd, group, false) {
            Ok(Some(_)) => {}
            Ok(None) => {
                audit(&format!("{caller_name} failed to authenticate for {}",
                               power_action::name(action)));
                send_denied(conn, "mikudo: authentication failed");
                return;
            }
            Err(why) => {
                send_denied(conn, &format!("mikudo: {why}"));
                return;
            }
        }
    }

    audit(&format!("{caller_name} asked to {}{}", power_action::name(action),
                   if seat_taken { "" } else { " (nobody is logged in)" }));
    // Logged before the request
    let _ = miku::power::request(match action {
        power_action::REBOOT => miku::power::REBOOT,
        _ => miku::power::OFF,
    });
    // Reached only if the kernel refused
    send_denied(conn, "mikudo: the kernel refused to stop the machine");
}

/// MAX_SESSIONS in kernel/process/session.rs
const MAX_LISTED_SESSIONS: usize = 8;

/// Lists logged-in users. Not privileged
fn do_sessions(conn: i64, passwd: &str) {
    let mut buf = [miku::session::Info::default(); MAX_LISTED_SESSIONS];
    let n = miku::session::list(&mut buf);
    if n == 0 {
        send(conn, Reply::Line, "nobody is logged in");
    }
    for s in &buf[..n] {
        let who = db::find_uid(passwd, s.uid).map(|p| p.name).unwrap_or("?");
        let leader = if s.leader != 0 {
            format!("  leader pid {}", s.leader)
        } else {
            String::new()
        };
        send(conn, Reply::Line, &format!(
            "{:<12} {:<10} seat{}  session {}{}",
            who, session_kind::name(s.kind), s.seat, s.id, leader));
    }
    send(conn, Reply::End, "");
}

/// Resolves a command to an absolute path. A name with '/' is used as written;
/// a bare name searches SAFE_PATH
fn resolve_command(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    if name.contains('/') {
        let p = c(name);
        return if unsafe { miku::miku_isfile(p.as_ptr()) } {
            Some(name.to_string())
        } else {
            None
        };
    }
    for dir in SAFE_PATH.split(':') {
        let full = format!("{dir}/{name}");
        let p = c(&full);
        if unsafe { miku::miku_isfile(p.as_ptr()) } {
            return Some(full);
        }
    }
    None
}

/// Reads the policy file after checking owner and mode
fn load_config() -> Result<Config, String> {
    let path = c(mikuauth::CONF_PATH);
    let mut st = miku::MikuStat::default();
    if unsafe { miku::miku_stat(path.as_ptr(), &mut st) } < 0 {
        return Err("/etc/mikudo.conf is missing".to_string());
    }
    if st.uid != 0 {
        return Err("/etc/mikudo.conf is not owned by root".to_string());
    }
    if st.mode & 0o022 != 0 {
        return Err("/etc/mikudo.conf is writable by group or others".to_string());
    }
    let text = match miku::read_file_string(mikuauth::CONF_PATH) {
        Some(t) => t,
        None => return Err("/etc/mikudo.conf cannot be read".to_string()),
    };
    Config::parse(&text).map_err(|e| format!("/etc/mikudo.conf is not valid: {e:?}"))
}

fn read_db(path: &str) -> String {
    match miku::read_file_string(path) {
        Some(t) if t.len() <= MAX_DB_BYTES => t,
        _ => String::new(),
    }
}

// Descriptors

// Authentication

/// Asks the client for the password and checks it here. Only the foreground
/// client gets keystrokes (Reply::NeedPassword). The hash and /etc/shadow stay
/// in this process
fn authenticate_over_socket<'a>(
    conn: i64,
    user: &str,
    passwd: &'a str,
    group: &str,
    allow_empty: bool,
) -> Result<Option<(Identity<'a>, bool)>, String> {
    let shadow = read_db(mikuauth::SHADOW_PATH);
    if shadow.is_empty() {
        return Err("/etc/shadow cannot be read".to_string());
    }
    let today = (miku::time::realtime_secs() / 86400) as u32;

    for _ in 0..MAX_TRIES {
        send(conn, Reply::NeedPassword, user);

        let mut raw = [0u8; proto::MAX_MESSAGE];
        let body = match read_message_within(conn, &mut raw, PROMPT_TIMEOUT_MS) {
            Some(b) => b,
            None => return Err("no password given".to_string()),
        };
        let mut r = Reader::new(body);
        let pw = match r.bytes() {
            Some(p) => p,
            None => return Err("malformed password message".to_string()),
        };
        // Copied out so the buffer can be wiped
        let mut pwbuf = [0u8; proto::MAX_PASSWORD];
        let pwlen = pw.len().min(pwbuf.len());
        pwbuf[..pwlen].copy_from_slice(&pw[..pwlen]);

        // `mikuauth` verification. An unknown user costs the same Argon2id as a
        // known one, so timing does not reveal accounts
        let verdict = mikuauth::session::authenticate(
            passwd, &shadow, group, user, &pwbuf[..pwlen], today, allow_empty);
        // Checked while the bytes exist, used only for a correct password
        let shipped = mikuauth::is_shipped_password(&pwbuf[..pwlen]);
        mikuauth::ct::wipe(&mut pwbuf);
        mikuauth::ct::wipe(&mut raw);

        match verdict {
            Ok(id) => return Ok(Some((id, shipped))),
            Err(Refusal::WrongPassword) | Err(Refusal::NoSuchUser) => {}
            Err(other) => return Err(other.as_str().to_string()),
        }
    }
    Ok(None)
}

// Changing identity

/// Forks, drops to the target identity, and execs, checking every step. Groups,
/// then gid, then uid. `chdir` is after the drop
#[allow(clippy::too_many_arguments)]
fn spawn(
    cmd: &str,
    args: &[String],
    target: &db::Passwd<'_>,
    term: &str,
    cwd: &str,
    opts: &mikuauth::conf::RuleOptions,
    caller_pid: u64,
) -> i64 {
    let pid = unsafe { miku::miku_fork() };
    if pid < 0 {
        return -1;
    }
    if pid == 0 {
        // Child: joins the caller's terminal before dropping privilege, since
        // that needs admin rights. The terminal comes from the kernel by
        // `peercred`'s pid, not from the request
        if let Ok(their_tty) = miku::tty::of_pid(caller_pid) {
            if their_tty != 0 && their_tty != miku::tty::current() {
                if miku::tty::attach(their_tty).is_err() {
                    fail_child("mikudo: cannot reach the terminal you asked from");
                }
            }
        }

        let groups = read_db(mikuauth::GROUP_PATH);
        let mut gbuf = [0u16; miku::cred::MAX_GROUPS];
        let ng = db::groups_of(&groups, target.name, target.gid, &mut gbuf);

        // Every return is checked
        if miku::cred::setgroups(&gbuf[..ng]).is_err() {
            fail_child("mikudo: cannot set the target's groups");
        }
        if miku::cred::setresgid(Some(target.gid), Some(target.gid), Some(target.gid)).is_err() {
            fail_child("mikudo: cannot become the target group");
        }
        if miku::cred::setresuid(Some(target.uid), Some(target.uid), Some(target.uid)).is_err() {
            fail_child("mikudo: cannot become the target user");
        }
        // Verified afterwards
        if miku::cred::getuid() != target.uid
            || miku::cred::geteuid() != target.uid
            || miku::cred::getgid() != target.gid
        {
            fail_child("mikudo: the identity change did not take - refusing to run");
        }

        let dir = if cwd.is_empty() { "/" } else { cwd };
        let dirp = c(dir);
        if unsafe { miku::miku_chdir(dirp.as_ptr()) } < 0 {
            let rootp = c("/");
            unsafe { miku::miku_chdir(rootp.as_ptr()) };
        }

        // Kernel command: no exec. After the drop, so CAP_SYS_ADMIN is checked
        // against the target identity
        if let Some(name) = cmd.strip_prefix(KERNEL_PREFIX) {
            run_machine_command(name, args);
        }

        let env = build_env(target, term, opts);
        exec_child(cmd, args, &env);
        fail_child("mikudo: cannot run that");
    }

    // parent
    let mut status: i64 = 0;
    let reaped = unsafe { miku::miku_waitpid(pid as u64, &mut status) };
    // The exit code, or 128+signal, decoded from `wait4`'s POSIX status
    if reaped < 0 { -1 } else { miku::proc::wait_status_code(status) }
}

/// Sends a kernel command as a line of single-spaced arguments. Arguments with
/// spaces, quotes, or backslashes, or empty ones, are double-quoted
/// (`kcore::words`)
fn run_machine_command(name: &str, args: &[String]) -> ! {
    let mut line = String::from(name);
    for a in args {
        line.push(' ');
        push_word(&mut line, a);
    }
    let rc = unsafe { miku::miku_kcmd(line.as_ptr(), line.len()) };
    if rc < 0 {
        match -rc {
            1 => fail_child("mikudo: the kernel refused - that command is not one a process may ask for"),
            2 => fail_child("mikudo: command not found"),
            22 => fail_child("mikudo: the kernel would not take that command line"),
            _ => fail_child("mikudo: the kernel refused the command"),
        }
    }
    miku::exit(0);
}

/// One argument onto a kernel command line, quoted when it has to be
fn push_word(line: &mut String, a: &str) {
    let plain = !a.is_empty()
        && !a.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'' || c == '\\');
    if plain {
        line.push_str(a);
        return;
    }
    line.push('"');
    for c in a.chars() {
        if c == '"' || c == '\\' {
            line.push('\\');
        }
        line.push(c);
    }
    line.push('"');
}

fn fail_child(msg: &str) -> ! {
    miku::write(2, msg.as_bytes());
    miku::write(2, b"\n");
    miku::exit(126);
}

/// Builds the child's environment from nothing. A rule-named variable is
/// validated
fn build_env(target: &db::Passwd<'_>, term: &str, opts: &mikuauth::conf::RuleOptions) -> Vec<String> {
    let mut env = Vec::new();
    env.push(format!("PATH={SAFE_PATH}"));
    env.push(format!("HOME={}", target.home));
    env.push(format!("SHELL={}", db::DEFAULT_SHELL));
    env.push(format!("USER={}", target.name));
    env.push(format!("LOGNAME={}", target.name));

    // TERM, if it is a plain name
    if valid_term(term) {
        env.push(format!("TERM={term}"));
    } else {
        env.push("TERM=miku".to_string());
    }

    for (key, value) in &opts.setenv {
        match value {
            Some(v) if valid_env_value(v) => env.push(format!("{key}={v}")),
            Some(_) => {}
            None => {
                // A bare name copies this daemon's value, set only by mikuD
                let k = c(key);
                let got = unsafe { miku::miku_getenv(k.as_ptr()) };
                if !got.is_null() {
                    if let Some(v) = cstr_to_str(got) {
                        if valid_env_value(v) {
                            env.push(format!("{key}={v}"));
                        }
                    }
                }
            }
        }
    }
    env
}

/// Terminal name validation
fn valid_term(t: &str) -> bool {
    !t.is_empty()
        && t.len() <= 32
        && t.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.' || b == b'_')
}

/// A value may not contain NUL or newline
fn valid_env_value(v: &str) -> bool {
    v.len() <= 1024 && !v.contains('\0')
}

fn exec_child(cmd: &str, args: &[String], env: &[String]) {
    // argv[0] is the resolved path
    let mut argv_storage: Vec<alloc::vec::Vec<u8>> = Vec::new();
    argv_storage.push(c(cmd));
    for a in args {
        argv_storage.push(c(a));
    }
    let argv: Vec<*const u8> = argv_storage.iter().map(|v| v.as_ptr()).collect();

    let mut env_storage: Vec<alloc::vec::Vec<u8>> = Vec::new();
    for e in env {
        env_storage.push(c(e));
    }
    let envp: Vec<*const u8> = env_storage.iter().map(|v| v.as_ptr()).collect();

    let path = c(cmd);
    unsafe {
        miku::miku_execve(
            path.as_ptr(),
            argv.as_ptr(),
            argv.len(),
            envp.as_ptr(),
            envp.len(),
        );
    }
}

// List and passwd

fn do_list(conn: i64, user: &str, uid: u16, gnames: &[&str], gids: &[u16]) {
    let conf = match load_config() {
        Ok(c) => c,
        Err(why) => {
            send(conn, Reply::Line, &format!("the policy is unusable: {why}"));
            send(conn, Reply::End, "");
            return;
        }
    };
    let mine = conf.rules_for(user, uid, gnames, gids);
    if mine.is_empty() {
        send(conn, Reply::Line, &format!("{user} may not use mikudo at all"));
    } else {
        send(conn, Reply::Line, &format!("what {user} may do, in the order the file says it:"));
        for rule in mine {
            send(conn, Reply::Line, &format!("  line {:>3}  {}", rule.line, describe(rule)));
        }
    }
    send(conn, Reply::End, "");
}

fn describe(rule: &mikuauth::conf::Rule) -> String {
    let mut s = String::new();
    s.push_str(if rule.action == Action::Permit { "permit " } else { "deny   " });
    match &rule.target {
        Some(t) => { s.push_str("as "); s.push_str(t); }
        None => s.push_str("as anyone"),
    }
    match &rule.cmd {
        Some(cmd) => { s.push_str(", running "); s.push_str(cmd); }
        None => s.push_str(", running anything"),
    }
    if let Some(args) = &rule.args {
        if args.is_empty() {
            s.push_str(" with no arguments");
        } else {
            s.push_str(" with exactly");
            for a in args {
                s.push(' ');
                s.push_str(a);
            }
        }
    }
    if rule.options.nopass { s.push_str(" [no password]"); }
    if rule.options.persist { s.push_str(" [remembered]"); }
    if rule.options.nolog { s.push_str(" [not logged]"); }
    s
}

/// Changes a password. Only this process touches /etc/shadow. Your own requires
/// the old password; another's requires a rule allowing the caller to become
/// root
fn do_passwd(conn: i64, r: &mut Reader<'_>, peer: &miku::cred::PeerCred, caller: &str, passwd: &str) {
    let who = r.str().unwrap_or("");
    let old = r.bytes().unwrap_or(&[]);
    let new = r.bytes().unwrap_or(&[]);

    let subject = if who.is_empty() { caller } else { who };
    if db::find_user(passwd, subject).is_none() {
        send_denied(conn, "passwd: no such user");
        return;
    }

    if subject != caller {
        // Another user's password: asks the policy
        let conf = match load_config() {
            Ok(c) => c,
            Err(why) => { send_denied(conn, &format!("passwd: {why}")); return; }
        };
        let group = read_db(mikuauth::GROUP_PATH);
        let mut gbuf = [0u16; miku::cred::MAX_GROUPS];
        let caller_pw = match db::find_uid(passwd, peer.uid) {
            Some(p) => p,
            None => { send_denied(conn, "passwd: you have no account"); return; }
        };
        let ng = db::groups_of(&group, caller, caller_pw.gid, &mut gbuf);
        let gnames: Vec<&str> = db::Lines::new(&group)
            .filter_map(|(_, l)| db::Group::parse(l).ok())
            .filter(|g| gbuf[..ng].contains(&g.gid) || g.has_member(caller))
            .map(|g| g.name)
            .collect();
        let d = conf.decide(&Request {
            user: caller, uid: peer.uid, gid: caller_pw.gid,
            groups: &gbuf[..ng], group_names: &gnames,
            target: "root", cmd: "/bin/passwd", args: &[subject],
        });
        if d.action != Action::Permit {
            audit(&format!("DENIED {caller} tried to change {subject}'s password"));
            send_denied(conn, "passwd: you may not change that account's password");
            return;
        }
    } else {
        // Your own: requires the current password
        let shadow = read_db(mikuauth::SHADOW_PATH);
        match db::find_shadow(&shadow, caller) {
            Some(e) => match hash::verify(old, e.hash) {
                Verdict::Ok | Verdict::NoPassword => {}
                _ => {
                    audit(&format!("{caller} failed to prove their current password"));
                    send_denied(conn, "passwd: that is not your current password");
                    return;
                }
            },
            None => { send_denied(conn, "passwd: you have no password entry"); return; }
        }
    }

    if let Err(why) = check_password_quality(new) {
        send_denied(conn, &format!("passwd: {why}"));
        return;
    }

    match write_new_password(subject, new) {
        Ok(()) => {
            audit(&format!("{caller} changed the password for {subject}"));
            send(conn, Reply::End, "passwd: password changed");
        }
        Err(why) => send_denied(conn, &format!("passwd: {why}")),
    }
}

/// Length is the only rule
fn check_password_quality(new: &[u8]) -> Result<(), &'static str> {
    if new.len() < 8 {
        return Err("a password must be at least 8 characters");
    }
    if new.len() > proto::MAX_PASSWORD {
        return Err("that password is too long");
    }
    if new.contains(&0) || new.contains(&b'\n') || new.contains(&b':') {
        return Err("a password may not contain a colon, a newline or a NUL");
    }
    Ok(())
}

/// Rewrites /etc/shadow with one line replaced, through a temporary and rename,
/// which is atomic
fn write_new_password(user: &str, new: &[u8]) -> Result<(), String> {
    let mut salt = [0u8; 16];
    // getrandom, since the userspace PRNG is a xorshift seeded from uptime and
    // pid
    if !miku::getrandom(&mut salt) {
        return Err("no entropy for a salt".to_string());
    }

    let mut buf = [0u8; 256];
    let field = hash::encode_into(new, &salt, Default::default(), &mut buf)
        .map_err(|e| format!("cannot hash that password: {e:?}"))?;

    let shadow = read_db(mikuauth::SHADOW_PATH);
    if shadow.is_empty() {
        return Err("/etc/shadow cannot be read".to_string());
    }
    let today = miku::time::realtime_secs() / 86400;

    let mut out = String::new();
    let mut replaced = false;
    for line in shadow.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with('#') && !trimmed.is_empty() {
            if let Ok(e) = db::Shadow::parse(line) {
                if e.name == user {
                    out.push_str(&format!(
                        "{}:{}:{}:{}:{}:{}:{}:{}:\n",
                        e.name, field, today,
                        e.min_days.map(|v| v.to_string()).unwrap_or_default(),
                        e.max_days.map(|v| v.to_string()).unwrap_or_default(),
                        e.warn_days.map(|v| v.to_string()).unwrap_or_default(),
                        e.inactive_days.map(|v| v.to_string()).unwrap_or_default(),
                        e.expires.map(|v| v.to_string()).unwrap_or_default(),
                    ));
                    replaced = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        return Err("no shadow entry to change".to_string());
    }

    let tmp = "/etc/shadow.new";
    if !miku::write_file(tmp, out.as_bytes()) {
        // Usually means the machine booted from the read-only GRUB module
        return Err("cannot write /etc/shadow. The root filesystem is read-only - \
                    boot with 'root=<drive>:<lba> rootrw' for a persistent /etc, \
                    or run 'fs.rw' at the console to write to this one".to_string());
    }
    let tmpc = c(tmp);
    let dstc = c(mikuauth::SHADOW_PATH);
    // 0640 before the rename, so the umask never applies to /etc/shadow
    unsafe { miku::miku_chmod(tmpc.as_ptr(), 0o640) };
    if unsafe { miku::miku_rename(tmpc.as_ptr(), dstc.as_ptr()) } < 0 {
        unsafe { miku::miku_unlink(tmpc.as_ptr()) };
        return Err("cannot replace /etc/shadow".to_string());
    }
    Ok(())
}

// Replies and the journal

fn send(conn: i64, kind: Reply, text: &str) {
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(kind as u8).str(text);
    if let Some(msg) = w.finish() {
        unsafe { miku::miku_send(conn, msg.as_ptr(), msg.len()) };
    }
}

fn send_denied(conn: i64, why: &str) {
    send(conn, Reply::Denied, why);
}

fn send_status(conn: i64, status: i64) {
    let mut buf = [0u8; 64];
    let mut w = Writer::new(&mut buf);
    w.u8(Reply::Exited as u8).i64(status);
    if let Some(msg) = w.finish() {
        unsafe { miku::miku_send(conn, msg.as_ptr(), msg.len()) };
    }
}

/// Records every decision; denials regardless of policy
fn audit(line: &str) {
    let msg = format!("[mikudo] {line}");
    miku::klog(&msg);
}

fn klog(s: &str) {
    miku::klog(s);
}

// Helpers

/// A NUL-terminated copy, for the C-shaped syscall wrappers
fn c(s: &str) -> alloc::vec::Vec<u8> {
    let mut v = alloc::vec::Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

fn cstr_to_str(p: *const u8) -> Option<&'static str> {
    if p.is_null() {
        return None;
    }
    let mut n = 0usize;
    while n < 4096 && unsafe { *p.add(n) } != 0 {
        n += 1;
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(p, n) }).ok()
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::klog("mikudod: panic");
    miku::exit(1);
}
