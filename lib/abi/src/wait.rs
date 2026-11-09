//! `<sys/wait.h>`

#![allow(unused_imports)]

use super::prelude::*;

pub const WNOHANG: c_int = 1;
pub const WUNTRACED: c_int = 2;
// Below: for Rust only, not in the C headers
/// `waitpid` options. `WNOHANG` and `WUNTRACED` are above; these three belong
/// to `waitid`, which MikuOS lacks. `wait4`(44) is the only wait call.
/// `rustix::process` enumerates them
pub const WCONTINUED: c_int = 8;
pub const WEXITED: c_int = 4;
pub const WSTOPPED: c_int = 2;
pub const WNOWAIT: c_int = 0x0100_0000;
/// Taking a status word apart
///
/// POSIX's encoding, written by the kernel in
/// `kernel/syscall/adapters/process.rs`:
///
/// ```text
///   exited     code << 8
///   signalled  sig                (the low seven bits)
///   stopped    0x7f | (sig << 8)
/// ```
///
/// `const fn` here, since in C they are macros over one integer.
pub const fn WIFEXITED(status: c_int) -> bool {
    (status & 0x7f) == 0
}
pub const fn WEXITSTATUS(status: c_int) -> c_int {
    (status >> 8) & 0xff
}
pub const fn WIFSIGNALED(status: c_int) -> bool {
    ((status & 0x7f) + 1) as i8 >= 2
}
pub const fn WTERMSIG(status: c_int) -> c_int {
    status & 0x7f
}
pub const fn WIFSTOPPED(status: c_int) -> bool {
    (status & 0xff) == 0x7f
}
pub const fn WSTOPSIG(status: c_int) -> c_int {
    (status >> 8) & 0xff
}
/// Always false. `SIGCONT` resumes a stopped task, and `wait4`(44) reports
/// nothing when it does
pub const fn WIFCONTINUED(_status: c_int) -> bool {
    false
}
pub const fn WCOREDUMP(status: c_int) -> bool {
    (status & 0x80) != 0
}
/// `waitid`'s `idtype`. `waitid` answers `ENOSYS`; `wait4`(44) is the only wait
/// call
pub const P_ALL: c_int = 0;
pub const P_PID: c_int = 1;
pub const P_PGID: c_int = 2;
