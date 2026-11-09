// Userspace signal handler table: callbacks registered by signal number and
// called on delivery. SpinLock for the table, AtomicU32 for the blocked mask

use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

const MAX_SIGNALS: usize = 32;

// Kernel delivery
//
// The kernel starts __miku_sig_entry with RSP at a frame it wrote
// (kernel/process/signal/frame.rs):
//   [rsp+0]    siginfo_t   (si_signo first)
//   [rsp+128]  ucontext_t  (uc_mcontext.gregs holds every register the
//                           interrupted code had, in Linux's REG_* order)
//   [rsp+560]  512 bytes for the FPU image, which is ours to fill
// The kernel does not touch the FPU, since ring 0 uses no SSE, so the
// interrupted code's xmm registers are still in the CPU here. The stub saves
// them before the handler and restores them after. sigreturn(69) then restores
// every general register from the frame, including any a handler rewrote.
// Unhandled fatal-default signals (HUP/INT/QUIT) exit with 128+sig, POSIX
// style.
//
// The frame is 16-aligned, so RSP is right for the call
core::arch::global_asm!(
    ".global __miku_sig_entry",
    "__miku_sig_entry:",
    "fxsave64 [rsp + 560]",
    "mov edi, dword ptr [rsp]",    // sig
    "mov rsi, rsp",                // siginfo_t *
    "lea rdx, [rsp + 128]",        // ucontext_t *
    "call __miku_sig_dispatch_any", // al = handled
    "mov edi, dword ptr [rsp]",    // sig again (frame is intact)
    "test al, al",
    "jnz 2f",
    "cmp edi, 1",                  // SIGHUP
    "je 3f",
    "cmp edi, 2",                  // SIGINT
    "je 3f",
    "cmp edi, 3",                  // SIGQUIT
    "je 3f",
    "2:",                          // handled (or ignorable): resume
    "fxrstor64 [rsp + 560]",
    "mov rdi, rsp",                // frame ptr
    "mov eax, {sigreturn}",
    "syscall",
    "ud2",
    "3:",                          // unhandled fatal default
    "lea rdi, [rdi + 128]",
    "mov eax, {exit}",
    "syscall",
    "ud2",
    sigreturn = const miku_abi::sys::SYS_SIGRETURN,
    exit = const miku_abi::sys::SYS_EXIT,
);

/// `uc_flags` bit the kernel sets on a frame for a fault it raised
use miku_abi::ksignal::UC_MIKU_FAULT;

unsafe extern "C" {
    fn __miku_sig_entry();
}

static ENTRY_REGISTERED: AtomicBool = AtomicBool::new(false);

/// The two tables in a child of `fork` (`proc::miku_fork`), freed whoever held
/// them, since that thread is not in the child and `std` resets SIGPIPE through
/// them. Not held across the fork as the heap is: the dispatcher takes them,
/// and SIGCHLD can arrive at the end of the fork. A holder copies or writes one
/// entry, so the worst the child sees is that entry half written, before its
/// `exec`.
///
/// # Safety
/// Only in the child, where no other thread exists
pub unsafe fn fork_child_reset_tables() {
    SIGNAL_TABLE.unlock_raw();
    SIGACTION_TABLE.unlock_raw();
}

/// Unified dispatch for the kernel entry stub: the sigaction table first (flags
/// and mask), then the plain miku_signal table. `info` and `uc` are the
/// kernel's frame.
///
/// A fault (`UC_MIKU_FAULT`) differs. The mask never holds it, as the mask is
/// process-wide. And it is not returned from without a handler, since the
/// instruction would fault into the same frame; a fault nobody takes resets its
/// signal to default, and the retried instruction ends the process with the
/// kernel's report, as on Linux
#[no_mangle]
pub extern "C" fn __miku_sig_dispatch_any(sig: u32, info: *mut u8, uc: *mut u8) -> bool {
    let info = info as *mut SigInfo;
    let fault = !uc.is_null() && unsafe { (uc as *const u64).read() } & UC_MIKU_FAULT != 0;
    if fault {
        if run_action(sig, info, uc, false) || miku_signal_dispatch(sig) {
            return true;
        }
        let none = EMPTY_SIGACTION;
        miku_sigaction(sig, &none, core::ptr::null_mut());
        return true;
    }
    // A blocked signal is held, not lost and not fatal
    if miku_signal_is_blocked(sig) {
        PENDING_MASK.fetch_or(1 << sig, Ordering::Relaxed);
        return true;
    }
    if run_action(sig, info, uc, true) || miku_signal_dispatch(sig) {
        HANDLED.fetch_add(1, Ordering::Release);
        return true;
    }
    false
}

