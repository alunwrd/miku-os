// Terminals, from ring 3
//
// A terminal is a character grid plus a line discipline: what a program prints
// goes into the grid (`print!` routes by `Process::ctty`), and whoever draws
// the terminal reads that grid from shared memory over `may_console`.
//
// A pty is made with `open("/dev/ptmx")`. This call puts a process on a
// terminal and hands its keys around. As `/bin/mayterm` does:
//
//      let master = open("/dev/ptmx");      // this process is now the master
//      let tty = ioctl(master, TIOCGPTN);
//      may_console.bind(tty);               // and draws it
//      if fork() == 0 {
//          tty_attach(tty);                 // the child's ctty, inherited
//          exec("/bin/msh");                //   through exec by every child
//      }
//
// and what the shell then does with it, the other half of a terminal and the
// reason `setpgid` exists:
//
//      let child = fork();                  // both sides call setpgid, so
//      setpgid(child, child);                //   neither ordering leaves a
//      tty(TTY_SETFG, child);                //   window in the wrong group
//      waitpid(child);                      // Ctrl-C now reaches the child
//      tty(TTY_SETFG, 0);                   // and the keys come back
//
// `tty_attach` is the security boundary: only the terminal's master, or a
// process descended from it, may join. A terminal is not reachable by name or
// number from outside the family that made it, so there is nothing to inject
// into without `TIOCSTI`.

use crate::io::tty::{self, TtyId};
use crate::syscall::errno::{err, EBADF, EINVAL, EMFILE, EPERM};
use crate::syscall::usercopy::current_pid;

// 0 is reserved: a pty comes from `open("/dev/ptmx")`, and an old binary
// calling it gets EINVAL
/// Join one: the caller's controlling terminal becomes `arg`
pub use miku_abi::kproc::TTY_ATTACH;
// 2 is reserved: closing the master descriptor is the hangup, whether the
// holder closed it or was killed
/// Which terminal the caller is on. 0 is the console
pub use miku_abi::kproc::TTY_SELF;
/// Hands the terminal's keys to a job: `tcsetpgrp`. `arg` is a process group
/// id; 0 means the caller's own group, how a shell takes them back
pub use miku_abi::kproc::TTY_SETFG;
/// Which job has them: `tcgetpgrp`. A shell asks to find out whether it is in
/// the foreground before it prompts
pub use miku_abi::kproc::TTY_GETFG;
/// Blanks the caller's terminal and puts the pen at the top left
///
/// A request and not an escape sequence: a terminal's text is a cell grid the
/// kernel owns and `may_console` delivers, and a program cannot clear somebody
/// else's screen by printing at it
pub use miku_abi::kproc::TTY_CLEAR;
/// Which terminal another process is on. `arg` is its pid
///
/// For a daemon running a command for somebody else, to put it on their
/// terminal: a pid from `peercred` is the kernel's word, and a terminal id sent
/// over a socket is the caller's
pub use miku_abi::kproc::TTY_OF_PID;

// 99  tty(op, arg) -> id / 0 / -errno
/// Makes terminal `id` the controlling terminal of `pid`, and `pid` the leader
/// of the job in its foreground: `tty(ATTACH)` and `ioctl(TIOCSCTTY)` alike, as
/// `login_tty` asks for on the slave it has opened
pub fn attach(pid: u64, id: TtyId) -> u64 {
    if !tty::exists(id) {
        return err(EBADF);
    }
    // The console is everybody's: a process may go back to it. A pty is its
    // master's and its master's children's: `io::tty::may_use` decides, and
    // `may_console.bind` asks the same function
    if !tty::may_use(pid, id) {
        return err(EPERM);
    }
    crate::sched::set_ctty(pid, id.raw());
    // Joining a terminal starts new work at it, so the caller leads a job of
    // its own from here.
    //
    // The part of POSIX's `setsid` that means something here. A shell keeping
    // the group it was forked into (the terminal program's, at a different
    // terminal) could hand its keys to a child and never take them back:
    // `tcsetpgrp` would refuse the group as not of this terminal
    crate::sched::set_pgid(pid, pid);
    // And the keys are this process's job until it says otherwise.
    //
    // A terminal with no foreground delivers no signals: Ctrl-C would become a
    // SIGINT with nobody to send it to. `tcsetpgrp` moves the keys on when it
    // starts a job
    crate::io::tty::set_foreground(id, pid);
    0
}

pub fn sys_tty(op: u64, arg: u64) -> u64 {
    let pid = current_pid();

    match op {
        TTY_ATTACH => attach(pid, TtyId(arg as u32)),

        TTY_SELF => crate::sched::current_tty() as u64,

        // `tcsetpgrp`
        //
        // A shell starting a program hands the keys over, or Ctrl-C interrupts
        // the shell sitting in `waitpid`. It moves a process group, so a
        // pipeline is interrupted as one piece of work.
        //
        // Who may: a process on that terminal, handing it to a group on the
        // same terminal. Without the first anything could take any terminal's
        // keys; without the second a shell could point a terminal at a job that
        // will never read it
        TTY_SETFG => {
            let raw = crate::sched::current_tty();
            let id = if raw == 0 { tty::SYSTEM } else { TtyId(raw) };
            if !tty::exists(id) {
                return err(EBADF);
            }
            let target = if arg == 0 { crate::sched::pgid_of(pid) } else { arg };
            if target == 0 {
                return err(EINVAL);
            }
            // A group, named by its leader, at this terminal: a pid not a group
            // leader is a group id meaning nothing, and a group at another
            // terminal will never read what is typed here
            if crate::sched::pgid_of(target) != target {
                return err(EPERM);
            }
            let their_tty = crate::sched::ctty_of(target);
            let theirs = if their_tty == 0 { tty::SYSTEM } else { TtyId(their_tty) };
            if theirs != id {
                return err(EPERM);
            }
            tty::set_foreground(id, target);
            0
        }

        TTY_CLEAR => {
            let raw = crate::sched::current_tty();
            let id = if raw == 0 { tty::SYSTEM } else { TtyId(raw) };
            if !tty::exists(id) {
                return err(EBADF);
            }
            // The console has pixels behind its grid and a whole flush path;
            // a pty is a grid and nothing else
            if id == tty::SYSTEM {
                crate::io::console::clear_screen();
            } else {
                tty::with_grid(id, |g| g.clear());
            }
            0
        }

        TTY_OF_PID => {
            let target = arg;
            if target == 0 || !crate::sched::process_exists(target) {
                return err(EBADF);
            }
            // Yourself, a process you started, or the authority to administer
            // the machine. The middle case is a shell asking about its
            // children; the last is `mikudod`
            let mine = target == pid
                || crate::sched::get_ppid(target) == pid
                || crate::sched::cred_of(pid).has(crate::process::cred::CAP_SYS_ADMIN);
            if !mine {
                return err(EPERM);
            }
            crate::sched::ctty_of(target) as u64
        }

        TTY_GETFG => {
            let raw = crate::sched::current_tty();
            let id = if raw == 0 { tty::SYSTEM } else { TtyId(raw) };
            if !tty::exists(id) {
                return err(EBADF);
            }
            tty::foreground(id)
        }

        _ => err(EINVAL),
    }
}
