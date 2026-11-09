#![no_std]
#![no_main]

//! mikudo: runs a command as another user
//!
//! The unprivileged half: same uid, groups, and empty capability set as its
//! parent. mikudod makes the decisions.
//!
//! 1. parses its command line
//! 2. connects to /run/mikudo.sock
//! 3. sends the request
//! 4. reads the password when asked and sends it
//! 5. relays signals and exits with the command's status
//!
//! Usage:
//!
//!   mikudo [-u user] [-n] [-s] [-l] [-c] [-L] [--] command [args...]
//!
//!     -u user   become this user instead of root
//! -n        never prompt; fail if a password is needed
//!     -s        run the target's shell
//! -l        list what is allowed, running nothing
//! -c        report whether the command is allowed, running nothing
//! -L        forget remembered authentication on this terminal
//!        everything after is the command

extern crate alloc;

pub use miku;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use mikuauth::client::Conn;
use mikuauth::proto::{self, flags, Op, Reader, Reply, Writer};

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let cli = match parse_args(&args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln(&msg);
            eprintln(USAGE);
            miku::exit(2);
        }
    };

    // Socket, framing, and length checks are `mikuauth::client`'s. The loop
    // below maps the last reply to an exit status
    let conn = match Conn::open() {
        Some(c) => c,
        None => {
            eprintln("mikudo: the elevation service is not running");
            eprintln("mikudo: start it with `mikuctl start mikudod`, or check the journal");
            miku::exit(1);
        }
    };

    // No descriptors are sent: MikuOS's fds 0, 1, and 2 are routed by the
    // syscall layer, not table entries. Only the foreground process gets
    // keystrokes, so the prompt happens here on Reply::NeedPassword

    if !send_request(&conn, &cli) {
        eprintln("mikudo: cannot send the request");
        miku::exit(1);
    }

    miku::exit(read_reply(&conn) as i64);
}

const USAGE: &str = "usage: mikudo [-u user] [-n] [-s] [-l] [-c] [-L] [--] command [args...]";

struct Cli {
    op: Op,
    target: String,
    flags: u16,
    args: Vec<String>,
}

fn parse_args(args: &[&str]) -> Result<Cli, String> {
    let mut cli = Cli {
        op: Op::Run,
        target: "root".to_string(),
        flags: 0,
        args: Vec::new(),
    };

    let mut i = 1usize;
    while i < args.len() {
        let a = args[i];
        if !a.starts_with('-') || a == "-" {
            break;
        }
        match a {
            // `--` keeps the command's options out of this parser
            "--" => { i += 1; break; }
            "-n" => cli.flags |= flags::NON_INTERACTIVE,
            "-s" => cli.flags |= flags::SHELL,
            "-l" => cli.op = Op::List,
            "-c" => cli.op = Op::Check,
            "-L" => cli.op = Op::ClearPersist,
            "-u" => {
                i += 1;
                let u = args.get(i).ok_or_else(|| "mikudo: -u needs a user name".to_string())?;
                if !valid_user(u) {
                    return Err("mikudo: that is not a user name".to_string());
                }
                cli.target = u.to_string();
            }
            "-h" | "--help" => return Err(USAGE.to_string()),
            other => return Err(alloc::format!("mikudo: unknown option {other}")),
        }
        i += 1;
    }

    for a in &args[i..] {
        if cli.args.len() >= proto::MAX_ARGS {
            return Err("mikudo: too many arguments".to_string());
        }
        cli.args.push(a.to_string());
    }

    if cli.op == Op::Run
        && cli.args.is_empty()
        && cli.flags & flags::SHELL == 0
    {
        return Err("mikudo: no command given".to_string());
    }
    Ok(cli)
}

/// Validates a user name; mikudod checks again
fn valid_user(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && !s.starts_with('-')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-' || b == b'.')
}

fn send_request(conn: &Conn, cli: &Cli) -> bool {
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(cli.op as u8).u16(cli.flags);

    if matches!(cli.op, Op::Run | Op::Check) {
        w.str(&cli.target);
        w.str(&cwd());
        w.str(&term());
        w.u16(cli.args.len() as u16);
        for a in &cli.args {
            w.str(a);
        }
    }

    match w.finish() {
        Some(msg) => conn.send(msg),
        None => false,
    }
}

