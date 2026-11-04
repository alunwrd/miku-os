//! Line editor for the shell's prompt
//!
//! The terminal's line discipline (`io::tty::ldisc`) serves every other
//! program. This requests raw mode only during its own prompt:
//! 1. Raw mode is on in `read_line` and off in its `Drop`;
//!    `tty::set_foreground` resets the discipline before a child runs.
//! 2. Keys present in both mean the same: Ctrl-U kills the whole line, Ctrl-W
//!    eats spaces and then non-spaces.
//! 3. With no terminal, it falls back to plain `read`.
//!
//! Redraws use `CSI K` and `CSI n G`.

use miku::{print, read};

use super::LINE_MAX;

/// How many lines are remembered
const HIST_LINES: usize = 32;
/// Longest stored history line. Longer ones are not stored, rather than
/// truncated
const HIST_LINE_MAX: usize = 256;

static mut HIST: [[u8; HIST_LINE_MAX]; HIST_LINES] = [[0; HIST_LINE_MAX]; HIST_LINES];
static mut HIST_LEN: [u16; HIST_LINES] = [0; HIST_LINES];
/// Lines ever pushed; the ring index is this modulo `HIST_LINES`
static mut HIST_COUNT: usize = 0;

fn hist() -> (&'static mut [[u8; HIST_LINE_MAX]; HIST_LINES], &'static mut [u16; HIST_LINES]) {
    unsafe {
        (
            &mut *core::ptr::addr_of_mut!(HIST),
            &mut *core::ptr::addr_of_mut!(HIST_LEN),
        )
    }
}

fn hist_count() -> usize {
    unsafe { *core::ptr::addr_of!(HIST_COUNT) }
}

/// How many lines can actually be recalled
fn hist_live() -> usize {
    hist_count().min(HIST_LINES)
}

/// `back` of 1 is the most recent line
fn hist_get(back: usize) -> Option<&'static [u8]> {
    let live = hist_live();
    if back == 0 || back > live {
        return None;
    }
    let idx = (hist_count() - back) % HIST_LINES;
    let (buf, len) = hist();
    Some(&buf[idx][..len[idx] as usize])
}

fn hist_push(line: &[u8]) {
    if line.is_empty() || line.len() > HIST_LINE_MAX {
        return;
    }
    // Consecutive duplicates only
    if hist_get(1) == Some(line) {
        return;
    }
    let idx = hist_count() % HIST_LINES;
    let (buf, len) = hist();
    buf[idx][..line.len()].copy_from_slice(line);
    len[idx] = line.len() as u16;
    unsafe {
        let c = &mut *core::ptr::addr_of_mut!(HIST_COUNT);
        *c += 1;
    }
}

// Decoded keystroke

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Key {
    /// A character decoded from UTF-8
    Char(char),
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    Prev,
    Next,
    KillToEnd,
    KillLine,
    KillWord,
    Clear,
    Interrupt,
    /// Ctrl-D: end of input on an empty line, delete otherwise
    EndOfFile,
    /// Part of a sequence, or something this editor has no meaning for
    None,
}

/// Decodes bytes into keys with a state machine, since a `read` can return
/// mid-sequence. Unknown sequences are swallowed
pub struct Keys {
    state: u8,
    /// First parameter of a CSI, for the `~` family
    param: u16,
    /// Bytes taken of a sequence, so an unterminated one cannot swallow later
    /// typing
    eaten: u8,
    /// Partial UTF-8 character and its expected length
    utf: [u8; 4],
    utf_have: u8,
    utf_need: u8,
}

const KEY_SEQ_MAX: u8 = 12;

impl Keys {
    pub const fn new() -> Self {
        Keys { state: 0, param: 0, eaten: 0, utf: [0; 4], utf_have: 0, utf_need: 0 }
    }

