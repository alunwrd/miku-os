pub mod commands;

use crate::shell::commands::ext2_cmds;
use crate::io::console;
use crate::{cprint, cprintln, print, serial_println};
use core::sync::atomic::{AtomicBool, Ordering};
use lazy_static::lazy_static;
use pc_keyboard::DecodedKey;
use spin::Mutex;

const MAX_PATH: usize = 64;
const MAX_CMD: usize = 64;
const MAX_HISTORY: usize = 16;
pub(crate) const KBD_POLL_TICKS: u64 = 1;
pub(crate) const CMD_POLL_TICKS: u64 = 5;

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
    /// Where the prompt ended, in cells
    ///
    /// The pixel position beside it is what the kernel paints with; this is
    /// what the line belongs to. It gives the caret, which a client drawing the
    /// console takes from the console's own pen, and shows whether somebody
    /// else has printed since: the row moves under the editor when a driver or
    /// the window manager writes a line, and the input line goes with it
    pub prompt_row: usize,
    pub prompt_col: usize,
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
        // And the pen, which a client drawing the console reads as the cursor.
        // The two pixels the kernel paints are the same answer for the window
        // the kernel draws itself
        console::set_pen(self.prompt_row, self.prompt_col + self.cursor);
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
        prompt_row: 0,
        prompt_col: 0,
        history: [const { HistoryEntry::empty() }; MAX_HISTORY],
        history_count: 0,
        history_pos: 0,
        browsing: false,
        saved_buf: [0; MAX_CMD],
        saved_len: 0,
    });
}

/// Drops the session back to / after an ext subtree is unmounted: the cwd may
/// no longer exist. Registered with fs::ext::mount so the filesystem can
/// announce the event without knowing about the shell
fn on_ext_unmount() {
    let mut s = SESSION.lock();
    s.cwd = 0;
    s.path[0] = b'/';
    s.plen = 1;
}

pub fn init() {
    crate::fs::ext::mount::set_unmount_hook(on_ext_unmount);
    serial_println!("[shell] init");
    cprintln!(57, 197, 187, "MikuOS v{}", env!("CARGO_PKG_VERSION"));

    // Anything the boot-time root mount refused to do happened before there
    // was a screen to say it on
    let trouble = crate::fs::ext::mount::take_root_trouble();
    if !trouble.is_empty() {
        crate::println!();
        cprintln!(255, 80, 80, "  the root filesystem was not mounted:");
        for line in trouble {
            crate::println!("{}", line);
        }
        crate::println!();
    }

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
        "fs.rw"     => ext2_cmds::cmd_fs_remount(rest, true),
        "fs.ro"     => ext2_cmds::cmd_fs_remount(rest, false),
        "fs.umount" => ext2_cmds::cmd_fs_umount(rest),
        _ => { commands::execute(line); }
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
    // Whose console this is, before anything is run on it
    adopt_terminal_owner();
    serial_println!("[shell] exec: '{}'", s);
    dispatcher(s);
    serial_println!("[shell] exec done");
    prompt();
    // One flush for the whole command rather than one per printed line. A no-op
    // unless the console is on a display the CPU cannot store into: the GB206,
    // where a pixel costs an MMIO write
    console::flush();
}

/// Runs as whoever owns the terminal this shell is being typed at
///
/// On a text boot the console is the machine's own, `login` has run, and the
/// shell thread already carries that person's credential, so this finds nothing
/// and changes nothing.
///
/// `ask_who_is_there` returns early on a graphical boot, because the greeter is
/// the login, which leaves the shell thread at `Cred::root()` with `CAP_ALL`.
/// `/bin/mayterm`, started by a session and showing this shell, delivers
/// keystrokes into it (`mayland::input::drain` sets `TO_SHELL` when the console
/// window has focus), which would be a way to root past mikudo,
/// `/etc/mikudo.conf`, the password and the journal.
///
/// So the shell belongs to whoever holds the terminal. The session is found
/// from the process drawing the console, which the display server knows and a
/// client cannot forge: binding the machine's console needs `CAP_MAY_PRIV`, and
/// the pid on the connection was written by the kernel when the socket was
/// accepted.
///
/// An interim: with a pty per terminal the shell is a program with a session of
/// its own, and there is no shared ring-0 credential to adopt
fn adopt_terminal_owner() {
    let Some(pid) = crate::mayland::server::console_client_pid() else { return };
    let Some(session) = crate::process::session::of_process(pid) else { return };
    let want = session.cred();
    let have = crate::sched::current_cred();
    if have.uid == want.uid && have.euid == want.euid {
        return;
    }
    crate::sched::update_current_cred(|c| *c = want);
    // The prompt says who this is. Looked up here because this branch runs only
    // when the credential changed, once per session
    let mut name_buf = [0u8; 33];
    let name = crate::syscall::adapters::process::passwd_name(session.uid, &mut name_buf)
        .unwrap_or(if session.uid == 0 { "root" } else { "user" });
    set_session_name(name, session.uid);
    serial_println!(
        "[shell] the console belongs to uid {} (session {}, drawn by pid {})",
        session.uid, session.id, pid
    );
}

