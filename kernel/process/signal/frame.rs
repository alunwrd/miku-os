// The frame a signal handler runs on: an ABI, and this is its only writer
//
// Every delivery writes one of these onto the user stack, and the libmiku entry
// stub starts with RSP pointing at it:
//
//   +0      siginfo_t    128   si_signo, si_errno, si_code; si_addr at +16
//   +128    ucontext_t   424   Linux's layout up to the end of uc_sigmask:
//             +0   uc_flags          UC_MIKU_* below
//             +8   uc_link           0
//             +16  uc_stack          ss_sp, ss_flags (+8), ss_size (+16)
//             +40  uc_mcontext.gregs[23], in Linux's REG_* order
//             +224 uc_mcontext.fpregs   -> the fpstate below
//             +232 reserved[8]
//             +296 uc_sigmask        128 bytes, zero
//   +552    pad            8
//   +560    fpstate      512   filled and restored by the stub (fxsave64)
//   +1072   end
//
// Linux's shape so a crate reading a machine context (wasmtime first) needs
// `target_os = "miku"` added to a cfg rather than a new arm. The reader's side
// is `lib/rust-libc/libc/src/unix/miku/mod.rs`, and the two have to agree byte
// for byte. See dev/signal-architecture.md.

use crate::arch::x86_64::trap::TrapFrame;
use crate::syscall::usercopy;

pub const UC_OFF: u64 = 128;
pub const FPSTATE_OFF: u64 = 560;
pub const FRAME_SIZE: u64 = 1072;
const _: () = assert!(FPSTATE_OFF % 16 == 0 && FRAME_SIZE % 16 == 0);

// Inside ucontext_t
const UC_FLAGS: u64 = 0;
const UC_STACK: u64 = 16;
const UC_GREGS: u64 = 40;
const UC_FPREGS: u64 = 224;

/// `uc_flags`: the kernel raised this for an instruction that faulted. The stub
/// must not return from such a frame without a handler having run: the
/// instruction would fault into the same frame for ever
pub const UC_MIKU_FAULT: u64 = 1;
/// `uc_flags`: the task was already running a handler when this frame was
/// written. `sigreturn` puts that back, so a fault inside an asynchronous
/// handler returns to a handler still marked as running
pub const UC_MIKU_WAS_DELIVERING: u64 = 1 << 32;

/// `ss_flags` values, Linux's
pub const SS_ONSTACK: u32 = 1;
pub const SS_DISABLE: u32 = 2;

/// Linux's `gregs` indices
pub mod reg {
    pub const R8: usize = 0;
    pub const R9: usize = 1;
    pub const R10: usize = 2;
    pub const R11: usize = 3;
    pub const R12: usize = 4;
    pub const R13: usize = 5;
    pub const R14: usize = 6;
    pub const R15: usize = 7;
    pub const RDI: usize = 8;
    pub const RSI: usize = 9;
    pub const RBP: usize = 10;
    pub const RBX: usize = 11;
    pub const RDX: usize = 12;
    pub const RAX: usize = 13;
    pub const RCX: usize = 14;
    pub const RSP: usize = 15;
    pub const RIP: usize = 16;
    pub const EFL: usize = 17;
    pub const CSGSFS: usize = 18;
    pub const ERR: usize = 19;
    pub const TRAPNO: usize = 20;
    pub const OLDMASK: usize = 21;
    pub const CR2: usize = 22;
}
pub const NGREG: usize = 23;

pub type Gregs = [u64; NGREG];

/// What `siginfo_t` says
pub struct Info {
    pub sig: u32,
    pub code: i32,
    pub addr: u64,
}

/// What `uc_stack` says: the task's alternate stack as `sigaltstack` would
/// report it at the moment of delivery
pub struct Stack {
    pub sp: u64,
    pub size: u64,
    pub flags: u32,
}

