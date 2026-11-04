// Getting a signal into ring 3, and back out
//
// Three ways in, one frame (`frame.rs`) and one way out:
//
//   - `deliver_pending`: an asynchronous signal, at the tail of a syscall.
//     The interrupted context is what the syscall saved; the redirect is a
//     different `sysret` target
//   - `deliver_fault`: a signal the CPU raised, from `trap::user_trap`, with
//     the whole register file in a `TrapFrame`
//   - `sigreturn_full`: leaving a handler, through `iretq`, and taking the
//     next pending signal on the way out if there is one
//
// The frame goes on the task's alternate stack when the handler asked for that
// and the task is not already on it; otherwise below the interrupted RSP, past
// the red zone. See dev/signal-architecture.md.

use core::sync::atomic::Ordering;

use super::frame::{self, Info, Stack};
use crate::arch::x86_64::trap::TrapFrame;

/// The 128 bytes below RSP a leaf function may use without moving RSP. A
/// fault can land in the middle of one
const RED_ZONE: u64 = 128;

/// Cleared on the way into a handler: a single-step or a direction flag left
/// by the interrupted code is not the handler's, and the ABI promises DF=0 at
/// every call. AC is SMAP's and never a user program's to hand on
const RFLAGS_HANDLER_CLEAR: u64 = (1 << 8) | (1 << 10) | (1 << 18);

/// `sigaltstack` refuses anything smaller: Linux's `MINSIGSTKSZ`
const MIN_ALT_SIZE: u64 = 2048;

/// The running task's alternate stack, `(lowest address, size)`
pub fn current_altstack() -> (u64, u64) {
    let ptr = unsafe { crate::sched::proc_index_raw(crate::sched::current_pid()) };
    if ptr.is_null() {
        return (0, 0);
    }
    let p = unsafe { &*ptr };
    (p.sig_alt_sp.load(Ordering::Relaxed), p.sig_alt_size.load(Ordering::Relaxed))
}

/// True when `rsp` is inside the alternate stack `(sp, size)`: Linux's test, so
/// an empty stack's top counts as on it
fn on_stack(rsp: u64, sp: u64, size: u64) -> bool {
    size != 0 && rsp > sp && rsp - sp <= size
}

/// Where the frame for `sig` goes, given the interrupted RSP; and what
/// `uc_stack` should say about the alternate stack
fn place(pid: u64, sig: u32, rsp: u64) -> (u64, Stack) {
    let (sp, size) = current_altstack();
    let on = on_stack(rsp, sp, size);
    let use_alt = size != 0 && !on && super::wants_altstack(pid, sig);
    let base = if use_alt { sp + size } else { rsp.wrapping_sub(RED_ZONE) };
    let at = base.wrapping_sub(frame::FRAME_SIZE) & !0xF;
    let flags = if size == 0 {
        frame::SS_DISABLE
    } else if on {
        frame::SS_ONSTACK
    } else {
        0
    };
    (at, Stack { sp, size, flags })
}

/// Write the frame for `info` onto the context in `tf` and point `tf` at the
/// entry stub. False, with `tf` untouched, if the frame could not be written
fn push(tf: &mut TrapFrame, pid: u64, entry: u64, info: &Info, cr2: u64, uc_flags: u64) -> bool {
    let (at, stack) = place(pid, info.sig, tf.rsp);
    let g = frame::gregs_from_trap(tf, cr2);
    if !frame::write(at, info, &g, uc_flags, &stack) {
        return false;
    }
    tf.rip = entry;
    tf.rsp = at;
    tf.rflags &= !RFLAGS_HANDLER_CLEAR;
    true
}

/// The next pending asynchronous signal onto `tf`, if the task can take one
/// now. `tf` is left alone otherwise
fn deliver_onto(tf: &mut TrapFrame) {
    let pid = crate::sched::current_pid();
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return; }
    let p = unsafe { &*ptr };

    let entry = p.sig_entry.load(Ordering::Relaxed);
    if entry == 0 || p.sig_in_delivery.load(Ordering::Relaxed) {
        return;
    }
    let pending = p.pending_sig.load(Ordering::Relaxed);
    if pending == 0 {
        return;
    }
    let sig = pending.trailing_zeros();
    p.pending_sig.fetch_and(!(1u32 << sig), Ordering::Relaxed);

    let info = Info { sig, code: 0, addr: 0 };
    if !push(tf, pid, entry, &info, 0, 0) {
        // Can't build a frame (stack blown?): fall back to the default action
        crate::serial_println!("[signal] pid={} sig={} frame unwritable, killing", pid, sig);
        crate::sched::kill_by_signal(pid, sig);
        return;
    }
    p.sig_in_delivery.store(true, Ordering::Relaxed);
}