    pub fn feed(&mut self, b: u8) -> Key {
        // A non-continuation byte drops the partial character and is read as
        // itself
        if self.utf_need != 0 {
            if b & 0xc0 == 0x80 {
                self.utf[self.utf_have as usize] = b;
                self.utf_have += 1;
                if self.utf_have < self.utf_need {
                    return Key::None;
                }
                let n = self.utf_need as usize;
                self.utf_need = 0;
                return match core::str::from_utf8(&self.utf[..n]).ok().and_then(|t| t.chars().next()) {
                    Some(c) => Key::Char(c),
                    None => Key::None,
                };
            }
            self.utf_need = 0;
        }
        if self.state != 0 {
            self.eaten = self.eaten.saturating_add(1);
            if self.eaten > KEY_SEQ_MAX {
                self.state = 0;
                self.eaten = 0;
            }
        }
        match self.state {
            0 => match b {
                0x1b => { self.state = 1; self.eaten = 1; Key::None }
                b'\r' | b'\n' => Key::Enter,
                0x7f | 0x08 => Key::Backspace,
                0x01 => Key::Home,
                0x05 => Key::End,
                0x02 => Key::Left,
                0x06 => Key::Right,
                0x03 => Key::Interrupt,
                0x04 => Key::EndOfFile,
                0x0b => Key::KillToEnd,
                0x0c => Key::Clear,
                0x0e => Key::Next,
                0x10 => Key::Prev,
                0x15 => Key::KillLine,
                0x17 => Key::KillWord,
                0x20..=0x7e => Key::Char(b as char),
                // First byte of a multi-byte character; 0xc0, 0xc1, and 0xf5 up
                // are invalid
                0xc2..=0xf4 => {
                    self.utf[0] = b;
                    self.utf_have = 1;
                    self.utf_need = if b < 0xe0 { 2 } else if b < 0xf0 { 3 } else { 4 };
                    Key::None
                }
                // Tab does nothing; no completion
                _ => Key::None,
            },
            1 => {
                if b == b'[' {
                    self.state = 2;
                    self.param = 0;
                } else {
                    self.state = 0;
                }
                Key::None
            }
            _ => match b {
                b'0'..=b'9' => {
                    self.param = self.param.saturating_mul(10)
                        .saturating_add((b - b'0') as u16);
                    Key::None
                }
                b'A' => { self.state = 0; Key::Prev }
                b'B' => { self.state = 0; Key::Next }
                b'C' => { self.state = 0; Key::Right }
                b'D' => { self.state = 0; Key::Left }
                b'H' => { self.state = 0; Key::Home }
                b'F' => { self.state = 0; Key::End }
                b'~' => {
                    self.state = 0;
                    match self.param {
                        1 | 7 => Key::Home,
                        3 => Key::Delete,
                        4 | 8 => Key::End,
                        // Insert, PageUp, and PageDown are ignored
                        _ => Key::None,
                    }
                }
                _ => { self.state = 0; Key::None }
            },
        }
    }
}

// Line state

pub struct Line {
    pub buf: [u8; LINE_MAX],
    pub len: usize,
    pub cur: usize,
}

impl Line {
    pub const fn new() -> Self {
        Line { buf: [0; LINE_MAX], len: 0, cur: 0 }
    }

    pub fn text(&self) -> &[u8] { &self.buf[..self.len] }

    pub fn clear(&mut self) { self.len = 0; self.cur = 0; }

    pub fn set(&mut self, text: &[u8]) {
        let n = text.len().min(self.buf.len());
        self.buf[..n].copy_from_slice(&text[..n]);
        self.len = n;
        self.cur = n;
    }

    /// Inserts at the cursor, refusing rather than truncating when full. UTF-8,
    /// with the cursor always on a character boundary
    pub fn insert(&mut self, c: char) {
        let mut enc = [0u8; 4];
        let bytes = c.encode_utf8(&mut enc).as_bytes();
        let n = bytes.len();
        if self.len + n >= self.buf.len() { return; }
        self.buf.copy_within(self.cur..self.len, self.cur + n);
        self.buf[self.cur..self.cur + n].copy_from_slice(bytes);
        self.len += n;
        self.cur += n;
    }

    pub fn backspace(&mut self) {
        if self.cur == 0 { return; }
        let at = prev_char(&self.buf, self.cur);
        self.buf.copy_within(self.cur..self.len, at);
        self.len -= self.cur - at;
        self.cur = at;
    }

