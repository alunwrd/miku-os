// Kernel-side signals
//
// Three parts, one file each:
//   - here: what a process wants done about each signal (the disposition
//     table, `SA_ONSTACK`), and sending: set a pending bit on the target,
//     or apply the default for a process with no handler for it
//   - deliver.rs: getting a signal into ring 3, at the tail of a syscall for
//     one somebody sent, straight from `trap::user_trap` for one the CPU
//     raised, and back out through `sigreturn`
//   - frame.rs: the frame a handler runs on, `siginfo_t` + `ucontext_t` in
//     Linux's layout. The one writer of that ABI
//
// A signal somebody sent waits for a syscall boundary: a CPU-bound loop will
// not see its handler run until it makes one. Fatal defaults are applied in
// send_signal, so Ctrl+C still kills such loops. A signal the CPU raised does
// not wait, because the fault is itself the entry. See
// dev/signal-architecture.md.

mod deliver;
pub mod frame;

pub use deliver::{
    current_altstack, deliver_fault, deliver_pending, sigreturn_full, sys_sigaltstack,
    sys_sigentry,
};

use core::sync::atomic::Ordering;

pub const SIGHUP:  u32 = 1;
pub const SIGINT:  u32 = 2;
pub const SIGQUIT: u32 = 3;
/// `ud2`, or anything else the CPU would not decode
pub const SIGILL:  u32 = 4;
/// Integer division by zero
pub const SIGFPE:  u32 = 8;
pub const SIGKILL: u32 = 9;
/// A page the program may not touch, or a `#GP`
pub const SIGSEGV: u32 = 11;
/// A write to a pipe nobody is reading
///
/// Fatal by default: a program in a pipeline whose reader has gone should stop,
/// and `yes | head` ends because of this. A shell that arranged to survive it
/// gets `EPIPE` from the write
pub const SIGPIPE: u32 = 13;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;
pub const SIGCONT: u32 = 18;
pub const SIGSTOP: u32 = 19;
pub const SIGTSTP: u32 = 20;
/// A background job tried to read its terminal
pub const SIGTTIN: u32 = 21;
/// ...or to write to it, or to change its settings
pub const SIGTTOU: u32 = 22;
/// The terminal changed shape
///
/// Ignored by default: every signal this table does not name is fatal, and a
/// window being dragged must not kill what runs in it. A program that wants to
/// re-lay itself out catches it and asks `TIOCGWINSZ`
pub const SIGWINCH: u32 = 28;

// `si_code` for the signals the CPU raises, Linux's numbers. A handler reads
// them to tell a fault from a `kill`, which has `si_code` 0

/// `SIGFPE`: integer divide by zero
pub const FPE_INTDIV:  i32 = 1;
/// `SIGILL`: an illegal opcode
pub const ILL_ILLOPN:  i32 = 2;
/// `SIGSEGV`: no mapping at the address
pub const SEGV_MAPERR: i32 = 1;
/// `SIGSEGV`: a mapping that does not allow this access
pub const SEGV_ACCERR: i32 = 2;
/// `SIGSEGV` from a `#GP`, which has no address to report
pub const SI_KERNEL:   i32 = 0x80;

/// The most signals this kernel knows about. A `pending_sig` is a `u32` of bits
/// and a disposition table is two bits each in a `u64`, so this is what both
/// can hold
pub const NSIG: u32 = 32;

// What a process wants done about a signal
//
// A disposition is per signal, so a service can trap SIGTERM to shut down
// cleanly and a program trapping nothing still dies on it.
//
// Two bits per signal in one `u64`, one atomic write per change: a pair of
// masks would be two words, and a reader between the two would see a
// disposition the process never asked for.

/// Do whatever the kernel does for this signal when nobody says otherwise
pub const DISP_DEFAULT: u64 = 0;
/// Throw it away
pub const DISP_IGNORE:  u64 = 1;
/// Queue it for the userspace dispatcher
pub const DISP_CATCH:   u64 = 2;
/// Not a disposition: `sigdisp` with this changes nothing and answers with the
/// one set
pub const DISP_QUERY:   u64 = 3;
/// Not a disposition either: a bit beside `DISP_CATCH` saying the handler runs
/// on the task's alternate stack (`SA_ONSTACK`). Kept in a mask word of its own
/// (`sig_onstack`) so the table stays two bits, and a query answers with it set
/// when it is
pub const DISP_ONSTACK: u64 = 4;

