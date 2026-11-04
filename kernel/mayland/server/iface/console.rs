// may_console: the kernel's text, drawn by somebody else
//
// The kernel keeps the text and one program draws it. What crosses this
// interface is a shared-memory handle onto the character grid, and which rows
// of it have moved.
//
// Nothing here calls into `io::console` while holding the server's lock, the
// shared-memory table's lock, or any other. The console is written to from
// interrupts, from the shell and from every process that prints, so its lock is
// taken by everybody, and a path taking it second can deadlock. So
// `console_pump` asks the console everything first, then talks to the client,
// and a request that changes the console records what it wants.
//
// The handover with the in-kernel console window is in `console_pump` because
// both halves connect or disconnect a client, and bind already holds the lock
// they need.

use alloc::vec::Vec;
use spin::Mutex;

use crate::mayland::protocol;
use crate::mayland::wire::{Arg, Reader};

use crate::mayland::server::{shm};
use crate::mayland::server::state::*;

pub(in crate::mayland) fn on_console(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;

    // `bind` comes first and is the only request answerable before it
    if opcode == protocol::console::BIND {
        let raw = r.uint().map_err(|_| "bind: truncated")?;
        return bind_terminal(s, idx, sender, raw);
    }

    // Which terminal this client is drawing. A client that has not said is not
    // drawing one
    let Some(tty) = s.clients[idx].console_tty else {
        return Err("may_console: bind a terminal before anything else");
    };
    let slot = tty.index();
    if !matches!(console_holder(s, slot), Some(l) if l.client == cid) {
        return Err("may_console: this client is not drawing that terminal");
    }

    match opcode {
        protocol::console::SET_SIZE => {
            let cols = r.uint().map_err(|_| "set_size: truncated")? as usize;
            let rows = r.uint().map_err(|_| "set_size: truncated")? as usize;
            // Recorded, not done. Re-flowing the text takes the console's lock
            // and allocates, and this runs inside the server's (see the rule at
            // the top of this section). The next frame applies it
            if cols > 0 && rows > 0 {
                s.clients[idx].want_grid = Some((cols, rows));
            }
            Ok(())
        }
        protocol::console::SET_VIEW => {
            let back = r.uint().map_err(|_| "set_view: truncated")? as usize;
            // Recorded, not done, as for `SET_SIZE` above: moving the window
            // marks every row of the grid, and the grid's lock may not be taken
            // inside the server's
            s.clients[idx].want_view = Some(back);
            Ok(())
        }
        protocol::console::DETACH => {
            CONSOLES.lock()[slot] = None;
            s.clients[idx].console_tty = None;
            Ok(())
        }
        protocol::console::PASTE => {
            let text = r.string().map_err(|_| "paste: bad string")?.unwrap_or("");
            if text.len() > MAX_PASTE {
                return Err("may_console: a paste longer than the console takes at once");
            }
            // Recorded, not typed. Reaching the keyboard's decoder prints an
            // echo, and printing takes the console's own lock, which this path
            // may not take
            let mut pending = PASTE[slot].lock();
            if pending.len() + text.len() > MAX_PASTE {
                return Err("may_console: pasting faster than the console reads");
            }
            pending.extend_from_slice(text.as_bytes());
            Ok(())
        }
        _ => Err("may_console: unknown request"),
    }
}

