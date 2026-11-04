// Terminals
//
// `console::TextGrid` is the text, separate from the pixels, and this is a
// table of terminals: tty0 is the console and the rest are pseudo-terminals
// with no pixels of their own.
//
// tty0 is `console::WRITER`, and this table holds the ones after it. `print!`
// reaches `WRITER` from interrupt handlers, the panic handler and the
// scheduler, with interrupts off. tty0 owns the panel, is the only terminal
// surviving the compositor stopping, and is where a panic goes.
//
// Every terminal shares the grid, which `may_console` and the shell talk to.
// `with_grid` answers for any of them.
//
// Not built:
//
// No `TIOCSTI`, and no input-injection primitive of any kind: Linux's
// most-exploited tty feature. A master end may write into its own pty, and is
// only obtained by creating that pty, so another's terminal cannot be named.
//
// No pluggable line disciplines: Linux's ldisc-switch machinery is a standing
// source of use-after-free.
//
// No allocation on any input or output path. Rings and grids are built with the
// terminal and never grown, as they are written to from an interrupt.

pub mod ldisc;
pub mod selftest;

use spin::Mutex;
use x86_64::instructions::interrupts;

use super::console::TextGrid;
use ldisc::{Feed, Foreground, LineDiscipline, Termios};

/// Terminals, tty0 included in the numbering but not in the table
pub const MAX_TTY: usize = 8;

/// A terminal, as a number that cannot be confused with a stale one
///
/// Index in the low eight bits, generation above. Never zero: `0` means no
/// controlling terminal everywhere
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TtyId(pub u32);

impl TtyId {
    pub fn index(self) -> usize { (self.0 & 0xff) as usize }
    pub fn generation(self) -> u32 { self.0 >> 8 }
    pub fn raw(self) -> u32 { self.0 }
}

/// The console. Its text and its pixels are in `io::console`, where every
/// `print!` reaches them
pub const SYSTEM: TtyId = TtyId(0x0100);

/// One pseudo-terminal: a grid, and who is allowed to feed it
pub struct Pty {
    pub grid: TextGrid,
    /// The process holding the master end, entitled to draw this terminal; see
    /// `mayland::server::iface::console`
    pub owner: u64,
    /// Generation of the `TtyId` this slot currently answers to
    pub generation: u32,
    /// The master has gone: reads give EOF, writes give EIO, and the slot is
    /// not reused until the last reference to it is dropped
    pub dying: bool,
    pub refs: u32,
}

static PTYS: [Mutex<Option<Pty>>; MAX_TTY] =
    [const { Mutex::new(None) }; MAX_TTY];

/// Read without a lock: the generation of each slot, or 0 for empty
///
/// The compositor asks whether this terminal exists and whose it is while
/// holding the display server's lock, and `iface/console.rs` must not take
/// anything else there. Written under the slot's own lock and read from
/// anywhere
static GEN:   [core::sync::atomic::AtomicU32; MAX_TTY] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_TTY];
static OWNER: [core::sync::atomic::AtomicU64; MAX_TTY] =
    [const { core::sync::atomic::AtomicU64::new(0) }; MAX_TTY];

/// The highest generation a slot has ever handed out, never going back
///
/// Separate from `GEN`, which is cleared when a slot is freed (how "empty" is
/// spelled). Deriving the next generation from that would reset the count on
/// release, and a `TtyId` kept across a hangup would silently name another's
/// terminal
static SEQ: [core::sync::atomic::AtomicU32; MAX_TTY] =
    [const { core::sync::atomic::AtomicU32::new(0) }; MAX_TTY];

/// One line discipline per terminal, slot 0 being the console's
///
/// A side table beside `PTYS`, as for `GEN` and `OWNER`: feeding a character
/// echoes it to the terminal's grid, and a discipline inside `Mutex<Pty>` would
/// have to take the lock it holds.
///
/// Slot 0 is the console's and has no `Pty`; `with_grid` answers for either
/// kind
static LDISC: [Mutex<LineDiscipline>; MAX_TTY] =
    [const { Mutex::new(LineDiscipline::new()) }; MAX_TTY];

/// Which job each terminal's input belongs to: a process group id, read from
/// the keyboard interrupt on every keystroke, so atomic.
///
/// A group is the whole of `sort | uniq`, as Ctrl-C has to reach
static FOREGROUND: [Foreground; MAX_TTY] =
    [const { Foreground::new() }; MAX_TTY];

