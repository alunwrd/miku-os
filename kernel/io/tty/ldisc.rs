// The line discipline: what a keystroke means
//
// One terminal's input, as a state machine. It is handed a character and
// answers with what should be echoed and what signal should be raised; it does
// neither itself, and holds no lock but its own.
//
// Echo goes to a grid behind another lock and a signal wakes the scheduler, so
// doing either holding its own state could deadlock. `feed` returns a `Feed`
// and the caller acts on it after the lock is gone; see `io::tty::feed_char`.

use core::sync::atomic::{AtomicU64, Ordering};

pub const READ_BUF_SIZE: usize = 1024;
const LINE_BUF_SIZE: usize = 256;

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

    /// Put a byte back where the next `pop` will find it
    ///
    /// The ring is written at the tail and read at the head, so this is the one
    /// operation moving the head backwards. Full means the byte is dropped, as
    /// `push` does at the other end
    fn push_front(&mut self, byte: u8) {
        let prev = if self.head == 0 { READ_BUF_SIZE - 1 } else { self.head - 1 };
        if prev == self.tail { return; }
        self.buf[prev] = byte;
        self.head = prev;
    }

    fn pop(&mut self) -> Option<u8> {
        if self.head == self.tail { return None; }
        let b = self.buf[self.head];
        self.head = (self.head + 1) % READ_BUF_SIZE;
        Some(b)
    }

    fn is_empty(&self) -> bool { self.head == self.tail }

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

    /// A whole character, or nothing: a line with the first two bytes of a
    /// three-byte character is not text
    fn push_char(&mut self, c: char) -> bool {
        let mut enc = [0u8; 4];
        let bytes = c.encode_utf8(&mut enc).as_bytes();
        if self.len + bytes.len() > LINE_BUF_SIZE { return false; }
        self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
        self.len += bytes.len();
        true
    }

    /// Erase one character, one to four bytes
    ///
    /// A backspace over `й` taking one byte would leave half a character in the
    /// buffer
    fn pop(&mut self) -> bool {
        if self.len == 0 { return false; }
        // Continuation bytes are `10xxxxxx`; walk back over them to the lead
        while self.len > 1 && (self.buf[self.len - 1] & 0xC0) == 0x80 {
            self.len -= 1;
        }
        self.len -= 1;
        true
    }

    fn clear(&mut self) { self.len = 0; }

    /// How many characters are in the line: how many cells an erase rubs out
    ///
    /// Not `len`, which is bytes: Ctrl-U on a line of Cyrillic would send twice
    /// the backspaces and eat the prompt
    fn chars(&self) -> usize {
        self.buf[..self.len].iter().filter(|b| (**b & 0xC0) != 0x80).count()
    }
}

/// Terminal settings, the subset a line discipline needs.
///
/// The POSIX names, so a libc can map termios onto them. Not Linux's sixty-byte
/// struct: this machine has no baud rate and no modem lines
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Termios {
    /// Canonical mode: input is delivered a line at a time, with editing
    pub icanon: bool,
    /// Echo input characters back to the terminal
    pub echo: bool,
    /// Turn INTR/QUIT/SUSP characters into signals instead of data
    pub isig: bool,
    /// Which characters do those things; see `cc`
    pub c_cc: [u8; NCCS],
    /// How few characters a raw-mode read may return with. `0` means however
    /// many are there, even none, which with `VTIME` at 0 is a read that does
    /// not wait
    pub vmin: u8,
    /// How long a raw-mode read waits, in tenths of a second. `0` is forever
    pub vtime: u8,
    /// Stop a background job writing to this terminal, so its output does not
    /// interleave with the foreground's.
    ///
    /// POSIX's `TOSTOP`, off by default: a job writing in the background is
    /// ordinary (`make &`), while a job reading in the background is a bug, so
    /// SIGTTIN has no such switch
    pub tostop: bool,
}

/// How many control characters a terminal names
///
/// The POSIX slots this machine can act on. Each below is read by a line in
/// `feed`
pub use miku_abi::ktermios::NCCS;

