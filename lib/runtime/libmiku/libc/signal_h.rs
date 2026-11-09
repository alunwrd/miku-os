//! `<signal.h>`

use crate::libc::*;

pub const SIGHUP:  u32 = signal::SIG_HUP;

pub const SIGINT:  u32 = signal::SIG_INT;

pub const SIGQUIT: u32 = signal::SIG_QUIT;

pub const SIGILL:  u32 = signal::SIG_ILL;

pub const SIGTRAP: u32 = signal::SIG_TRAP;

pub const SIGABRT: u32 = signal::SIG_ABRT;

pub const SIGFPE:  u32 = signal::SIG_FPE;

pub const SIGKILL: u32 = signal::SIG_KILL;

pub const SIGSEGV: u32 = signal::SIG_SEGV;

pub const SIGPIPE: u32 = signal::SIG_PIPE;

pub const SIGALRM: u32 = signal::SIG_ALRM;

pub const SIGTERM: u32 = signal::SIG_TERM;

pub const SIGCHLD: u32 = signal::SIG_CHLD;

pub const SIGCONT: u32 = signal::SIG_CONT;

pub const SIGSTOP: u32 = signal::SIG_STOP;

pub const SIGUSR1: u32 = signal::SIG_USR1;

pub const SIGUSR2: u32 = signal::SIG_USR2;

pub use miku_abi::libtypes::sighandler_t;

pub const SIG_DFL: Option<sighandler_t> = None;

#[no_mangle]
#[inline(never)]
pub extern "C" fn signal_register(
    sig: i32,
    handler: Option<sighandler_t>,
) -> Option<sighandler_t> {
    signal::miku_signal(sig as u32, handler)
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn raise(sig: i32) -> i32 {
    signal::miku_signal_raise(sig as u32)
}

/// `struct sigaction` as `libc` declares it for MikuOS, Linux's shape: handler,
/// a 1024-bit mask, flags, an unused restorer
pub use miku_abi::signal::sigaction as c_sigaction;

// Linux's `sa_flags` bits, as `libc` passes them
pub(crate) use miku_abi::signal::SA_NOCLDSTOP as C_SA_NOCLDSTOP;

pub(crate) use miku_abi::signal::SA_SIGINFO as C_SA_SIGINFO;

pub(crate) use miku_abi::signal::SA_ONSTACK as C_SA_ONSTACK;

pub(crate) use miku_abi::signal::SA_RESTART as C_SA_RESTART;

pub(crate) use miku_abi::signal::SA_NODEFER as C_SA_NODEFER;

pub(crate) use miku_abi::signal::SA_RESETHAND as C_SA_RESETHAND;

pub(crate) const C_SIG_DFL: usize = 0;

pub(crate) const C_SIG_IGN: usize = 1;

// What `sys_miku`'s table acts on. `SA_RESTART` and `SA_NOCLDSTOP` are accepted
// and have no effect
const HONOURED: u32 = signal::SA_SIGINFO | signal::SA_NODEFER | signal::SA_RESETHAND | signal::SA_ONSTACK;

/// POSIX `sigaction`, translated onto the handler table in `sys_miku.so` and
/// the kernel's disposition (`sigdisp`(100)).
///
/// What does not translate:
/// * `SA_RESTART` is accepted and not honored. The only call a signal
///   interrupts is a blocked
///   `wait4`, answering `EINTR`; every caller of it in `std` retries.
/// * `SA_ONSTACK` runs the handler on the thread's `sigaltstack`, if it has one
///   and is not
///   already on it. The kernel places the frame, so this travels with the disposition.
/// * `SA_NOCLDSTOP` is accepted; `SIGCHLD` is not sent for a stop.
/// * The mask covers signals 1-31, all MikuOS numbers.
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaction(sig: i32, act: *const c_sigaction, oldact: *mut c_sigaction) -> i32 {
    if sig <= 0 || sig >= 32 { return fail(EINVAL); }
    let s = sig as u32;
    if !act.is_null() && (s == signal::SIG_KILL || s == signal::SIG_STOP) {
        return fail(EINVAL);
    }

    // Current state before any change: the disposition and the handler entry.
    // `sigdisp` with the query value changes nothing
    let disp = proc::miku_sigdisp(s as u64, miku_abi::ksignal::SIG_DISP_QUERY);
    if disp < 0 { return fail(-disp as i32); }
    let mut cur: signal::MikuSigaction = unsafe { core::mem::zeroed() };
    signal::miku_sigaction(s, core::ptr::null(), &mut cur);

    if !act.is_null() {
        let a = unsafe { &*act };
        let known = C_SA_NOCLDSTOP | C_SA_SIGINFO | C_SA_ONSTACK | C_SA_RESTART
            | C_SA_NODEFER | C_SA_RESETHAND;
        if a.sa_flags & !known != 0 { return fail(EINVAL); }
        let r = match a.sa_sigaction {
            C_SIG_IGN => signal::miku_signal_ignore(s),
            C_SIG_DFL => {
                let none: signal::MikuSigaction = unsafe { core::mem::zeroed() };
                signal::miku_sigaction(s, &none, core::ptr::null_mut())
            }
            h => {
                let flags = a.sa_flags as u32 & HONOURED;
                // C's bit n-1 is signal n; the table's bit n is signal n
                let mask = ((a.sa_mask[0] << 1) & 0xffff_fffe) as u32;
                let mut m: signal::MikuSigaction = unsafe { core::mem::zeroed() };
                m.handler = Some(unsafe { core::mem::transmute(h) });
                m.flags = flags;
                m.mask = mask;
                signal::miku_sigaction(s, &m, core::ptr::null_mut())
            }
        };
        if r < 0 { return fail(EINVAL); }
    }

    if !oldact.is_null() {
        let mut o = c_sigaction { sa_sigaction: C_SIG_DFL, sa_mask: [0; 16], sa_flags: 0, sa_restorer: None };
        if disp as u64 & !miku_abi::ksignal::SIG_DISP_ONSTACK == miku_abi::ksignal::SIG_DISP_IGNORE {
            o.sa_sigaction = C_SIG_IGN;
        } else if let Some(h) = cur.handler {
            o.sa_sigaction = h as usize;
            o.sa_flags |= (cur.flags & HONOURED) as i32;
            o.sa_mask[0] = (cur.mask >> 1) as u64;
        }
        unsafe { oldact.write(o) };
    }
    0
}

/// ISO C's `signal` with glibc's BSD meaning: the handler stays installed after
/// running, and the signal is held off while it runs. Returns the previous
/// handler or `SIG_ERR` (-1)
#[no_mangle]
#[inline(never)]
pub extern "C" fn signal(sig: i32, handler: usize) -> usize {
    let act = c_sigaction { sa_sigaction: handler, sa_mask: [0; 16], sa_flags: C_SA_RESTART, sa_restorer: None };
    let mut old = c_sigaction { sa_sigaction: 0, sa_mask: [0; 16], sa_flags: 0, sa_restorer: None };
    if sigaction(sig, &act, &mut old) < 0 {
        return usize::MAX;
    }
    old.sa_sigaction
}

/// `stack_t`, Linux's shape
pub use miku_abi::signal::stack_t;

/// POSIX `sigaltstack`: this thread's alternate signal stack for `SA_ONSTACK`
/// handlers. Per thread, kept by the kernel (`sigaltstack`(115)), which places
/// the frame. `SS_DISABLE` removes it; changing it while on it is `EPERM`; less
/// than `MINSIGSTKSZ` (2048) is `ENOMEM`
#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaltstack(ss: *const stack_t, old: *mut stack_t) -> i32 {
    let r = unsafe { crate::sys::sc2(crate::sys::SYS_SIGALTSTACK, ss as u64, old as u64) };
    ret(r) as i32
}

