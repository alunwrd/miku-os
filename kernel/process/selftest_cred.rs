// The credential model, asserted.
//
// The one that matters most is the last group: exec never raises privilege,
// which removes the class of privilege-escalation bugs of sudo, doas and
// pkexec.
//
// Run headless with `run=cred_test` on the GRUB line, or `cred test` in the
// shell. Every case is pure kernel state (no disk, no network, no userspace),
// so it runs in a few milliseconds.

extern crate alloc;
use alloc::format;
use alloc::string::String;

use crate::kcore::selftest::{Report, Runner};
use crate::process::cred::{
    self, CapSet, Cred, CAP_ALL, CAP_DAC_OVERRIDE, CAP_DAC_READ, CAP_KILL, CAP_NONE, CAP_SETGID,
    CAP_SETUID, CAP_SYS_ADMIN, CAP_SYS_TIME,
};

pub fn run() -> Report {
    let mut t = Runner::new();
    ids(&mut t);
    capabilities(&mut t);
    transitions(&mut t);
    the_exec_invariant(&mut t);
    file_permissions(&mut t);
    the_session(&mut t);
    the_session_table(&mut t);
    t.report()
}

// ids

fn ids(t: &mut Runner) {
    // CVE-2019-14287 in this system's arithmetic: truncated to sixteen bits,
    // setuid(0x10000) would store uid 0, after a check on the truncated value
    t.check(
        "an id that does not fit is refused, not truncated",
        cred::checked_id(0x10000).is_none()
            && cred::checked_id(0xFFFF).is_none()
            && cred::checked_id(u64::MAX).is_none()
            && cred::checked_id(0xFFFE) == Some(0xFFFE)
            && cred::checked_id(0) == Some(0),
        String::from("setuid(0x10000) would otherwise have been setuid(0)"),
    );

    // An ext4 volume written elsewhere can name uid 100000. The low sixteen
    // bits are a different user who may well exist here, and 65536 truncates
    // to root - a foreign disk would mount with root-owned files on it
    t.check(
        "a 32-bit on-disk owner becomes nobody, not somebody else",
        cred::fold_id(65536) == cred::UID_NOBODY
            && cred::fold_id(100_000) == cred::UID_NOBODY
            && cred::fold_id(0xFFFF) == cred::UID_NOBODY
            && cred::fold_id(1000) == 1000
            && cred::fold_id(0) == 0,
        format!("uid 65536 folds to {} rather than 0", cred::fold_id(65536)),
    );
}

// capabilities

fn capabilities(t: &mut Runner) {
    t.check(
        "a capability set holds what was put in it and nothing else",
        CAP_NONE.with(CAP_KILL).has(CAP_KILL)
            && !CAP_NONE.with(CAP_KILL).has(CAP_SETUID)
            && CAP_ALL.without(CAP_SETUID).has(CAP_SETGID)
            && !CAP_ALL.without(CAP_SETUID).has(CAP_SETUID),
        String::from("with / without / has agree"),
    );

    // A capset() that quietly dropped an unknown bit would let a future
    // kernel's rule be worked around by an older one's tolerance
    t.check(
        "a bit that is not a capability is discarded on the way in",
        CapSet(u64::MAX).sanitized() == CAP_ALL
            && CapSet(1u64 << 63).sanitized().is_empty()
            && !CapSet(u64::MAX).has(63),
        String::from("only the defined bits survive sanitized()"),
    );

    t.check(
        "subset_of is what capset checks against",
        CAP_NONE.with(CAP_KILL).subset_of(CAP_ALL)
            && !CAP_ALL.subset_of(CAP_NONE.with(CAP_KILL))
            && CAP_NONE.subset_of(CAP_NONE),
        String::from("a narrower set is a subset; a wider one is not"),
    );

    // Uid 0 that has dropped a capability does not get it back by being uid 0
    let mut stripped = Cred::root();
    stripped.effective = CAP_ALL.without(CAP_SYS_TIME);
    t.check(
        "root without a capability does not have it",
        !stripped.has(CAP_SYS_TIME) && stripped.has(CAP_SYS_ADMIN) && stripped.is_root(),
        String::from("euid 0 is not the test; the effective set is"),
    );

    // And the converse, which lets maywm run as the user
    let mut granted = Cred::user(1000, 1000);
    granted.effective = CAP_NONE.with(crate::process::cred::CAP_MAY_PRIV);
    t.check(
        "a user with a capability has it without being root",
        granted.has(crate::process::cred::CAP_MAY_PRIV)
            && !granted.is_root()
            && !granted.has(CAP_SETUID),
        String::from("the display server's globals no longer require root"),
    );
}

