// Kernel-side signals
//
// Two halves:
//   - send_signal / send_sigchld: set a pending bit on the target (and
//     apply kernel default actions for processes with no userspace
//     dispatcher registered)
//   - deliver_pending: called at the tail of every syscall dispatch. If
//     the current process has pending signals and registered a dispatch
//     entry (sys_sigentry, from libmiku), the interrupted user context
//     (RIP/RSP/RAX/RFLAGS) is saved in a frame on the user stack and the
//     sysret target is redirected to the entry stub. The stub runs
//     miku_sigaction_dispatch and calls sys_sigreturn, which restores the
//     saved context. See MikuOS_ABI.md sec 6.
//
// Delivery happens only at syscall boundaries (v1): a CPU-bound loop that
// never makes syscalls will not see handlers run until its next syscall.
// Fatal defaults (SIGKILL/SIGTERM, and SIGINT/SIGQUIT without a handler
// entry) are applied in send_signal, so Ctrl+C still kills such loops.

use core::sync::atomic::Ordering;

pub const SIGHUP:  u32 = 1;
pub const SIGINT:  u32 = 2;
pub const SIGQUIT: u32 = 3;
pub const SIGKILL: u32 = 9;
pub const SIGTERM: u32 = 15;
pub const SIGCHLD: u32 = 17;

/// Signal frame the kernel writes on the user stack, consumed by the
/// libmiku entry stub and sys_sigreturn. Layout (from the new user RSP):
///   [rsp+0]  sig
///   [rsp+8]  saved RIP
///   [rsp+16] saved RSP
///   [rsp+24] saved RAX (syscall return value of the interrupted context)
///   [rsp+32] saved RFLAGS
pub const SIGFRAME_WORDS: u64 = 5;

/// Send a signal to a process by setting a bit in pending_sig
pub fn send_signal(pid: u64, sig: u32) {
    if sig >= 32 { return; }
    x86_64::instructions::interrupts::without_interrupts(|| {
        let ptr = unsafe { crate::sched::proc_index_raw(pid) };
        if ptr.is_null() { return; }
        let p = unsafe { &*ptr };

        // SIGKILL/SIGTERM: always fatal, not catchable here
        if sig == SIGKILL || sig == SIGTERM {
            crate::sched::kill(pid);
            return;
        }

        // SIGINT/SIGQUIT/SIGHUP default to terminate; only queue them if
        // the process registered a userspace dispatcher
        let catchable_fatal = sig == SIGINT || sig == SIGQUIT || sig == SIGHUP;
        if catchable_fatal && p.sig_entry.load(Ordering::Relaxed) == 0 {
            crate::sched::kill_with_code(pid, 128 + sig as u64);
            return;
        }

        p.pending_sig.fetch_or(1 << sig, Ordering::Relaxed);
    });
    // Kick the target out of a blocked syscall (wait4/sleep) so delivery
    // happens promptly rather than at the next natural wakeup
    crate::sched::wakeup(pid);
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

/// True if the current process has a queued signal that userspace wants
/// delivered. Blocking syscalls (wait4) use this to bail out with EINTR
/// so the dispatch tail can inject the handler without waiting forever
pub fn has_deliverable(pid: u64) -> bool {
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return false; }
    let p = unsafe { &*ptr };
    p.sig_entry.load(Ordering::Relaxed) != 0
        && !p.sig_in_delivery.load(Ordering::Relaxed)
        && p.pending_sig.load(Ordering::Relaxed) != 0
}

