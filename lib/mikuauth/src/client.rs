// mikudod client for `login` and `maygreet`
//
// They differ only in where the password comes from, which is a callback
// (`Ask`). Only built with the `userspace` feature.
//
// Grants nothing: both callers stay unprivileged. The password goes over a
// socket; this side never reads /etc/shadow, hashes, or changes identity.

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::proto::{self, frame_len, session_kind, Op, Reader, Reply, Writer};

/// Open connection, closed on drop
pub struct Conn {
    fd: i64,
}

impl Conn {
    /// Connect to `/run/mikudo.sock`, or `None` if nothing is listening
    pub fn open() -> Option<Conn> {
        let fd = unsafe { miku::miku_socket_unix() };
        if fd < 0 {
            return None;
        }
        let mut path = Vec::with_capacity(crate::SOCKET_PATH.len() + 1);
        path.extend_from_slice(crate::SOCKET_PATH.as_bytes());
        path.push(0);
        if unsafe { miku::miku_connect_unix(fd, path.as_ptr()) } < 0 {
            unsafe { miku::miku_close(fd) };
            return None;
        }
        Some(Conn { fd })
    }

    /// The raw descriptor, for a caller that must poll it alongside others
    pub fn fd(&self) -> i64 {
        self.fd
    }

    /// Sends one message, adding the length prefix
    pub fn send(&self, body: &[u8]) -> bool {
        (unsafe { miku::miku_send(self.fd, body.as_ptr(), body.len()) }) > 0
    }

    /// Reads one message into `buf`, returning the body. The length is checked
    /// first, and a frame that does not fit closes the connection
    pub fn recv<'a>(&self, buf: &'a mut [u8]) -> Option<&'a [u8]> {
        let mut prefix = [0u8; 4];
        if !self.recv_exact(&mut prefix) {
            return None;
        }
        let n = frame_len(prefix)?;
        if n > buf.len() {
            return None;
        }
        if !self.recv_exact(&mut buf[..n]) {
            return None;
        }
        Some(&buf[..n])
    }

    fn recv_exact(&self, buf: &mut [u8]) -> bool {
        let mut got = 0usize;
        while got < buf.len() {
            let n = unsafe { miku::miku_recv(self.fd, buf[got..].as_mut_ptr(), buf.len() - got) };
            if n <= 0 {
                return false;
            }
            got += n as usize;
        }
        true
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        unsafe { miku::miku_close(self.fd) };
    }
}

/// Password source and message sink, implemented by `login` and the greeter
pub trait Ask {
    /// Password for `user`, or `None` to give up. Called up to three times; the
    /// daemon counts attempts
    fn password(&mut self, user: &str) -> Option<Vec<u8>>;

    /// A daemon message for the user. Default drops it
    fn note(&mut self, _line: &str) {}
}

/// Why a login did not happen
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// Nothing is listening on the socket
    NoDaemon,
    /// Wrong password, or the account may not do this
    Denied(String),
    /// Other failure
    Error(String),
    /// The daemon stopped talking half way through
    Closed,
}

impl Refused {
    /// One line to show. Wrong password and unknown account read the same
    pub fn message(&self) -> &str {
        match self {
            Refused::NoDaemon => "the login service is not running",
            Refused::Denied(m) => m,
            Refused::Error(m) => m,
            Refused::Closed => "the login service closed the connection",
        }
    }
}

/// Verifies identity and opens a session. Returns the id; the kernel has
/// already applied the identity
pub fn open_session(user: &str, kind: u8, seat: u8, ask: &mut dyn Ask) -> Result<u32, Refused> {
    let conn = Conn::open().ok_or(Refused::NoDaemon)?;

    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(Op::OpenSession as u8).u16(0);
    w.str(user).u8(kind).u8(seat);
    let msg = w.finish().ok_or(Refused::Closed)?;
    if !conn.send(msg) {
        return Err(Refused::Closed);
    }

    converse(&conn, user, ask)
}

/// End a session by id, or 0 for the caller's own
pub fn close_session(id: u32) -> Result<(), Refused> {
    let conn = Conn::open().ok_or(Refused::NoDaemon)?;
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(Op::CloseSession as u8).u16(0);
    w.u32(id);
    let msg = w.finish().ok_or(Refused::Closed)?;
    if !conn.send(msg) {
        return Err(Refused::Closed);
    }
    let mut noask = Silent;
    converse(&conn, "", &mut noask).map(|_| ())
}

