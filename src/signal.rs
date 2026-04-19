use core::sync::atomic::Ordering;

pub const SIGCHLD: u32 = 17;
pub const SIGTERM: u32 = 15;
pub const SIGKILL: u32 = 9;

/// Send a signal to a process by setting bit in pending_sig
pub fn send_signal(pid: u64, sig: u32) {
    if sig >= 32 { return; }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let ptr = unsafe { crate::scheduler::proc_index_raw(pid) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };
        p.pending_sig.fetch_or(1 << sig, Ordering::Relaxed);

        // SIGKILL/SIGTERM: immediately kill
        if sig == SIGKILL || sig == SIGTERM {
            crate::scheduler::kill(pid);
        }
    });
}

/// Send SIGCHLD to the parent of `child_pid`
pub fn send_sigchld(child_pid: u64) {
    let ppid = crate::scheduler::get_ppid(child_pid);
    if ppid != 0 {
        send_signal(ppid, SIGCHLD);
        // Wake parent in case it's blocking on wait4
        crate::scheduler::wakeup(ppid);
    }
}