/// `may_console.bind`: which terminal this client draws
///
/// Three rules, all from the protocol:
///
///   * Once. A client changing terminals under the compositor would have a
///     frame of somebody else's text in its pool: the pool is allocated at the
///     terminal's ceiling on first use and never again.
///   * Yours. `io::tty::may_use` decides, and the `tty` syscall asks the same
///     function: the console is the machine's, a pty is its master's and its
///     master's children's. So an ordinary user's terminal needs no capability
///     beyond having made it, and no program can bind a terminal it did not make.
///   * One each. Two programs drawing one grid would rub out each other's
///     caret.
///     Asked through `console_holder`, so a lease left by a dead client reads as
///     free
fn bind_terminal(s: &mut Server, idx: usize, sender: u32, raw: u32)
    -> Result<(), &'static str>
{
    use crate::io::tty::{self, TtyId};

    let cid = s.clients[idx].id;
    if s.clients[idx].console_tty.is_some() {
        return Err("may_console: bind once - a second one would leave a frame \
                    of the old terminal in the pool");
    }

    // Zero is "the machine's console", without having to know the generation
    // the kernel gave it
    let id = if raw == 0 { tty::SYSTEM } else { TtyId(raw) };
    if !tty::exists(id) {
        return Err("may_console: no such terminal");
    }
    // The machine's own console is the one terminal that is nobody's property,
    // and drawing it is reading everything the kernel says, so it still costs
    // the capability. A pty is its master's, as `may_use` answers
    if id == tty::SYSTEM {
        if !crate::mayland::server::access::privileged(s.clients[idx].pid) {
            return Err("may_console: the machine's console needs may_priv");
        }
    } else if !tty::may_use(s.clients[idx].pid, id) {
        return Err("may_console: that terminal is not this client's to draw");
    }

    let slot = id.index();
    if matches!(console_holder(s, slot), Some(l) if l.client != cid) {
        return Err("may_console: another client is drawing that terminal");
    }

    let version = s.clients[idx].objects.get(sender).map(|o| o.version).unwrap_or(1);
    CONSOLES.lock()[slot] = Some(Lease { client: cid, object: sender, version });
    s.clients[idx].console_tty = Some(id);
    crate::serial_println!(
        "[console] client {} (pid {}) draws tty {:#x}", cid, s.clients[idx].pid, id.raw());
    Ok(())
}

/// The process drawing the console, if a program is
///
/// That decides whose console this is. The kernel's shell reads the keys
/// arriving at the console window, and on a graphical boot it never asked
/// anyone to log in: `shell::ask_who_is_there` returns early for a graphical
/// target, because the greeter is the login. A ring-0 shell holding
/// `Cred::root()` behind a logged-in user's window would be a way to root past
/// mikudo, the password and the journal.
///
/// So the shell asks who is drawing it: see `shell::terminal_owner`
pub fn console_client_pid() -> Option<u64> {
    let s = SERVER.lock();
    let l = console_holder(&s, 0)?;
    let idx = s.client_idx(l.client)?;
    let pid = s.clients[idx].pid;
    if pid == 0 { None } else { Some(pid) }
}

/// Which window is showing the console, if a program is drawing it
///
/// Asked once a frame by the input drain, to decide that a key belongs to the
/// shell rather than the focused client
pub fn console_window() -> Option<(u32, u32)> {
    terminal_window(crate::io::tty::SYSTEM)
}

/// The window showing one terminal, if a program is drawing it
fn terminal_window(id: crate::io::tty::TtyId) -> Option<(u32, u32)> {
    // The server first and the lease second, the order everything else in this
    // file uses
    let s = SERVER.lock();
    let l = console_holder(&s, id.index())?;
    let idx = s.client_idx(l.client)?;
    if s.clients[idx].console_tty != Some(id) { return None; }
    let cid = s.clients[idx].id;
    // The first toplevel this client has. A terminal has one; a program that
    // took the console and opened several has not said which, so the one it
    // made first
    s.surfaces.iter()
        .find(|x| x.client == cid && x.role == Role::Toplevel)
        .map(|x| (cid, x.id))
}

/// Which terminal the focused window is showing, if any
///
/// Asked once a frame by the input drain. The answer moves the keyboard
/// (`io::tty::set_active`), and everything downstream of that is the
/// discipline's
pub fn focused_terminal() -> Option<crate::io::tty::TtyId> {
    // One lock, taken once. `focus_key()` takes the server itself, so asking it
    // and then locking here would be a second lock, which stops the desktop on
    // a machine that does not preempt inside a spinlock
    let s = SERVER.lock();
    let focus = s.focus?;
    for (slot, l) in console_leases(&s) {
        let Some(idx) = s.client_idx(l.client) else { continue };
        let Some(tty) = s.clients[idx].console_tty else { continue };
        if tty.index() != slot { continue }
        let cid = s.clients[idx].id;
        let win = s.surfaces.iter()
            .find(|x| x.client == cid && x.role == Role::Toplevel)
            .map(|x| (cid, x.id));
        if win == Some(focus) {
            return Some(tty);
        }
    }
    None
}