/// The name of whoever the console belongs to, its length, and their uid.
///
/// A cached copy, because a lookup in /etc/passwd from `prompt()` deadlocks:
/// `reprompt()` is called from the console's print path whenever output moved
/// the pen, that output can come from inside a `with_vfs` closure, and VFS_LOCK
/// is a SchedMutex, which is not reentrant.
///
/// So the lookup happens once, where a session is established, and the prompt
/// reads bytes. `set_session_name` is called by syscall 92 and by `logout`.
///
/// The uid rides along because `current_cred()` is the credential of whichever
/// thread is drawing, and only one of the four callers of `prompt` is the shell
/// service thread. The keyboard thread draws it after a bare Enter, the
/// compositor's frame loop through `feed_pasted`, and mikuD on a shell restart;
/// all three are kernel threads born `Cred::root()`. The prompt has to describe
/// the identity the shell holds
static SESSION_NAME: Mutex<([u8; 33], usize, u16)> =
    Mutex::new((*b"root\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0", 4, 0));

/// Records who the console now belongs to. Called from the one place a session
/// is established and the one place it ends
pub fn set_session_name(name: &str, uid: u16) {
    {
        let mut slot = SESSION_NAME.lock();
        let n = name.as_bytes().len().min(slot.0.len());
        slot.0[..n].copy_from_slice(&name.as_bytes()[..n]);
        slot.1 = n;
        slot.2 = uid;
    }
    // And says so on the screen. The prompt already there was drawn for
    // somebody else, and the shell only draws another after a command. Outside
    // the lock above, because `prompt` reads it
    prompt();
}

/// Who the console belongs to right now. The colour follows the credential the
/// kernel holds
fn draw_who() {
    let slot = SESSION_NAME.lock();
    let uid = slot.2;
    let name = core::str::from_utf8(&slot.0[..slot.1]).unwrap_or("?");
    if uid == 0 {
        cprint!(255, 120, 120, "{}", name);
    } else {
        cprint!(100, 160, 255, "{}", name);
    }
    drop(slot);
    cprint!(150, 160, 170, "@");
    cprint!(255, 255, 255, "os");
}

/// `#` for root, `$` for everybody else
fn prompt_sigil() -> &'static str {
    if SESSION_NAME.lock().2 == 0 { " # " } else { " $ " }
}

fn prompt() {
    {
        let s = SESSION.lock();
        let p = unsafe { core::str::from_utf8_unchecked(&s.path[..s.plen]) };
        print!("\n");
        draw_who();
        cprint!(150, 160, 170, ":");
        cprint!(57, 197, 187, "{}", p);
        cprint!(255, 255, 255, "{}", prompt_sigil());
    }
    let mut sh = SHELL.lock();
    sh.prompt_end_x = console::get_x();
    let (row, col) = console::pen();
    sh.prompt_row = row;
    sh.prompt_col = col;
    if sh.len > 0 {
        sh.redraw_full(0);
    }
    sh.draw_cursor();
}

/// Puts the prompt back where the pen is now, keeping what was typed
///
/// Anything else printing while a line is typed moves the pen: the window
/// manager saying a workspace changed, a driver finding a disk, another
/// thread's output. The line being edited belongs to the prompt's row, so once
/// that is no longer where the console writes, the whole line is drawn again
/// where it now is
fn reprompt() {
    {
        let s = SESSION.lock();
        let p = unsafe { core::str::from_utf8_unchecked(&s.path[..s.plen]) };
        // No leading newline: whatever printed last left the pen at the start
        // of a row
        draw_who();
        cprint!(150, 160, 170, ":");
        cprint!(57, 197, 187, "{}", p);
        cprint!(255, 255, 255, "{}", prompt_sigil());
    }
    let mut sh = SHELL.lock();
    sh.prompt_end_x = console::get_x();
    let (row, col) = console::pen();
    sh.prompt_row = row;
    sh.prompt_col = col;
    if sh.len > 0 {
        sh.redraw_full(0);
    }
    sh.draw_cursor();
}