pub fn gregs_from_trap(tf: &TrapFrame, cr2: u64) -> Gregs {
    let mut g = [0u64; NGREG];
    g[reg::R8] = tf.r8;
    g[reg::R9] = tf.r9;
    g[reg::R10] = tf.r10;
    g[reg::R11] = tf.r11;
    g[reg::R12] = tf.r12;
    g[reg::R13] = tf.r13;
    g[reg::R14] = tf.r14;
    g[reg::R15] = tf.r15;
    g[reg::RDI] = tf.rdi;
    g[reg::RSI] = tf.rsi;
    g[reg::RBP] = tf.rbp;
    g[reg::RBX] = tf.rbx;
    g[reg::RDX] = tf.rdx;
    g[reg::RAX] = tf.rax;
    g[reg::RCX] = tf.rcx;
    g[reg::RSP] = tf.rsp;
    g[reg::RIP] = tf.rip;
    g[reg::EFL] = tf.rflags;
    // Linux packs CS into the low 16 bits and SS into the top 16
    g[reg::CSGSFS] = (tf.cs & 0xFFFF) | ((tf.ss & 0xFFFF) << 48);
    g[reg::ERR] = tf.error;
    g[reg::TRAPNO] = tf.vector;
    g[reg::CR2] = cr2;
    g
}

/// The general registers, RIP, RSP and RFLAGS from `g` into `tf`, as they are.
/// Selectors and the RFLAGS sanitising are the caller's: `sigreturn` decides
/// what a frame may ask for
pub fn trap_from_gregs(g: &Gregs, tf: &mut TrapFrame) {
    tf.r8 = g[reg::R8];
    tf.r9 = g[reg::R9];
    tf.r10 = g[reg::R10];
    tf.r11 = g[reg::R11];
    tf.r12 = g[reg::R12];
    tf.r13 = g[reg::R13];
    tf.r14 = g[reg::R14];
    tf.r15 = g[reg::R15];
    tf.rdi = g[reg::RDI];
    tf.rsi = g[reg::RSI];
    tf.rbp = g[reg::RBP];
    tf.rbx = g[reg::RBX];
    tf.rdx = g[reg::RDX];
    tf.rax = g[reg::RAX];
    tf.rcx = g[reg::RCX];
    tf.rsp = g[reg::RSP];
    tf.rip = g[reg::RIP];
    tf.rflags = g[reg::EFL];
}

/// Writes a whole frame at `at`, which is 16-aligned. False when the memory is
/// not the program's to write: this delivery is then a crash
pub fn write(at: u64, info: &Info, g: &Gregs, uc_flags: u64, stack: &Stack) -> bool {
    let cr3 = usercopy::current_cr3();
    // Faults every page of the range in first, so nothing below takes a #PF
    let _ua = match usercopy::user_ptr_writable(cr3, at, FRAME_SIZE) {
        Some(g) => g,
        None => return false,
    };
    unsafe {
        let base = at as *mut u8;
        core::ptr::write_bytes(base, 0, FRAME_SIZE as usize);
        // siginfo_t
        (base as *mut i32).write(info.sig as i32);
        (base.add(8) as *mut i32).write(info.code);
        (base.add(16) as *mut u64).write(info.addr);
        // ucontext_t
        let uc = base.add(UC_OFF as usize);
        (uc.add(UC_FLAGS as usize) as *mut u64).write(uc_flags);
        (uc.add(UC_STACK as usize) as *mut u64).write(stack.sp);
        (uc.add(UC_STACK as usize + 8) as *mut u32).write(stack.flags);
        (uc.add(UC_STACK as usize + 16) as *mut u64).write(stack.size);
        let gregs = uc.add(UC_GREGS as usize) as *mut u64;
        for (i, v) in g.iter().enumerate() {
            gregs.add(i).write(*v);
        }
        (uc.add(UC_FPREGS as usize) as *mut u64).write(at + FPSTATE_OFF);
    }
    true
}

/// The registers and `uc_flags` a frame at `at` holds now, after the handler
/// has had its chance to change them. `None` if it is not readable
pub fn read(at: u64) -> Option<(Gregs, u64)> {
    if at & 0xF != 0 {
        return None;
    }
    let cr3 = usercopy::current_cr3();
    let _ua = usercopy::user_ptr_mapped(cr3, at, FRAME_SIZE)?;
    let mut g = [0u64; NGREG];
    let flags;
    unsafe {
        let uc = (at + UC_OFF) as *const u8;
        flags = (uc.add(UC_FLAGS as usize) as *const u64).read();
        let gregs = uc.add(UC_GREGS as usize) as *const u64;
        for (i, v) in g.iter_mut().enumerate() {
            *v = gregs.add(i).read();
        }
    }
    Some((g, flags))
}
