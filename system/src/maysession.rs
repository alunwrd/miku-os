#![no_std]
#![no_main]

//! maysession: the graphical session program
//!
//! Field seven of /etc/passwd, started by mikudod as the logged-in user and
//! recorded as the session's leader. Reads `/etc/mayland/session.d/*.conf`,
//! starts what it names, and stays alive: the kernel closes a session when its
//! leader exits (`session::on_process_exit`).
//!
//! Passes `CAP_MAY_PRIV` only to the programs listed here, which bind
//! privileged globals. Programs launched later do not get it. `spawn::run_with`
//! is where this happens.

extern crate alloc;

pub use miku;

pub use libmay::conf;
pub use libmay::manifest;
pub use libmay::spawn;

use libmay::conf::{Note, Sink, Text, Where};

/// Where a session is written down
const SESSION_D: &str = "/etc/mayland/session.d";

/// How many programs one session may start
const MAX_PROGRAMS: usize = 8;

/// Default programs with no config
const BUILT_IN: [&str; 3] = ["/bin/maywm", "/bin/aosh", "/bin/mayterm"];

fn say(msg: &str, detail: &str) {
    miku::print("  maysession: ");
    miku::print(msg);
    if !detail.is_empty() {
        miku::print(" - ");
        miku::print(detail);
    }
    miku::println("");
}

/// Session config: programs, and the one holding the session open
struct Session {
    programs: [Text; MAX_PROGRAMS],
    n: usize,
    /// Program whose exit ends the session; empty for all of them
    main: Text,
}

impl Session {
    fn new() -> Session {
        Session { programs: [Text::EMPTY; MAX_PROGRAMS], n: 0, main: Text::EMPTY }
    }
}

impl Sink for Session {
    fn key(&mut self, at: Where, section: &str, key: &str, value: &str) {
        if section != "session" && !section.is_empty() {
            return;
        }
        match key {
            "main" => self.main = Text::from(value.trim()),
            _ => {
                let _ = (at, key);
            }
        }
    }

    /// `start = /bin/maywm`, additive across lines and files
    fn list(&mut self, at: Where, key: &str, value: &str) {
        if key != "start" {
            return;
        }
        let cmd = value.trim();
        if cmd.is_empty() {
            return;
        }
        if self.n == MAX_PROGRAMS {
            let _ = at;
            say("more programs than a session may start", cmd);
            return;
        }
        self.programs[self.n] = Text::from(cmd);
        self.n += 1;
    }

    fn is_list(&self, key: &str) -> bool {
        key == "start"
    }

    fn note(&mut self, _at: Where, _note: Note, text: &str) {
        say("in the session config", text);
    }
}

fn read_session() -> Session {
    let mut s = Session::new();
    let mut files = manifest::DiskFiles;
    let m = manifest::Manifest::build(&mut files, &[manifest::Layer::Dir(SESSION_D)]);
    if let Some((note, text)) = m.complaint() {
        say(note.as_str(), text.as_str());
    }
    let vars = conf::Vars::new();
    for &file in m.roots() {
        conf::parse_into(m.text(file), &mut s, &vars, file);
    }
    if s.n == 0 {
        say("nothing is written down in /etc/mayland/session.d", "starting the built-in desktop");
        for cmd in BUILT_IN {
            s.programs[s.n] = Text::from(cmd);
            s.n += 1;
        }
    }
    s
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    // Waits to be a session leader before starting anything. mikudod forks this
    // process before asking the kernel to open the session, which grants
    // CAP_MAY_PRIV. Polled and bounded
    if !wait_for_the_session() {
        say("the kernel never opened a session for this process", "not starting a desktop");
        miku::exit(1);
    }

    // Warns if the capability is missing; not fatal
    if !miku::cred::has_cap(miku::cred::CAP_MAY_PRIV) {
        say("this session holds no may_priv", "the window manager will be refused");
    }

    // `XDG_CURRENT_DESKTOP`, inherited by everything below
    // (`etc/aosh/negi/services/WM.qml`)
    unsafe {
        miku::miku_setenv(b"XDG_CURRENT_DESKTOP\0".as_ptr(), b"Mayland\0".as_ptr());
    }

    let session = read_session();
    let mut pids = [0u64; MAX_PROGRAMS];
    let mut main_pid = 0u64;

    for i in 0..session.n {
        let cmd = session.programs[i].as_str();
        // Keeps the capability for listed programs only; later launches go
        // through `spawn::run`
        match spawn::run_with(cmd, true) {
            Ok(pid) => {
                pids[i] = pid;
                if !session.main.is_empty() && session.main.as_str() == cmd {
                    main_pid = pid;
                }
            }
            Err(e) => say(cmd, e.as_str()),
        }
    }

    // With no `main`, the first program that started
    if main_pid == 0 {
        main_pid = pids.iter().copied().find(|p| *p != 0).unwrap_or(0);
    }
    if main_pid == 0 {
        say("nothing started", "ending the session");
        miku::exit(1);
    }

    miku::klog("[maysession] the desktop is up");

    // Reaps other children as they exit; ends when the main program does
    loop {
        let mut status = 0i64;
        // `u64::MAX` is any child (`find_zombie_child`)
        let pid = unsafe { miku::miku_waitpid(u64::MAX, &mut status as *mut i64) };
        if pid < 0 {
            // No children left
            break;
        }
        if pid as u64 == main_pid {
            break;
        }
    }

    miku::klog("[maysession] the desktop is over; the session ends with it");
    miku::exit(0);
}

/// Polls until this process leads a session, for at most 2 s
fn wait_for_the_session() -> bool {
    const WAIT_MS: u64 = 2000;
    const STEP_MS: u64 = 10;
    let mut waited = 0;
    while waited < WAIT_MS {
        if miku::session::current() != 0 {
            return true;
        }
        miku::sleep_ms(STEP_MS);
        waited += STEP_MS;
    }
    // Final check, in case the last sleep lost the race
    miku::session::current() != 0
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::klog("maysession: panic");
    miku::exit(1);
}