/// The terminal the keyboard is talking to
///
/// One at a time: there is one keyboard. The console until something says
/// otherwise; `mayland::input` moves it when the focused window is a terminal
static ACTIVE: core::sync::atomic::AtomicU32 =
    core::sync::atomic::AtomicU32::new(SYSTEM.0);

/// Which terminal a keystroke goes to
pub fn active() -> TtyId {
    let raw = ACTIVE.load(Ordering::Acquire);
    let id = TtyId(raw);
    if exists(id) { id } else { SYSTEM }
}

/// Point the keyboard at a terminal. A terminal that has gone takes the
/// keyboard back to the console, not into a slot nobody owns
pub fn set_active(id: TtyId) {
    if exists(id) {
        ACTIVE.store(id.0, Ordering::Release);
    } else {
        ACTIVE.store(SYSTEM.0, Ordering::Release);
    }
}

/// The slot a `TtyId` addresses in the side tables
#[inline]
fn slot_of(id: TtyId) -> Option<usize> {
    if id == SYSTEM { return Some(0); }
    if !exists(id) { return None; }
    Some(id.index())
}

/// Run something against a terminal's line discipline
pub fn with_ldisc<R>(id: TtyId, f: impl FnOnce(&mut LineDiscipline) -> R) -> Option<R> {
    let slot = slot_of(id)?;
    Some(interrupts::without_interrupts(|| f(&mut LDISC[slot].lock())))
}

/// The terminal's settings
pub fn termios(id: TtyId) -> Termios {
    with_ldisc(id, |l| l.termios).unwrap_or(Termios::cooked())
}

/// Change them: what `ioctl(TCSETS)` does
pub fn set_termios(id: TtyId, t: Termios) {
    with_ldisc(id, |l| l.termios = t);
}

/// Which job a terminal's keys belong to, or 0 for nobody
pub fn foreground(id: TtyId) -> u64 {
    slot_of(id).map(|s| FOREGROUND[s].get()).unwrap_or(0)
}

/// Hand a terminal to a job, in a known state
///
/// `tcsetpgrp`. The reset: a program that died in raw mode would leave the next
/// with no echo and no line editing
pub fn set_foreground(id: TtyId, pgid: u64) {
    let Some(slot) = slot_of(id) else { return };
    interrupts::without_interrupts(|| LDISC[slot].lock().reset());
    FOREGROUND[slot].set(pgid);
}

/// Nobody is in the foreground: the shell has the keys again
pub fn clear_foreground(id: TtyId) {
    if let Some(slot) = slot_of(id) {
        FOREGROUND[slot].set(0);
    }
}

/// Would a read on this terminal return without sleeping?
pub fn readable(id: TtyId) -> bool {
    with_ldisc(id, |l| l.readable()).unwrap_or(false)
}

/// One character from the keyboard, into a terminal
///
/// The discipline decides and this acts: the echo and the signal come after the
/// lock is gone. Both reach code that takes locks of its own (the grid's, the
/// scheduler's), and doing either under the discipline's could deadlock with
/// two terminals open
pub fn feed_char(id: TtyId, c: char) {
    let Some(slot) = slot_of(id) else { return };

    // Typed before anything was reading is still typed: the window is up while
    // the program is starting. The discipline holds what was typed; the
    // foreground decides who a signal goes to
    let fg = FOREGROUND[slot].get();

    // Typing brings the window back to the live screen: showing one thing while
    // sending another is what a scrollback must not do.
    //
    // Output does not do this (see `TextGrid::snap_to_live`), or scrolling back
    // through a busy log would be impossible
    snap_to_live(id);

    let feed: Feed = interrupts::without_interrupts(|| LDISC[slot].lock().feed(c));

    if let Some(text) = feed.echo() {
        for _ in 0..feed.echo_repeat {
            echo(id, text);
        }
    }
    if let Some(sig) = feed.signal {
        if fg != 0 {
            // To the group: a shell starting `a | b` puts both in one group and
            // one Ctrl-C ends both
            crate::process::signal::send_to_group(fg, sig);
        }
    }
    if feed.wake || feed.signal.is_some() {
        // A poller watching this terminal has nothing else to tell it a key was
        // pressed, and the lock is gone by now, per the rule in `sched::wait`.
        // The echo is output, for the master's reader
        crate::sched::wait::wake_key(crate::sched::wait::key::tty_input(id));
        crate::sched::wait::wake_key(crate::sched::wait::key::pty_output(id));
    }
}