// identity transitions

fn transitions(t: &mut Runner) {
    let root = Cred::root();
    let user = Cred::user(1000, 1000);

    t.check(
        "an ordinary user starts with no capability at all",
        user.permitted.is_empty() && user.effective.is_empty() && !user.has(CAP_DAC_OVERRIDE),
        String::from("Cred::user grants nothing"),
    );

    t.check(
        "a user may return to an id they already hold, and to no other",
        user.may_become_euid(1000)
            && !user.may_become_euid(0)
            && !user.may_become_euid(1001)
            && root.may_become_euid(0),
        String::from("the POSIX saved-set-id rule"),
    );

    // A process that lowered itself for one operation must be able to come
    // back: suid
    let mut dropped = Cred::root();
    dropped.euid = 1000;
    t.check(
        "a saved id is a way back that needs no capability",
        dropped.may_become_euid(0) && dropped.suid == 0 && dropped.euid == 1000,
        String::from("euid moved, suid remembers root"),
    );

    // The irreversible form, used by login and mikudod: nothing the session
    // later runs can find its way back
    let mut becoming = Cred::root();
    becoming.uid = 1000;
    becoming.euid = 1000;
    becoming.suid = 1000;
    becoming.drop_all_caps();
    t.check(
        "becoming a user for good leaves nothing behind",
        becoming.permitted.is_empty()
            && becoming.effective.is_empty()
            && !becoming.may_become_euid(0)
            && !becoming.has(CAP_SETUID),
        String::from("no id, no capability, and no way back"),
    );

    let mut in_groups = Cred::user(1000, 1000);
    in_groups.set_groups(&[1000, 10, 100]);
    t.check(
        "supplementary groups are carried and matched",
        in_groups.in_group(10)
            && in_groups.in_group(100)
            && in_groups.in_group(1000)
            && !in_groups.in_group(44),
        String::from("wheel membership is what /etc/mikudo.conf matches on"),
    );

    // The array is fixed-size and the input is not
    let mut many = Cred::user(1000, 1000);
    let long: alloc::vec::Vec<u16> = (0..100u16).collect();
    many.set_groups(&long);
    t.check(
        "more groups than fit are dropped, not written past the array",
        many.ngroups as usize == crate::fs::vfs::types::MAX_GROUPS,
        format!("kept {} of 100", many.ngroups),
    );
}

// the invariant

fn the_exec_invariant(t: &mut Runner) {
    // exec() reads a file's mode bits, `ls` renders them, and the loader
    // ignores them. No path by which running a program raises the process that
    // runs it, so exec cannot be a step in an attack
    let user = Cred::user(1000, 1000);
    let after = user.after_exec();
    t.check(
        "exec does not raise an unprivileged process",
        after.permitted.is_empty()
            && after.effective.is_empty()
            && after.uid == 1000
            && after.euid == 1000,
        String::from("a user execing a mode-4755 root-owned file is still that user"),
    );

    // The mode bits exist and are readable. This would catch somebody wiring
    // them up to the loader
    let setuid_mode = crate::fs::vfs::types::FileMode::new(0o4755);
    t.check(
        "a setuid bit is recorded, reported, and does nothing",
        setuid_mode.setuid()
            && Cred::user(1000, 1000).after_exec().effective.is_empty(),
        String::from("mode 4755 is visible to ls and invisible to exec"),
    );

    // Authority survives its own exec, which is what lets mikuD start a
    // service that needs a capability
    let root = Cred::root();
    let after_root = root.after_exec();
    t.check(
        "a privileged process keeps what it had across exec",
        after_root.effective == CAP_ALL && after_root.euid == 0,
        String::from("mikuD can still start a service that needs authority"),
    );

    // The bounding set is the ceiling, and it only ever cuts
    let mut confined = Cred::root();
    confined.bounding = CAP_NONE.with(CAP_KILL);
    let after_confined = confined.after_exec();
    t.check(
        "the bounding set cuts the permitted set at every exec",
        after_confined.effective == CAP_NONE.with(CAP_KILL)
            && !after_confined.has(CAP_SETUID)
            && !after_confined.has(CAP_DAC_OVERRIDE),
        String::from("a confined subtree stays confined through exec"),
    );

    // Repeatedly: a transition growing anything on the second application would
    // be a slow leak
    let twice = after_confined.after_exec().after_exec();
    t.check(
        "the transition is idempotent - it cannot grow on a later exec",
        twice.effective == after_confined.effective && twice.permitted == after_confined.permitted,
        String::from("exec is a fixed point once the sets have been cut"),
    );
}

