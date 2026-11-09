// mikudo wire protocol
//
// Not in a request:
// - The caller's identity. mikudod gets it from `peercred` (syscall 83).
// - The terminal (see `Reply::NeedPassword`).
// - A uid or program for a session. `OpenSession` names a user and seat; the
//   rest comes from /etc/passwd.
//
// Framing: a 4-byte little-endian length, then that many bytes. The length is
// checked against MAX_MESSAGE first, and bodies decode into fixed storage.

/// The largest message either side will send or accept
pub const MAX_MESSAGE: usize = 16 * 1024;
/// Arguments a command may be given
pub const MAX_ARGS: usize = 64;
/// The longest single argument, path or name
pub const MAX_STR: usize = 1024;
/// Longest password read. Longer is refused, not truncated
pub const MAX_PASSWORD: usize = 256;

/// Protocol version; mismatches are refused
pub const VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    /// Run a command as the target user
    Run = 1,
    /// Say whether it would be allowed, and run nothing
    Check = 2,
    /// List what the caller may do
    List = 3,
    /// Changes a password: the caller's, or another's with authority
    Passwd = 4,
    /// Forget any remembered authentication for this caller
    ClearPersist = 5,
    /// Verifies identity and opens a session. Body: `str user`, `u8 kind`
    /// (`session_kind`), `u8 seat`. Reply: `NeedPassword`, then `Exited` with
    /// the session id, or `Denied`.
    ///
    /// No op only checks a password, which would be an unjournaled oracle
    OpenSession = 6,
    /// Ends a session. Body: `u32 id`, 0 for the caller's own
    CloseSession = 7,
    /// Lists sessions. No body; `Line` per session, then `End`
    Sessions = 8,
    /// Power off or reboot. Body: `u8 action` (`power_action`). Granted without
    /// a password only when nobody owns the seat
    Power = 9,
}

/// What `Power` is being asked for
pub mod power_action {
    pub const OFF: u8 = 0;
    pub const REBOOT: u8 = 1;

    pub fn valid(a: u8) -> bool { a == OFF || a == REBOOT }
    pub fn name(a: u8) -> &'static str {
        match a { OFF => "power off", REBOOT => "reboot", _ => "?" }
    }
}

/// What kind of session `OpenSession` is asking for
pub mod session_kind {
    /// Console: the in-kernel shell takes the identity; nothing starts
    pub const CONSOLE: u8 = 0;
    /// Desktop: the /etc/passwd session program starts as the user and owns the
    /// seat
    pub const GRAPHICAL: u8 = 1;

    pub fn valid(k: u8) -> bool { k == CONSOLE || k == GRAPHICAL }
    pub fn name(k: u8) -> &'static str {
        match k { CONSOLE => "console", GRAPHICAL => "graphical", _ => "?" }
    }
}

impl Op {
    pub fn from_u8(v: u8) -> Option<Op> {
        Some(match v {
            1 => Op::Run,
            2 => Op::Check,
            3 => Op::List,
            4 => Op::Passwd,
            5 => Op::ClearPersist,
            6 => Op::OpenSession,
            7 => Op::CloseSession,
            8 => Op::Sessions,
            9 => Op::Power,
            _ => return None,
        })
    }
}

/// Client flags, which can only make the daemon stricter or more talkative
pub mod flags {
    /// Never prompt; fail if a password is needed
    pub const NON_INTERACTIVE: u16 = 1 << 0;
    /// Runs the target's shell
    pub const SHELL: u16 = 1 << 1;
    /// Ask for the target's environment as a login would set it
    pub const LOGIN: u16 = 1 << 2;
}