/// A run of bytes as input, decoded as UTF-8
///
/// What a master end's `write` and a paste do. A byte not part of a valid
/// sequence is skipped, not fed as itself
pub fn feed_bytes(id: TtyId, bytes: &[u8]) {
    let mut rest = bytes;
    while !rest.is_empty() {
        let (c, used) = match core::str::from_utf8(rest) {
            Ok(valid) => match valid.chars().next() {
                Some(c) => (Some(c), c.len_utf8()),
                None => break,
            },
            Err(e) => {
                let good = e.valid_up_to();
                if good > 0 {
                    match core::str::from_utf8(&rest[..good]).ok().and_then(|v| v.chars().next()) {
                        Some(c) => (Some(c), c.len_utf8()),
                        None => (None, 1),
                    }
                } else {
                    (None, e.error_len().unwrap_or(rest.len()).max(1))
                }
            }
        };
        if let Some(c) = c { feed_char(id, c); }
        rest = &rest[used.min(rest.len())..];
    }
}

/// A key that is not a character (an arrow, Home, Delete), as its escape
/// sequence
///
/// Only in raw mode. In canonical mode the discipline assembles a line for a
/// program that asked for a line, and `ESC [ A` would put `^[[A` in the middle
/// of a filename. A program wanting the keys asks for raw mode first, as a
/// shell with a readline does
pub fn feed_key(id: TtyId, seq: &str) {
    if termios(id).icanon {
        return;
    }
    for ch in seq.chars() {
        feed_char(id, ch);
    }
}

/// A process has gone: take the terminals it held
///
/// The foreground of a terminal, anybody's, since a program started from one is
/// on it. Called from the one place a process is known to have died
/// (`sched::lifecycle::kill_with_code`): a foreground slot holding a dead pid
/// would signal whoever is given that number next
pub fn on_process_exit(pid: u64) {
    // A terminal's foreground is a job, over when its last member has gone: the
    // keys stay with the job while the rest of the pipeline runs, and a Ctrl-C
    // reaches the work
    let pgid = crate::sched::pgid_of(pid);
    if pgid != 0 && !crate::sched::group_alive(pgid, pid) {
        for slot in 0..MAX_TTY {
            if FOREGROUND[slot].get() == pgid {
                FOREGROUND[slot].set(0);
            }
        }
    }
    // A master is a descriptor: it can be duplicated, inherited across fork and
    // held by several processes, and the terminal is over when the last of them
    // closes, reported by the master node's reference count (`master_unref`,
    // from `VNode::dec_ref`). Not hung up here: others may still hold it open.
    // The keyboard does not stay pointed at a terminal that has gone
    let a = TtyId(ACTIVE.load(Ordering::Acquire));
    if !exists(a) {
        ACTIVE.store(SYSTEM.0, Ordering::Release);
    }
}

/// Put echoed text on a terminal
///
/// The console goes through `print!` so its pixels are painted; a pty is a
/// grid, written directly: `print!` routes by the running task's `ctty`, and
/// the task running when a key arrives is whichever the interrupt landed on
fn echo(id: TtyId, text: &str) {
    if id == SYSTEM {
        crate::print!("{}", text);
    } else {
        with_grid(id, |g| g.write_str(text, super::console::COLOR_MIKU));
    }
}

/// A background job tried to read this terminal: SIGTTIN, as POSIX says
///
/// True when the caller was signalled and should look again: it will have
/// stopped, so that happens when it is continued, by which time it may be the
/// foreground. False when the signal was ignored or caught; it gets end of
/// input.
///
/// The signal goes out after any lock this file holds is gone
pub fn background_read(id: TtyId) -> bool {
    signal_background(id, crate::process::signal::SIGTTIN)
}

/// The write side
///
/// POSIX gates this on `TOSTOP`: a job writing in the background is ordinary
/// and a job reading in the background is a bug, so it is off by default and a
/// shell turns it on.
///
/// Changing the terminal's settings is not gated on `TOSTOP`: `tcsetattr` from
/// the background is SIGTTOU unconditionally, enforced by `ioctl`
pub fn background_write(id: TtyId) -> bool {
    if !termios(id).tostop { return false; }
    signal_background(id, crate::process::signal::SIGTTOU)
}