/// Handlers run so far in this process. `pause` compares it across a sleep
static HANDLED: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
pub extern "C" fn miku_signal_handled() -> u64 {
    HANDLED.load(Ordering::Acquire)
}

/// Tells the kernel where to deliver signals. Called on the first handler
/// install; safe to repeat
#[no_mangle]
pub extern "C" fn miku_signal_init() {
    if !ENTRY_REGISTERED.swap(true, Ordering::Relaxed) {
        unsafe {
            crate::sys::sc1(crate::sys::SYS_SIGENTRY, __miku_sig_entry as usize as u64);
        }
    }
}

/// Tells the kernel what this process wants done with one signal. The kernel
/// keeps a disposition per signal (default, ignore, deliver), and a sender
/// consults it.
///
/// Called from every path that installs or removes a handler, with no table
/// lock held: this makes a syscall, a signal can arrive at its boundary, and
/// delivery takes those locks
fn sync_disposition(sig: u32) {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return; }
    if sig == SIG_KILL { return; }
    let caught = {
        let act = SIGACTION_TABLE.lock().actions[sig as usize].handler.is_some();
        let simple = SIGNAL_TABLE.lock().handlers[sig as usize].is_some();
        act || simple
    };
    let onstack = SIGACTION_TABLE.lock().actions[sig as usize].flags & SA_ONSTACK != 0;
    let disp = if !caught {
        miku_abi::ksignal::SIG_DISP_DEFAULT
    } else if onstack {
        miku_abi::ksignal::SIG_DISP_CATCH | miku_abi::ksignal::SIG_DISP_ONSTACK
    } else {
        miku_abi::ksignal::SIG_DISP_CATCH
    };
    unsafe { crate::sys::sc2(crate::sys::SYS_SIGDISP, sig as u64, disp); }
}

/// Ignores a signal in the kernel. Differs from having no handler, where a
/// fatal default kills. `SIGPIPE` is the usual case
#[no_mangle]
pub extern "C" fn miku_signal_ignore(sig: u32) -> i32 {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return -1; }
    if sig == SIG_KILL { return -1; }
    miku_signal(sig, None);
    unsafe {
        crate::sys::sc2(crate::sys::SYS_SIGDISP, sig as u64, miku_abi::ksignal::SIG_DISP_IGNORE);
    }
    0
}

// signal constants
pub const SIG_HUP: u32 = miku_abi::signal::SIGHUP as u32;
pub const SIG_INT: u32 = miku_abi::signal::SIGINT as u32;
pub const SIG_QUIT: u32 = miku_abi::signal::SIGQUIT as u32;
pub const SIG_ILL: u32 = miku_abi::signal::SIGILL as u32;
pub const SIG_TRAP: u32 = miku_abi::signal::SIGTRAP as u32;
pub const SIG_ABRT: u32 = miku_abi::signal::SIGABRT as u32;
pub const SIG_FPE: u32 = miku_abi::signal::SIGFPE as u32;
pub const SIG_KILL: u32 = miku_abi::signal::SIGKILL as u32;
pub const SIG_SEGV: u32 = miku_abi::signal::SIGSEGV as u32;
pub const SIG_PIPE: u32 = miku_abi::signal::SIGPIPE as u32;
pub const SIG_ALRM: u32 = miku_abi::signal::SIGALRM as u32;
pub const SIG_TERM: u32 = miku_abi::signal::SIGTERM as u32;
pub const SIG_CHLD: u32 = miku_abi::signal::SIGCHLD as u32;
pub const SIG_CONT: u32 = miku_abi::signal::SIGCONT as u32;
pub const SIG_STOP: u32 = miku_abi::signal::SIGSTOP as u32;
pub const SIG_USR1: u32 = miku_abi::signal::SIGUSR1 as u32;
pub const SIG_USR2: u32 = miku_abi::signal::SIGUSR2 as u32;
pub const SIG_TSTP: u32 = miku_abi::signal::SIGTSTP as u32;
/// The terminal changed shape. Ignored unless caught, so a handler subscribes:
/// ask `TIOCGWINSZ` from it and lay out again
pub const SIG_WINCH: u32 = miku_abi::signal::SIGWINCH as u32;

use miku_abi::libtypes::SignalHandler;

struct SignalTable {
    handlers: [Option<SignalHandler>; MAX_SIGNALS],
}

static SIGNAL_TABLE: SpinLock<SignalTable> = SpinLock::new(SignalTable {
    handlers: [None; MAX_SIGNALS],
});

