///////////////////////////////////////////////////////////////////////
//            Signal handler registration                            //
//                                                                   //
// Simple signal handler table for userspace                         //
// Registers callbacks for signal numbers                            //
// When the OS delivers a signal, the registered handler is called   //
// Uses SpinLock for handler table, AtomicU32 for blocked mask       //
///////////////////////////////////////////////////////////////////////

use crate::sync::SpinLock;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const MAX_SIGNALS: usize = 32;

// Kernel delivery glue //
//
// The kernel delivers signals by redirecting the sysret of any syscall to
// __miku_sig_entry with a frame on the user stack:
//   [rsp+0]  sig     [rsp+8]  saved RIP   [rsp+16] saved RSP
//   [rsp+24] saved RAX (syscall return)   [rsp+32] saved RFLAGS
// The stub dispatches through miku_sigaction_dispatch and returns to the
// interrupted context via sys_sigreturn(frame). Unhandled fatal-default
// signals (HUP/INT/QUIT) terminate the process with 128+sig, POSIX style.
//
// Only caller-saved registers are used here: the interrupted code's
// callee-saved set is preserved by the handler chain per the C ABI, and
// libmiku's syscall wrappers declare the entire caller-saved set as
// clobbered by any syscall (see sys.rs)
core::arch::global_asm!(
    ".global __miku_sig_entry",
    "__miku_sig_entry:",
    "mov rdi, [rsp]",              // sig
    "call __miku_sig_dispatch_any", // al = handled
    "mov rdi, [rsp]",              // sig again (frame is intact)
    "test al, al",
    "jnz 2f",
    "cmp rdi, 1",                  // SIGHUP
    "je 3f",
    "cmp rdi, 2",                  // SIGINT
    "je 3f",
    "cmp rdi, 3",                  // SIGQUIT
    "je 3f",
    "2:",                          // handled (or ignorable): resume
    "mov rdi, rsp",                // frame ptr
    "mov rax, 69",                 // SYS_SIGRETURN
    "syscall",
    "ud2",
    "3:",                          // unhandled fatal default
    "lea rdi, [rdi + 128]",
    "mov rax, 0",                  // SYS_EXIT
    "syscall",
    "ud2",
);

unsafe extern "C" {
    fn __miku_sig_entry();
}

static ENTRY_REGISTERED: AtomicBool = AtomicBool::new(false);

/// Unified dispatch used by the kernel entry stub: sigaction table first
/// (flags/mask semantics), then the plain miku_signal table
#[no_mangle]
pub extern "C" fn __miku_sig_dispatch_any(sig: u32) -> bool {
    if miku_sigaction_dispatch(sig) {
        return true;
    }
    miku_signal_dispatch(sig)
}

/// Tell the kernel where to deliver signals. Called automatically the
/// first time a handler is installed; safe to call repeatedly
#[no_mangle]
pub extern "C" fn miku_signal_init() {
    if !ENTRY_REGISTERED.swap(true, Ordering::Relaxed) {
        unsafe {
            crate::sys::sc1(crate::sys::SYS_SIGENTRY, __miku_sig_entry as usize as u64);
        }
    }
}

// signal constants
pub const SIG_HUP: u32 = 1;
pub const SIG_INT: u32 = 2;
pub const SIG_QUIT: u32 = 3;
pub const SIG_ILL: u32 = 4;
pub const SIG_TRAP: u32 = 5;
pub const SIG_ABRT: u32 = 6;
pub const SIG_FPE: u32 = 8;
pub const SIG_KILL: u32 = 9;
pub const SIG_SEGV: u32 = 11;
pub const SIG_PIPE: u32 = 13;
pub const SIG_ALRM: u32 = 14;
pub const SIG_TERM: u32 = 15;
pub const SIG_CHLD: u32 = 17;
pub const SIG_CONT: u32 = 18;
pub const SIG_STOP: u32 = 19;
pub const SIG_USR1: u32 = 10;
pub const SIG_USR2: u32 = 12;

type SignalHandler = extern "C" fn(u32);

struct SignalTable {
    handlers: [Option<SignalHandler>; MAX_SIGNALS],
}