/// Changing the settings from the background: SIGTTOU, whatever `TOSTOP` says
///
/// True when the caller was stopped and the change must not happen. POSIX does
/// not let `TOSTOP` switch this off: it is not output that can be interleaved
/// and read later, it is the terminal changing under someone
pub fn stop_for_tcsetattr(id: TtyId) -> bool {
    signal_background(id, crate::process::signal::SIGTTOU)
}

/// Is the caller in this terminal's foreground job?
pub fn in_foreground(id: TtyId) -> bool {
    let fg = foreground(id);
    fg != 0 && fg == crate::sched::pgid_of(crate::sched::current_pid())
}

/// Send a job-control signal to the caller's own group, and say whether it
/// stopped
///
/// An orphaned group cannot be stopped, as nobody could continue it, so POSIX
/// makes this an error. Here it is the end-of-input answer the caller already
/// gives
fn signal_background(id: TtyId, sig: u32) -> bool {
    let me = crate::sched::current_pid();
    let my_pgid = crate::sched::pgid_of(me);
    let fg = foreground(id);
    // Nobody holds this terminal. A stop here could not be lifted: no
    // foreground job to be brought forward ahead of, and the shell that would
    // do it is absent. The answer is the error, which `msh` reads to say "this
    // terminal's keys belong to job N"
    if fg == 0 || my_pgid == 0 || fg == my_pgid {
        return false;
    }
    // A program that ignores or catches it has said it would rather carry on
    if crate::process::signal::disposition(me, sig) != crate::process::signal::DISP_DEFAULT {
        return false;
    }
    crate::process::signal::send_to_group(my_pgid, sig);
    // It stopped, so this only returns once somebody has continued it
    crate::sched::process_alive(me)
}

/// Put a program's output on a named terminal
///
/// `print!` answers "my terminal"; this answers "that one", for `/dev/console`
/// (tty0 whoever writes) and `/dev/tty` (resolved to a terminal before this).
///
/// False when the terminal has gone, so the caller can report it
pub fn write_text(id: TtyId, text: &str) -> bool {
    // A background job writing here is stopped when the terminal asks for it.
    // Off by default: a job printing in the background is ordinary
    if background_write(id) {
        return false;
    }
    if id == SYSTEM {
        // Through the console's own path, not `print!`, which would route this
        // to the writer's terminal
        crate::io::console::write_console(format_args!("{}", text));
        return true;
    }
    let ok = with_grid(id, |g| g.write_str(text, super::console::COLOR_MIKU)).is_some();
    // What the program printed is waiting for its master's reader
    if ok {
        crate::sched::wait::wake_key(crate::sched::wait::key::pty_output(id));
    }
    ok
}

/// Take what has been typed at a terminal. `None` is end of input
pub fn read_input(id: TtyId, out: &mut [u8], max: usize) -> Option<usize> {
    with_ldisc(id, |l| l.take(out, max))?
}

/// Put bytes back at the front of what has been typed
///
/// For `VMIN`: a raw-mode read that asked for five characters and found three
/// returns them to the ring and waits, in the order they left.
///
/// Not a general input injection: it takes no pid, cannot name another
/// terminal's ring, and the only caller is the read that just emptied it
pub fn unread(id: TtyId, bytes: &[u8]) {
    with_ldisc(id, |l| l.unread(bytes));
}

// The text of any terminal
//
// `may_console` copies a grid into a client's pool and says which rows moved.
// These are the questions asked of a terminal, the console among them.

/// The shape a terminal is being used at
pub fn size(id: TtyId) -> (usize, usize) {
    if id == SYSTEM {
        super::console::text_size()
    } else {
        with_grid(id, |g| g.size()).unwrap_or((0, 0))
    }
}

/// The largest shape it could be reshaped to. A pool is allocated at this once,
/// so a client reading without a lock knows the memory under it stays
pub fn capacity(id: TtyId) -> (usize, usize) {
    if id == SYSTEM {
        super::console::grid_capacity()
    } else {
        TextGrid::capacity()
    }
}

/// Where the pen is
pub fn cursor(id: TtyId) -> (usize, usize) {
    if id == SYSTEM {
        super::console::cursor()
    } else {
        with_grid(id, |g| g.cursor()).unwrap_or((0, 0))
    }
}

