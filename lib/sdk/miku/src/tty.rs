//! Terminals: a character grid plus a line discipline. The kernel routes output
//! by controlling terminal, and the drawing program reads the grid over
//! `may_console`.
//! 
//! ```ignore
//! let tty = tty::open(80, 24)?;     // this process is the master
//! // may_console.bind(tty)         // and draws it
//! if proc::fork()? == 0 {
//! tty::attach(tty)?;            // the child's terminal, inherited by
//! proc::exec("/bin/msh", &[])?; //   everything it starts in turn
//! }
//! ```
//! 
//! Only the master and its descendants may join a terminal

use miku_abi::kproc::TTY_ATTACH as ATTACH;
use miku_abi::kproc::TTY_SELF as SELF;
use miku_abi::kproc::TTY_SETFG as SETFG;
use miku_abi::kproc::TTY_GETFG as GETFG;
use miku_abi::kproc::TTY_CLEAR as CLEAR;
use miku_abi::kproc::TTY_OF_PID as OF_PID;

/// The console
pub use miku_abi::kproc::TTY_SYSTEM as SYSTEM;

/// Creates a terminal with `open("/dev/ptmx")`. Returns
/// `(master_fd, tty_id)`. When the last copy of the descriptor closes, the
/// terminal hangs up and its reader gets SIGHUP. The id names it to
/// `may_console` and `attach`. Opened close-on-exec, so a child cannot hold
/// it open
pub fn open_pty() -> Result<(i64, u32), i64> {
    const O_RDWR: u32 = miku_abi::fcntl::O_RDWR as u32;
    const O_CLOEXEC: u32 = miku_abi::fcntl::O_CLOEXEC as u32;
    use miku_abi::ioctl::TIOCGPTN;
    let path = b"/dev/ptmx\0";
    let fd = unsafe { super::miku_open(path.as_ptr(), 9, O_RDWR | O_CLOEXEC, 0) };
    if fd < 0 { return Err(fd); }
    let mut id: u32 = 0;
    let r = unsafe { super::miku_ioctl(fd as u64, TIOCGPTN, &mut id as *mut u32 as u64) };
    if r < 0 {
        super::close(fd);
        return Err(r);
    }
    Ok((fd, id))
}

/// Joins a terminal; inherited across fork and kept across exec
pub fn attach(id: u32) -> Result<(), i64> {
    let r = unsafe { super::miku_tty(ATTACH, id as u64) };
    if r < 0 { Err(r) } else { Ok(()) }
}

// A terminal hangs up when its master descriptor closes

/// Which terminal this process is on. `0` is no terminal at all
pub fn current() -> u32 {
    let r = unsafe { super::miku_tty(SELF, 0) };
    if r < 0 { 0 } else { r as u32 }
}

/// Gives this terminal's keys to a process group, or `0` to take them back
/// (`tcsetpgrp`). Both must be on this terminal; use `proc::setpgid` first
pub fn foreground(pgid: u64) -> Result<(), i64> {
    let r = unsafe { super::miku_tty(SETFG, pgid) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Blanks this terminal and homes the pen, atomically, unlike
/// `\x1b[2J\x1b[H`
pub fn clear() -> Result<(), i64> {
    let r = unsafe { super::miku_tty(CLEAR, 0) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Raw id of another process's terminal; 0 for none. Yours, a child's, or
/// with admin rights anyone's. `mikudod` uses it with a `peercred` pid
pub fn of_pid(pid: u64) -> Result<u32, i64> {
    let r = unsafe { super::miku_tty(OF_PID, pid) };
    if r < 0 { Err(r) } else { Ok(r as u32) }
}

/// Process group holding this terminal's keys (`tcgetpgrp`)
pub fn foreground_of() -> Result<u64, i64> {
    let r = unsafe { super::miku_tty(GETFG, 0) };
    if r < 0 { Err(r) } else { Ok(r as u64) }
}

// Terminal modes

use miku_abi::ioctl::TCGETS;
use miku_abi::ioctl::TCSETS;

use miku_abi::ktermios::MikuTermios as Wire;

/// Control character count and slots, mirroring `io::tty::ldisc`. Zero
/// disables a character
pub use miku_abi::ktermios::NCCS;
pub use miku_abi::ktermios::VINTR;
pub use miku_abi::ktermios::VQUIT;
pub use miku_abi::ktermios::VERASE;
pub use miku_abi::ktermios::VKILL;
pub use miku_abi::ktermios::VEOF;
pub use miku_abi::ktermios::VWERASE;
pub use miku_abi::ktermios::VSUSP;

/// Line discipline settings: `icanon` line or keystroke reads, `echo`
/// echoes input, `isig` Ctrl-C as a signal or byte
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Termios {
    pub icanon: bool,
    pub echo: bool,
    pub isig: bool,
    /// Control characters, indexed by `VINTR` and neighbors
    pub c_cc: [u8; NCCS],
    /// Raw-mode read minimum and timeout in tenths of a second; ignored in
    /// canonical mode
    pub vmin: u8,
    pub vtime: u8,
    /// POSIX `TOSTOP`: stops a background job writing to this terminal. Off
    /// by default
    pub tostop: bool,
}

/// Terminal settings, or `None` when stdin is not a terminal
pub fn termios() -> Option<Termios> {
    let mut w = Wire::default();
    let r = unsafe { super::miku_ioctl(0, TCGETS, &mut w as *mut _ as u64) };
    if r < 0 { return None; }
    Some(Termios {
        icanon: w.icanon != 0,
        echo: w.echo != 0,
        isig: w.isig != 0,
        c_cc: w.c_cc,
        vmin: w.vmin,
        vtime: w.vtime,
        tostop: w.tostop != 0,
    })
}

pub fn set_termios(t: Termios) -> Result<(), i64> {
    let w = Wire {
        icanon: t.icanon as u8,
        echo: t.echo as u8,
        isig: t.isig as u8,
        vmin: t.vmin,
        vtime: t.vtime,
        tostop: t.tostop as u8,
        c_cc: t.c_cc,
        _pad: [0; 1],
    };
    let r = unsafe { super::miku_ioctl(0, TCSETS, &w as *const _ as u64) };
    if r < 0 { Err(r) } else { Ok(()) }
}

/// Holds a changed terminal and restores it in `Drop`. Not run on `exit()`
/// or panic; `tty::set_foreground` and `tty::hangup` reset the discipline
/// for the next program
pub struct Saved(Option<Termios>);

impl Saved {
    /// Applies `f` to a copy of the settings, keeping the original. Does
    /// nothing when stdin is not a terminal
    pub fn change(f: impl FnOnce(&mut Termios)) -> Self {
        let Some(was) = termios() else { return Saved(None) };
        let mut want = was;
        f(&mut want);
        if want != was {
            let _ = set_termios(want);
        }
        Saved(Some(was))
    }

    /// True when there was a terminal to change
    pub fn is_tty(&self) -> bool { self.0.is_some() }

    /// Restores now, for a caller about to `exit`
    pub fn restore(&mut self) {
        if let Some(was) = self.0.take() {
            let _ = set_termios(was);
        }
    }
}

impl Drop for Saved {
    fn drop(&mut self) { self.restore(); }
}