/// Signals that cannot be caught, ignored or redirected
///
/// SIGKILL and SIGSTOP, the POSIX pair: the machine's owner's last word about a
/// process
#[inline]
fn uncatchable(sig: u32) -> bool { sig == SIGKILL || sig == SIGSTOP }

/// What this process has asked for, for one signal
pub fn disposition(pid: u64, sig: u32) -> u64 {
    if sig == 0 || sig >= NSIG { return DISP_DEFAULT; }
    let table = crate::sched::sig_disp_of(pid);
    (table >> (2 * sig)) & 0b11
}

/// The table a process keeps across `execve`: an ignored signal stays ignored
/// (how a shell's `nohup` and a parent's SIGPIPE choice reach the program), and
/// a caught one goes back to its default, because the handler was code in the
/// image just replaced (POSIX `exec`)
pub fn disp_after_exec(table: u64) -> u64 {
    let mut out = table;
    for sig in 1..NSIG {
        if (table >> (2 * sig)) & 0b11 == DISP_CATCH {
            out &= !(0b11 << (2 * sig));
        }
    }
    out
}

/// Records what a process wants done about a signal. False when there is no
/// such process, or the signal is one nobody may redirect
pub fn set_disposition(pid: u64, sig: u32, disp: u64) -> bool {
    swap_disposition(pid, sig, disp).is_some()
}

/// The same, answering with the disposition it replaced, from the one
/// compare-exchange that made the change, so a sibling thread changing it at
/// the same moment cannot make the answer describe a state that never existed
pub fn swap_disposition(pid: u64, sig: u32, disp: u64) -> Option<u64> {
    if sig == 0 || sig >= NSIG || disp > DISP_CATCH { return None; }
    if uncatchable(sig) { return None; }
    // Through the leader, where `disposition` reads it, so a `sigaction` made
    // by any thread takes effect
    let ptr = unsafe { crate::sched::leader_ptr(pid) };
    if ptr.is_null() { return None; }
    let p = unsafe { &*ptr };
    let shift = 2 * sig;
    let mask = 0b11u64 << shift;
    // Read-modify-write rather than a store: the other thirty-one signals in
    // this word belong to calls that have already happened
    let mut cur = p.sig_disp.load(Ordering::Relaxed);
    loop {
        let next = (cur & !mask) | (disp << shift);
        match p.sig_disp.compare_exchange_weak(
            cur, next, Ordering::Relaxed, Ordering::Relaxed)
        {
            Ok(_)  => return Some((cur >> shift) & 0b11),
            Err(c) => cur = c,
        }
    }
}

/// True when a signal nobody has said anything about ends the process (the
/// POSIX defaults)
fn default_is_fatal(sig: u32) -> bool {
    matches!(default_action(sig), Default_::Kill)
}

/// What a signal nobody has said anything about actually does
#[derive(Clone, Copy, PartialEq, Eq)]
enum Default_ {
    /// End the process
    Kill,
    /// Take it off the CPU until somebody continues it
    Stop,
    /// Put it back on
    Continue,
    /// Nothing at all
    Ignore,
}

/// The POSIX default actions.
///
/// The fallthrough is `Kill`: every signal this table does not name ends the
/// process
fn default_action(sig: u32) -> Default_ {
    match sig {
        SIGSTOP | SIGTSTP => Default_::Stop,
        // SIGTTIN and SIGTTOU stop a background job touching the terminal in
        // POSIX, and here, though nothing sends them yet: the read side answers
        // a background reader with end of input
        // (`io::input::user_stdin::may_read`), and there is no write-side check
        // to send SIGTTOU from
        SIGTTIN | SIGTTOU => Default_::Stop,
        SIGCONT => Default_::Continue,
        SIGCHLD | SIGWINCH => Default_::Ignore,
        23 => Default_::Ignore,              // SIGURG
        _ => Default_::Kill,
    }
}

