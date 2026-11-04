#![no_std]
#![no_main]

//! login: asks for credentials at the console and passes them to mikudod
//!
//! Holds no authority: same uid as its parent and an empty capability set.
//! mikudod reads the shadow file, runs the hash, and asks the kernel to open
//! the session.
//!
//! No setuid exists and `exec` never raises privileges. Authority is given by a
//! privileged process to a child, through the one logged process that does so.

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use mikuauth::client::{self, Ask, Refused};

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    // Warns if started with capabilities, since a unit file grants what is not
    // needed
    if miku::cred::has_cap(miku::cred::CAP_SETUID) {
        miku::klog("[login] started with CAP_SETUID, which it no longer uses - \
                    check the unit that starts it");
    }

    banner();

    let mut term = Terminal::default();
    loop {
        let user = match prompt_line("login: ") {
            Some(u) if !u.is_empty() => u,
            Some(_) => continue,
            None => miku::exit(0),
        };

        // Cleared per attempt
        term.notes.clear();
        match client::open_session(&user, client::CONSOLE, client::SEAT0, &mut term) {
            Ok(id) => welcome(&user, id, &term.notes),
            Err(Refused::NoDaemon) => {
                // No login service, so a password cannot be checked
                eprintln("login: the login service is not running");
                eprintln("login: start it with `mikuctl start mikudod`, or check the journal");
                miku::exit(1);
            }
            Err(why) => {
                println(&format!("login: {}", why.message()));
                miku::sleep_ms(FAIL_DELAY_MS);
            }
        }
    }
}

/// Delay after a refusal, in milliseconds, to slow scripted guessing. mikudod
/// counts attempts
const FAIL_DELAY_MS: u64 = 2000;

/// The console's `Ask`: a prompt with echo off. Notes are held, since they
/// arrive before the session id and should not print above "Welcome"
#[derive(Default)]
struct Terminal {
    notes: Vec<String>,
}

impl Ask for Terminal {
    fn password(&mut self, user: &str) -> Option<Vec<u8>> {
        prompt_password(&format!("password for {user}: "))
    }
    fn note(&mut self, line: &str) {
        self.notes.push(line.to_string());
    }
}

/// Prints the greeting once the kernel has the session. The session is already
/// open, so this is cosmetic
fn welcome(user: &str, id: u32, notes: &[String]) {
    println(&format!("\nWelcome, {user}."));
    for n in notes {
        println(&format!("  {n}"));
    }
    println("  The console is yours. `id` says who you are, `mikudo` does what");
    println("  needs the machine, and `logout` hands it back.");
    println("");
    miku::klog(&format!("[login] {user} logged in on the console (session {id})"));
    miku::exit(0);
}

/// The default-password warning comes from mikudod as a `Reply::Line`, printed
/// by `Terminal::note`

fn banner() {
    println("");
    println("MikuOS");
    println("");
}

fn prompt_line(prompt: &str) -> Option<String> {
    miku::write(1, prompt.as_bytes());
    let mut out = String::new();
    loop {
        let mut ch = [0u8; 1];
        if miku::read(0, &mut ch) <= 0 {
            return None;
        }
        if ch[0] == b'\n' || ch[0] == b'\r' {
            return Some(out);
        }
        // Accepts name characters only
        if out.len() < 32
            && (ch[0].is_ascii_alphanumeric() || ch[0] == b'_' || ch[0] == b'-' || ch[0] == b'.')
        {
            out.push(ch[0] as char);
        }
    }
}

/// Reads a line with echo off, restored by `miku::tty::Saved`'s `Drop` on every
/// path
fn prompt_password(prompt: &str) -> Option<Vec<u8>> {
    miku::write(1, prompt.as_bytes());

    let _echo_off = miku::tty::Saved::change(|t| t.echo = false);

    let mut out = Vec::new();
    let got = loop {
        let mut ch = [0u8; 1];
        if miku::read(0, &mut ch) <= 0 {
            break None;
        }
        if ch[0] == b'\n' || ch[0] == b'\r' {
            break Some(());
        }
        // Refused rather than truncated
        if out.len() >= mikuauth::proto::MAX_PASSWORD {
            break None;
        }
        out.push(ch[0]);
    };

    miku::write(1, b"\n");
    got.map(|_| out)
}

fn println(s: &str) { miku::write(1, s.as_bytes()); miku::write(1, b"\n"); }
fn eprintln(s: &str) { miku::write(2, s.as_bytes()); miku::write(2, b"\n"); }

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::klog("login: panic");
    miku::exit(1);
}
