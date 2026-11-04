#![no_std]
#![no_main]

//! passwd: changes a password through mikudod, which checks the old one
//!
//! Usage: passwd [user]

extern crate alloc;

pub use miku;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use mikuauth::proto::{self, frame_len, Op, Reader, Reply, Writer};

#[no_mangle]
pub extern "C" fn _start_main(argc: i32, argv: *const *const u8) -> ! {
    let args = miku::args(argc, argv);
    let mut subject = "";
    for a in args.iter().skip(1) {
        if *a == "-h" || *a == "--help" {
            println("usage: passwd [user]");
            miku::exit(0);
        }
        if a.starts_with('-') {
            eprintln("passwd: unknown option");
            miku::exit(2);
        }
        subject = a;
    }

    let me = miku::cred::getuid();
    let passwd = miku::read_file_string(mikuauth::PASSWD_PATH).unwrap_or_default();
    let my_name = mikuauth::db::find_uid(&passwd, me)
        .map(|p| p.name)
        .unwrap_or("you");
    let changing_own = subject.is_empty() || subject == my_name;

    // The old password only for your own account; otherwise mikudod asks the
    // policy
    let old = if changing_own {
        match prompt_password(&format!("current password for {my_name}: ")) {
            Ok(p) => p,
            Err(e) => { eprintln(&e); miku::exit(1); }
        }
    } else {
        Vec::new()
    };

    let who = if subject.is_empty() { my_name } else { subject };
    let new = match prompt_password(&format!("new password for {who}: ")) {
        Ok(p) => p,
        Err(e) => { eprintln(&e); miku::exit(1); }
    };
    let again = match prompt_password("retype the new password: ") {
        Ok(p) => p,
        Err(e) => { eprintln(&e); miku::exit(1); }
    };

    // Compared here before anything is sent
    if new != again {
        eprintln("passwd: the two do not match - nothing changed");
        miku::exit(1);
    }

    let conn = match connect() {
        Some(c) => c,
        None => {
            eprintln("passwd: the elevation service is not running");
            miku::exit(1);
        }
    };
    // No descriptors: fds 0, 1, and 2 are routed by the syscall layer

    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(Op::Passwd as u8).u16(0);
    w.str(subject).bytes(&old).bytes(&new);
    let msg = match w.finish() {
        Some(m) => m,
        None => { eprintln("passwd: that password is too long"); miku::exit(1); }
    };
    unsafe { miku::miku_send(conn, msg.as_ptr(), msg.len()) };

    miku::exit(read_reply(conn) as i64);
}

fn connect() -> Option<i64> {
    let fd = unsafe { miku::miku_socket_unix() };
    if fd < 0 { return None; }
    let path = cstr(mikuauth::SOCKET_PATH);
    if unsafe { miku::miku_connect_unix(fd, path.as_ptr()) } < 0 {
        unsafe { miku::miku_close(fd) };
        return None;
    }
    Some(fd)
}

fn read_reply(conn: i64) -> i32 {
    let mut prefix = [0u8; 4];
    if !recv_exact(conn, &mut prefix) {
        eprintln("passwd: no answer from the elevation service");
        return 1;
    }
    let n = match frame_len(prefix) { Some(n) => n, None => return 1 };
    let mut body = [0u8; proto::MAX_MESSAGE];
    if n > body.len() || !recv_exact(conn, &mut body[..n]) {
        return 1;
    }
    let mut r = Reader::new(&body[..n]);
    match r.u8().and_then(Reply::from_u8) {
        Some(Reply::End) => {
            let s = r.str().unwrap_or("passwd: password changed");
            println(if s.is_empty() { "passwd: password changed" } else { s });
            0
        }
        Some(Reply::Denied) | Some(Reply::Error) => {
            eprintln(r.str().unwrap_or("passwd: refused"));
            1
        }
        _ => 1,
    }
}

fn recv_exact(conn: i64, buf: &mut [u8]) -> bool {
    let mut got = 0usize;
    while got < buf.len() {
        let n = unsafe { miku::miku_recv(conn, buf[got..].as_mut_ptr(), buf.len() - got) };
        if n <= 0 { return false; }
        got += n as usize;
    }
    true
}

/// Reads a line with echo off, restored by `miku::tty::Saved`'s `Drop` on every
/// path
fn prompt_password(prompt: &str) -> Result<Vec<u8>, String> {
    miku::write(2, prompt.as_bytes());

    let _echo_off = miku::tty::Saved::change(|t| t.echo = false);

    let mut out = Vec::new();
    let result = loop {
        let mut ch = [0u8; 1];
        let n = miku::read(0, &mut ch);
        if n <= 0 {
            break Err(String::from("passwd: no password given"));
        }
        if ch[0] == b'\n' || ch[0] == b'\r' {
            break Ok(());
        }
        if out.len() >= proto::MAX_PASSWORD {
            break Err(String::from("passwd: that password is too long"));
        }
        out.push(ch[0]);
    };

    miku::write(2, b"\n");
    result.map(|_| out)
}

fn println(s: &str) { miku::write(1, s.as_bytes()); miku::write(1, b"\n"); }
fn eprintln(s: &str) { miku::write(2, s.as_bytes()); miku::write(2, b"\n"); }

fn cstr(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { miku::exit(1); }
