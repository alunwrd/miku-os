use crate::commands;
use crate::commands::ext2_cmds;
use crate::{console, cprint, cprintln, print, serial_println};
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use pc_keyboard::DecodedKey;
use spin::Mutex;

const MAX_PATH: usize = 64;
const MAX_CMD: usize = 64;
const MAX_HISTORY: usize = 16;
const KBD_POLL_TICKS: u64 = 1;
const CMD_POLL_TICKS: u64 = 5;

pub struct Session {
    pub cwd: usize,
    pub path: [u8; MAX_PATH],
    pub plen: usize,
}

pub struct HistoryEntry {
    pub buf: [u8; MAX_CMD],
    pub len: usize,
}

impl HistoryEntry {
    const fn empty() -> Self {
        Self { buf: [0; MAX_CMD], len: 0 }
    }
}

pub struct Shell {
    pub buf: [u8; MAX_CMD],
    pub len: usize,
    pub cursor: usize,
    pub prompt_end_x: usize,
    pub history: [HistoryEntry; MAX_HISTORY],
    pub history_count: usize,
    pub history_pos: usize,
    pub browsing: bool,
    pub saved_buf: [u8; MAX_CMD],
    pub saved_len: usize,
}

impl Shell {
    #[inline(always)]
    fn cursor_x(&self) -> usize {
        self.prompt_end_x + self.cursor * console::CHAR_WIDTH
    }

    #[inline(always)]
    fn draw_cursor(&self) {
        console::draw_cursor(self.cursor_x());
    }

    #[inline(always)]
    fn erase_cursor(&self) {
        let x = self.cursor_x();
        console::erase_cursor(x);
        if self.cursor < self.len {
            console::write_char_at_x(x, self.buf[self.cursor], 255, 255, 255);
        }
    }

    #[inline]
    fn draw_append(&self) {
        let ch_pos = self.cursor - 1;
        let x = self.prompt_end_x + ch_pos * console::CHAR_WIDTH;
        console::write_char_at_x(x, self.buf[ch_pos], 255, 255, 255);
    }

    #[inline(always)]
    fn redraw_from(&self, dirty_start: usize, old_len: usize) {
        console::redraw_input_line(
            self.prompt_end_x,
            &self.buf,
            self.len,
            self.cursor,
            old_len,
            dirty_start,
        );
    }

    #[inline(always)]
    fn redraw_full(&self, old_len: usize) {
        console::redraw_input_line_full(
            self.prompt_end_x,
            &self.buf,
            self.len,
            self.cursor,
            old_len,
        );
    }

    #[inline]
    fn save_to_history(&mut self) {
        let cl = self.len;
        if cl == 0 { return; }
        let idx = self.history_count % MAX_HISTORY;
        unsafe {
            core::ptr::copy_nonoverlapping(self.buf.as_ptr(), self.history[idx].buf.as_mut_ptr(), cl);
        }
        self.history[idx].len = cl;
        self.history_count += 1;
    }

    #[inline]
    fn load_history(&mut self, idx: usize) {
        let hlen = self.history[idx].len;
        unsafe {
            core::ptr::copy_nonoverlapping(self.history[idx].buf.as_ptr(), self.buf.as_mut_ptr(), hlen);
        }
        self.len = hlen;
        self.cursor = hlen;
    }

    #[inline]
    fn save_input(&mut self) {
        unsafe {
            core::ptr::copy_nonoverlapping(self.buf.as_ptr(), self.saved_buf.as_mut_ptr(), self.len);
        }
        self.saved_len = self.len;
    }

    #[inline]
    fn restore_input(&mut self) {
        let slen = self.saved_len;
        unsafe {
            core::ptr::copy_nonoverlapping(self.saved_buf.as_ptr(), self.buf.as_mut_ptr(), slen);
        }
        self.len = slen;
        self.cursor = slen;
    }

    #[inline]
    fn delete_at(&mut self, pos: usize) {
        let len = self.len;
        if pos < len - 1 {
            self.buf.copy_within(pos + 1..len, pos);
        }
        self.buf[len - 1] = 0;
        self.len -= 1;
    }

    #[inline]
    fn insert_at(&mut self, pos: usize, byte: u8) {
        let len = self.len;
        if pos < len {
            self.buf.copy_within(pos..len, pos + 1);
        }
        self.buf[pos] = byte;
        self.len += 1;
        self.cursor += 1;
    }
}

struct PendingCmd {
    buf: [u8; MAX_CMD],
    len: usize,
    ready: bool,
}

impl PendingCmd {
    const fn new() -> Self {
        Self { buf: [0; MAX_CMD], len: 0, ready: false }
    }
}

static PENDING: Mutex<PendingCmd> = Mutex::new(PendingCmd::new());