// what the VFS asks

fn file_permissions(t: &mut Runner) {
    use crate::fs::vfs::permissions::{check_access, AccessMode};
    use crate::fs::vfs::types::{FileMode, PermWho};

    let owner = Cred::user(1000, 1000);
    let other = Cred::user(1001, 1001);
    let mode = FileMode::new(0o640);

    t.check(
        "the three permission triples are told apart",
        owner.who_for(1000, 1000) == PermWho::Owner
            && other.who_for(1000, 1000) == PermWho::Other
            && Cred::user(1001, 1000).who_for(1000, 1000) == PermWho::Group,
        String::from("owner, group, other"),
    );

    t.check(
        "a 0640 file is readable by its owner and not by a stranger",
        check_access(mode, 1000, 1000, &owner, AccessMode::Read)
            && !check_access(mode, 1000, 1000, &other, AccessMode::Read)
            && !check_access(mode, 1000, 1000, &other, AccessMode::Write),
        String::from("/etc/shadow is exactly this file"),
    );

    // The narrow capability, which is what a backup wants: everything opens,
    // nothing changes
    let mut reader = Cred::user(1002, 1002);
    reader.effective = CAP_NONE.with(CAP_DAC_READ);
    t.check(
        "CAP_DAC_READ opens a file it may not write",
        check_access(mode, 1000, 1000, &reader, AccessMode::Read)
            && !check_access(mode, 1000, 1000, &reader, AccessMode::Write),
        String::from("read and search, and no more than that"),
    );

    let mut writer = Cred::user(1003, 1003);
    writer.effective = CAP_NONE.with(CAP_DAC_OVERRIDE);
    t.check(
        "CAP_DAC_OVERRIDE opens and writes",
        check_access(mode, 1000, 1000, &writer, AccessMode::Read)
            && check_access(mode, 1000, 1000, &writer, AccessMode::Write),
        String::from("the broad one"),
    );

    // Linux draws the line here too: exec of a data file is nearly always a bug
    // being papered over
    let data = FileMode::new(0o600);
    t.check(
        "no capability makes a file with no execute bit executable",
        !check_access(data, 1000, 1000, &writer, AccessMode::Exec)
            && !check_access(data, 1000, 1000, &reader, AccessMode::Exec),
        String::from("CAP_DAC_OVERRIDE stops short of exec"),
    );

    // uid 0 with an empty set gets nothing
    let hollow = Cred {
        permitted: CAP_NONE,
        effective: CAP_NONE,
        ..Cred::root()
    };
    t.check(
        "uid 0 with no capabilities is measured against the mode bits",
        !check_access(FileMode::new(0o600), 1000, 1000, &hollow, AccessMode::Read),
        String::from("root is a capability set, not a number"),
    );

    let sticky = FileMode::new(0o1777);
    t.check(
        "the sticky bit is readable from a mode",
        sticky.sticky() && !FileMode::new(0o777).sticky() && FileMode::new(0o4755).setuid(),
        String::from("/tmp is 1777 and unlink consults it"),
    );
}

// the console session