/// Row damage taken from the console but not yet delivered to the client
///
/// `take_row_damage` hands over what moved and forgets it, so an early return
/// before the `damage` event would lose those rows. Unioned across frames and
/// cleared only once the event is queued. Per terminal
pub(in crate::mayland) static CARRIED_ROWS:
    [Mutex<Option<(usize, usize)>>; crate::io::tty::MAX_TTY] =
    [const { Mutex::new(None) }; crate::io::tty::MAX_TTY];

/// Where a row span is held between coming out of the console and going into
/// the client's pool.
///
/// The two ends take different locks (`WRITER` with interrupts off, `POOLS`
/// without), and neither may be held while the other is taken. So the span is
/// copied twice through a buffer grown to the widest span asked for and then
/// reused. For a console being typed into that is one row
static SPAN_BUF: Mutex<alloc::vec::Vec<u8>> = Mutex::new(alloc::vec::Vec::new());

/// Bytes the console has copied into the terminal's pool, in total and on the
/// last frame that copied any. Reported by `mayland status`
static CONSOLE_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static CONSOLE_LAST: Mutex<usize> = Mutex::new(0);

/// What the console has cost the terminal: total bytes copied, and the last
/// frame's
pub fn copy_stats() -> (u64, usize) {
    (
        CONSOLE_BYTES.load(core::sync::atomic::Ordering::Relaxed),
        *CONSOLE_LAST.lock(),
    )
}

/// Merge a row span into what is still owed, and answer with the whole of it
pub(in crate::mayland) fn carry_rows(slot: usize, moved: Option<(usize, usize)>)
    -> Option<(usize, usize)>
{
    let mut c = CARRIED_ROWS[slot].lock();
    *c = match (*c, moved) {
        (Some(a), Some(b)) => Some((a.0.min(b.0), a.1.max(b.1))),
        (Some(a), None) => Some(a),
        (None, b) => b,
    };
    *c
}

/// The most text one paste may carry, and the most that may be waiting
///
/// What the console will replay before the shell has had a chance to read any
/// of it. A client ignoring that is refused, so it cannot fill the kernel's
/// heap one paste at a time
const MAX_PASTE: usize = 4096;

/// Text a client has pasted and the console has not replayed yet
///
/// One per terminal, in a static, because the text belongs to the terminal and
/// not to whoever is drawing it: a program exiting mid-paste should not take
/// the rest of the line with it
static PASTE: [Mutex<Vec<u8>>; crate::io::tty::MAX_TTY] =
    [const { Mutex::new(Vec::new()) }; crate::io::tty::MAX_TTY];

/// Replays whatever has been pasted, as if typed
///
/// Runs from the frame loop with nothing held: a character reaching the shell
/// echoes itself, and an echo takes the console's lock
fn replay_paste(id: crate::io::tty::TtyId) {
    let text = {
        let mut p = PASTE[id.index()].lock();
        if p.is_empty() {
            return;
        }
        core::mem::take(&mut *p)
    };
    crate::serial_println!("[console] replaying {} pasted bytes", text.len());
    // Decoded as UTF-8, not walked as bytes: pasting "Привет" byte by byte
    // would feed twelve characters in the U+0080..U+00FF range.
    //
    // A byte outside a valid sequence is skipped: text that does not decode is
    // not given a meaning here
    let mut rest: &[u8] = &text;
    while !rest.is_empty() {
        let (c, used) = match core::str::from_utf8(rest) {
            Ok(valid) => match valid.chars().next() {
                Some(c) => (Some(c), c.len_utf8()),
                None => break,
            },
            Err(e) => {
                let good = e.valid_up_to();
                if good > 0 {
                    let valid = core::str::from_utf8(&rest[..good]).unwrap_or("");
                    match valid.chars().next() {
                        Some(c) => (Some(c), c.len_utf8()),
                        None => (None, 1),
                    }
                } else {
                    // Skip one byte and look again: `error_len` is None when
                    // the sequence is cut short, which for a paste means the
                    // end of the text
                    (None, e.error_len().unwrap_or(rest.len()).max(1))
                }
            }
        };
        if let Some(c) = c {
            // Through the same path a decoded keystroke takes, which says what
            // a character means whether a foreground process is reading or the
            // shell's own line editor.
            //
            // Which path depends on the terminal. The console's is the kernel
            // shell's line editor; a pty has no editor in ring 0, so the
            // character goes into its discipline for whatever reads that
            // terminal
            if id == crate::io::tty::SYSTEM {
                crate::shell::feed_pasted(c);
            } else {
                crate::io::tty::feed_char(id, c);
            }
        }
        rest = &rest[used.min(rest.len())..];
    }
}