lazy_static! {
    pub static ref SESSION: Mutex<Session> = Mutex::new(Session {
        cwd: 0,
        path: { let mut p = [0; MAX_PATH]; p[0] = b'/'; p },
        plen: 1,
    });
    pub static ref SHELL: Mutex<Shell> = Mutex::new(Shell {
        buf: [0; MAX_CMD],
        len: 0,
        cursor: 0,
        prompt_end_x: 0,
        history: [const { HistoryEntry::empty() }; MAX_HISTORY],
        history_count: 0,
        history_pos: 0,
        browsing: false,
        saved_buf: [0; MAX_CMD],
        saved_len: 0,
    });
}

pub fn init() {
    serial_println!("[shell] init");
    cprintln!(57, 197, 187, "MikuOS v0.2.3-rc");
    prompt();
}

pub fn dispatcher(line: &str) {
    let line = line.trim();
    if line.is_empty() { return; }

    let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
    let rest = rest.trim();

    match cmd {
        "fs.list"   => ext2_cmds::cmd_fs_list(),
        "fs.select" => ext2_cmds::cmd_fs_select(rest),
        "fs.umount" => ext2_cmds::cmd_fs_umount(rest),
        _ => commands::execute(line),
    }
}

pub fn process_pending() {
    let mut cmd_buf = [0u8; MAX_CMD];
    let cmd_len;
    {
        let mut p = PENDING.lock();
        if !p.ready {
            return;
        }
        cmd_len = p.len;
        cmd_buf[..cmd_len].copy_from_slice(&p.buf[..cmd_len]);
        p.ready = false;
        p.len = 0;
    }
    let s = unsafe { core::str::from_utf8_unchecked(&cmd_buf[..cmd_len]) };
    serial_println!("[shell] exec: '{}'", s);
    dispatcher(s);
    serial_println!("[shell] exec done");
    prompt();
}

fn prompt() {
    {
        let s = SESSION.lock();
        let p = unsafe { core::str::from_utf8_unchecked(&s.path[..s.plen]) };
        print!("\n");
        cprint!(100, 160, 255, "miku");
        cprint!(150, 160, 170, "@");
        cprint!(255, 255, 255, "os");
        cprint!(150, 160, 170, ":");
        cprint!(57, 197, 187, "{}", p);
        cprint!(255, 255, 255, " $ ");
    }
    let mut sh = SHELL.lock();
    sh.prompt_end_x = console::get_x();
    if sh.len > 0 {
        sh.redraw_full(0);
    }
    sh.draw_cursor();
}

pub fn handle_keypress(key: DecodedKey) {
    let mut sh = SHELL.lock();
    match key {
        DecodedKey::Unicode(c) => match c {
            '\n' => {
                sh.erase_cursor();
                let cl = sh.len;
                if cl > 0 {
                    sh.save_to_history();
                    let mut p = PENDING.lock();
                    p.buf[..cl].copy_from_slice(&sh.buf[..cl]);
                    p.len = cl;
                    p.ready = true;
                }
                sh.len = 0;
                sh.cursor = 0;
                sh.browsing = false;
                drop(sh);
                print!("\n");
                if cl == 0 {
                    prompt();
                }
            }
            '\u{8}' => {
                if sh.cursor > 0 {
                    let old_len = sh.len;
                    let pos = sh.cursor - 1;
                    sh.delete_at(pos);
                    sh.cursor = pos;
                    sh.redraw_from(pos, old_len);
                    sh.draw_cursor();
                }
            }
            '\x03' => {
                crate::net::CTRL_C.store(true, Ordering::SeqCst);
                crate::println!("^C");
            }
            _ => {
                if sh.len < MAX_CMD {
                    let b = c as u8;
                    if b >= 0x20 && b <= 0x7E {
                        sh.browsing = false;
                        let cur = sh.cursor;
                        let old_len = sh.len;
                        sh.insert_at(cur, b);

                        if cur == old_len {
                            sh.draw_append();
                            sh.draw_cursor();
                        } else {
                            sh.redraw_from(cur, old_len);
                            sh.draw_cursor();
                        }
                    }
                }
            }
        }
        DecodedKey::RawKey(rk) => {
            use pc_keyboard::KeyCode;
            match rk {
                KeyCode::ArrowLeft if sh.cursor > 0 => {
                    sh.erase_cursor();
                    sh.cursor -= 1;
                    sh.draw_cursor();
                }
                KeyCode::ArrowRight if sh.cursor < sh.len => {
                    sh.erase_cursor();
                    sh.cursor += 1;
                    sh.draw_cursor();
                }
                KeyCode::Home if sh.cursor > 0 => {
                    sh.erase_cursor();
                    sh.cursor = 0;
                    sh.draw_cursor();
                }
                KeyCode::End if sh.cursor < sh.len => {
                    sh.erase_cursor();
                    sh.cursor = sh.len;
                    sh.draw_cursor();
                }
                KeyCode::Delete => {
                    let cur = sh.cursor;
                    if cur < sh.len {
                        let old_len = sh.len;
                        sh.delete_at(cur);
                        sh.redraw_from(cur, old_len);
                        sh.draw_cursor();
                    }
                }
                KeyCode::ArrowUp => {
                    if sh.history_count == 0 { return; }
                    let old_len = sh.len;
                    if !sh.browsing {
                        sh.save_input();
                        sh.history_pos = sh.history_count;
                        sh.browsing = true;
                    }
                    if sh.history_pos > 0 {
                        sh.history_pos -= 1;
                        let idx = sh.history_pos % MAX_HISTORY;
                        sh.load_history(idx);
                    }
                    sh.redraw_full(old_len);
                    sh.draw_cursor();
                }
                KeyCode::ArrowDown => {
                    if !sh.browsing { return; }
                    let old_len = sh.len;
                    if sh.history_pos < sh.history_count - 1 {
                        sh.history_pos += 1;
                        let idx = sh.history_pos % MAX_HISTORY;
                        sh.load_history(idx);
                    } else if sh.history_pos == sh.history_count - 1 {
                        sh.history_pos = sh.history_count;
                        sh.restore_input();
                        sh.browsing = false;
                    }
                    sh.redraw_full(old_len);
                    sh.draw_cursor();
                }
                _ => {}
            }
        }
    }
}