/// Who is logged in, one line per session, as the daemon renders them
pub fn sessions() -> Result<Vec<String>, Refused> {
    let conn = Conn::open().ok_or(Refused::NoDaemon)?;
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(Op::Sessions as u8).u16(0);
    let msg = w.finish().ok_or(Refused::Closed)?;
    if !conn.send(msg) {
        return Err(Refused::Closed);
    }
    let mut lines = Collect(Vec::new());
    converse(&conn, "", &mut lines)?;
    Ok(lines.0)
}

/// Asks the daemon to power off or reboot. `user` is who to ask about if a
/// password is needed; none at the login screen. `Ok` means the daemon agreed;
/// the connection then dies
pub fn power(action: u8, user: &str, ask: &mut dyn Ask) -> Result<(), Refused> {
    let conn = Conn::open().ok_or(Refused::NoDaemon)?;
    let mut buf = [0u8; proto::MAX_MESSAGE];
    let mut w = Writer::new(&mut buf);
    w.u8(proto::VERSION).u8(Op::Power as u8).u16(0);
    w.u8(action);
    let msg = w.finish().ok_or(Refused::Closed)?;
    if !conn.send(msg) {
        return Err(Refused::Closed);
    }
    match converse(&conn, user, ask) {
        Ok(_) => Ok(()),
        // The machine went down before the reply, which counts as success
        Err(Refused::Closed) => Ok(()),
        Err(e) => Err(e),
    }
}

/// An `Ask` that answers no password and keeps nothing
pub struct Silent;
impl Ask for Silent {
    fn password(&mut self, _user: &str) -> Option<Vec<u8>> { None }
}

/// An `Ask` that keeps the lines
struct Collect(Vec<String>);
impl Ask for Collect {
    fn password(&mut self, _user: &str) -> Option<Vec<u8>> { None }
    fn note(&mut self, line: &str) { self.0.push(line.to_string()); }
}

/// Reply loop for all ops. `Exited` carries the session id for `OpenSession`
fn converse(conn: &Conn, user: &str, ask: &mut dyn Ask) -> Result<u32, Refused> {
    let mut raw = [0u8; proto::MAX_MESSAGE];
    loop {
        let body = match conn.recv(&mut raw) {
            Some(b) => b,
            None => return Err(Refused::Closed),
        };
        let mut r = Reader::new(body);
        let kind = match r.u8().and_then(Reply::from_u8) {
            Some(k) => k,
            None => return Err(Refused::Closed),
        };
        match kind {
            Reply::NeedPassword => {
                // The daemon names the user; they differ on a `Passwd` op
                let who = r.str().unwrap_or(user);
                let mut pw = match ask.password(who) {
                    Some(p) => p,
                    None => return Err(Refused::Denied("cancelled".to_string())),
                };
                let mut out = [0u8; proto::MAX_MESSAGE];
                let mut w = Writer::new(&mut out);
                w.bytes(&pw);
                let sent = match w.finish() {
                    Some(msg) => conn.send(msg),
                    None => false,
                };
                // Wiped on both paths, along with the frame buffer
                crate::ct::wipe(&mut pw);
                crate::ct::wipe(&mut out);
                if !sent {
                    return Err(Refused::Closed);
                }
            }
            Reply::Exited => {
                let status = r.i64().unwrap_or(0);
                return Ok(status.max(0) as u32);
            }
            Reply::Denied => {
                return Err(Refused::Denied(r.str().unwrap_or("refused").to_string()));
            }
            Reply::Error => {
                return Err(Refused::Error(r.str().unwrap_or("something went wrong").to_string()));
            }
            Reply::Line => ask.note(r.str().unwrap_or("")),
            Reply::Would => ask.note(r.str().unwrap_or("")),
            Reply::End => {
                let s = r.str().unwrap_or("");
                if !s.is_empty() {
                    ask.note(s);
                }
                return Ok(0);
            }
        }
    }
}

/// Kind constants, re-exported
pub use session_kind::{CONSOLE, GRAPHICAL};

/// The only seat
pub const SEAT0: u8 = 0;