/// One frame of console work, for every terminal somebody is drawing
///
/// Called from `console::pump`, which the compositor runs at the top of every
/// frame, so writes from interrupts and from several threads coalesce to one
/// message a frame. The order is the rule at the top of this section: ask the
/// console, then talk to the client, so the size a client asked for is applied
/// here.
///
/// What it asks is `io::tty`, which answers for the console and for a pty with
/// the same four questions
pub fn console_pump() {
    // A holder killed by a protocol error is not a holder: when the transport
    // notices a crashed terminal there is nothing to copy the grid to, and this
    // does nothing until another one starts. Collected once with the server
    // held once, and the work done afterwards with nothing held
    let leases = {
        let s = SERVER.lock();
        console_leases(&s)
    };
    for (slot, _) in leases {
        pump_one(slot);
    }
}

fn pump_one(slot: usize) {
    // What the client last asked for, what it has been given, and which
    // terminal it is drawing
    let (want, want_view, had, tty) = {
        let s = SERVER.lock();
        let Some(l) = console_holder(&s, slot) else { return };
        let Some(idx) = s.client_idx(l.client) else { return };
        let Some(tty) = s.clients[idx].console_tty else { return };
        (s.clients[idx].want_grid, s.clients[idx].want_view, s.clients[idx].grid, tty)
    };

    replay_paste(tty);

    // Everything the terminal has to say, asked for while nothing else is held
    if let Some((cols, rows)) = want {
        crate::io::tty::set_size(tty, cols, rows);
    }
    if let Some(back) = want_view {
        crate::io::tty::set_view(tty, back);
    }
    let cursor = crate::io::tty::cursor(tty);
    let moved = carry_rows(slot, crate::io::tty::take_row_damage(tty));
    // The shape, which is two numbers, before the contents, which are a hundred
    // kilobytes
    //
    // The terminal may have gone since the last frame: its master exited, or it
    // was hung up. `GONE` is what the protocol says happens here. Otherwise the
    // lease stays held and the client's window keeps accepting keys that reach
    // nobody
    if !crate::io::tty::exists(tty) {
        let mut s = SERVER.lock();
        if let Some(l) = console_holder(&s, slot) {
            let obj = l.object;
            let client = l.client;
            if let Some(idx) = s.client_idx(client) {
                s.clients[idx].queue(obj, protocol::console::GONE, &[]);
                s.clients[idx].console_tty = None;
            }
        }
        CONSOLES.lock()[slot] = None;
        *CARRIED_ROWS[slot].lock() = None;
        *PASTE[slot].lock() = alloc::vec::Vec::new();
        return;
    }

    let (cols, rows) = crate::io::tty::size(tty);
    if cols == 0 || rows == 0 {
        return;
    }
    let regrid = match had {
        Some(g) => g.cols != cols || g.rows != rows,
        None => true,
    };
    if !regrid && moved.is_none() && had.map(|g| g.cursor == cursor).unwrap_or(false) {
        // Nothing moved and the caret is where the client last drew it
        if want.is_some() {
            let mut s = SERVER.lock();
            if let Some(idx) = console_holder(&s, slot).and_then(|l| s.client_idx(l.client)) {
                s.clients[idx].want_grid = None;
                s.clients[idx].want_view = None;
            }
        }
        return;
    }
    // Something to say after all, and only as much of it as moved.
    //
    // The row span already worked out above decides what is copied: a keystroke
    // touches one row of the grid.
    //
    // The whole grid goes only on the first frame and on a change of shape. A
    // client reads the pool without synchronising, so a partly written grid of
    // a new shape would be rubbish on the screen
    let whole = had.is_none() || regrid;
    let span = if whole { (0, rows) } else { moved.unwrap_or((0, 0)) };
    // An empty span means copy nothing, not say nothing. The caret moves on its
    // own (an arrow key edits nothing), and the client has to be told or it
    // goes on drawing the caret where it was
    let copy = span.1 > span.0;

    // The pool is allocated once, at the console's ceiling. Re-allocating on a
    // shape change would destroy the old handle and unmap it from the client,
    // which reads the grid without holding anything, in the middle of drawing a
    // frame. A grid at its ceiling is a hundred kilobytes
    let (cap_cols, cap_rows) = crate::io::tty::capacity(tty);
    let cap = cap_cols * cap_rows * protocol::console::CELL_BYTES;
    let fresh = if had.is_none() {
        let pid = {
            let s = SERVER.lock();
            let Some(l) = console_holder(&s, slot) else { return };
            let Some(idx) = s.client_idx(l.client) else { return };
            s.clients[idx].pid
        };
        let Ok(handle) = shm::create(pid, cap) else { return };
        // Recorded on the client immediately: a pool nothing holds is a pool
        // nothing frees
        let g = Grid { handle, size: cap, cols, rows, cursor };
        let mut s = SERVER.lock();
        match console_holder(&s, slot).and_then(|l| s.client_idx(l.client)) {
            Some(idx) => s.clients[idx].grid = Some(g),
            None => {
                // The client let go while the memory was being made. Nothing is
                // left to own it, and nothing holds the server while shm is
                // asked
                drop(s);
                let _ = shm::destroy(handle);
                return;
            }
        }
        drop(s);
        Some(g)
    } else {
        None
    };

    let g = match fresh.or(had) { Some(g) => g, None => return };

    // Out of the console and into the client's pool, one row span.
    //
    // Two locks and neither may hold the other: the console takes `WRITER` with
    // interrupts off, the pool takes `POOLS`. So the span goes through a static
    // scratch buffer that grows to the widest span asked for and is then reused
    if copy {
        let stride = cols * protocol::console::CELL_BYTES;
        let mut scratch = SPAN_BUF.lock();
        let want = (span.1 - span.0) * stride;
        if scratch.len() < want {
            scratch.resize(want, 0);
        }
        if let Some((off, bytes)) =
            crate::io::tty::copy_row_range(tty, &mut scratch, span.0, span.1)
        {
            let _ = shm::with_bytes_mut(g.handle, off, bytes, |b| {
                let n = bytes.min(b.len()).min(scratch.len());
                b[..n].copy_from_slice(&scratch[..n]);
            });
            drop(scratch);
            CONSOLE_BYTES.fetch_add(bytes as u64, core::sync::atomic::Ordering::Relaxed);
            *CONSOLE_LAST.lock() = bytes;
        }
    }

    // And now the client
    let mut s = SERVER.lock();
    let Some(l) = console_holder(&s, slot) else { return };
    let Some(idx) = s.client_idx(l.client) else { return };
    s.clients[idx].want_grid = None;
    s.clients[idx].want_view = None;
    s.clients[idx].grid = Some(Grid { cols, rows, cursor, ..g });
    // The handle is the same each time; what changes is the shape used out of
    // it, so the client is told when that moves
    if fresh.is_some() || regrid {
        s.clients[idx].queue(l.object, protocol::console::GRID, &[
            Arg::Uint(g.handle), Arg::Uint(g.size as u32),
            Arg::Uint(cols as u32), Arg::Uint(rows as u32),
        ]);
    }
    // A whole grid when it is new, the rows that moved otherwise. The caret
    // rides along either way, since the client has to rub it out of where it
    // was.
    //
    // The same span that was copied: a damage event not matching what was
    // written is a row the terminal repaints from stale bytes
    let (r0, r1) = span;
    s.clients[idx].queue(l.object, protocol::console::DAMAGE, &[
        Arg::Uint(r0 as u32), Arg::Uint(r1 as u32),
        Arg::Uint(cursor.0 as u32), Arg::Uint(cursor.1 as u32),
    ]);
    // Delivered, so no longer owed. Every path that returned before this point
    // leaves it owed and the next frame sends it
    *CARRIED_ROWS[slot].lock() = None;
}