    pub fn delete(&mut self) {
        if self.cur >= self.len { return; }
        let end = next_char(&self.buf, self.cur, self.len);
        self.buf.copy_within(end..self.len, self.cur);
        self.len -= end - self.cur;
    }

    /// Ctrl-W, as in `io::tty::ldisc`: trailing spaces, then non-spaces
    pub fn kill_word(&mut self) {
        let mut at = self.cur;
        while at > 0 && self.buf[at - 1] == b' ' { at -= 1; }
        while at > 0 && self.buf[at - 1] != b' ' { at -= 1; }
        let cut = self.cur - at;
        if cut == 0 { return; }
        self.buf.copy_within(self.cur..self.len, at);
        self.len -= cut;
        self.cur = at;
    }

    /// Applies one key; true when the line is finished. Separate from reading
    /// and drawing for host tests
    pub fn apply(&mut self, k: Key, hist_at: &mut usize, stash: &mut ([u8; LINE_MAX], usize))
        -> bool
    {
        match k {
            Key::Char(c) => self.insert(c),
            Key::Backspace => self.backspace(),
            Key::Delete => self.delete(),
            Key::Left => self.cur = prev_char(&self.buf, self.cur),
            Key::Right => { if self.cur < self.len { self.cur = next_char(&self.buf, self.cur, self.len); } }
            Key::Home => self.cur = 0,
            Key::End => self.cur = self.len,
            Key::KillToEnd => self.len = self.cur,
            // Ctrl-U clears the whole line, as in the discipline
            Key::KillLine => self.clear(),
            Key::KillWord => self.kill_word(),
            Key::Prev => {
                // First step back saves the line being typed
                if *hist_at == 0 {
                    stash.0[..self.len].copy_from_slice(self.text());
                    stash.1 = self.len;
                }
                if let Some(h) = hist_get(*hist_at + 1) {
                    *hist_at += 1;
                    self.set(h);
                }
            }
            Key::Next => {
                if *hist_at > 1 {
                    *hist_at -= 1;
                    if let Some(h) = hist_get(*hist_at) { self.set(h); }
                } else if *hist_at == 1 {
                    *hist_at = 0;
                    let n = stash.1;
                    self.set(&stash.0[..n]);
                }
            }
            _ => {}
        }
        matches!(k, Key::Enter)
    }
}

/// UTF-8 continuation byte
fn continues(b: u8) -> bool {
    b & 0xc0 == 0x80
}

/// Start of the character before byte `at`
fn prev_char(buf: &[u8], at: usize) -> usize {
    let mut i = at.saturating_sub(1);
    while i > 0 && continues(buf[i]) { i -= 1; }
    i
}

/// Where the character after the one starting at byte `at` starts
fn next_char(buf: &[u8], at: usize, len: usize) -> usize {
    let mut i = at + 1;
    while i < len && continues(buf[i]) { i += 1; }
    i.min(len)
}

/// Character count of a run of bytes, equal to columns in this font
fn columns(bytes: &[u8]) -> usize {
    bytes.iter().filter(|&&b| !continues(b)).count()
}

/// The byte where the line's `n`th character starts
fn byte_of(buf: &[u8], len: usize, n: usize) -> usize {
    let mut seen = 0;
    for (i, &b) in buf[..len].iter().enumerate() {
        if !continues(b) {
            if seen == n { return i; }
            seen += 1;
        }
    }
    len
}

// Drawing

/// Start of the visible part of a long line. Long lines scroll sideways rather
/// than wrap, since the grid may scroll between keystrokes and there is no
/// cursor-position report
pub fn window(cur: usize, len: usize, avail: usize, view: usize) -> usize {
    let _ = len;
    if avail == 0 { return 0; }
    let mut v = view;
    if cur < v { v = cur; }
    if cur >= v + avail { v = cur + 1 - avail; }
    v
}