/// How far back this terminal can be scrolled, in rows
pub fn history_depth(id: TtyId) -> usize {
    with_grid(id, |g| g.history_depth()).unwrap_or(0)
}

/// Where this terminal's window is. `0` is the live screen
pub fn view(id: TtyId) -> usize {
    with_grid(id, |g| g.view()).unwrap_or(0)
}

/// Move the window back through the history. `0` is the live screen.
///
/// Answers with where it landed, clamped to the history: a client asking a
/// thousand rows on a terminal that has scrolled ten gets ten
pub fn set_view(id: TtyId, rows_back: usize) -> usize {
    with_grid(id, |g| g.set_view(rows_back)).unwrap_or(0)
}

/// Back to the live screen. What a keystroke does
pub fn snap_to_live(id: TtyId) -> bool {
    with_grid(id, |g| g.snap_to_live()).unwrap_or(false)
}

/// Which rows have changed since this was last asked. Destructive
pub fn take_row_damage(id: TtyId) -> Option<(usize, usize)> {
    if id == SYSTEM {
        super::console::take_row_damage()
    } else {
        with_grid(id, |g| g.take_row_damage()).flatten()
    }
}

/// Copy a span of rows out, answering with the byte offset into a whole-grid
/// buffer and the length written
pub fn copy_row_range(id: TtyId, dst: &mut [u8], r0: usize, r1: usize)
    -> Option<(usize, usize)>
{
    if id == SYSTEM {
        super::console::copy_row_range(dst, r0, r1)
    } else {
        with_grid(id, |g| g.copy_rows(dst, r0, r1)).flatten()
    }
}

/// Reshape a terminal to what the window showing it can hold, and tell the job
/// at it with SIGWINCH.
///
/// A program laying text out in columns (`ls`, anything with a progress bar)
/// asks `TIOCGWINSZ` when it starts. SIGWINCH is ignored by default, so a
/// program that does not care is not killed.
///
/// The signal goes out after the grid's lock is gone: it reaches the scheduler,
/// never entered under a terminal's own lock
pub fn set_size(id: TtyId, cols: usize, rows: usize) -> bool {
    let before = size(id);
    let ok = if id == SYSTEM {
        super::console::set_grid(cols, rows)
    } else {
        with_grid(id, |g| g.resize(cols, rows)).unwrap_or(false)
    };
    // Only when the shape moved. The compositor asks for a size every frame,
    // almost always the one it has
    if ok && size(id) != before {
        let fg = foreground(id);
        if fg != 0 {
            crate::process::signal::send_to_group(fg, crate::process::signal::SIGWINCH);
        }
    }
    ok
}

use core::sync::atomic::Ordering;

/// Does this terminal exist, and is the id current rather than a stale one?
pub fn exists(id: TtyId) -> bool {
    if id == SYSTEM { return true; }
    let i = id.index();
    if i == 0 || i >= MAX_TTY { return false; }
    let g = GEN[i].load(Ordering::Acquire);
    g != 0 && g == id.generation()
}

/// Which process holds this terminal's master end, if it is a pty at all
pub fn master_owner(id: TtyId) -> Option<u64> {
    if id == SYSTEM { return None; }
    if !exists(id) { return None; }
    let o = OWNER[id.index()].load(Ordering::Acquire);
    if o == 0 { None } else { Some(o) }
}

/// The next generation for a slot. Starts at 1 so a `TtyId` is never zero, and
/// only goes up
///
/// Wrapping skips zero: generation zero is how an empty slot is spelled.
/// Reaching that takes four billion terminals through one slot
fn next_generation(slot: usize) -> u32 {
    let prev = SEQ[slot].load(Ordering::Relaxed);
    let next = if prev == u32::MAX { 1 } else { prev + 1 };
    SEQ[slot].store(next, Ordering::Relaxed);
    next
}

/// Open a pseudo-terminal owned by `owner`, `cols` by `rows`
///
/// The ceiling is fixed for the terminal's life: `may_console`'s shared pool is
/// allocated at it once and clients read it without synchronising
pub fn open_pty(owner: u64, cols: usize, rows: usize) -> Option<TtyId> {
    interrupts::without_interrupts(|| {
        // Slot 0 is the console and is never handed out here
        for slot in 1..MAX_TTY {
            let mut s = PTYS[slot].lock();
            if s.is_some() { continue; }
            let generation = next_generation(slot);
            *s = Some(Pty {
                grid: TextGrid::new(cols, rows)?,
                owner,
                generation,
                dying: false,
                refs: 1,
            });
            OWNER[slot].store(owner, Ordering::Release);
            GEN[slot].store(generation, Ordering::Release);
            return Some(TtyId((generation << 8) | slot as u32));
        }
        None
    })
}

