// Faults from ring 3, and the frame they share with `sigreturn`
//
// A CPU exception raised by user code means a page it has not touched yet, a
// copy of a page it shares, or else a signal. Every one gets the whole register
// file, since a signal handler may read and write any register (see
// dev/signal-architecture.md).
//
// Each vector that can become a signal gets a naked stub here, in its IDT gate
// in place of the `x86-interrupt` handler:
//
//   - from ring 0 the stub drops what it pushed and jumps to the old handler,
//     which sees the frame the CPU built
//   - from ring 3 it swaps GS, pushes the general registers to complete a
//     `TrapFrame`, and moves that frame to the top of the task's own kernel
//     stack before calling anything. `#PF` and `#GP` arrive on an IST stack,
//     which every nested entry restarts from the top: a user fault that slept
//     there (reading a file-backed page) could be overwritten by the next task
//     to fault on the same CPU. On the task's own stack a user fault may sleep,
//     take a nested `#PF`, or be preempted
//
// `sigreturn` builds and leaves through `TrapFrame` too.

use x86_64::structures::idt::PageFaultErrorCode;

use crate::process::crash::{self, Cause, Regs};
use crate::process::signal;

/// The user context of a fault, in the order the stubs push it: the fifteen
/// general registers (RAX last pushed, so first here), the vector and error
/// code, then the CPU's own interrupt frame. 22 words
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct TrapFrame {
    pub rax: u64,
    pub rbx: u64,
    pub rcx: u64,
    pub rdx: u64,
    pub rsi: u64,
    pub rdi: u64,
    pub rbp: u64,
    pub r8:  u64,
    pub r9:  u64,
    pub r10: u64,
    pub r11: u64,
    pub r12: u64,
    pub r13: u64,
    pub r14: u64,
    pub r15: u64,
    pub vector: u64,
    pub error:  u64,
    pub rip:    u64,
    pub cs:     u64,
    pub rflags: u64,
    pub rsp:    u64,
    pub ss:     u64,
}

pub const TRAP_FRAME_SIZE: usize = core::mem::size_of::<TrapFrame>();
const _: () = assert!(TRAP_FRAME_SIZE == 22 * 8);
// The stubs address the saved CS as [rsp + 24] after pushing vector and error
const _: () = assert!(core::mem::offset_of!(TrapFrame, cs) - core::mem::offset_of!(TrapFrame, vector) == 24);

/// A stub for a vector the CPU pushes no error code for: push a zero in its
/// place so every `TrapFrame` has the same shape
macro_rules! trap_stub {
    ($name:ident, $vector:expr, $kernel:path) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "push 0",
                "push {vector}",
                // vector(+0) error(+8) rip(+16) cs(+24)
                "test byte ptr [rsp + 24], 3",
                "jnz {user}",
                "add rsp, 16",
                "jmp {kernel}",
                vector = const $vector,
                user = sym user_trap_entry,
                kernel = sym $kernel,
            )
        }
    };
}

/// The same, for a vector whose error code the CPU has already pushed
macro_rules! trap_stub_err {
    ($name:ident, $vector:expr, $kernel:path) => {
        #[unsafe(naked)]
        pub unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                "push {vector}",
                "test byte ptr [rsp + 24], 3",
                "jnz {user}",
                "add rsp, 8",
                "jmp {kernel}",
                vector = const $vector,
                user = sym user_trap_entry,
                kernel = sym $kernel,
            )
        }
    };
}

trap_stub!(divide_error_stub, 0, crate::arch::x86_64::interrupts::divide_error_handler);
trap_stub!(invalid_opcode_stub, 6, crate::arch::x86_64::interrupts::ud_handler);
trap_stub_err!(general_protection_stub, 13, crate::arch::x86_64::interrupts::gpf_handler);
trap_stub_err!(page_fault_stub, 14, crate::arch::x86_64::interrupts::page_fault_handler);

