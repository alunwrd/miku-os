// The x87/SSE register file, as a thing a task owns
//
// `xmm0-15`, the x87 stack, and `MXCSR` are one set of registers per core.
// `software_context_switch` saves fifteen general-purpose registers and an iret
// frame; this file saves the rest. The kernel is compiled for
// `x86_64-unknown-none` (`-mmx,-sse,...,+soft-float`), so kernel code never
// touches `xmm`; the app target has SSE2 on, and every program uses `xmm`.
//
// FXSAVE, not XSAVE. `CR4.OSXSAVE` (bit 18) is not set, only `OSFXSR` (9) and
// `OSXMMEXCPT` (10) in `kernel/main.rs` and `smp.rs`, so `XGETBV`/`XSETBV` are
// unavailable and AVX cannot be entered. The state that can exist is the 512
// bytes FXSAVE writes. Turning `OSXSAVE` on without changing this file loses
// the upper halves of the vector registers on every switch.
//
// Eager, not lazy. `CR0.TS` plus `#NM` would skip the save for tasks that never
// touch the FPU, at the cost of a fault handler that has to be right under SMP,
// and the pair costs on the order of a hundred cycles. `interrupts.rs` has an
// `nm_handler`.

use core::cell::UnsafeCell;

/// FXSAVE's area is 512 bytes, 16-byte aligned or the instruction faults
pub const FXSAVE_LEN: usize = 512;

/// The state a task starts with: what `FNINIT` leaves, spelled out.
///
/// Zeroing is wrong: `MXCSR` 0 is legal and means every SSE exception unmasked,
/// so the first denormal or inexact result raises `#XM`. The architectural
/// defaults are `FCW = 0x037F` (extended precision, round to nearest, all six
/// x87 exceptions masked) and `MXCSR = 0x1F80` (all six SSE exceptions masked,
/// no flush-to-zero).
///
/// `MXCSR_MASK` at offset 28 is written by FXSAVE and ignored by FXRSTOR, so
/// the value only has to be legal. `0x0000FFBF` is what every processor since
/// the PIII reports.
const INIT_IMAGE: [u8; FXSAVE_LEN] = {
    let mut a = [0u8; FXSAVE_LEN];
    a[0] = 0x7F; a[1] = 0x03;               // FCW   = 0x037F
    a[24] = 0x80; a[25] = 0x1F;             // MXCSR = 0x00001F80
    a[28] = 0xBF; a[29] = 0xFF;             // MXCSR_MASK = 0x0000FFBF
    a
};

/// One task's x87/SSE registers.
///
/// `UnsafeCell` because the scheduler holds `&Process`, reached through a raw
/// pointer out of `PROC_INDEX`, while FXSAVE writes. Only the CPU running a
/// task touches that task's area: the scheduler saves into the outgoing task's
/// area before another CPU may resume it (`on_cpu` is released after the stack
/// switch), and restores from the incoming task's area after that CPU has
/// confirmed nobody else is on it.
#[repr(C, align(16))]
pub struct FpuState(UnsafeCell<[u8; FXSAVE_LEN]>);

// SAFETY: see the type's doc comment; access is serialised by `on_cpu`
unsafe impl Sync for FpuState {}
unsafe impl Send for FpuState {}

impl FpuState {
    /// A fresh register file
    pub const fn new() -> Self {
        FpuState(UnsafeCell::new(INIT_IMAGE))
    }

    /// A copy of what is in the registers now, for `fork`: the child continues
    /// from the parent's arithmetic state
    pub fn from_current() -> Self {
        let s = FpuState::new();
        // SAFETY: nothing else can hold this area, it was made a line ago
        unsafe { s.save() };
        s
    }

    /// Put this task's registers back on the CPU as they were.
    ///
    /// # Safety
    /// The caller must own the task this area belongs to: either it is the
    /// task about to run on this CPU, or nothing else can be running it.
    #[inline(always)]
    pub unsafe fn restore(&self) {
        core::arch::asm!(
            "fxrstor [{}]",
            in(reg) self.0.get(),
            options(nostack, preserves_flags)
        );
    }

    /// Take the CPU's registers into this task's area.
    ///
    /// # Safety
    /// As `restore`: this must be the area of the task currently on this CPU.
    #[inline(always)]
    pub unsafe fn save(&self) {
        core::arch::asm!(
            "fxsave [{}]",
            in(reg) self.0.get(),
            options(nostack, preserves_flags)
        );
    }

    /// Back to the starting state, without allocating. For `exec`: a rounding
    /// mode or masked exception left behind would make one program depend on
    /// its predecessor
    pub fn reset(&self) {
        // SAFETY: as the type's doc comment, the running task's own area
        unsafe { core::ptr::write(self.0.get(), INIT_IMAGE) };
    }
}

impl Default for FpuState {
    fn default() -> Self { Self::new() }
}