/// Make a terminal and take a master reference on it
///
/// What `open("/dev/ptmx")` does. Answers with the slot, which names the
/// master's device node; the `TtyId` is read from `GEN[slot]` when wanted, so a
/// descriptor cannot outlive the generation it was made in without `exists`
/// noticing.
///
/// A default shape: a master has no window yet, and its opener sets the real
/// one with `TIOCSWINSZ`
pub fn open_master() -> Option<usize> {
    // The creator, for the permission question `may_use` asks (whose terminal
    // this is), not for the lifetime question, which the descriptor owns
    let id = open_pty(crate::sched::current_pid(), 80, 24)?;
    // Not counted here: the descriptor `open` is about to make is a reference
    // on the master node, and `master_ref` counts those
    Some(id.index())
}

/// Slots whose last master reference has gone and whose hangup has not run; see
/// `master_unref`
static HANGUPS: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// A reference to master `slot`'s node was made: `open("/dev/ptmx")`, a `dup`,
/// a `fork` inheriting it, one passed over a socket.
///
/// Called from `VNode::inc_ref`, the one place all of those pass through. A
/// `fork`ed child inheriting the master and exiting without `exec` must not
/// take the count to zero while its parent holds the master (`openpty` + `fork`
/// + `login_tty`)
pub fn master_ref(slot: usize) {
    if let Some(id) = master_tty(slot) {
        with_grid(id, |g| g.master_opened());
    }
}

/// A reference to master `slot`'s node went (`VNode::dec_ref`). The last one
/// hangs the terminal up, not from here: this runs under the VFS lock, and a
/// hangup signals a process group, entering the scheduler. Queued, and
/// `run_hangups` does it once the lock is released
pub fn master_unref(slot: usize) {
    let Some(id) = master_tty(slot) else { return };
    if with_grid(id, |g| g.master_closed()).unwrap_or(true) {
        HANGUPS.fetch_or(1 << slot, Ordering::AcqRel);
    }
}

/// Hang up every terminal whose last master has gone. Called by `with_vfs`
/// after the VFS lock is released
pub fn run_hangups() {
    let mut bits = HANGUPS.swap(0, Ordering::AcqRel);
    while bits != 0 {
        let slot = bits.trailing_zeros() as usize;
        bits &= bits - 1;
        if let Some(id) = master_tty(slot) {
            // The other end now talks to nobody. SIGHUP, and the slot goes
            hangup(id);
            release(id);
        }
    }
}

/// A terminal `open_master` made and no descriptor ever named: the one path
/// with no reference to drop
pub fn abandon_master(slot: usize) {
    if let Some(id) = master_tty(slot) {
        release(id);
    }
}

/// Which terminal a master node's slot names, if it is still the same one
pub fn master_tty(slot: usize) -> Option<TtyId> {
    if slot == 0 || slot >= MAX_TTY { return None; }
    let g = GEN[slot].load(Ordering::Acquire);
    if g == 0 { return None; }
    Some(TtyId((g << 8) | slot as u32))
}

/// How many master descriptors are open on this terminal
pub fn master_count(id: TtyId) -> u32 {
    with_grid(id, |g| g.master_count()).unwrap_or(0)
}

/// Take what a program on this terminal has written, for its master to read
pub fn master_read(id: TtyId, buf: &mut [u8]) -> usize {
    with_grid(id, |g| g.out_take(buf)).unwrap_or(0)
}

/// How much is waiting for a master, for `poll`, without taking it
pub fn master_pending(id: TtyId) -> usize {
    with_grid(id, |g| g.out_pending()).unwrap_or(0)
}