// signal.h: masks
//
// `sigset_t` is Linux's 1024 bits (the `libc` crate's `[u64; 16]`), bit `n-1`
// for signal `n`. The mask is `sys_miku.so`'s and process-wide: there is no
// per-thread mask (dev/signal-architecture.md, "What is not here"), so
// `pthread_sigmask` is this under another name. Unblocking a signal that
// arrived while blocked delivers it before the call returns, per POSIX

/// C bits (signal n at bit n-1) to the table's (signal n at bit n), 1..31
pub(crate) fn c_set_to_mask(set: *const [u64; 16]) -> u32 {
    let w = unsafe { (*set)[0] };
    let mut m = ((w << 1) & 0xffff_fffe) as u32;
    // SIGKILL and SIGSTOP cannot be blocked; asking is not an error
    m &= !(1 << signal::SIG_KILL) & !(1 << signal::SIG_STOP);
    m
}

pub(crate) fn mask_to_c_set(m: u32, set: *mut [u64; 16]) {
    unsafe {
        *set = [0; 16];
        (*set)[0] = (m >> 1) as u64;
    }
}

pub use miku_abi::signal::SIG_BLOCK;

pub use miku_abi::signal::SIG_UNBLOCK;

pub use miku_abi::signal::SIG_SETMASK;

/// The mask change, with errno-style failure; shared with `pthread_sigmask`
pub(crate) fn change_mask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> Result<(), i32> {
    if !set.is_null() && !(SIG_BLOCK..=SIG_SETMASK).contains(&how) {
        return Err(EINVAL);
    }
    let cur = signal::miku_signal_get_mask();
    if !old.is_null() {
        mask_to_c_set(cur, old);
    }
    if set.is_null() {
        return Ok(());
    }
    let req = c_set_to_mask(set);
    let (block, unblock) = match how {
        SIG_BLOCK => (req, 0),
        SIG_UNBLOCK => (0, req & cur),
        _ => (req & !cur, cur & !req),
    };
    if block != 0 {
        signal::miku_signal_set_mask(signal::miku_signal_get_mask() | block);
    }
    for sig in 1..32 {
        if unblock & (1 << sig) != 0 {
            signal::miku_signal_unblock(sig);
        }
    }
    Ok(())
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigprocmask(how: i32, set: *const [u64; 16], old: *mut [u64; 16]) -> i32 {
    match change_mask(how, set, old) {
        Ok(()) => 0,
        Err(e) => fail(e),
    }
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigemptyset(set: *mut [u64; 16]) -> i32 {
    if set.is_null() { return fail(EINVAL); }
    unsafe { *set = [0; 16] };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigfillset(set: *mut [u64; 16]) -> i32 {
    if set.is_null() { return fail(EINVAL); }
    unsafe { *set = [u64::MAX; 16] };
    0
}

pub(crate) fn sig_bit(sig: i32) -> Option<(usize, u64)> {
    if !(1..=1024).contains(&sig) { return None; }
    let b = (sig - 1) as usize;
    Some((b / 64, 1u64 << (b % 64)))
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigaddset(set: *mut [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    unsafe { (*set)[w] |= b };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigdelset(set: *mut [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    unsafe { (*set)[w] &= !b };
    0
}

#[no_mangle]
#[inline(never)]
pub extern "C" fn sigismember(set: *const [u64; 16], sig: i32) -> i32 {
    let Some((w, b)) = sig_bit(sig) else { return fail(EINVAL) };
    if set.is_null() { return fail(EINVAL); }
    (unsafe { (*set)[w] } & b != 0) as i32
}