/// Tail of every syscall: if the current process has a pending signal and
/// a registered dispatch entry, rewrite the sysret context so it lands in
/// the entry stub instead of the interrupted code. `ret` is the syscall's
/// return value; it is preserved in the frame and restored by sigreturn
pub fn deliver_pending(ret: u64) -> u64 {
    let pid = crate::sched::current_pid();
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return ret; }
    let p = unsafe { &*ptr };

    let entry = p.sig_entry.load(Ordering::Relaxed);
    if entry == 0 || p.sig_in_delivery.load(Ordering::Relaxed) {
        return ret;
    }
    let pending = p.pending_sig.load(Ordering::Relaxed);
    if pending == 0 {
        return ret;
    }
    let sig = pending.trailing_zeros();
    p.pending_sig.fetch_and(!(1u32 << sig), Ordering::Relaxed);

    // Interrupted user context: RIP = saved rcx at kernel stack top - 8,
    // RFLAGS = saved r11 at top - 16 (see the syscall_handler prologue),
    // RSP = percpu user_rsp slot
    let cpu = crate::arch::x86_64::percpu::current();
    let ktop: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) ktop, options(nostack, preserves_flags));
    }
    let user_rip    = unsafe { *((ktop - 8)  as *const u64) };
    let user_rflags = unsafe { *((ktop - 16) as *const u64) };
    let user_rsp    = cpu.user_rsp.load(Ordering::Relaxed);

    // Frame goes below the current user stack pointer, 16-byte aligned
    let frame = user_rsp.wrapping_sub(SIGFRAME_WORDS * 8) & !0xF;
    let cr3 = crate::syscall::usercopy::current_cr3();
    if !crate::syscall::usercopy::user_ptr_writable(cr3, frame, SIGFRAME_WORDS * 8) {
        // Can't build a frame (stack blown?): fall back to default action
        crate::serial_println!("[signal] pid={} sig={} frame unwritable, killing", pid, sig);
        crate::sched::kill_with_code(pid, 128 + sig as u64);
        return ret;
    }
    unsafe {
        let f = frame as *mut u64;
        f.add(0).write(sig as u64);
        f.add(1).write(user_rip);
        f.add(2).write(user_rsp);
        f.add(3).write(ret);
        f.add(4).write(user_rflags);
    }

    p.sig_in_delivery.store(true, Ordering::Relaxed);
    unsafe {
        *((ktop - 8) as *mut u64) = entry; // sysret RIP -> entry stub
    }
    cpu.user_rsp.store(frame, Ordering::Relaxed);

    // rax entering the stub is the syscall return value; the stub ignores
    // it and sigreturn restores it from the frame
    ret
}

/// sys_sigentry(addr): register the userspace dispatch entry (68)
pub fn sys_sigentry(addr: u64) -> u64 {
    if addr == 0 || addr > crate::syscall::usercopy::USER_MAX {
        return crate::syscall::errno::err(crate::syscall::errno::EINVAL);
    }
    let pid = crate::sched::current_pid();
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() {
        return crate::syscall::errno::err(crate::syscall::errno::ESRCH);
    }
    unsafe { (*ptr).sig_entry.store(addr, Ordering::Relaxed); }
    crate::serial_println!("[signal] pid={} sigentry={:#x}", pid, addr);
    0
}

/// sys_sigreturn(frame_ptr): restore the context saved by deliver_pending
/// (69). Returns the interrupted syscall's return value so the epilogue
/// puts the original RAX back
pub fn sys_sigreturn(frame_ptr: u64) -> u64 {
    let pid = crate::sched::current_pid();
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() {
        return crate::syscall::errno::err(crate::syscall::errno::ESRCH);
    }
    let p = unsafe { &*ptr };

    let cr3 = crate::syscall::usercopy::current_cr3();
    if !crate::syscall::usercopy::user_ptr_mapped(cr3, frame_ptr, SIGFRAME_WORDS * 8) {
        // Bad frame: nothing to restore into; kill rather than sysret to garbage
        crate::sched::kill_with_code(pid, 128 + SIGQUIT as u64);
        return 0;
    }
    let (rip, rsp, rax, rflags) = unsafe {
        let f = frame_ptr as *const u64;
        (f.add(1).read(), f.add(2).read(), f.add(3).read(), f.add(4).read())
    };

    // Sanitize: user RIP/RSP must stay in the user half; RFLAGS keeps
    // status flags only, IF forced on
    let user_max = crate::syscall::usercopy::USER_MAX;
    if rip > user_max || rsp > user_max {
        crate::sched::kill_with_code(pid, 128 + SIGQUIT as u64);
        return 0;
    }
    let rflags = (rflags & 0xCD5) | 0x202;

    let ktop: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) ktop, options(nostack, preserves_flags));
        *((ktop - 8)  as *mut u64) = rip;
        *((ktop - 16) as *mut u64) = rflags;
    }
    crate::arch::x86_64::percpu::current().user_rsp.store(rsp, Ordering::Relaxed);
    p.sig_in_delivery.store(false, Ordering::Relaxed);

    // If more signals queued up meanwhile, the dispatch tail delivers the
    // next one right away (deliver_pending runs after this returns)
    rax
}