/// Tail of every syscall: if the current process has a pending signal and
/// a registered dispatch entry, rewrite the sysret context so it lands in
/// the entry stub instead of the interrupted code. `ret` is the syscall's
/// return value; the frame keeps it as RAX and `sigreturn` puts it back
pub fn deliver_pending(ret: u64) -> u64 {
    let pid = crate::sched::current_pid();
    if !super::has_deliverable(pid) {
        return ret;
    }
    // With interrupts off, all of it. A syscall that slept comes back here with
    // them on, and the timer could move this task to another processor between
    // reading the per-CPU `user_rsp` below and writing it back: the frame would
    // go onto the stack the other CPU's slot names, a sibling thread's, and two
    // threads would share one stack. Nothing below sleeps: a stack page is
    // filled or copied with interrupts off, as at the start of any syscall
    x86_64::instructions::interrupts::without_interrupts(|| deliver_pending_here(ret))
}

fn deliver_pending_here(ret: u64) -> u64 {

    // The interrupted user context, from where `syscall_handler` left it:
    // RCX (the user RIP) at the kernel stack top - 8, R11 (RFLAGS) at - 16,
    // then RBP, RBX, R12-R15, R10, R9, R8, RDI, RSI, RDX; RSP in the per-CPU
    // slot. The last three are what the program called with, and `syscall`
    // keeps them, so the frame says so and `sigreturn` puts them back
    let cpu = crate::arch::x86_64::percpu::current();
    let ktop: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[0x10]", out(reg) ktop, options(nostack, preserves_flags));
    }
    let at = |off: u64| unsafe { *((ktop - off) as *const u64) };
    let mut tf = TrapFrame {
        rax: ret,
        rcx: at(8),
        r11: at(16),
        rbp: at(24),
        rbx: at(32),
        r12: at(40),
        r13: at(48),
        r14: at(56),
        r15: at(64),
        r10: at(72),
        r9: at(80),
        r8: at(88),
        rdi: at(96),
        rsi: at(104),
        rdx: at(112),
        rip: at(8),
        rflags: at(16),
        rsp: cpu.user_rsp.load(Ordering::Relaxed),
        cs: crate::arch::x86_64::gdt::user_code_selector().0 as u64,
        ss: crate::arch::x86_64::gdt::user_data_selector().0 as u64,
        ..TrapFrame::default()
    };
    let before = tf.rip;
    deliver_onto(&mut tf);
    if tf.rip != before {
        unsafe {
            *((ktop - 8) as *mut u64) = tf.rip;     // sysret RIP -> entry stub
            *((ktop - 16) as *mut u64) = tf.rflags;
        }
        cpu.user_rsp.store(tf.rsp, Ordering::Relaxed);
    }
    // rax entering the stub is the syscall return value; the stub ignores
    // it and sigreturn restores it from the frame
    ret
}

/// A signal the CPU raised for the instruction at `tf.rip`. True when a handler
/// will run; false means nobody asked for it and it is a crash.
///
/// Delivered whether or not a handler is already running: a handler that faults
/// has faulted, and the stack bounds a loop of them: a frame that cannot be
/// written is the crash
pub fn deliver_fault(tf: &mut TrapFrame, sig: u32, code: i32, addr: u64, cr2: u64) -> bool {
    let pid = crate::sched::current_pid();
    if super::disposition(pid, sig) != super::DISP_CATCH {
        return false;
    }
    let entry = crate::sched::sig_entry_of(pid);
    if entry == 0 {
        return false;
    }
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() { return false; }
    let p = unsafe { &*ptr };

    let was = p.sig_in_delivery.load(Ordering::Relaxed);
    let uc_flags = frame::UC_MIKU_FAULT | if was { frame::UC_MIKU_WAS_DELIVERING } else { 0 };
    let info = Info { sig, code, addr };
    if !push(tf, pid, entry, &info, cr2, uc_flags) {
        crate::serial_println!("[signal] pid={} sig={} fault frame unwritable at rsp={:#x}", pid, sig, tf.rsp);
        return false;
    }
    p.sig_in_delivery.store(true, Ordering::Relaxed);
    true
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
    crate::trace_println!("[signal] pid={} sigentry={:#x}", pid, addr);
    0
}