fn the_session(t: &mut Runner) {
    use crate::process::cred::{CAP_MAY_PRIV, CAP_SYS_BOOT};
    use crate::process::session::{Kind, Session};
    use crate::fs::vfs::types::MAX_GROUPS;

    // What syscall 92 builds, from a real `Session`. It is never put in the
    // table: the syscall's effect is on another process, and a selftest logging
    // the console in would leave the machine somebody else
    let session = Session {
        id: 1, uid: 1000, gid: 1000,
        groups: [0; MAX_GROUPS], ngroups: 0,
        kind: Kind::Console, seat: 0, leader: 0, opened_tick: 0,
    }.cred();

    // The two capabilities a session gets. Without the display, starting the
    // desktop fails at the handshake, because maywm, mayterm and aosh each bind
    // a privileged global. Without the power button a desktop cannot shut the
    // machine down from its own menu
    let expect = CapSet::empty().with(CAP_MAY_PRIV).with(CAP_SYS_BOOT);
    t.check(
        "a session may command the display and switch the machine off, and nothing else",
        session.has(CAP_MAY_PRIV)
            && session.has(CAP_SYS_BOOT)
            && !session.has(CAP_SETUID)
            && !session.has(CAP_DAC_OVERRIDE)
            && !session.has(CAP_SYS_ADMIN),
        String::from("may_priv and sys_boot - the window manager cannot read /etc/shadow \
                      and cannot mount a disk"),
    );

    // The bounding set is the same two bits, so nothing started from the
    // session can acquire anything the session did not have
    t.check(
        "nothing a session starts can gain more than the session had",
        session.after_exec().effective == expect && session.bounding == expect,
        String::from("the ceiling travels with the subtree"),
    );

    // A session is refused what its uid is refused
    {
        use crate::fs::vfs::permissions::{check_access, AccessMode};
        use crate::fs::vfs::types::FileMode;
        let shadow = FileMode::new(0o640);
        t.check(
            "a session is refused a file its uid is refused",
            !check_access(shadow, 0, 42, &session, AccessMode::Read)
                && check_access(shadow, 0, 42, &Cred::root(), AccessMode::Read),
            String::from("/etc/shadow is 0640 root:shadow; the session is neither"),
        );
    }

    // A session may drop what it holds, and may not pick anything up. libmay's
    // spawn relies on this to take CAP_MAY_PRIV from a program launched off a
    // keybind
    let mut narrowed = session;
    narrowed.permitted = CapSet::empty();
    narrowed.effective = CapSet::empty();
    narrowed.bounding  = CapSet::empty();
    t.check(
        "a launched program can be given less than the session",
        narrowed.after_exec().effective.is_empty()
            && !narrowed.has(CAP_MAY_PRIV),
        String::from("a terminal started from a keybind cannot take the screen"),
    );

    // root logging in is still root: `logout` restores exactly this
    t.check(
        "root's session keeps everything",
        Cred::root().effective == CAP_ALL && Cred::root().uid == 0,
        String::from("logout hands the console back to this"),
    );
}

// the session table