fn cwd() -> String {
    let mut buf = [0u8; 4096];
    let p = unsafe { miku::miku_getcwd(buf.as_mut_ptr(), buf.len()) };
    if p.is_null() {
        return "/".to_string();
    }
    let n = buf.iter().position(|&b| b == 0).unwrap_or(0);
    core::str::from_utf8(&buf[..n]).unwrap_or("/").to_string()
}

fn term() -> String {
    let key = cstring("TERM");
    let v = unsafe { miku::miku_getenv(key.as_ptr()) };
    if v.is_null() {
        return "miku".to_string();
    }
    let mut n = 0usize;
    while n < 64 && unsafe { *v.add(n) } != 0 {
        n += 1;
    }
    core::str::from_utf8(unsafe { core::slice::from_raw_parts(v, n) })
        .unwrap_or("miku")
        .to_string()
}

/// Reads replies until done and maps the last to an exit status: the command's
/// own, 1 for refusal, 126 for not runnable, 127 for no service
fn read_reply(conn: &Conn) -> i32 {
    let mut raw = [0u8; proto::MAX_MESSAGE];
    loop {
        let body = match conn.recv(&mut raw) {
            Some(b) => b,
            None => {
                // The daemon left without answering
                eprintln("mikudo: the elevation service closed the connection");
                return 1;
            }
        };
        let mut r = Reader::new(body);
        let kind = match r.u8().and_then(Reply::from_u8) {
            Some(k) => k,
            None => return 1,
        };
        match kind {
            Reply::Exited => {
                let status = r.i64().unwrap_or(1);
                // A command killed by a signal reports 128 + signal
                return (status & 0xff) as i32;
            }
            Reply::Denied => {
                eprintln(r.str().unwrap_or("mikudo: refused"));
                return 1;
            }
            Reply::Would => {
                println(r.str().unwrap_or("permitted"));
                return 0;
            }
            Reply::Line => println(r.str().unwrap_or("")),
            Reply::End => {
                let s = r.str().unwrap_or("");
                if !s.is_empty() {
                    println(s);
                }
                return 0;
            }
            Reply::NeedPassword => {
                let user = r.str().unwrap_or("");
                match prompt_password(&alloc::format!("[mikudo] password for {user}: ")) {
                    Some(pw) => {
                        if !send_password(conn, &pw) {
                            eprintln("mikudo: could not send the password");
                            return 1;
                        }
                    }
                    None => {
                        eprintln("");
                        return 1;
                    }
                }
            }
            Reply::Error => {
                eprintln(r.str().unwrap_or("mikudo: something went wrong"));
                return 126;
            }
        }
    }
}

fn send_password(conn: &Conn, pw: &[u8]) -> bool {
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.bytes(pw);
    let sent = match w.finish() {
        Some(msg) => conn.send(msg),
        None => false,
    };
    // Wipes the frame, which held the password
    mikuauth::ct::wipe(&mut buf);
    sent
}

use miku_abi::ioctl::TCGETS;
use miku_abi::ioctl::TCSETS;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Termios { icanon: u8, echo: u8, isig: u8, _pad: u8 }

/// Reads a line with echo off, restored on every path
fn prompt_password(prompt: &str) -> Option<Vec<u8>> {
    miku::write(2, prompt.as_bytes());

    let mut saved = Termios::default();
    let have_tty = unsafe { miku::miku_ioctl(0, TCGETS, &mut saved as *mut _ as u64) } >= 0;
    if have_tty {
        let mut off = saved;
        off.echo = 0;
        unsafe { miku::miku_ioctl(0, TCSETS, &off as *const _ as u64) };
    }

    let mut out = Vec::new();
    let got = loop {
        let mut ch = [0u8; 1];
        if miku::read(0, &mut ch) <= 0 {
            break None;
        }
        if ch[0] == b'\n' || ch[0] == b'\r' {
            break Some(());
        }
        if out.len() >= proto::MAX_PASSWORD {
            break None;
        }
        out.push(ch[0]);
    };

    if have_tty {
        unsafe { miku::miku_ioctl(0, TCSETS, &saved as *const _ as u64) };
    }
    miku::write(2, b"\n");
    got.map(|_| out)
}

fn println(s: &str) {
    miku::write(1, s.as_bytes());
    miku::write(1, b"\n");
}

fn eprintln(s: &str) {
    miku::write(2, s.as_bytes());
    miku::write(2, b"\n");
}

fn cstring(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    eprintln("mikudo: internal error");
    miku::exit(1);
}