/// `sigreturn`(69): leaves a handler. Called from `syscall_handler`'s own path
/// for this one number, with `tf` on the kernel stack to be filled and left
/// through with `iretq`, the only way to hand back RCX and R11, which `sysret`
/// overwrites.
///
/// The frame is the program's memory and may hold anything, so what it asks for
/// is checked: RIP and RSP in the user half, RFLAGS status bits only with IF
/// on, and the user selectors whatever it says
pub extern "C" fn sigreturn_full(frame_ptr: u64, tf: &mut TrapFrame) {
    let pid = crate::sched::current_pid();
    let user_max = crate::syscall::usercopy::USER_MAX;
    let restored = frame::read(frame_ptr).filter(|(g, _)| {
        g[frame::reg::RIP] <= user_max && g[frame::reg::RSP] <= user_max
    });
    let (g, flags) = match restored {
        Some(v) => v,
        None => {
            // Nothing sane to return to. `iretq` to a non-canonical RIP would
            // fault in ring 0, so this does not return at all
            crate::serial_println!("[signal] pid={} sigreturn with a bad frame at {:#x}", pid, frame_ptr);
            crate::sched::kill_by_signal(pid, super::SIGSEGV);
            loop {
                crate::sched::yield_now();
            }
        }
    };

    frame::trap_from_gregs(&g, tf);
    tf.rflags = (tf.rflags & 0xCD5) | 0x202;
    tf.cs = crate::arch::x86_64::gdt::user_code_selector().0 as u64;
    tf.ss = crate::arch::x86_64::gdt::user_data_selector().0 as u64;
    tf.vector = 0;
    tf.error = 0;

    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if !ptr.is_null() {
        let was = flags & frame::UC_MIKU_WAS_DELIVERING != 0;
        unsafe { (*ptr).sig_in_delivery.store(was, Ordering::Relaxed) };
    }

    // Whatever queued up while the handler ran goes now, onto the context just
    // restored, as at the tail of any other syscall
    deliver_onto(tf);
}

/// `sigaltstack`(115): `(new *stack_t, old *stack_t)`, either may be null.
/// `stack_t` is Linux's: `ss_sp` at 0, `ss_flags` (an int) at 8, `ss_size`
/// at 16
pub fn sys_sigaltstack(new_ptr: u64, old_ptr: u64) -> u64 {
    use crate::syscall::errno::{err, EFAULT, EINVAL, ENOMEM, EPERM, ESRCH};
    use crate::syscall::usercopy;

    let pid = crate::sched::current_pid();
    let ptr = unsafe { crate::sched::proc_index_raw(pid) };
    if ptr.is_null() {
        return err(ESRCH);
    }
    let p = unsafe { &*ptr };
    let cr3 = usercopy::current_cr3();

    // Read the new one before writing the old: they may be the same memory
    let new = if new_ptr != 0 {
        let _ua = match usercopy::user_ptr_mapped(cr3, new_ptr, 24) {
            Some(g) => g,
            None => return err(EFAULT),
        };
        unsafe {
            let b = new_ptr as *const u8;
            Some((
                (b as *const u64).read_unaligned(),
                (b.add(8) as *const u32).read_unaligned(),
                (b.add(16) as *const u64).read_unaligned(),
            ))
        }
    } else {
        None
    };

    let sp = p.sig_alt_sp.load(Ordering::Relaxed);
    let size = p.sig_alt_size.load(Ordering::Relaxed);
    let rsp = crate::arch::x86_64::percpu::current().user_rsp.load(Ordering::Relaxed);
    let on = on_stack(rsp, sp, size);

    // Decide before touching anything, so a refused change leaves `old`
    // unwritten as well
    let change = match new {
        None => None,
        Some(_) if on => return err(EPERM),
        Some((_, flags, _)) if flags & frame::SS_DISABLE != 0 => Some((0, 0)),
        // `SS_ONSTACK` is accepted and means nothing, as on Linux; anything
        // else (`SS_AUTODISARM` among it) is not something this kernel does
        Some((_, flags, _)) if flags & !frame::SS_ONSTACK != 0 => return err(EINVAL),
        Some((_, _, nsize)) if nsize < MIN_ALT_SIZE => return err(ENOMEM),
        Some((nsp, _, nsize)) => match nsp.checked_add(nsize) {
            Some(end) if nsp != 0 && end <= usercopy::USER_MAX + 1 => Some((nsp, nsize)),
            _ => return err(EINVAL),
        },
    };

    if old_ptr != 0 {
        let _ua = match usercopy::user_ptr_writable(cr3, old_ptr, 24) {
            Some(g) => g,
            None => return err(EFAULT),
        };
        let flags = if size == 0 {
            frame::SS_DISABLE
        } else if on {
            frame::SS_ONSTACK
        } else {
            0
        };
        unsafe {
            let b = old_ptr as *mut u8;
            (b as *mut u64).write_unaligned(sp);
            (b.add(8) as *mut u32).write_unaligned(flags);
            (b.add(12) as *mut u32).write_unaligned(0);
            (b.add(16) as *mut u64).write_unaligned(size);
        }
    }

    if let Some((nsp, nsize)) = change {
        p.sig_alt_sp.store(nsp, Ordering::Relaxed);
        p.sig_alt_size.store(nsize, Ordering::Relaxed);
    }
    0
}
