use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;

const READ_BUF_SIZE: usize = 1024;
const LINE_BUF_SIZE: usize = 256;

static FOREGROUND_PID: AtomicU64 = AtomicU64::new(0);

struct ReadRing {
    buf:  [u8; READ_BUF_SIZE],
    head: usize,
    tail: usize,
}

impl ReadRing {
    const fn new() -> Self {
        Self { buf: [0; READ_BUF_SIZE], head: 0, tail: 0 }
    }

    fn push(&mut self, byte: u8) {
        let next = (self.tail + 1) % READ_BUF_SIZE;
        if next == self.head { return; }
        self.buf[self.tail] = byte;
        self.tail = next;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail { return None; }
        let b = self.buf[self.head];
        self.head = (self.head + 1) % READ_BUF_SIZE;
        Some(b)
    }

    fn is_empty(&self) -> bool {
        self.head == self.tail
    }

    fn clear(&mut self) {
        self.head = 0;
        self.tail = 0;
    }
}

struct LineBuf {
    buf: [u8; LINE_BUF_SIZE],
    len: usize,
}

impl LineBuf {
    const fn new() -> Self {
        Self { buf: [0; LINE_BUF_SIZE], len: 0 }
    }

    fn push(&mut self, b: u8) -> bool {
        if self.len >= LINE_BUF_SIZE { return false; }
        self.buf[self.len] = b;
        self.len += 1;
        true
    }

    fn pop(&mut self) -> bool {
        if self.len == 0 { return false; }
        self.len -= 1;
        true
    }

    fn clear(&mut self) {
        self.len = 0;
    }
}

/// Terminal settings, the subset a line discipline actually needs.
///
/// Until now the discipline was hardwired: always canonical, always echoing,
/// and Ctrl-C always killed. Nothing in ring 3 could read a password without
/// showing it, implement a pager, or take over the keys - the settings simply
/// did not exist to be changed. These flags mirror the POSIX names so a libc
/// can map termios onto them without inventing its own vocabulary
#[derive(Clone, Copy)]
pub struct Termios {
    /// Canonical mode: input is delivered a line at a time, with editing
    pub icanon: bool,
    /// Echo input characters back to the terminal
    pub echo: bool,
    /// Turn INTR/QUIT/SUSP characters into signals instead of data
    pub isig: bool,
}

impl Termios {
    pub const fn cooked() -> Self {
        Self { icanon: true, echo: true, isig: true }
    }
}

struct UserStdinInner {
    read_ring: ReadRing,
    line_buf:  LineBuf,
    termios:   Termios,
    /// Set by Ctrl-D on an empty line; the next read returns 0 (EOF)
    eof:       bool,
}

impl UserStdinInner {
    const fn new() -> Self {
        Self {
            read_ring: ReadRing::new(),
            line_buf:  LineBuf::new(),
            termios:   Termios::cooked(),
            eof:       false,
        }
    }
}

static INNER: Mutex<UserStdinInner> = Mutex::new(UserStdinInner::new());

pub fn termios() -> Termios {
    INNER.lock().termios
}

pub fn set_termios(t: Termios) {
    INNER.lock().termios = t;
}

pub fn set_foreground(pid: u64) {
    {
        let mut inner = INNER.lock();
        inner.read_ring.clear();
        inner.line_buf.clear();
        inner.eof = false;
        // A new foreground program inherits a sane terminal rather than
        // whatever mode its predecessor happened to die in
        inner.termios = Termios::cooked();
    }
    FOREGROUND_PID.store(pid, Ordering::Release);
}

pub fn clear_foreground() {
    FOREGROUND_PID.store(0, Ordering::Release);
}

pub fn foreground_pid() -> u64 {
    FOREGROUND_PID.load(Ordering::Acquire)
}

pub fn is_foreground_active() -> bool {
    foreground_pid() != 0
}

pub fn feed_char(c: char) {
    if !is_foreground_active() { return; }

    let t = INNER.lock().termios;

    // Signal characters come first and are consumed, never delivered as data
    if t.isig {
        match c {
            '\u{0003}' => {                       // Ctrl-C -> INTR
                let pid = foreground_pid();
                if pid != 0 {
                    if t.echo { crate::print!("^C\n"); }
                    let mut inner = INNER.lock();
                    inner.line_buf.clear();
                    inner.read_ring.clear();
                    drop(inner);
                    // Deliver a signal instead of killing outright: a program
                    // with a SIGINT handler now gets a chance to run it, and
                    // the default action is still fatal
                    crate::process::signal::send_signal(pid, crate::process::signal::SIGINT);
                }
                return;
            }
            '\u{001C}' => {                       // Ctrl-\ -> QUIT
                let pid = foreground_pid();
                if pid != 0 {
                    if t.echo { crate::print!("^\\\n"); }
                    crate::process::signal::send_signal(pid, crate::process::signal::SIGQUIT);
                }
                return;
            }
            _ => {}
        }
    }

    if !t.icanon {
        // Raw mode: every byte goes straight through, no editing, no
        // line assembly. This is what a pager or an editor wants
        let mut inner = INNER.lock();
        inner.read_ring.push(c as u8);
        drop(inner);
        if t.echo { crate::print!("{}", c); }
        return;
    }

    match c {
        '\n' => {
            if t.echo { crate::print!("\n"); }
            let mut inner = INNER.lock();
            for i in 0..inner.line_buf.len {
                let b = inner.line_buf.buf[i];
                inner.read_ring.push(b);
            }
            inner.read_ring.push(b'\n');
            inner.line_buf.clear();
        }
        '\u{0008}' | '\u{007F}' => {
            let mut inner = INNER.lock();
            if inner.line_buf.pop() && t.echo {
                drop(inner);
                crate::print!("\u{0008} \u{0008}");
            }
        }
        '\u{0004}' => {                            // Ctrl-D
            let mut inner = INNER.lock();
            if inner.line_buf.len == 0 {
                // End of input: the pending read returns 0
                inner.eof = true;
            } else {
                // Mid-line it just flushes what has been typed so far
                for i in 0..inner.line_buf.len {
                    let b = inner.line_buf.buf[i];
                    inner.read_ring.push(b);
                }
                inner.line_buf.clear();
            }
        }
        '\u{0015}' => {                            // Ctrl-U: kill the line
            let mut inner = INNER.lock();
            let n = inner.line_buf.len;
            inner.line_buf.clear();
            drop(inner);
            if t.echo {
                for _ in 0..n { crate::print!("\u{0008} \u{0008}"); }
            }
        }
        c if c >= ' ' && (c as u32) < 127 => {
            let mut inner = INNER.lock();
            if inner.line_buf.push(c as u8) {
                drop(inner);
                if t.echo { crate::print!("{}", c); }
            }
        }
        _ => {}
    }
}

pub fn read(buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 || buf_ptr < 0x1000 || buf_ptr > 0x0000_7FFF_FFFF_FFFF {
        return u64::MAX;
    }

    let max_read = (len as usize).min(READ_BUF_SIZE);

    loop {
        if foreground_pid() == 0 {
            return 0;
        }

        {
            let mut inner = INNER.lock();
            if inner.read_ring.is_empty() && inner.eof {
                inner.eof = false;
                return 0;
            }
            if !inner.read_ring.is_empty() {
                let mut count = 0usize;
                while count < max_read {
                    match inner.read_ring.pop() {
                        Some(b) => {
                            unsafe { *((buf_ptr + count as u64) as *mut u8) = b; }
                            count += 1;
                        }
                        None => break,
                    }
                }
                return count as u64;
            }
        }

        crate::sched::sleep(1);
    }
}