/// Interrupt: SIGINT to the foreground job. Ctrl-C
pub use miku_abi::ktermios::VINTR;
/// Quit: SIGQUIT. Ctrl-\
pub use miku_abi::ktermios::VQUIT;
/// Erase one character. Backspace
pub use miku_abi::ktermios::VERASE;
/// Erase the whole line. Ctrl-U
pub use miku_abi::ktermios::VKILL;
/// End of file on an empty line, flush otherwise. Ctrl-D
pub use miku_abi::ktermios::VEOF;
/// Erase one word. Ctrl-W
pub use miku_abi::ktermios::VWERASE;
/// Suspend: SIGTSTP to the foreground job. Ctrl-Z
pub use miku_abi::ktermios::VSUSP;

/// A control character being disabled: `_POSIX_VDISABLE`
///
/// A terminal with `c_cc[VINTR] = 0` has no interrupt character, as a
/// full-screen editor may want
pub use miku_abi::ktermios::VDISABLE;

impl Termios {
    pub const fn cooked() -> Self {
        Self {
            icanon: true,
            echo: true,
            isig: true,
            c_cc: Self::DEFAULT_CC,
            // Canonical mode delivers a line, so neither applies until a
            // program asks for raw, which gets a read waiting for at least one
            // character, as a pager expects
            vmin: 1,
            vtime: 0,
            tostop: false,
        }
    }

    /// What each control character is before anybody changes it, so `stty` can
    /// swap erase for backspace or turn the interrupt character off
    pub const DEFAULT_CC: [u8; NCCS] = {
        let mut cc = [VDISABLE; NCCS];
        cc[VINTR]   = 0x03; // Ctrl-C
        cc[VQUIT]   = 0x1c; // Ctrl-backslash
        cc[VERASE]  = 0x7f; // DEL, with 0x08 accepted beside it; see `feed`
        cc[VKILL]   = 0x15; // Ctrl-U
        cc[VEOF]    = 0x04; // Ctrl-D
        cc[VWERASE] = 0x17; // Ctrl-W
        cc[VSUSP]   = 0x1a; // Ctrl-Z
        cc
    };

    /// Is `c` the character in slot `i`?
    ///
    /// Disabled slots match nothing: otherwise a `c_cc` of 0 would make NUL the
    /// interrupt
    #[inline]
    pub fn is(&self, i: usize, c: char) -> bool {
        let want = self.c_cc[i];
        want != VDISABLE && (c as u32) == want as u32
    }
}

/// The most a single keystroke can ask to have echoed
///
/// Four bytes covers every arm: one character (up to four, as a character is a
/// codepoint), `^C\n`, or the backspace-space-backspace that rubs one out.
/// Ctrl-U asks for that last one repeated, hence the count: nothing here
/// allocates, as this is reached from an interrupt
const ECHO_MAX: usize = 4;

/// What the caller should do about a character, once the lock is gone
pub struct Feed {
    echo: [u8; ECHO_MAX],
    echo_len: usize,
    /// How many times to repeat the echo: Ctrl-U rubbing out a whole line
    pub echo_repeat: usize,
    /// A signal for the terminal's foreground, if this keystroke was one
    pub signal: Option<u32>,
    /// Whether a reader could now make progress
    pub wake: bool,
}

impl Feed {
    const fn nothing() -> Self {
        Self { echo: [0; ECHO_MAX], echo_len: 0, echo_repeat: 0, signal: None, wake: false }
    }

    fn echoing(s: &str) -> Self {
        let mut f = Self::nothing();
        let b = s.as_bytes();
        let n = b.len().min(ECHO_MAX);
        f.echo[..n].copy_from_slice(&b[..n]);
        f.echo_len = n;
        f.echo_repeat = 1;
        f
    }

    /// What to put on the terminal, if anything
    pub fn echo(&self) -> Option<&str> {
        if self.echo_len == 0 || self.echo_repeat == 0 {
            return None;
        }
        core::str::from_utf8(&self.echo[..self.echo_len]).ok()
    }
}