/// What the daemon says back
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Reply {
    /// The command ran; payload is its exit status
    Exited = 1,
    /// Refused; payload is a reason for the user
    Denied = 2,
    /// Allowed, nothing run: the answer to Check
    Would = 3,
    /// A line of output, for List
    Line = 4,
    /// The end of a multi-line answer
    End = 5,
    /// A non-policy failure
    Error = 6,
    /// The rule requires a password and no persist token is live. The client
    /// prompts and sends a bare `bytes` message. A round trip, since MikuOS's
    /// fds 0, 1, and 2 are routed by the syscall layer and only the foreground
    /// process reads the keyboard
    NeedPassword = 7,
}

impl Reply {
    pub fn from_u8(v: u8) -> Option<Reply> {
        Some(match v {
            1 => Reply::Exited,
            2 => Reply::Denied,
            3 => Reply::Would,
            4 => Reply::Line,
            5 => Reply::End,
            6 => Reply::Error,
            7 => Reply::NeedPassword,
            _ => return None,
        })
    }
}

/// Bounded writer over a caller-owned buffer. Each put returns whether it
/// fitted, and the encoder stops at the first refusal; a message that does not
/// fit is not sent
pub struct Writer<'a> {
    buf: &'a mut [u8],
    pos: usize,
    ok: bool,
}

impl<'a> Writer<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Writer { buf, pos: 4, ok: true } // room for the length prefix
    }

    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.raw(&[v])
    }

    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.raw(&v.to_le_bytes())
    }

    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.raw(&v.to_le_bytes())
    }

    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.raw(&v.to_le_bytes())
    }

    /// Length-prefixed string. The u16 length is capped at MAX_STR
    pub fn str(&mut self, s: &str) -> &mut Self {
        if s.len() > MAX_STR {
            self.ok = false;
            return self;
        }
        self.u16(s.len() as u16).raw(s.as_bytes())
    }

    pub fn bytes(&mut self, b: &[u8]) -> &mut Self {
        if b.len() > MAX_STR {
            self.ok = false;
            return self;
        }
        self.u16(b.len() as u16).raw(b)
    }

    fn raw(&mut self, b: &[u8]) -> &mut Self {
        if !self.ok || self.pos + b.len() > self.buf.len() {
            self.ok = false;
            return self;
        }
        self.buf[self.pos..self.pos + b.len()].copy_from_slice(b);
        self.pos += b.len();
        self
    }

    /// Writes the length prefix and returns the framed message, or None if
    /// anything did not fit
    pub fn finish(self) -> Option<&'a [u8]> {
        if !self.ok {
            return None;
        }
        let body = self.pos - 4;
        if body > MAX_MESSAGE {
            return None;
        }
        self.buf[..4].copy_from_slice(&(body as u32).to_le_bytes());
        Some(&self.buf[..self.pos])
    }
}

/// Bounded reader over a received body; every read is checked against the bytes
/// remaining
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    pub fn u8(&mut self) -> Option<u8> {
        let b = *self.buf.get(self.pos)?;
        self.pos += 1;
        Some(b)
    }

    pub fn u16(&mut self) -> Option<u16> {
        let s = self.take(2)?;
        Some(u16::from_le_bytes([s[0], s[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        let s = self.take(4)?;
        Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
    }

    pub fn i64(&mut self) -> Option<i64> {
        let s = self.take(8)?;
        let mut b = [0u8; 8];
        b.copy_from_slice(s);
        Some(i64::from_le_bytes(b))
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = self.u16()? as usize;
        if n > MAX_STR {
            return None;
        }
        self.take(n)
    }

    /// A string, refused if not UTF-8. A lossy conversion would exec a path
    /// other than the one checked against the rule
    pub fn str(&mut self) -> Option<&'a str> {
        core::str::from_utf8(self.bytes()?).ok()
    }

    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.remaining() < n {
            return None;
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Some(s)
    }
}

/// Reads the length prefix of a framed message. None for a length over the
/// limit, before anything is read into a buffer
pub fn frame_len(prefix: [u8; 4]) -> Option<usize> {
    let n = u32::from_le_bytes(prefix) as usize;
    if n == 0 || n > MAX_MESSAGE {
        return None;
    }
    Some(n)
}