/// The ring-3 half every stub jumps to, with vector and error code pushed
#[unsafe(naked)]
unsafe extern "C" fn user_trap_entry() {
    core::arch::naked_asm!(
        "swapgs",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rbp",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        // The direction flag is the program's, and a program can stop in the
        // middle of a copy running backwards (`std; rep movsq`, an overlapping
        // memmove). An exception does not clear it (`syscall` does, through
        // SFMASK), and every `rep movs` the kernel runs, this frame's own copy
        // and a copy-on-write page included, would go backwards. `iretq` gives
        // the program its flag back
        "cld",
        // Off the IST, onto the task's own kernel stack
        "mov rdi, rsp",
        "call {to_task_stack}",
        "mov rsp, rax",
        "mov rdi, rsp",
        "call {user_trap}",
        // `user_trap` may have slept and come back with interrupts on. From the
        // `swapgs` below to `iretq` this CPU is in ring 0 with the user's GS,
        // and an interrupt there would run the kernel on it
        "cli",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop rbp",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "add rsp, 16",
        "swapgs",
        "iretq",
        to_task_stack = sym to_task_stack,
        user_trap = sym user_trap,
    )
}

/// Move a `TrapFrame` to the top of the running task's kernel stack and
/// answer with its new address. For a gate without an IST the CPU already
/// put it there, and the copy is onto itself
extern "C" fn to_task_stack(frame: *const TrapFrame) -> u64 {
    let top = crate::arch::x86_64::percpu::current()
        .kernel_rsp
        .load(core::sync::atomic::Ordering::Relaxed);
    let dst = ((top & !0xF) - TRAP_FRAME_SIZE as u64) as *mut TrapFrame;
    unsafe { core::ptr::copy(frame, dst, 1) };
    dst as u64
}

/// A fault from ring 3, on the task's own stack, with every register in `f`.
/// Resolved, delivered as a signal, or the end of the process
extern "C" fn user_trap(f: &mut TrapFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    // Before anything that could take a fault of its own
    let cr2: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) cr2, options(nomem, nostack, preserves_flags)) };
    let pid = crate::sched::current_pid();

    let (sig, code, addr, cause) = match f.vector {
        0 => (signal::SIGFPE, signal::FPE_INTDIV, f.rip, Cause::DivideError),
        6 => (signal::SIGILL, signal::ILL_ILLOPN, f.rip, Cause::InvalidOpcode),
        13 => (signal::SIGSEGV, signal::SI_KERNEL, 0, Cause::GeneralProtection { error_code: f.error }),
        14 => {
            let ec = PageFaultErrorCode::from_bits_truncate(f.error);
            let cr3 = crate::syscall::usercopy::current_cr3();
            if crate::arch::x86_64::interrupts::resolve_page_fault(cr3, cr2, ec, true) {
                return;
            }
            let present = ec.contains(PageFaultErrorCode::PROTECTION_VIOLATION);
            let code = if present { signal::SEGV_ACCERR } else { signal::SEGV_MAPERR };
            let cause = Cause::PageFault {
                addr: cr2,
                write: ec.contains(PageFaultErrorCode::CAUSED_BY_WRITE),
                present,
                exec: ec.contains(PageFaultErrorCode::INSTRUCTION_FETCH),
            };
            (signal::SIGSEGV, code, cr2, cause)
        }
        v => {
            // A stub was installed for a vector this match does not know,
            // which is a bug here and not in the program
            panic!("user_trap: no signal for vector {}", v);
        }
    };

    if signal::deliver_fault(f, sig, code, addr, cr2) {
        return;
    }

    // Nobody asked for it: the crash, with the real registers, and the whole
    // process dies of the signal
    match f.vector {
        14 => {
            crate::serial_println!("[page fault] addr={:#x} code={:#x} user=true", cr2, f.error);
            crate::arch::x86_64::interrupts::dump_pte(crate::syscall::usercopy::current_cr3(), cr2);
        }
        13 => crate::cprintln!(255, 60, 60,
            "[gpf] code={} rip={:#x} rsp={:#x}", f.error, f.rip, f.rsp),
        _ => {}
    }
    let regs = Regs { rip: f.rip, rsp: f.rsp, rbp: f.rbp, rflags: f.rflags, cs: f.cs };
    crash::report_user(pid, cause, &regs);
    crate::process::coredump::on_crash(pid, &regs);
    crate::serial_println!("[crash] killing pid={}", pid);
    crate::sched::kill_by_signal(pid, sig);
    // A dead task is never picked again, so this does not come back
    loop {
        crate::sched::yield_now();
    }
}