/// Sends a signal to a process: queues it, drops it, or applies its default
///
///   - SIGKILL cannot be caught or ignored
///   - an ignored signal is dropped here rather than queued, so a program
///     ignoring SIGPIPE does not accumulate a pending bit
///   - a caught signal is queued for the dispatcher, if there is one: a process
///     that registered a disposition and lost its entry point would queue signals
///     nothing can deliver
///   - anything else gets the default, which for most signals is death
pub fn send_signal(pid: u64, sig: u32) {
    if sig == 0 || sig >= NSIG { return; }
    // A signal is sent to a process, never to a thread. POSIX lets any thread
    // of the group take it; the leader is chosen here, whose `sig_entry` and
    // `sig_disp` the kernel reads through `leader_raw`. A thread may have no
    // `sig_entry`, and the signal would be ignored
    let pid = crate::sched::tgid_of(pid);
    // Decided inside the interrupts-off block, acted on outside it
    let mut stop = false;
    let mut cont = false;
    x86_64::instructions::interrupts::without_interrupts(|| {
        let ptr = unsafe { crate::sched::proc_index_raw(pid) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };

        // SIGKILL is the only one that ends a process from here. SIGSTOP is
        // uncatchable too, but that means no disposition may redirect it, not
        // "kill it"
        if sig == SIGKILL {
            crate::sched::kill_by_signal(pid, sig);
            return;
        }

        // A disposition may not touch an uncatchable signal; its default is all
        // that can happen to it
        if !uncatchable(sig) {

            let disp = disposition(pid, sig);
            if disp == DISP_IGNORE {
                return;
            }
            if disp == DISP_CATCH && p.sig_entry.load(Ordering::Relaxed) != 0 {
                p.pending_sig.fetch_or(1 << sig, Ordering::Relaxed);
                return;
            }
        }

        match default_action(sig) {
            Default_::Kill => crate::sched::kill_by_signal(pid, sig),
            // Asked for, not done: `request_stop` explains why
            Default_::Stop => stop = true,
            Default_::Continue => cont = true,
            Default_::Ignore => {}
        }
    });

    // Outside the interrupts-off block, because both of these enter the
    // scheduler: `cont` takes a run queue's lock and may send an IPI, and
    // `request_stop` may hand this processor away for good
    if cont {
        // Forget that the last stop was reported, or a job stopped, continued
        // and stopped again would be reported only once
        crate::sched::clear_stop_report(pid);
        crate::sched::cont(pid);
    }
    if stop {
        // Which one, so `wait4` can tell a shell whether this was Ctrl-Z or a
        // `kill -STOP`
        x86_64::instructions::interrupts::without_interrupts(|| {
            let ptr = unsafe { crate::sched::proc_index_raw(pid) };
            if !ptr.is_null() {
                unsafe { (*ptr).stop_sig.store(sig, Ordering::Relaxed) };
            }
        });
        request_stop(pid);
        // A process that stopped itself does not come back here until somebody
        // continues it, so nothing below runs for that case
    }
    // Kick the target out of a blocked syscall (wait4/sleep) so delivery
    // happens promptly rather than at the next natural wakeup
    crate::sched::wakeup(pid);
}

/// Stops a process, whether or not it is the one asking
///
/// Stopping yourself is `sched::stop_current`, which records the request in
/// `pending_state` and hands the processor away, the channel `sleep` and
/// `block_current` use, so a task never writes its own state while a switch is
/// putting it away.
///
/// Stopping somebody else cannot write their state: they may be running on
/// another processor. So the request is left in their `pending_state` and
/// applied by the switch that next puts them away (`apply_pending`), on their
/// own core, with their stack saved.
///
/// A program making syscalls stops at its next one, and a program in a pure
/// counting loop stops when the scheduler preempts it, within a tick
fn request_stop(pid: u64) {
    if pid == crate::sched::current_pid() {
        crate::sched::stop_current();
        return;
    }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let ptr = unsafe { crate::sched::proc_index_raw(pid) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        if p.state.load(Ordering::Relaxed) == crate::process::STATE_DEAD { return; }
        p.stop_pending.store(true, Ordering::Release);
    });
}

/// Signals every process in a job
///
/// A terminal's foreground is a process group, so Ctrl-C interrupts all of
/// `sort | uniq`.
///
/// Collected in passes into a fixed buffer rather than signalled while walking
/// the process table: a signal kills, and a process dying underneath the walk
/// leaves it reading a freed slot. Returns how many were signalled
pub fn send_to_group(pgid: u64, sig: u32) -> usize {
    if pgid == 0 || sig == 0 || sig >= NSIG { return 0; }
    let mut buf = [0u64; 32];
    let mut from = 0usize;
    let mut sent = 0usize;
    loop {
        let (n, next) = crate::sched::group_members(pgid, from, &mut buf);
        for &pid in &buf[..n] {
            send_signal(pid, sig);
            sent += 1;
        }
        if next >= crate::sched::MAX_PROCS { break; }
        from = next;
    }
    sent
}