static BLOCKED_MASK: AtomicU32 = AtomicU32::new(0);

// Registers a signal handler. Returns the previous one or null. A null handler
// restores the default
#[no_mangle]
pub extern "C" fn miku_signal(
    sig: u32,
    handler: Option<SignalHandler>,
) -> Option<SignalHandler> {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return None; }
    if sig == SIG_KILL || sig == SIG_STOP { return None; }

    if handler.is_some() {
        miku_signal_init();
    }
    let prev = {
        let mut table = SIGNAL_TABLE.lock();
        let prev = table.handlers[sig as usize];
        table.handlers[sig as usize] = handler;
        prev
    };
    // After the lock: a signal at this syscall's boundary would re-enter
    // dispatch and take it
    sync_disposition(sig);
    prev
}

// Dispatches to the registered handler. Called by the runtime on arrival; true
// if a handler was called
#[no_mangle]
pub extern "C" fn miku_signal_dispatch(sig: u32) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }

    let handler = {
        let table = SIGNAL_TABLE.lock();
        table.handlers[sig as usize]
    };

    if let Some(h) = handler {
        h(sig);
        true
    } else {
        false
    }
}

// check if handler is registered for signal
#[no_mangle]
pub extern "C" fn miku_signal_has_handler(sig: u32) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }
    let table = SIGNAL_TABLE.lock();
    table.handlers[sig as usize].is_some()
}

// reset all handlers to default
#[no_mangle]
pub extern "C" fn miku_signal_reset_all() {
    {
        let mut table = SIGNAL_TABLE.lock();
        for i in 0..MAX_SIGNALS {
            table.handlers[i] = None;
        }
    }
    for sig in 1..MAX_SIGNALS as u32 {
        sync_disposition(sig);
    }
}

// block a signal
#[no_mangle]
pub extern "C" fn miku_signal_block(sig: u32) {
    if sig > 0 && (sig as usize) < MAX_SIGNALS {
        BLOCKED_MASK.fetch_or(1 << sig, Ordering::Relaxed);
    }
}

// unblock a signal, and deliver it if one arrived while it was blocked
#[no_mangle]
pub extern "C" fn miku_signal_unblock(sig: u32) {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return; }
    BLOCKED_MASK.fetch_and(!(1 << sig), Ordering::Relaxed);
    // Blocking holds signals; it does not lose them
    let held = PENDING_MASK.fetch_and(!(1 << sig), Ordering::Relaxed);
    if held & (1 << sig) != 0 {
        let _ = miku_sigaction_dispatch(sig) || miku_signal_dispatch(sig);
    }
}

// check if signal is blocked
#[no_mangle]
pub extern "C" fn miku_signal_is_blocked(sig: u32) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }
    (BLOCKED_MASK.load(Ordering::Relaxed) & (1 << sig)) != 0
}

// get blocked signal mask
#[no_mangle]
pub extern "C" fn miku_signal_get_mask() -> u32 {
    BLOCKED_MASK.load(Ordering::Relaxed)
}

// set blocked signal mask
#[no_mangle]
pub extern "C" fn miku_signal_set_mask(mask: u32) -> u32 {
    BLOCKED_MASK.swap(mask, Ordering::Relaxed)
}

// sigaction-style API //

// signal action flags
pub const SA_RESETHAND: u32 = miku_abi::signal::SA_RESETHAND as u32; // reset handler after one delivery
pub const SA_NODEFER: u32 = miku_abi::signal::SA_NODEFER as u32; // don't block signal during handler
pub const SA_RESTART: u32 = miku_abi::signal::SA_RESTART as u32; // restart interrupted syscalls
/// Calls the handler as `(sig, siginfo_t *, ucontext_t *)`, both pointing into
/// the kernel's frame. For a fault, `si_code` and `si_addr` say what happened,
/// and the `ucontext_t` holds every register, which the handler may rewrite for
/// `sigreturn`. For a sent signal, `si_code` is zero
pub const SA_SIGINFO: u32 = miku_abi::signal::SA_SIGINFO as u32;
/// Runs the handler on the thread's `sigaltstack`, if it has one and is not
/// already on it. The kernel places the frame, so this goes down with the
/// disposition
pub const SA_ONSTACK: u32 = miku_abi::signal::SA_ONSTACK as u32;

/// `siginfo_t` as `libc` declares it for MikuOS: three ints, the fault address
/// at +16, padding to 128 bytes
#[repr(C, align(8))]
struct SigInfo {
    signo: i32,
    errno: i32,
    code: i32,
    _pad: [i32; 29],
}

type SigInfoHandler = extern "C" fn(u32, *mut SigInfo, *mut u8);