/// One terminal's input
pub struct LineDiscipline {
    read_ring: ReadRing,
    line_buf:  LineBuf,
    pub termios: Termios,
    /// Set by Ctrl-D on an empty line; the next read returns 0 (EOF)
    eof: bool,
}

impl LineDiscipline {
    pub const fn new() -> Self {
        Self {
            read_ring: ReadRing::new(),
            line_buf:  LineBuf::new(),
            termios:   Termios::cooked(),
            eof:       false,
        }
    }

    /// Forget everything typed and go back to a sane terminal
    ///
    /// A new foreground program inherits a cooked terminal, not the mode its
    /// predecessor died in, which might have no echo and no line editing
    pub fn reset(&mut self) {
        self.read_ring.clear();
        self.line_buf.clear();
        self.eof = false;
        self.termios = Termios::cooked();
    }

    /// Would a read return without sleeping?
    ///
    /// Mirrors the top of `take`'s loop: a poll reporting the keyboard ready is
    /// followed by a read, and if the two disagree the read blocks
    pub fn readable(&self) -> bool {
        !self.read_ring.is_empty() || self.eof
    }

    /// Move up to `max` bytes out of the ring. `None` means end of input
    pub fn take(&mut self, out: &mut [u8], max: usize) -> Option<usize> {
        if self.read_ring.is_empty() {
            if self.eof {
                self.eof = false;
                return None;
            }
            return Some(0);
        }
        let mut count = 0usize;
        while count < max && count < out.len() {
            match self.read_ring.pop() {
                Some(b) => { out[count] = b; count += 1; }
                None => break,
            }
        }
        Some(count)
    }

    /// Put bytes back at the head of the ring, in the order they left it
    ///
    /// The only caller is a `VMIN` read that took fewer characters than it may
    /// return with; see `user_stdin::read_from`. Bounded by the ring: what does
    /// not fit is dropped, possible only if a ring's worth was typed during the
    /// read's decision
    pub fn unread(&mut self, bytes: &[u8]) {
        for b in bytes.iter().rev() {
            self.read_ring.push_front(*b);
        }
    }

