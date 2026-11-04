// What a program reads from the keyboard
//
// The line discipline is `io::tty::ldisc`, one per terminal. Here are the names
// the rest of the kernel calls, each answering for the caller's own terminal.
// Each function names a terminal and passes the question on. See
// `dev/terminal-architecture.md`.

use crate::io::tty::{self, TtyId};

pub use crate::io::tty::ldisc::Termios;

/// The terminal the running task is at
///
/// `Process::ctty` is inherited on fork and preserved across exec, and `print!`
/// routes output by it. This is the input half
pub fn mine() -> TtyId {
    ctty().unwrap_or(tty::SYSTEM)
}

/// The caller's controlling terminal, or nothing
///
/// `mine()` cannot tell the console from no terminal, answering both with the
/// console. `/dev/tty` needs the difference: a process with no controlling
/// terminal cannot open it
pub fn ctty() -> Option<TtyId> {
    let raw = crate::sched::current_tty();
    if raw == 0 { None } else { Some(TtyId(raw)) }
}

pub fn termios() -> Termios {
    tty::termios(mine())
}

/// How wide and tall the caller's terminal is, in characters: what `TIOCGWINSZ`
/// answers
pub fn size() -> (usize, usize) {
    tty::size(mine())
}

pub fn set_termios(t: Termios) {
    tty::set_termios(mine(), t);
}

/// Hand this terminal to a job
pub fn set_foreground(pgid: u64) {
    tty::set_foreground(mine(), pgid);
}

pub fn clear_foreground() {
    tty::clear_foreground(mine());
}

/// The job holding this terminal's keys, 0 for nobody
pub fn foreground_pid() -> u64 {
    tty::foreground(mine())
}

pub fn is_foreground_active() -> bool {
    foreground_pid() != 0
}

/// One character from the keyboard
///
/// To the terminal the keyboard is pointed at, not necessarily the caller's:
/// this is reached from the shell's keyboard thread, deciding where a keystroke
/// goes
pub fn feed_char(c: char) {
    tty::feed_char(tty::active(), c);
}

/// Would `read` return without sleeping?
///
/// Mirrors the top of `read`'s loop line for line: a poll reporting the
/// keyboard ready is followed by a read, and if the two disagree the read
/// blocks.
///
/// The cases where `read` refuses are included: it returns end-of-file without
/// blocking when this terminal's keys are not the caller's, and a call
/// returning at once is ready in poll's sense
pub fn readable() -> bool {
    readable_on(mine())
}

/// The same question about a named terminal: what `poll` asks of a descriptor
/// open on `/dev/tty` or `/dev/console`
pub fn readable_on(id: TtyId) -> bool {
    if !may_read(id) { return true; }
    tty::readable(id)
}

/// Are this terminal's keys the caller's to read?
///
/// The foreground is a job, and only the job holding a terminal may read it. A
/// background reader must not succeed: two readers on one terminal take
/// alternate halves of every line typed. POSIX stops such a reader with SIGTTIN
/// (`sched::stop_current`, `tty::background_read`), where a job waits until
/// brought forward
pub fn may_read(id: TtyId) -> bool {
    let fg = tty::foreground(id);
    if fg == 0 { return false; }
    fg == crate::sched::pgid_of(crate::sched::current_pid())
}

pub fn read(buf_ptr: u64, len: u64) -> u64 {
    read_from(mine(), buf_ptr, len)
}

/// Read a named terminal
///
/// `read` is this with the caller's own filled in. A descriptor open on
/// `/dev/tty` resolves to the same terminal by another route, and one on
/// `/dev/console` names tty0 whoever asks. Both wait here, outside the VFS
/// lock, not in `devfs::dev_read`
pub fn read_from(id: TtyId, buf_ptr: u64, len: u64) -> u64 {
    if buf_ptr == 0 || len == 0 || buf_ptr < 0x1000 || buf_ptr > 0x0000_7FFF_FFFF_FFFF {
        return u64::MAX;
    }

    let max_read = (len as usize).min(tty::ldisc::READ_BUF_SIZE);
    // Staged, not written a byte at a time under the discipline's lock: the
    // user pointer is another address space's and the copy must not happen with
    // interrupts off
    let mut staging = [0u8; tty::ldisc::READ_BUF_SIZE];

    // VMIN and VTIME, which apply only in raw mode; canonical mode delivers a
    // line.
    //
    //   VMIN 0, VTIME 0: take whatever is there, even nothing. A poll.
    //   VMIN n, VTIME 0: wait for n characters, however long that takes.
    //   VTIME t: give up after t tenths of a second and return what there is,
    //            which is how a program tells a lone ESC from the start of an arrow key
    let t = tty::termios(id);
    let (vmin, vtime) = if t.icanon { (1u8, 0u8) } else { (t.vmin, t.vtime) };
    // 250 Hz, so a tenth of a second is 25 ticks
    let deadline = if vtime == 0 { None } else {
        Some(crate::fs::procfs::uptime_ticks() + vtime as u64 * 25)
    };
    use crate::sched::wait::{join, key, keyed, POLL};
    let queues = [keyed(key::tty_input(id)), &POLL];
    let mut joined = None;

    loop {
        // Asked again on every pass: a shell moves the terminal's keys to a job
        // it has just started, and this read may be that job's
        if !may_read(id) {
            // SIGTTIN, as POSIX says: the caller stops until made the
            // foreground. If it survives that (ignored or caught), the read
            // ends in end of file
            if tty::background_read(id) {
                continue;
            }
            return 0;
        }

        match tty::read_input(id, &mut staging, max_read) {
            // End of input
            None => return 0,
            // Nothing there. With VMIN 0 that is the answer: a program that
            // asked not to block is told there was nothing, as `poll` on a
            // terminal relies on
            Some(0) => {
                if vmin == 0 && vtime == 0 {
                    return 0;
                }
            }
            Some(n) => {
                // Fewer than asked for, with time left to wait for the rest.
                // Put back, not a short read: VMIN means not before this many
                if n < vmin as usize && deadline.is_some_and(|d| crate::fs::procfs::uptime_ticks() < d) {
                    tty::unread(id, &staging[..n]);
                } else {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            staging.as_ptr(), buf_ptr as *mut u8, n);
                    }
                    return n as u64;
                }
            }
        }

        // The timer ran out. Whatever is there is the answer, including none
        if deadline.is_some_and(|d| crate::fs::procfs::uptime_ticks() >= d) {
            return 0;
        }

        // Off the run queue until something is typed. `tty::feed_char` wakes
        // this queue after the discipline has taken the character and the lock
        // is gone, the rule in `sched::wait`.
        //
        // The backstop turns a missed wakeup into a late read. Joined before
        // the look that decides it, so a key typed in between is not left to
        // the backstop (`wait::join`)
        match joined.take() {
            None => joined = Some(join(&queues)),
            Some(j) => { j.sleep(crate::sched::tick_deadline(READ_BACKSTOP)); }
        }
    }
}

/// Ticks a blocked read sleeps for when nothing wakes it (4 ms each)
const READ_BACKSTOP: u64 = 25;