pub fn update_path(s: &mut Session, arg: &str) {
    if arg.is_empty() { return; }
    if arg.starts_with('/') {
        s.path[0] = b'/';
        s.plen = 1;
    }
    for component in arg.split('/') {
        if component.is_empty() || component == "." { continue; }
        if component == ".." {
            if s.plen > 1 {
                let mut nl = s.plen - 1;
                while nl > 0 && s.path[nl] != b'/' { nl -= 1; }
                s.plen = if nl == 0 { 1 } else { nl };
            }
            continue;
        }
        append_component(s, component);
    }
}

#[inline]
fn append_component(s: &mut Session, name: &str) {
    if s.plen == 0 { return; }
    if s.plen > 1 && s.plen < MAX_PATH {
        s.path[s.plen] = b'/';
        s.plen += 1;
    }
    let bytes = name.as_bytes();
    let n = bytes.len().min(MAX_PATH - s.plen);
    if n > 0 {
        s.path[s.plen..s.plen + n].copy_from_slice(&bytes[..n]);
        s.plen += n;
    }
}

pub fn kbd_thread() -> ! {
    use pc_keyboard::{layouts, HandleControl, Keyboard, ScancodeSet1};
    let mut keyboard = Keyboard::new(
        ScancodeSet1::new(),
        layouts::Us104Key,
        HandleControl::MapLettersToUnicode,
    );
    crate::serial_println!("[kbd] thread started");
    loop {
        if !crate::boot::is_done() {
            crate::scheduler::sleep(CMD_POLL_TICKS);
            continue;
        }
        let mut got = false;
        while let Some(sc) = crate::stdin::pop() {
            got = true;
            if let Ok(Some(ev)) = keyboard.add_byte(sc) {
                if let Some(key) = keyboard.process_keyevent(ev) {
                    if crate::user_stdin::is_foreground_active() {
                        match key {
                            DecodedKey::Unicode(c) => {
                                crate::user_stdin::feed_char(c);
                            }
                            DecodedKey::RawKey(_) => {}
                        }
                    } else {
                        match key {
                            DecodedKey::Unicode('\u{0003}') => {
                                crate::net::CTRL_C.store(true, Ordering::SeqCst);
                                crate::println!("^C");
                            }
                            other => handle_keypress(other),
                        }
                    }
                }
            }
        }
        if !got {
            crate::scheduler::sleep(KBD_POLL_TICKS);
        }
    }
}

pub fn shell_thread() -> ! {
    crate::serial_println!("[shell] thread started");
    loop {
        if !crate::boot::is_done() {
            crate::scheduler::sleep(CMD_POLL_TICKS);
            continue;
        }
        crate::commands::ext_cmds_common::periodic_flush_check();
        if PENDING.lock().ready {
            process_pending();
        } else {
            crate::scheduler::sleep(CMD_POLL_TICKS);
        }
    }
}

/// Called by mikuD before restarting the shell service
/// Resets pending command buffer and redraws prompt so the user
/// sees a clean shell after restart
pub fn on_shell_restart() {
    {
        let mut p = PENDING.lock();
        p.ready = false;
        p.len = 0;
    }
    {
        let mut sh = SHELL.lock();
        sh.len = 0;
        sh.cursor = 0;
        sh.browsing = false;
    }
    // Clear foreground process (if any was running when shell died)
    crate::user_stdin::clear_foreground();
    serial_println!("[shell] reinit after restart");
    prompt();
}

/// Called by mikuD before restarting the kbd service
/// Drains stale scancodes from the stdin buffer
pub fn on_kbd_restart() {
    while crate::stdin::pop().is_some() {}
    serial_println!("[kbd] reinit after restart");
}