/// Hang a terminal up: the master has gone
///
/// The window closed, or the program drawing it died. Whoever was working at
/// that terminal gets SIGHUP. Without it a shell on the pty stays in `read` on
/// a terminal nobody will type at; with it, a shell dies with its window and a
/// service can save its work, as SIGHUP is catchable
pub fn hangup(id: TtyId) {
    if id == SYSTEM || !exists(id) { return; }
    let fg = interrupts::without_interrupts(|| {
        let mut s = PTYS[id.index()].lock();
        if let Some(p) = s.as_mut() {
            p.dying = true;
            p.owner = 0;
        }
        OWNER[id.index()].store(0, Ordering::Release);
        slot_of(id).map(|slot| FOREGROUND[slot].get()).unwrap_or(0)
    });
    // And the discipline goes back to a sane terminal: a program that died in
    // raw mode leaves no echo and no line editing, as in `set_foreground`. A
    // hangup is the other way a program stops owning a terminal
    if let Some(slot) = slot_of(id) {
        interrupts::without_interrupts(|| LDISC[slot].lock().reset());
    }
    // After the lock: a signal reaches the scheduler, never entered under a
    // terminal's own lock
    if fg != 0 {
        crate::process::signal::send_to_group(fg, crate::process::signal::SIGHUP);
    }
    crate::sched::wait::wake_key(crate::sched::wait::key::tty_input(id));
    crate::sched::wait::wake_key(crate::sched::wait::key::pty_output(id));
    crate::sched::wait::wake_pollers();
}

/// Drop one reference. The slot is freed, and its generation bumped, only when
/// the last one goes, so a `TtyId` captured before a hangup cannot name the
/// terminal taking its place
pub fn release(id: TtyId) {
    if id == SYSTEM || !exists(id) { return; }
    interrupts::without_interrupts(|| {
        let mut s = PTYS[id.index()].lock();
        let gone = match s.as_mut() {
            Some(p) => { p.refs = p.refs.saturating_sub(1); p.refs == 0 }
            None => false,
        };
        if gone {
            *s = None;
            OWNER[id.index()].store(0, Ordering::Release);
            GEN[id.index()].store(0, Ordering::Release);
        }
    });
}

/// How far up a parent chain a terminal is inherited
///
/// The same bound as `session::of_process`: a cycle in the table would
/// otherwise make the walk hang
const MAX_WALK: usize = 64;

/// May this process use this terminal: join it, or draw it?
///
/// The one rule, asked by the `tty` syscall and by `may_console.bind`.
///
/// The console is everybody's, the machine's own, and the registry filter
/// hiding the privileged global limits who may draw it. A pty is its master's
/// and its master's children's; the useful case is two deep, a terminal forking
/// a shell and the shell forking a program. Nothing else can name it: a `TtyId`
/// is not derivable from anything a stranger holds, which gives the absence of
/// an input-injection request its meaning
pub fn may_use(pid: u64, id: TtyId) -> bool {
    if id == SYSTEM { return true; }
    let Some(owner) = master_owner(id) else { return false };
    if owner == 0 { return false; }
    // A process administering the machine may join any terminal on it.
    //
    // A holder of CAP_SYS_ADMIN can already open `/dev/blk0`, read anyone's
    // files and signal anyone's processes. `mikudod` runs a command for
    // someone, on the terminal the person asked from, and is not descended from
    // it. The rule still forbids an ordinary process naming another's terminal
    if crate::sched::cred_of(pid).has(crate::process::cred::CAP_SYS_ADMIN) {
        return true;
    }
    let mut cur = pid;
    for _ in 0..MAX_WALK {
        if cur == 0 { return false; }
        if cur == owner { return true; }
        if cur == 1 { return false; }
        cur = crate::sched::get_ppid(cur);
    }
    false
}

/// Every terminal that exists, console first
pub fn list() -> alloc::vec::Vec<TtyId> {
    let mut out = alloc::vec![SYSTEM];
    for slot in 1..MAX_TTY {
        let g = GEN[slot].load(Ordering::Acquire);
        if g != 0 {
            out.push(TtyId((g << 8) | slot as u32));
        }
    }
    out
}

/// Do something with a terminal's text, whichever kind
///
/// The console's grid is in `io::console` behind the writer every `print!` goes
/// through; a pty's is here. One function, so nothing above has to know which
/// it holds
pub fn with_grid<R>(id: TtyId, f: impl FnOnce(&mut TextGrid) -> R) -> Option<R> {
    if id == SYSTEM {
        return super::console::with_grid(f);
    }
    if !exists(id) { return None; }
    interrupts::without_interrupts(|| {
        let mut s = PTYS[id.index()].lock();
        s.as_mut().map(|p| f(&mut p.grid))
    })
}