/// Sessions as objects, not as a credential on a thread
///
/// These run against the live table, so they open and close real sessions. A
/// console session opened here writes the identity onto the shell thread, and
/// the close at the end of each case puts it back, which is also asserted.
///
/// The cases needing a process (a graphical session's leader and the seat
/// handover) are skipped. A session pointing at a pid that is not a process is
/// what `open` refuses, and that refusal is asserted
fn the_session_table(t: &mut Runner) {
    use crate::process::cred::CAP_MAY_PRIV;
    use crate::process::session::{self, Kind, OpenError, SEAT0};

    // Nothing may be open before this runs, or the count assertions below are
    // about whatever the machine was doing
    let already = {
        let mut buf = [session::Session::default(); session::MAX_SESSIONS];
        session::list(&mut buf)
    };
    if already != 0 {
        t.skip(
            "the session table",
            format!("{} session(s) already open - not touching a live login", already),
        );
        return;
    }

    // A graphical session names a process. One naming nothing or a pid not
    // running is refused: nothing could ever close it, and it would hold a seat
    // for ever
    t.check(
        "a graphical session must name a running leader",
        session::open(1000, 1000, &[1000], Kind::Graphical, SEAT0, 0)
            == Err(OpenError::NoLeader)
            && session::open(1000, 1000, &[1000], Kind::Graphical, SEAT0, 0xFFFF_FFFF)
                == Err(OpenError::NoLeader),
        String::from("a session with no leader is a seat nobody can take back"),
    );

    // One seat on this machine, and it is named rather than assumed
    t.check(
        "there is no seat 1",
        session::open(1000, 1000, &[1000], Kind::Console, 1, 0) == Err(OpenError::NoSeat),
        String::from("a second screen is a change, not an accident"),
    );

    let id = match session::open(1000, 1000, &[1000, 10], Kind::Console, SEAT0, 0) {
        Ok(id) => id,
        Err(e) => {
            t.check("a console session opens", false, format!("{:?}", e));
            return;
        }
    };

    t.check(
        "an open session is findable by its id and in the list",
        session::by_id(id).map(|s| (s.uid, s.gid, s.ngroups)) == Some((1000, 1000, 2))
            && session::seat_has_session(SEAT0),
        format!("session {} on seat {}", id, SEAT0),
    );

    // The identity the session publishes is the one the console runs with, with
    // exactly two capabilities: the screen and the power button. The set is
    // spelled out, because what is not in it matters
    if let Some(s) = session::by_id(id) {
        let c = s.cred();
        let want = CapSet::empty()
            .with(CAP_MAY_PRIV)
            .with(crate::process::cred::CAP_SYS_BOOT);
        t.check(
            "a session holds may_priv and sys_boot and nothing else",
            c.effective == want
                && c.permitted == c.effective
                && c.bounding == c.effective
                && c.uid == 1000,
            String::from("it may command the desktop and switch the machine off, \
                          and may not read /etc/shadow"),
        );
    }

    // A second console login on the same seat replaces the first rather than
    // adding to it. Two consoles is one console
    let second = session::open(1001, 1001, &[1001], Kind::Console, SEAT0, 0);
    let after = {
        let mut buf = [session::Session::default(); session::MAX_SESSIONS];
        session::list(&mut buf)
    };
    t.check(
        "logging in twice at one console is one session",
        second.is_ok() && after == 1,
        format!("{} open after the second login", after),
    );
    t.check(
        "the id is not reused",
        matches!(second, Ok(new) if new != id) && session::by_id(id).is_none(),
        String::from("a recycled id would answer about a session that has ended"),
    );

    // Closing the last one puts the console back to root, which is what
    // physical access at a ring-0 console means
    let last = second.unwrap_or(0);
    t.check(
        "closing the session empties the seat",
        session::close(last) && !session::seat_has_session(SEAT0),
        String::from("and closing it twice is false, not a second close"),
    );
    t.check(
        "closing a session that is not there says so",
        !session::close(last) && !session::close(0xDEAD_BEEF),
        String::from("no such session is an answer, not a panic"),
    );

    // Not tested here: filling the table. Two sessions of the same kind on one
    // seat replace each other, so eight opens on seat 0 leave one session.
    //
    // Also not tested: a graphical session's leader. `open` writes the identity
    // onto that process and `close` kills it, and the only pid this test could
    // name is the shell thread it runs on
    t.skip(
        "a graphical session's leader",
        String::from("it would have to be this thread, and closing one kills its leader"),
    );

    let left = {
        let mut buf = [session::Session::default(); session::MAX_SESSIONS];
        session::list(&mut buf)
    };
    t.check(
        "the test left nothing behind",
        left == 0 && !session::seat_has_session(SEAT0),
        String::from("a test that left the console owned by somebody would be worse than no test"),
    );
}

pub fn cmd_selftest() {
    crate::cprintln!(57, 197, 187, "cred: credential and capability selftest");
    let report = run();

    for c in &report.cases {
        if c.skipped {
            crate::cprintln!(220, 220, 100, "  skip  {}", c.name);
        } else if c.ok {
            crate::cprintln!(100, 220, 150, "  ok    {}", c.name);
        } else {
            crate::print_error!("  FAIL  {}", c.name);
        }
        crate::println!("          {}", c.detail);
        crate::serial_println!("[cred-test] {} {} - {}",
            if c.skipped { "SKIP" } else if c.ok { "ok  " } else { "FAIL" },
            c.name, c.detail);
    }

    let (pass, fail, skip) = (report.passed(), report.failed(), report.skipped());
    crate::serial_println!("[cred-test] {} passed, {} failed, {} skipped", pass, fail, skip);
    if fail == 0 {
        crate::print_success!("cred: {} passed, {} skipped", pass, skip);
    } else {
        crate::print_error!("cred: {} passed, {} FAILED, {} skipped", pass, fail, skip);
    }
}