/// Send SIGCHLD to the parent of 'child_pid'
pub fn send_sigchld(child_pid: u64) {
    let ppid = crate::sched::get_ppid(child_pid);
    if ppid != 0 {
        send_signal(ppid, SIGCHLD);
        // Wake parent in case it's blocking on wait4
        crate::sched::wakeup(ppid);
    }
}

/// True if the current process has a queued signal userspace wants delivered.
/// Blocking syscalls (wait4) use this to bail out with EINTR so the dispatch
/// tail can inject the handler
pub fn has_deliverable(pid: u64) -> bool {
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return false; }
    let p = unsafe { &*ptr };
    p.sig_entry.load(Ordering::Relaxed) != 0
        && !p.sig_in_delivery.load(Ordering::Relaxed)
        && p.pending_sig.load(Ordering::Relaxed) != 0
}

/// Is any signal waiting for `pid`, handler or not?
///
/// For a kernel command on a process's thread (`kcmd`, under `mikudo`) looping
/// until told to stop: Ctrl+C in the terminal arrives as SIGINT pending on the
/// process, and with no handler registered `has_deliverable` would not say so
pub fn pending(pid: u64) -> bool {
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return false; }
    unsafe { &*ptr }.pending_sig.load(Ordering::Relaxed) != 0
}

/// sys_sigdisp(sig, disp): says what to do about one signal (100)
///
/// The half of `sigaction` the kernel needs. libmiku keeps the handler itself
/// and tells the kernel which of the three things should happen. SIGKILL is
/// rejected here: a program believing it trapped SIGKILL writes cleanup code
/// that never runs
pub fn sys_sigdisp(sig: u64, disp: u64) -> u64 {
    // `SA_ONSTACK` rides beside the disposition rather than in it
    let onstack = disp & DISP_ONSTACK != 0;
    let disp = disp & !DISP_ONSTACK;
    // `DISP_QUERY` changes nothing and answers with what is set: the other half
    // of `sigaction(sig, NULL, &old)`
    if disp == DISP_QUERY && sig != 0 && sig < NSIG as u64 {
        let pid = crate::sched::current_pid();
        return disposition(pid, sig as u32) | onstack_bit(pid, sig as u32);
    }
    if sig == 0 || sig >= NSIG as u64 || disp > DISP_CATCH {
        return crate::syscall::errno::err(crate::syscall::errno::EINVAL);
    }
    if uncatchable(sig as u32) {
        return crate::syscall::errno::err(crate::syscall::errno::EINVAL);
    }
    let pid = crate::sched::current_pid();
    // The previous disposition is the answer, which lets `sigaction` report
    // `SIG_IGN` in `oldact`: the handler table in libmiku knows nothing about
    // ignoring
    let was_onstack = onstack_bit(pid, sig as u32);
    match swap_disposition(pid, sig as u32, disp) {
        Some(prev) => {
            set_onstack(pid, sig as u32, onstack && disp == DISP_CATCH);
            prev | was_onstack
        }
        None => crate::syscall::errno::err(crate::syscall::errno::ESRCH),
    }
}

/// `DISP_ONSTACK` if this process's handler for `sig` asked for the
/// alternate stack, else 0
fn onstack_bit(pid: u64, sig: u32) -> u64 {
    if wants_altstack(pid, sig) { DISP_ONSTACK } else { 0 }
}

/// True when `sig`'s handler was installed with `SA_ONSTACK`. Process-wide,
/// like the disposition it rides beside
pub fn wants_altstack(pid: u64, sig: u32) -> bool {
    if sig == 0 || sig >= NSIG { return false; }
    crate::sched::sig_onstack_of(pid) & (1 << sig) != 0
}

fn set_onstack(pid: u64, sig: u32, on: bool) {
    let ptr = unsafe { crate::sched::leader_ptr(pid) };
    if ptr.is_null() { return; }
    let word = unsafe { &(*ptr).sig_onstack };
    if on {
        word.fetch_or(1 << sig, Ordering::Relaxed);
    } else {
        word.fetch_and(!(1 << sig), Ordering::Relaxed);
    }
}