static SIGNAL_TABLE: SpinLock<SignalTable> = SpinLock::new(SignalTable {
    handlers: [None; MAX_SIGNALS],
});

static BLOCKED_MASK: AtomicU32 = AtomicU32::new(0);

//   register a signal handler
// Returns previous handler, or null if none was set
// Pass null handler to restore default action
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
    let mut table = SIGNAL_TABLE.lock();
    let prev = table.handlers[sig as usize];
    table.handlers[sig as usize] = handler;
    prev
}

//   dispatch signal to registered handler
// Called by the runtime when a signal arrives
// Returns true if a handler was called
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
    let mut table = SIGNAL_TABLE.lock();
    for i in 0..MAX_SIGNALS {
        table.handlers[i] = None;
    }
}

// block a signal
#[no_mangle]
pub extern "C" fn miku_signal_block(sig: u32) {
    if sig > 0 && (sig as usize) < MAX_SIGNALS {
        BLOCKED_MASK.fetch_or(1 << sig, Ordering::Relaxed);
    }
}

// unblock a signal
#[no_mangle]
pub extern "C" fn miku_signal_unblock(sig: u32) {
    if sig > 0 && (sig as usize) < MAX_SIGNALS {
        BLOCKED_MASK.fetch_and(!(1 << sig), Ordering::Relaxed);
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
pub const SA_RESETHAND: u32 = 1; // reset handler after one delivery
pub const SA_NODEFER:   u32 = 2; // don't block signal during handler
pub const SA_RESTART:   u32 = 4; // restart interrupted syscalls

#[repr(C)]
#[derive(Copy, Clone)]
pub struct MikuSigaction {
    pub handler: Option<SignalHandler>,
    pub flags: u32,
    pub mask: u32, // signals to block during handler execution
}

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

    // Register the kernel entry BEFORE taking any lock: init makes a
    // syscall, and a signal delivered at that boundary re-enters the
    // dispatch path, which takes these locks
    if !act.is_null() && unsafe { (*act).handler.is_some() } {
        miku_signal_init();
    }

    let mut table = SIGACTION_TABLE.lock();
    let idx = sig as usize;

    if !oldact.is_null() {
        unsafe { *oldact = table.actions[idx]; }
    }

    if !act.is_null() {
        let new_act = unsafe { *act };
        table.actions[idx] = new_act;

        // sync with simple handler table for backward compatibility
        let mut simple = SIGNAL_TABLE.lock();
        simple.handlers[idx] = new_act.handler;
    }

    0
}

// dispatch with sigaction semantics (block mask, resethand)
#[no_mangle]
pub extern "C" fn miku_sigaction_dispatch(sig: u32) -> bool {
    if sig == 0 || sig as usize >= MAX_SIGNALS { return false; }

    // check blocked
    if miku_signal_is_blocked(sig) { return false; }

    let act = {
        let table = SIGACTION_TABLE.lock();
        table.actions[sig as usize]
    };

    let handler = match act.handler {
        Some(h) => h,
        None => return false,
    };

    let mask_bits = if act.flags & SA_NODEFER == 0 {
        act.mask | (1u32 << sig)
    } else {
        act.mask
    };
    let old_mask = if mask_bits != 0 {
        BLOCKED_MASK.fetch_or(mask_bits, Ordering::Relaxed)
    } else {
        BLOCKED_MASK.load(Ordering::Relaxed)
    };

    handler(sig);

    // restore: clear only bits we actually set (other threads may have set
    // bits concurrently; we shouldn't clobber their changes with a raw store)
    if mask_bits != 0 {
        let newly_set = mask_bits & !old_mask;
        if newly_set != 0 {
            BLOCKED_MASK.fetch_and(!newly_set, Ordering::Relaxed);
        }
    }

    // reset handler if SA_RESETHAND
    if act.flags & SA_RESETHAND != 0 {
        let mut table = SIGACTION_TABLE.lock();
        table.actions[sig as usize].handler = None;
        let mut simple = SIGNAL_TABLE.lock();
        simple.handlers[sig as usize] = None;
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