pub use miku_abi::libtypes::MikuSigaction;

const EMPTY_SIGACTION: MikuSigaction = MikuSigaction {
    handler: None, flags: 0, mask: 0,
};

struct SigactionTable {
    actions: [MikuSigaction; MAX_SIGNALS],
}

static SIGACTION_TABLE: SpinLock<SigactionTable> = SpinLock::new(SigactionTable {
    actions: [EMPTY_SIGACTION; MAX_SIGNALS],
});

// set signal action, returns previous action
#[no_mangle]
pub extern "C" fn miku_sigaction(
    sig: u32,
    act: *const MikuSigaction,
    oldact: *mut MikuSigaction,
) -> i32 {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return -1; }
    if sig == SIG_KILL || sig == SIG_STOP { return -1; }

    // Registers the kernel entry before any lock: init makes a syscall, and a
    // signal at that boundary would take these locks
    if !act.is_null() && unsafe { (*act).handler.is_some() } {
        miku_signal_init();
    }

    let mut table = SIGACTION_TABLE.lock();
    let idx = sig as usize;

    if !oldact.is_null() {
        unsafe { *oldact = table.actions[idx]; }
    }

    let changed = !act.is_null();
    if changed {
        let new_act = unsafe { *act };
        table.actions[idx] = new_act;

        // sync with simple handler table for backward compatibility
        let mut simple = SIGNAL_TABLE.lock();
        simple.handlers[idx] = new_act.handler;
    }
    drop(table);

    // Tells the kernel with both tables unlocked; it decides delivery from the
    // disposition
    if changed {
        sync_disposition(sig);
    }

    0
}

// Dispatches with sigaction semantics (block mask, resethand), for a signal
// with no kernel frame: one the mask released, or raised by the program
#[no_mangle]
pub extern "C" fn miku_sigaction_dispatch(sig: u32) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }
    let mut info = SigInfo { signo: sig as i32, errno: 0, code: 0, _pad: [0; 29] };
    run_action(sig, &mut info, core::ptr::null_mut(), true)
}

/// Runs the sigaction handler for `sig`, if any. `masked` is false for a fault,
/// which neither reads nor changes the mask (see `__miku_sig_dispatch_any`)
fn run_action(sig: u32, info: *mut SigInfo, uc: *mut u8, masked: bool) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }

    // check blocked
    if masked && miku_signal_is_blocked(sig) { return false; }

    let act = {
        let table = SIGACTION_TABLE.lock();
        table.actions[sig as usize]
    };

    let handler = match act.handler {
        Some(h) => h,
        None => return false,
    };

    let mask_bits = if !masked {
        0
    } else if act.flags & SA_NODEFER == 0 {
        act.mask | (1u32 << sig)
    } else {
        act.mask
    };
    let old_mask = if mask_bits != 0 {
        BLOCKED_MASK.fetch_or(mask_bits, Ordering::Relaxed)
    } else {
        BLOCKED_MASK.load(Ordering::Relaxed)
    };

    if act.flags & SA_SIGINFO != 0 {
        // Called with the three arguments its installer declared
        let h: SigInfoHandler = unsafe { core::mem::transmute(handler) };
        h(sig, info, uc);
    } else {
        handler(sig);
    }

    // Clears only bits set here, since other threads may have set bits
    // concurrently
    if mask_bits != 0 {
        let newly_set = mask_bits & !old_mask;
        if newly_set != 0 {
            BLOCKED_MASK.fetch_and(!newly_set, Ordering::Relaxed);
        }
    }

    // reset handler if SA_RESETHAND
    if act.flags & SA_RESETHAND != 0 {
        {
            let mut table = SIGACTION_TABLE.lock();
            table.actions[sig as usize].handler = None;
            let mut simple = SIGNAL_TABLE.lock();
            simple.handlers[sig as usize] = None;
        }
        // The next one gets the default, and the kernel is told
        sync_disposition(sig);
    }

    true
}

// get pending signals (signals that arrived while blocked)
static PENDING_MASK: AtomicU32 = AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn miku_signal_raise(sig: u32) -> i32 {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return -1; }
    if miku_signal_is_blocked(sig) {
        PENDING_MASK.fetch_or(1 << sig, Ordering::Relaxed);
        return 0;
    }
    if miku_sigaction_dispatch(sig) { 0 } else { -1 }
}

#[no_mangle]
pub extern "C" fn miku_signal_pending() -> u32 {
    PENDING_MASK.load(Ordering::Relaxed) & BLOCKED_MASK.load(Ordering::Relaxed)
}