/// One pasted character, taking the road a typed one takes
///
/// The keyboard thread decides, for every decoded character, between a
/// foreground process that is reading and the shell's own line editor. A paste
/// makes the same decision, so this arm is called from both.
/// `may_console.paste` is the other caller
pub fn feed_pasted(c: char) {
    // Ctrl+C is not pasteable. A clipboard carrying a control character puts
    // the character in the line and signals nothing: a paste is text arriving
    if crate::io::input::user_stdin::is_foreground_active() {
        crate::io::input::user_stdin::feed_char(c);
    } else {
        handle_keypress(DecodedKey::Unicode(c));
    }
}

pub fn handle_keypress(key: DecodedKey) {
    edit_line(key);
    // The typed character is echoed through the console's cursor and input-line
    // paths, not print!, so a display that has to be pushed to needs this here
    // as well as after a command
    console::flush();
}

fn edit_line(key: DecodedKey) {
    // Has anything else printed since the prompt was drawn? The pen is the
    // console's, so this answers whether the console is on the screen or in
    // somebody's window
    {
        let sh = SHELL.lock();
        let moved = console::pen().0 != sh.prompt_row;
        drop(sh);
        if moved {
            reprompt();
        }
    }
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
                // ASCII only, refused rather than cut: `c as u8` of 'й'
                // (U+0439) is '9'. The commands are all ASCII, and the line is
                // a byte per column
                if sh.len < MAX_CMD && (' '..='~').contains(&c) {
                    sh.browsing = false;
                    let cur = sh.cursor;
                    let old_len = sh.len;
                    sh.insert_at(cur, c as u8);

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


pub fn shell_thread() -> ! {
    crate::serial_println!("[shell] thread started");
    // Which target the console last asked "who is there" under. Once per target
    // rather than once per boot: a graphical boot whose greeter cannot be kept
    // up falls back to multi-user (see mikuD's burst limit), and the console is
    // then the only login
    let mut asked_under: Option<crate::mikud::Target> = None;
    loop {
        if !crate::kcore::boot_state::is_done() {
            crate::sched::sleep(CMD_POLL_TICKS);
            continue;
        }
        // Once, at the first prompt of a text boot: who is at this console?
        //
        // The console is ring 0 and a person at it can reboot into anything, so
        // this is not a gate: it is the question the graphical greeter asks,
        // where there is no greeter. It gives the console an identity: a uid on
        // every file it writes, a capability set, and a mikudo that can say who
        // asked.
        //
        // Whatever happens, the shell carries on: a machine whose login cannot
        // run has to be fixed from this console
        let target = crate::mikud::current_target();
        if asked_under != Some(target) && ask_who_is_there() {
            asked_under = Some(target);
        }
        if PENDING.lock().ready {
            process_pending();
        } else {
            crate::sched::sleep(CMD_POLL_TICKS);
        }
    }
}

/// Runs the console's login prompt at boot, if this boot wants one
///
/// Three reasons not to:
///
///   - `noauth` on the boot line: the documented way to a root console
///   - a graphical boot: the greeter is the login, and a console prompt behind
///     it would be one nobody can see
///   - no /bin/login: a kernel booted with no root filesystem
///
/// Returns whether the question is settled for this target. False means not
/// yet: `run_login` blocks in `waitpid` until somebody logs in, so starting it
/// against a console not being painted would leave a password prompt nobody can
/// see. The fallback out of the graphical target happens while the compositor
/// owns the screen, so the console waits for its pixels back and the caller
/// asks again
fn ask_who_is_there() -> bool {
    if crate::arch::x86_64::grub::cmdline_flag("noauth") {
        crate::serial_println!("[shell] 'noauth' - the console stays root");
        return true;
    }
    if crate::mikud::target_is(crate::mikud::Target::Graphical) {
        return true;
    }
    if crate::io::console::is_headless() {
        // Unconditional: this is the one branch of the three ending in a
        // console with no prompt and no explanation. It fires once per attempt,
        // and only while nothing is painting
        crate::serial_println!(
            "[shell] the console is not being painted - waiting before asking who is there");
        return false;
    }
    if crate::fs::vfs::core::with_vfs(|v| v.resolve_path(0, "/bin/login").is_err()) {
        crate::serial_println!("[shell] no /bin/login - the console stays root");
        return true;
    }
    crate::shell::commands::run_login();
    prompt();
    true
}

/// Called by mikuD before restarting the shell service. Resets the pending
/// command buffer and redraws the prompt
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
    crate::io::input::user_stdin::clear_foreground();
    serial_println!("[shell] reinit after restart");
    prompt();
}

/// Called by mikuD before restarting the kbd service
/// Drains stale scancodes from the stdin buffer
pub fn on_kbd_restart() {
    while crate::io::input::stdin::pop().is_some() {}
    serial_println!("[kbd] reinit after restart");
}