/// A number into a buffer, for a sequence that cannot allocate
fn digits(mut n: usize, out: &mut [u8; 8]) -> usize {
    if n == 0 { out[0] = b'0'; return 1; }
    let mut tmp = [0u8; 8];
    let mut i = 0;
    while n > 0 && i < tmp.len() {
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
        i += 1;
    }
    for j in 0..i { out[j] = tmp[i - 1 - j]; }
    i
}

/// Put the pen in an absolute column, one-based as the wire is
fn column(n: usize) {
    let mut d = [0u8; 8];
    let len = digits(n, &mut d);
    miku::write(1, b"\x1b[");
    miku::write(1, &d[..len]);
    miku::write(1, b"G");
}

/// Full redraw from column zero each keystroke, since other output can reach
/// the grid and a minimal redraw would drift
fn redraw(line: &Line, prompt: fn() -> usize, cols: usize, view: &mut usize) {
    print("\r");
    let pcols = prompt();
    // One column short of the edge, since the last cell leaves a pending wrap
    let avail = cols.saturating_sub(pcols + 1).max(1);
    // In characters, not bytes
    let (cur, len) = (columns(&line.buf[..line.cur]), columns(line.text()));
    *view = window(cur, len, avail, *view);
    let from = byte_of(&line.buf, line.len, *view);
    let to = byte_of(&line.buf, line.len, (*view + avail).min(len));
    if from < to {
        miku::write(1, &line.buf[from..to]);
    }
    // Whatever the last line left further right
    miku::write(1, b"\x1b[K");
    column(pcols + (cur - *view) + 1);
    // No `<` or `>` marker on a scrolled line
}

// Reading

/// Reads a line with editing. Returns a length, 0 for end of input, negative if
/// the terminal is gone. `prompt` prints the prompt and returns its width in
/// columns
pub fn read_line(out: &mut [u8], prompt: fn() -> usize) -> i64 {
    // No terminal: a pipe, redirect, or `msh < script`
    if miku::tty::termios().is_none() {
        prompt();
        return read(0, out);
    }

    // Raw, no echo, and Ctrl-C as a byte. Echo would make the grid obey arrow
    // key sequences. Without `isig` off, SIGINT would kill the shell, which is
    // its own foreground job here
    let mut saved = miku::tty::Saved::change(|t| {
        t.icanon = false;
        t.echo = false;
        t.isig = false;
    });

    let cols = term_cols();
    let mut line = Line::new();
    let mut keys = Keys::new();
    let mut view = 0usize;
    let mut hist_at = 0usize;
    let mut stash = ([0u8; LINE_MAX], 0usize);

    redraw(&line, prompt, cols, &mut view);

    loop {
        let mut b = [0u8; 1];
        let n = read(0, &mut b);
        if n < 0 {
            saved.restore();
            return n;
        }
        if n == 0 {
            // The terminal gave up
            saved.restore();
            return 0;
        }

        match keys.feed(b[0]) {
            Key::None => continue,

            Key::Enter => {
                print("\n");
                saved.restore();
                hist_push(line.text());
                let len = line.len.min(out.len());
                out[..len].copy_from_slice(&line.buf[..len]);
                return len as i64;
            }

            Key::Interrupt => {
                print("^C\n");
                // A newline, since a zero length would be end of input
                saved.restore();
                if out.is_empty() { return 0; }
                out[0] = b'\n';
                return 1;
            }

            Key::EndOfFile => {
                if line.len == 0 {
                    print("\n");
                    saved.restore();
                    return 0;
                }
                line.delete();
            }

            Key::Clear => {
                // Clears the screen
                print("\x1b[2J\x1b[H");
            }

            k => { line.apply(k, &mut hist_at, &mut stash); }
        }

        redraw(&line, prompt, cols, &mut view);
    }
}

/// Terminal width, or 80. Asked once per line
fn term_cols() -> usize {
    const TIOCGWINSZ: u64 = 0x5413;
    let mut ws = [0u16; 4]; // rows, cols, xpixel, ypixel
    let rc = unsafe { miku::miku_ioctl(0, TIOCGWINSZ, ws.as_mut_ptr() as u64) };
    if rc < 0 || ws[1] < 8 { 80 } else { ws[1] as usize }
}
