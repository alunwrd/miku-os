//! `<signal.h>`

#![allow(unused_imports)]

use super::prelude::*;

// Signals, and the status word `wait4`(44) answers with
/// The signals MikuOS delivers, with Linux's numbers
///
/// Every number `kernel/process/signal/mod.rs` does not name is fatal by
/// default. `SIGSEGV`, `SIGILL` and `SIGFPE` are raised by the CPU and
/// delivered to a handler with a `ucontext_t` it may rewrite
/// (`dev/signal-architecture.md`); uncaught, the process dies. Nothing raises
/// `SIGBUS` or `SIGTRAP`; crates match on them.
pub const SIGHUP: c_int = 1;
pub const SIGINT: c_int = 2;
pub const SIGQUIT: c_int = 3;
pub const SIGILL: c_int = 4;
pub const SIGTRAP: c_int = 5;
pub const SIGABRT: c_int = 6;
pub const SIGIOT: c_int = SIGABRT;
pub const SIGBUS: c_int = 7;
pub const SIGFPE: c_int = 8;
pub const SIGKILL: c_int = 9;
pub const SIGUSR1: c_int = 10;
pub const SIGSEGV: c_int = 11;
pub const SIGUSR2: c_int = 12;
pub const SIGPIPE: c_int = 13;
pub const SIGALRM: c_int = 14;
pub const SIGTERM: c_int = 15;
pub const SIGSTKFLT: c_int = 16;
pub const SIGCHLD: c_int = 17;
pub const SIGCONT: c_int = 18;
pub const SIGSTOP: c_int = 19;
pub const SIGTSTP: c_int = 20;
pub const SIGTTIN: c_int = 21;
pub const SIGTTOU: c_int = 22;
pub const SIGURG: c_int = 23;
pub const SIGXCPU: c_int = 24;
pub const SIGXFSZ: c_int = 25;
pub const SIGVTALRM: c_int = 26;
pub const SIGPROF: c_int = 27;
pub const SIGWINCH: c_int = 28;
pub const SIGIO: c_int = 29;
pub const SIGPOLL: c_int = SIGIO;
pub const SIGPWR: c_int = 30;
pub const SIGSYS: c_int = 31;
pub const SIGSTKSZ: size_t = 8192;
/// `sigprocmask`'s `how`. The mask is process-wide, so `pthread_sigmask` is the
/// same call
pub const SIG_BLOCK: c_int = 0;
pub const SIG_UNBLOCK: c_int = 1;
pub const SIG_SETMASK: c_int = 2;
pub const SA_NOCLDSTOP: c_int = 0x0000_0001;
/// `sigaction` refuses it with `EINVAL`: the kernel keeps every child until it
/// is waited for
pub const SA_NOCLDWAIT: c_int = 0x0000_0002;
pub const SA_SIGINFO: c_int = 0x0000_0004;
pub const SA_ONSTACK: c_int = 0x0800_0000;
pub const SA_RESTART: c_int = 0x1000_0000;
pub const SA_NODEFER: c_int = 0x4000_0000;
// Below: for Rust only, not in the C headers
/// `sigset_t`: 1024 bits, as on Linux. MikuOS numbers 31 signals, and
/// `change_mask` in `lib/runtime/libmiku/libc/` reads the low ones
pub type sigset_t = [u64; 16];
pub const NSIG: c_int = 32;
/// `siginfo_t`'s `si_code` for a child that changed state. Named, not delivered
pub const CLD_EXITED: c_int = 1;
pub const CLD_KILLED: c_int = 2;
pub const CLD_DUMPED: c_int = 3;
pub const CLD_TRAPPED: c_int = 4;
pub const CLD_STOPPED: c_int = 5;
pub const CLD_CONTINUED: c_int = 6;
impl siginfo_t {
    /// The child's exit status, as `waitid` would fill it. Always zero; see
    /// `siginfo_t`
    pub unsafe fn si_status(&self) -> c_int {
        self._pad[0]
    }

    pub unsafe fn si_pid(&self) -> pid_t {
        self._pad[1] as pid_t
    }

    /// The faulting address, for a signal the CPU raised: CR2 for `SIGSEGV`
    /// from a page fault, the instruction for `SIGILL` and `SIGFPE`, zero for a
    /// `SIGSEGV` from a general protection fault. At +16, as on Linux
    pub unsafe fn si_addr(&self) -> *mut c_void {
        (self as *const Self as *const u8).add(16).cast::<*mut c_void>().read()
    }
}
/// `ss_flags`
pub const SS_ONSTACK: c_int = 1;
pub const SS_DISABLE: c_int = 2;
/// The kernel refuses an alternate stack smaller than this
pub const MINSIGSTKSZ: size_t = 2048;
/// `si_code` for the signals the CPU raises
pub const ILL_ILLOPN: c_int = 2;
pub const FPE_INTDIV: c_int = 1;
pub const SEGV_MAPERR: c_int = 1;
pub const SEGV_ACCERR: c_int = 2;
/// `si_code` of a `SIGSEGV` from a general protection fault, which has no
/// address to report
pub const SI_KERNEL: c_int = 0x80;
/// `si_code` of a signal sent by `kill`
pub const SI_USER: c_int = 0;
pub const SA_RESETHAND: c_int = 0x8000_0000_u32 as c_int;
c_structs! {
    /// `sigaction`, in Linux's shape. A signal has a disposition in the kernel
    /// (`sigdisp`(100)) and a handler table in `sys_miku.so`, and
    /// `libc_miku.so`'s `sigaction` translates onto both. `SA_SIGINFO`,
    /// `SA_NODEFER`, `SA_RESETHAND` and `SA_ONSTACK` work; `SA_RESTART` and
    /// `SA_NOCLDSTOP` are accepted and have no effect (see
    /// `lib/runtime/libmiku/libc/`). The mask covers the 31 signals MikuOS numbers.
    pub struct sigaction {
        pub sa_sigaction: usize,
        pub sa_mask: sigset_t,
        pub sa_flags: c_int,
        pub sa_restorer: Option<extern "C" fn()>,
    }

    /// `siginfo_t`, in the usual shape
    ///
    /// For a signal the CPU raised (`SIGSEGV`, `SIGILL`, `SIGFPE`) the kernel
    /// fills `si_code` (`SEGV_MAPERR`, `ILL_ILLOPN`, ...) and the address at
    /// +16, read by `si_addr()`. For a sent signal `si_signo` is set and the
    /// rest is zero: it reaches ring 3 as a number, with no sender. `waitid`
    /// answers `ENOSYS` without touching the structure, so `rustix`'s
    /// `si_signo == 0` check reads the zero it supplied.
    pub struct siginfo_t {
        pub si_signo: c_int,
        pub si_errno: c_int,
        pub si_code: c_int,
        pub _pad: [c_int; 29],
        _align: [u64; 0],
    }

    /// A thread's alternate signal stack, for `sigaltstack`. Linux's shape.
    pub struct stack_t {
        pub ss_sp: *mut c_void,
        pub ss_flags: c_int,
        pub ss_size: size_t,
    }
}
