//! A terminal created like Zed's: `rustix-openpty` calls `libc::openpty`
//! (`/dev/ptmx`, `/dev/ptsN`, lib/libmiku/c/pty.rs), then `login_tty` in the
//! child.
//!
//! Checks both ends and the slave's name, the initial size, `tcgetattr`
//! defaults (canonical, echo), `cfmakeraw` with `tcsetattr`, and a child on the
//! slave writing to the master and returning its exit status.

use std::os::fd::{AsRawFd, IntoRawFd};

use rustix::termios::{self, Winsize};

fn check(name: &str, ok: bool, detail: String, failed: &mut i32) {
    if ok {
        println!("  ok    {name}");
    } else {
        println!("  FAIL  {name}: {detail}");
        *failed += 1;
    }
}

fn main() {
    println!("stdpty: a terminal through rustix-openpty");
    let mut failed = 0;

    let size = Winsize { ws_row: 30, ws_col: 100, ws_xpixel: 0, ws_ypixel: 0 };
    let pty = match rustix_openpty::openpty(None, Some(&size)) {
        Ok(p) => p,
        Err(e) => {
            println!("  FAIL  openpty: {e}");
            println!("stdpty: FAILURES");
            std::process::exit(1);
        }
    };
    let (m, s) = (pty.controller.as_raw_fd(), pty.user.as_raw_fd());
    check("openpty: a master and a slave", m >= 0 && s >= 0 && m != s, format!("{m} {s}"), &mut failed);

    let mut name = [0u8; 32];
    let r = unsafe { libc::ptsname_r(m, name.as_mut_ptr().cast(), name.len()) };
    let name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(0)]).into_owned();
    check("ptsname_r names the slave as /dev/ptsN", r == 0 && name.starts_with("/dev/pts"),
        format!("{r} {name:?}"), &mut failed);

    let got = termios::tcgetwinsize(&pty.user);
    check("the size given to openpty is the terminal's",
        matches!(&got, Ok(w) if w.ws_row == 30 && w.ws_col == 100), format!("{got:?}"), &mut failed);

    let t = termios::tcgetattr(&pty.user);
    let cooked = t.as_ref().map(|t| {
        t.local_modes.contains(termios::LocalModes::ICANON) && t.local_modes.contains(termios::LocalModes::ECHO)
    });
    check("a fresh terminal is canonical and echoing", cooked == Ok(true), format!("{cooked:?}"), &mut failed);

    let raw_ok = t.and_then(|mut t| {
        t.make_raw();
        termios::tcsetattr(&pty.user, termios::OptionalActions::Now, &t)?;
        termios::tcgetattr(&pty.user)
    });
    check("make_raw and tcsetattr take it out of that, and it reads back so",
        matches!(&raw_ok, Ok(t) if !t.local_modes.contains(termios::LocalModes::ICANON)
            && !t.local_modes.contains(termios::LocalModes::ECHO)),
        format!("{:?}", raw_ok.map(|t| t.local_modes)), &mut failed);

    // A child in the terminal, the way a shell is started in one
    let slave = pty.user.into_raw_fd();
    let pid = unsafe { libc::fork() };
    if pid == 0 {
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(slave) };
        if rustix_openpty::login_tty(fd).is_err() {
            unsafe { libc::_exit(90) };
        }
        let msg = b"hello from the slave\n";
        unsafe {
            libc::write(1, msg.as_ptr().cast(), msg.len());
            libc::_exit(7);
        }
    }
    unsafe { libc::close(slave) };
    let mut seen = Vec::new();
    let mut buf = [0u8; 256];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while std::time::Instant::now() < deadline && !String::from_utf8_lossy(&seen).contains("hello from the slave") {
        let mut p = libc::pollfd { fd: m, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut p, 1, 200) } > 0 {
            let n = unsafe { libc::read(m, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 { break; }
            seen.extend_from_slice(&buf[..n as usize]);
        }
    }
    let mut st = 0;
    let waited = unsafe { libc::waitpid(pid, &mut st, 0) };
    check("a child's login_tty, then its output read from the master",
        String::from_utf8_lossy(&seen).contains("hello from the slave"),
        format!("{:?}", String::from_utf8_lossy(&seen)), &mut failed);
    check("and its exit status comes back",
        waited == pid && libc::WIFEXITED(st) && libc::WEXITSTATUS(st) == 7,
        format!("waited {waited} status {st:#x}"), &mut failed);
    drop(pty.controller);

    println!("stdpty: {}", if failed == 0 { "all passed" } else { "FAILURES" });
    std::process::exit(if failed == 0 { 0 } else { 1 });
}

use std::os::fd::FromRawFd;