    /// One character from the keyboard
    ///
    /// Answers with what to do; does none of it. Signals first and consumed,
    /// raw mode straight through, and canonical mode assembling a line with
    /// editing
    pub fn feed(&mut self, c: char) -> Feed {
        let t = self.termios;

        // Signal characters come first and are consumed, never delivered as
        // data. What happens next is the target's to say: a program that
        // trapped SIGINT runs its handler, one that has not dies, decided in
        // `process::signal` by the disposition table for every sender. Which
        // character does which is the terminal's, in `c_cc`
        if t.isig {
            if t.is(VINTR, c) {
                self.line_buf.clear();
                self.read_ring.clear();
                let mut f = if t.echo { Feed::echoing("^C\n") } else { Feed::nothing() };
                f.signal = Some(crate::process::signal::SIGINT as u32);
                return f;
            }
            if t.is(VQUIT, c) {
                let mut f = if t.echo { Feed::echoing("^\\\n") } else { Feed::nothing() };
                f.signal = Some(crate::process::signal::SIGQUIT as u32);
                return f;
            }
            // Suspend. The signal is sent like any other, and `process::signal`
            // decides what happens next
            if t.is(VSUSP, c) {
                let mut f = if t.echo { Feed::echoing("^Z\n") } else { Feed::nothing() };
                f.signal = Some(crate::process::signal::SIGTSTP as u32);
                return f;
            }
        }

        if !t.icanon {
            // Raw mode: every byte goes straight through, no editing, no line
            // assembly, as a pager or an editor wants.
            //
            // Every byte of the character, not `c as u8`, which would truncate
            // anything outside Latin-1
            let mut enc = [0u8; 4];
            for b in c.encode_utf8(&mut enc).as_bytes() {
                self.read_ring.push(*b);
            }
            let mut f = if t.echo { Feed::echoing(c.encode_utf8(&mut [0u8; 4])) }
                        else { Feed::nothing() };
            f.wake = true;
            return f;
        }

        // The editing characters, each named by `c_cc`. Guards in front of the
        // `match`: its patterns must be constants, and these are settings
        if t.is(VERASE, c) || c == '\u{0008}' {
            // Backspace beside whatever VERASE is: the two keys called
            // backspace send different bytes on different keyboards
            return if self.line_buf.pop() && t.echo {
                Feed::echoing("\u{0008} \u{0008}")
            } else {
                Feed::nothing()
            };
        }
        if t.is(VEOF, c) {
            if self.line_buf.len == 0 {
                // End of input: the pending read returns 0
                self.eof = true;
            } else {
                // Mid-line it flushes what has been typed so far
                for i in 0..self.line_buf.len {
                    let b = self.line_buf.buf[i];
                    self.read_ring.push(b);
                }
                self.line_buf.clear();
            }
            let mut f = Feed::nothing();
            f.wake = true;
            return f;
        }
        if t.is(VKILL, c) {
            let n = self.line_buf.chars();
            self.line_buf.clear();
            return if t.echo && n > 0 {
                let mut f = Feed::echoing("\u{0008} \u{0008}");
                f.echo_repeat = n;
                f
            } else {
                Feed::nothing()
            };
        }
        if t.is(VWERASE, c) {
            // Trailing spaces first, then the word: a word is a run of
            // non-spaces, not what a shell means by one. The discipline cannot
            // know `/usr/local` is three words to a path and one to a line
            let before = self.line_buf.chars();
            while self.line_buf.len > 0
                && self.line_buf.buf[self.line_buf.len - 1] == b' '
            {
                self.line_buf.pop();
            }
            while self.line_buf.len > 0
                && self.line_buf.buf[self.line_buf.len - 1] != b' '
            {
                self.line_buf.pop();
            }
            let n = before - self.line_buf.chars();
            return if t.echo && n > 0 {
                let mut f = Feed::echoing("\u{0008} \u{0008}");
                f.echo_repeat = n;
                f
            } else {
                Feed::nothing()
            };
        }

        match c {
            '\n' => {
                for i in 0..self.line_buf.len {
                    let b = self.line_buf.buf[i];
                    self.read_ring.push(b);
                }
                self.read_ring.push(b'\n');
                self.line_buf.clear();
                let mut f = if t.echo { Feed::echoing("\n") } else { Feed::nothing() };
                f.wake = true;
                f
            }
            // Tab is data, not an editing key. Completion is the shell's
            // business and needs raw mode; in canonical mode a tab is a
            // character like any other, delivered to a program reading a line
            '\t' => {
                if self.line_buf.push(b'\t') && t.echo {
                    Feed::echoing("\t")
                } else {
                    Feed::nothing()
                }
            }
            // Anything printable, including non-ASCII (a pasted Cyrillic line).
            //
            // Refused: the C0 and C1 control ranges, as those are not typed
            // characters and the arms above have taken the ones with meaning
            c if c >= ' ' && !(0x7f..0xa0).contains(&(c as u32)) => {
                // A full line rings, as a sign it stopped accepting characters
                if self.line_buf.len + c.len_utf8() > LINE_BUF_SIZE {
                    return if t.echo { Feed::echoing("\u{0007}") } else { Feed::nothing() };
                }
                if self.line_buf.push_char(c) && t.echo {
                    Feed::echoing(c.encode_utf8(&mut [0u8; 4]))
                } else {
                    Feed::nothing()
                }
            }
            _ => Feed::nothing(),
        }
    }
}

/// The job that owns a terminal's input: a process group id, 0 for nobody
///
/// A group, as a keystroke's signal reaches the whole of `sort | uniq`. An
/// atomic beside the discipline: the keyboard asks on every keystroke whether
/// anybody is there, with no lock
pub struct Foreground(AtomicU64);

impl Foreground {
    pub const fn new() -> Self { Self(AtomicU64::new(0)) }

    pub fn get(&self) -> u64 { self.0.load(Ordering::Acquire) }

    pub fn set(&self, pgid: u64) { self.0.store(pgid, Ordering::Release); }

    pub fn active(&self) -> bool { self.get() != 0 }
}
