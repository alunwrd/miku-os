use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

pub const PIC_1_OFFSET: u8  = 32;
pub const PIC_2_OFFSET: u8  = PIC_1_OFFSET + 8;
pub const PIT_HZ:       u32 = 250;

static TICK: AtomicU64 = AtomicU64::new(0);

/// Advance the wall-clock tick, on the boot CPU only.
///
/// Every CPU arms its own LAPIC timer at the same rate and they all land in
/// this handler, so a counter bumped by all of them runs at TIMER_HZ per CPU,
/// and everything measuring a duration in ticks comes up short by the CPU
/// count. The APs still take the interrupt, which preempts them.
/// Returns the new tick on the CPU that advanced it, None on every other CPU
#[inline(always)]
fn advance_tick() -> Option<u64> {
    // gs:[0] is installed by gdt::init (BSP) and smp::ap_entry step 1, both
    // strictly before that CPU's timer is armed
    if crate::arch::x86_64::percpu::current_index() == 0 {
        Some(TICK.fetch_add(1, Ordering::Relaxed) + 1)
    } else {
        None
    }
}

pub static ATA_PRIMARY_IRQ:   AtomicBool = AtomicBool::new(false);
pub static ATA_SECONDARY_IRQ: AtomicBool = AtomicBool::new(false);

pub fn get_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

// Diagnostic pixel-marker, a no-op in normal builds. The call sites in ISRs and
// early-boot diagnostics stay; restore the framebuffer-poking body to chase an
// early hang before serial output
#[inline(always)]
pub fn pixel_mark(_slot: usize, _r: u8, _g: u8, _b: u8) {}

/// Drive scheduling from the timer interrupt.
///
/// A tick taken in ring 3 must 'swapgs' on entry and again on exit, or every
/// 'percpu::current()' reads userspace memory. A mostly-kernel QEMU workload
/// can hide that and still wedge real hardware on the first hardware-delivered
/// vector. The stub below makes both swaps conditional on the saved CS.
///
/// If a real machine hangs right after the first tick, set this to 'false':
/// falls back to the compiler-generated handler and cooperative 'yield_now()'
/// scheduling
const PREEMPTIVE_TIMER: bool = true;

// Compiler-generated fallback used when PREEMPTIVE_TIMER is false. Counts ticks
// and does the per-tick bookkeeping, but cannot switch tasks: the compiler owns
// the prologue, so it cannot resume on a different stack
extern "x86-interrupt" fn timer_interrupt_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    let cpu = crate::arch::x86_64::percpu::current_index();
    let now = crate::kcore::time::now_ns();
    let tick = if crate::arch::x86_64::apic::timer_due(cpu, now) { advance_tick() } else { None };
    crate::arch::x86_64::apic::eoi();
    if crate::kcore::boot_state::is_done() {
        crate::sched::expire_sleepers(cpu, now);
        if tick.is_some_and(|t| t % 250 == 0) {
            crate::mm::swap_map::request_reclaim_from_isr();
        }
    }
    crate::arch::x86_64::apic::arm_timer(cpu);
}

// Preemptive timer interrupt
//
// Builds the frame 'schedule_from_isr' expects (15 GPRs in push order, rax
// last, on top of the CPU's iret frame), hands it the stack pointer, and
// continues on whatever stack the scheduler returns.
//
// 'yield_now()' is the only other way into the scheduler.
/// The entry for an interrupt that may switch tasks: the timer, and the
/// reschedule IPI another CPU sends when it has queued work here
macro_rules! switching_stub {
    ($name:ident, $entry:ident) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            core::arch::naked_asm!(
                // CPU pushed: rip(+0) cs(+8) rflags(+16) rsp(+24) ss(+32)
                "cmp qword ptr [rsp + 8], 0x08",
                "je 2f",
                "swapgs",
                "2:",
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
                // The interrupted program's direction flag, cleared for the
                // kernel as `user_trap_entry` does. `x86-interrupt` handlers
                // get this from the compiler; a naked stub says it
                "cld",
                "mov rdi, rsp",
                "and rsp, -16",
                "call {entry}",
                // rax = stack to resume on, rdx = &prev.on_cpu (0 if we stayed
                // put).
                // Must land after the rsp switch: see sched::core_sched
                "mov rsp, rax",
                "test rdx, rdx",
                "jz 4f",
                "mov byte ptr [rdx], 0",
                "4:",
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
                // Decide on the frame we are actually returning to, which after
                // a
                // switch belongs to a different task than the one we entered on
                "cmp qword ptr [rsp + 8], 0x08",
                "je 3f",
                "swapgs",
                "3:",
                "iretq",
                entry = sym $entry,
            )
        }
    };
}

switching_stub!(timer_interrupt_stub, timer_isr_entry);
switching_stub!(resched_interrupt_stub, resched_isr_entry);

/// Per-tick bookkeeping, then the scheduler. Returns the stack pointer to
/// resume on, the same one when nothing switches
#[no_mangle]
unsafe extern "C" fn timer_isr_entry(frame_rsp: u64) -> crate::sched::SwitchOut {
    let cpu = crate::arch::x86_64::percpu::current_index();
    let now = crate::kcore::time::now_ns();
    // A tick, or only a sleeper's deadline (`apic::timer_due`)
    let is_tick = crate::arch::x86_64::apic::timer_due(cpu, now);
    let tick = if is_tick { advance_tick() } else { None };
    crate::arch::x86_64::apic::eoi();

    // Before the scheduler exists there is nothing to switch to, but the timer
    // is one-shot: an interrupt that does not arm the next one is this CPU's
    // last
    if !crate::kcore::boot_state::is_done() {
        crate::arch::x86_64::apic::arm_timer(cpu);
        return crate::sched::SwitchOut { new_rsp: frame_rsp, release_on_cpu: 0 };
    }

    if tick.is_some_and(|t| t % 250 == 0) {
        // Reclaim does not run here (`mm::swap_map`). The tick only raises a
        // flag for kswapd
        crate::mm::swap_map::request_reclaim_from_isr();
    }

    // The sleepers whose time has come, onto run queues, and the timer armed
    // for the next of them or the next tick
    crate::sched::expire_sleepers(cpu, now);
    crate::arch::x86_64::apic::arm_timer(cpu);

    // The stub pushed fifteen registers over the CPU's frame: RIP at +15
    // words, CS at +16
    let cs = unsafe { *((frame_rsp + 16 * 8) as *const u64) };
    crate::sched::schedule_from_timer(frame_rsp, cs & 3 == 0, is_tick)
}

/// Another CPU queued work here (`sched::wakeup`). Not a tick: nobody is
/// charged and the clock does not move, but a chance to switch at once
#[no_mangle]
unsafe extern "C" fn resched_isr_entry(frame_rsp: u64) -> crate::sched::SwitchOut {
    crate::arch::x86_64::apic::eoi();
    if !crate::kcore::boot_state::is_done() {
        return crate::sched::SwitchOut { new_rsp: frame_rsp, release_on_cpu: 0 };
    }
    let cs = unsafe { *((frame_rsp + 16 * 8) as *const u64) };
    crate::sched::schedule_from_timer(frame_rsp, cs & 3 == 0, false)
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer    = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
    Mouse    = PIC_2_OFFSET + 4,
    AtaIrq14 = PIC_2_OFFSET + 6,
    AtaIrq15 = PIC_2_OFFSET + 7,
}

impl InterruptIndex {
    fn as_u8(self)    -> u8     { self as u8 }
    fn as_usize(self) -> usize  { usize::from(self.as_u8()) }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        // The four vectors a user program can turn into a signal enter through
        // `trap`'s naked stubs, which hand a ring-0 fault straight on to the
        // handler below and give a ring-3 one the whole register file
        unsafe {
            idt.divide_error.set_handler_addr(
                x86_64::VirtAddr::new(crate::arch::x86_64::trap::divide_error_stub as *const () as u64));
            idt.invalid_opcode.set_handler_addr(
                x86_64::VirtAddr::new(crate::arch::x86_64::trap::invalid_opcode_stub as *const () as u64));
        }
        idt.device_not_available.set_handler_fn(nm_handler);
        // NMI (vector 2) has to be handled. Real hardware delivers NMI from
        // sources we cannot mask: an ACPI MADT LAPIC NMI Source (type 4) often
        // sets LINT0/LINT1 to delivery_mode=NMI, where the LVT mask bit is
        // irrelevant; also PMI, watchdog, and platform RAS sources. Without an
        // IDT entry the first sti causes #GP, a recursive fault, then a triple
        // fault
        unsafe {
            idt.non_maskable_interrupt
                .set_handler_fn(nmi_handler)
                .set_stack_index(crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        // Machine check (vector 18) is also non-maskable. Same reasoning...
        unsafe {
            idt.machine_check
                .set_handler_fn(mce_handler)
                .set_stack_index(crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(crate::arch::x86_64::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.page_fault
                .set_handler_addr(x86_64::VirtAddr::new(
                    crate::arch::x86_64::trap::page_fault_stub as *const () as u64))
                .set_stack_index(crate::arch::x86_64::gdt::PAGE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.general_protection_fault
                .set_handler_addr(x86_64::VirtAddr::new(
                    crate::arch::x86_64::trap::general_protection_stub as *const () as u64))
                .set_stack_index(crate::arch::x86_64::gdt::PAGE_FAULT_IST_INDEX);
        }
        // The preemptive path needs a raw address: a naked stub, not an
        // `extern "x86-interrupt"` fn, since it may return on a different
        // task's stack
        if PREEMPTIVE_TIMER {
            unsafe {
                idt[InterruptIndex::Timer.as_usize()].set_handler_addr(
                    x86_64::VirtAddr::new(timer_interrupt_stub as *const () as usize as u64),
                );
            }
        } else {
            idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        }
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_usize()].set_handler_fn(mouse_interrupt_handler);
        idt[InterruptIndex::AtaIrq14.as_usize()].set_handler_fn(ata_irq14_handler);
        idt[InterruptIndex::AtaIrq15.as_usize()].set_handler_fn(ata_irq15_handler);
        idt[crate::arch::x86_64::apic::VEC_APIC_ERR as usize].set_handler_fn(apic_error_handler);
        idt[crate::arch::x86_64::apic::VEC_SPURIOUS as usize].set_handler_fn(apic_spurious_handler);
        if PREEMPTIVE_TIMER {
            unsafe {
                idt[crate::arch::x86_64::apic::VEC_IPI_RESCHED as usize].set_handler_addr(
                    x86_64::VirtAddr::new(resched_interrupt_stub as *const () as usize as u64),
                );
            }
        } else {
            idt[crate::arch::x86_64::apic::VEC_IPI_RESCHED as usize].set_handler_fn(ipi_resched_handler);
        }
        idt[crate::arch::x86_64::apic::VEC_IPI_TLB as usize].set_handler_fn(ipi_tlb_handler);
        idt[crate::arch::x86_64::apic::VEC_IPI_HALT as usize].set_handler_fn(ipi_halt_handler);

        // MSI vector stubs: 16 slots starting at VEC_MSI_BASE. Each stub
        // forwards to apic::msi_dispatch(slot) and sends EOI
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x0].set_handler_fn(msi_stub_0);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x1].set_handler_fn(msi_stub_1);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x2].set_handler_fn(msi_stub_2);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x3].set_handler_fn(msi_stub_3);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x4].set_handler_fn(msi_stub_4);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x5].set_handler_fn(msi_stub_5);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x6].set_handler_fn(msi_stub_6);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x7].set_handler_fn(msi_stub_7);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x8].set_handler_fn(msi_stub_8);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0x9].set_handler_fn(msi_stub_9);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xA].set_handler_fn(msi_stub_10);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xB].set_handler_fn(msi_stub_11);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xC].set_handler_fn(msi_stub_12);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xD].set_handler_fn(msi_stub_13);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xE].set_handler_fn(msi_stub_14);
        idt[(crate::arch::x86_64::apic::VEC_MSI_BASE as usize) + 0xF].set_handler_fn(msi_stub_15);
        idt
    };
}

// Per-slot MSI stubs. Each one knows its slot at compile time, so the
// dispatch table stays lock-free on the hot path
macro_rules! msi_stub {
    ($name:ident, $slot:expr) => {
        extern "x86-interrupt" fn $name(_: InterruptStackFrame) {
            // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
            crate::arch::x86_64::cpu::clac();
            crate::arch::x86_64::apic::msi_dispatch($slot);
            crate::arch::x86_64::apic::eoi();
        }
    };
}
msi_stub!(msi_stub_0,  0);
msi_stub!(msi_stub_1,  1);
msi_stub!(msi_stub_2,  2);
msi_stub!(msi_stub_3,  3);
msi_stub!(msi_stub_4,  4);
msi_stub!(msi_stub_5,  5);
msi_stub!(msi_stub_6,  6);
msi_stub!(msi_stub_7,  7);
msi_stub!(msi_stub_8,  8);
msi_stub!(msi_stub_9,  9);
msi_stub!(msi_stub_10, 10);
msi_stub!(msi_stub_11, 11);
msi_stub!(msi_stub_12, 12);
msi_stub!(msi_stub_13, 13);
msi_stub!(msi_stub_14, 14);
msi_stub!(msi_stub_15, 15);

extern "x86-interrupt" fn apic_spurious_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    // No EOI for spurious
}

extern "x86-interrupt" fn apic_error_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    // Minimal: serial may not exist on real hardware, and any lock (like the
    // framebuffer console) from interrupt context risks deadlock. Latch + clear
    // ESR and EOI; count errors via an atomic for the shell diagnostics
    use core::sync::atomic::{AtomicU32, Ordering};
    static ERR_COUNT: AtomicU32 = AtomicU32::new(0);
    unsafe {
        crate::arch::x86_64::apic::lapic_write(crate::arch::x86_64::apic::LAPIC_ESR, 0);
        let _esr = crate::arch::x86_64::apic::lapic_read(crate::arch::x86_64::apic::LAPIC_ESR);
    }
    ERR_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ipi_resched_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    crate::arch::x86_64::apic::eoi();
    // schedule hint - next timer tick will preempt if needed
}

extern "x86-interrupt" fn ipi_tlb_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    crate::mm::tlb::on_ipi();
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ipi_halt_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    crate::arch::x86_64::apic::eoi();
    x86_64::instructions::interrupts::disable();
    loop { x86_64::instructions::hlt(); }
}

pub fn init_idt() {
    crate::trace_println!("[int] loading idt");
    IDT.load();
    crate::trace_println!("[int] idt loaded");
}

/// Early-boot diagnostic: print the raw IDT gate the CPU will use.  Keeping
/// this next to the table makes it possible to distinguish a bad gate from an
/// interrupt source problem before IF is enabled
pub fn dump_gate(vector: u8) {
    let idt = &*IDT as *const InterruptDescriptorTable as *const u64;
    unsafe {
        crate::trace_println!(
            "[int] idt[{:#04x}] = {:#018x} {:#018x}",
            vector,
            core::ptr::read_unaligned(idt.add(vector as usize * 2)),
            core::ptr::read_unaligned(idt.add(vector as usize * 2 + 1)),
        );
    }
}

/// Kept for compat - actual PIC disable happens in apic::init_bsp()
pub fn init_pics() {
    crate::arch::x86_64::apic::disable_8259();
}

pub fn init_pit() {
    // no-op: timing now comes from LAPIC timer. PIT is only used for channel-2
    // based LAPIC calibration inside apic::init_timer
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    use x86_64::instructions::port::Port;
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    // Filter out controller-reply bytes (ACK 0xFA, RESEND 0xFE, ERROR 0xFF,
    // BAT_FAIL 0xFC etc.). These are responses to commands we send, not key
    // events; pc-keyboard's Set-1 decoder hangs on them
    if scancode < 0xFA {
        crate::io::input::stdin::push(scancode);
        crate::drivers::input::ps2::on_key();
    } else {
        // An ACK or a RESEND is the keyboard answering a command: the
        // typematic, sent without waiting for the answer (`ps2::set_typematic`)
        crate::drivers::input::ps2::on_reply(scancode);
    }
    crate::arch::x86_64::apic::eoi();
}

/// IRQ 12: the auxiliary port. One byte a time, possibly several per interrupt:
/// a controller that buffered a whole packet delivers it in one go, and bytes
/// left in the buffer stall the port
extern "x86-interrupt" fn mouse_interrupt_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    while let Some(b) = crate::drivers::input::ps2::read_aux() {
        crate::drivers::input::mouse::feed(b);
    }
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ata_irq14_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    ATA_PRIMARY_IRQ.store(true, Ordering::Release);
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ata_irq15_handler(_: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    ATA_SECONDARY_IRQ.store(true, Ordering::Release);
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn nmi_handler(_stack_frame: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    // Paint a marker so we can SEE that NMI fired. White slot so it stands
    // out from the timer ISR pixels. NMI is rare and we cannot EOI it
    pixel_mark(9, 255, 255, 255);
    crate::serial_println!("[#NMI] received");
}

extern "x86-interrupt" fn mce_handler(_stack_frame: InterruptStackFrame) -> ! {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    pixel_mark(9, 255, 0, 255);
    crate::serial_println!("[#MC] machine check - halting");
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    crate::serial_println!("[int] breakpoint\n{:#?}", stack_frame);
}

/// A division by zero in ring 0. One from ring 3 goes through `trap`'s stub to
/// `user_trap`, where it is a `SIGFPE`
pub(crate) extern "x86-interrupt" fn divide_error_handler(stack_frame: InterruptStackFrame) {
    crate::arch::x86_64::cpu::clac();
    let saved_rbp = crate::process::crash::interrupted_rbp();
    use crate::process::crash::{self, Cause, Regs};
    let regs = Regs {
        rip:    stack_frame.instruction_pointer.as_u64(),
        rsp:    stack_frame.stack_pointer.as_u64(),
        rbp:    saved_rbp,
        rflags: stack_frame.cpu_flags,
        cs:     stack_frame.code_segment,
    };
    crash::report_kernel(Cause::DivideError, &regs);
    loop { x86_64::instructions::hlt(); }
}

/// An invalid opcode in ring 0; ring 3's goes to `trap::user_trap`
pub(crate) extern "x86-interrupt" fn ud_handler(stack_frame: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    let saved_rbp = crate::process::crash::interrupted_rbp();
    pixel_mark(12, 0, 255, 255);

    use crate::process::crash::{self, Cause, Regs};
    let regs = Regs {
        rip:    stack_frame.instruction_pointer.as_u64(),
        rsp:    stack_frame.stack_pointer.as_u64(),
        rbp:    saved_rbp,
        rflags: stack_frame.cpu_flags,
        cs:     stack_frame.code_segment,
    };
    crash::report_kernel(Cause::InvalidOpcode, &regs);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn nm_handler(stack_frame: InterruptStackFrame) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    pixel_mark(14, 100, 100, 255);
    crate::serial_println!("[#NM] device not available (SSE/FPU)\n{:#?}", stack_frame);
    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        crate::serial_println!("[#NM] cr0={:#x} cr4={:#x}", cr0, cr4);
    }
    let pid = crate::sched::current_pid();
    if pid != 0 { crate::sched::kill(pid); crate::sched::yield_now(); }
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    pixel_mark(13, 255, 0, 255);
    let cr2 = x86_64::registers::control::Cr2::read().as_u64();
    let (cr3f, _) = x86_64::registers::control::Cr3::read();
    let cr3 = cr3f.start_address().as_u64();
    unsafe {
        let tss = &*crate::arch::x86_64::gdt::tss_ptr();
        let ist0 = tss.interrupt_stack_table[0].as_u64();
        let ist1 = tss.interrupt_stack_table[1].as_u64();
        let rsp0 = tss.privilege_stack_table[0].as_u64();
        crate::serial_println!(
            "[double fault] code={} cr2={:#x} cr3={:#x}\n  rsp0={:#x}\n  ist0={:#x} ist1={:#x}\n{:#?}",
            error_code, cr2, cr3, rsp0, ist0, ist1, stack_frame
        );
    }
    loop { x86_64::instructions::hlt(); }
}

/// How deep in #PF each processor is. Zero everywhere in a healthy machine.
///
/// A page fault inside the page fault handler is a kernel bug, and since #PF
/// has its own IST stack it is unbounded: every nested entry resets RSP to the
/// same stack top, so the stack never reaches its guard page, no double fault
/// is raised, and the processor takes page faults at a few million a second for
/// ever. Nothing prints or dies, and on the processor that keeps time the clock
/// stops for the whole machine
static PF_DEPTH: [AtomicU64; crate::arch::x86_64::percpu::MAX_CPUS] =
    [const { AtomicU64::new(0) }; crate::arch::x86_64::percpu::MAX_CPUS];

struct PfDepth(usize);

impl Drop for PfDepth {
    fn drop(&mut self) {
        PF_DEPTH[self.0].fetch_sub(1, Ordering::Relaxed);
    }
}

/// A page fault in ring 0. One from ring 3 goes through `trap::user_trap`,
/// which shares `resolve_page_fault` with this and turns the rest into a signal
pub(crate) extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();

    // Nothing above this reads memory, so the check itself cannot recurse. CR2
    // and the interrupt frame are a register and the IST stack
    let nest_cpu = crate::arch::x86_64::percpu::current_index()
        .min(crate::arch::x86_64::percpu::MAX_CPUS - 1);
    let depth = PF_DEPTH[nest_cpu].fetch_add(1, Ordering::Relaxed);
    let _nesting = PfDepth(nest_cpu);
    if depth > 0 {
        // Deeper still means the report below faulted too; stop this
        // processor rather than recurse through the panic path
        if depth > 2 {
            loop { x86_64::instructions::hlt(); }
        }
        let inner = x86_64::registers::control::Cr2::read().as_u64();
        crate::serial_println!(
            "[page fault] fault inside the page fault handler: addr={:#x} rip={:#x} code={:?}",
            inner, stack_frame.instruction_pointer.as_u64(), error_code,
        );
        panic!(
            "page fault at {:#x} while handling a page fault (rip {:#x})",
            inner,
            stack_frame.instruction_pointer.as_u64(),
        );
    }
    // Read before this function calls anything: [rbp] is the interrupted
    // context's frame pointer, the only route to a backtrace
    let saved_rbp = crate::process::crash::interrupted_rbp();
    // Pixel marker first: serial does not exist on real HW, and this shows
    // whether #PF fired during the sti diagnostic
    pixel_mark(11, 255, 165, 0);
    use x86_64::registers::control::{Cr2, Cr3};

    let fault_addr = Cr2::read().as_u64();
    let page_addr  = fault_addr & !0xFFF;
    let (cr3_frame, _) = Cr3::read();
    let cr3 = cr3_frame.start_address().as_u64();


    // A fault in the kernel half landing on a stack guard page means a thread
    // ran off the bottom of its kernel stack. Say so by name
    if page_addr >= 0xFFFF_8000_0000_0000 {
        if let Some((pid, name)) = crate::mm::kstack::guard_fault_owner(fault_addr) {
            crate::serial_println!(
                "[stack] kernel stack overflow: pid={} '{}' hit its guard page at {:#x} (rip={:#x})",
                pid, name, fault_addr, stack_frame.instruction_pointer.as_u64()
            );
            panic!("kernel stack overflow in pid {} '{}'", pid, name);
        }
    }

    // A ring-0 fault on a present user page, with SMAP on and AC clear, is a
    // place in the kernel that touched user memory without a
    // usercopy::UserAccess. Name it. `nosmap` on the GRUB line gets the machine
    // booting while the missing guard is added
    if crate::arch::x86_64::cpu::smap_enabled()
        && stack_frame.cpu_flags & (1 << 18) == 0   // RFLAGS.AC
        && stack_frame.code_segment & 3 == 0        // CPL of the faulting CS
        && fault_addr < crate::mm::vmm::USER_END
        && error_code.contains(
            x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION)
    {
        crate::serial_println!(
            "[smap] ring-0 access to user address {:#x} from rip={:#x} with AC clear \
             - a usercopy::UserAccess is missing there",
            fault_addr, stack_frame.instruction_pointer.as_u64(),
        );
    }

    if resolve_page_fault(cr3, fault_addr, error_code, false) {
        return;
    }

    crate::serial_println!(
        "[page fault] addr={:#x} code={:?} user=false",
        fault_addr, error_code
    );
    dump_pte(cr3, fault_addr);

    use crate::process::crash::{self, Cause, Regs};
    let ec = &error_code;
    let cause = Cause::PageFault {
        addr:    fault_addr,
        write:   ec.contains(x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE),
        present: ec.contains(x86_64::structures::idt::PageFaultErrorCode::PROTECTION_VIOLATION),
        exec:    ec.contains(x86_64::structures::idt::PageFaultErrorCode::INSTRUCTION_FETCH),
    };
    let regs = Regs {
        rip:    stack_frame.instruction_pointer.as_u64(),
        rsp:    stack_frame.stack_pointer.as_u64(),
        rbp:    saved_rbp,
        rflags: stack_frame.cpu_flags,
        cs:     stack_frame.code_segment,
    };
    crash::report_kernel(cause, &regs);
    loop { x86_64::instructions::hlt(); }
}

/// The page faults that are not errors: a page swapped out, a copy-on-write
/// page written, a lazy VMA touched for the first time. True when the fault is
/// gone and the instruction can be retried.
///
/// Shared by the ring-0 handler above and `trap::user_trap`. `from_user` says
/// which: a file-backed VMA is filled only for ring 3, so the brief VFS lock
/// taken to read the file cannot deadlock against kernel code holding it
pub(crate) fn resolve_page_fault(
    cr3: u64,
    fault_addr: u64,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
    from_user: bool,
) -> bool {
    let page_addr = fault_addr & !0xFFF;
    // guard: only walk page tables for canonical user-range addresses
    let safe_to_walk = page_addr != 0 && page_addr < 0x0000_8000_0000_0000;
    if !safe_to_walk {
        return false;
    }

    if let Some(pte_raw) = crate::mm::vmm::read_pte_raw(cr3, page_addr) {
        if crate::mm::swap_map::is_swap_pte(pte_raw) {
            let slot = crate::mm::swap_map::slot_from_pte(pte_raw);
            if crate::mm::swap_map::try_swapin(cr3, page_addr, slot, pte_raw) {
                return true;
            }
        }

        // COW: write fault on present page with COW bit
        let is_write = error_code.contains(
            x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE
        );
        let is_present = pte_raw & crate::mm::vmm::PTE_PRESENT != 0;
        let is_cow = pte_raw & crate::mm::vmm::PTE_COW != 0;

        if is_write && is_present && is_cow {
            return crate::mm::vmm::break_cow_page(cr3, page_addr);
        }

        // Nothing to do: the entry already allows what faulted. The processor
        // raised it from a cached translation, read-only or not present, before
        // a sibling thread broke the copy-on-write or filled the page. A fault
        // drops that cached translation, so going back retries against the
        // entry as it is now. Ring 3 only: a kernel write finding a writable
        // user page is SMAP, and retrying it would loop
        let user_ok = pte_raw & (1 << 2) != 0;
        let fetch_ok = !error_code.contains(
            x86_64::structures::idt::PageFaultErrorCode::INSTRUCTION_FETCH
        ) || pte_raw & (1 << 63) == 0;
        let write_ok = !is_write || pte_raw & crate::mm::vmm::PTE_WRITABLE != 0;
        // A reserved bit is the table itself being wrong; retrying never ends
        let table_ok = !error_code.contains(
            x86_64::structures::idt::PageFaultErrorCode::MALFORMED_TABLE
        );
        if from_user && is_present && !is_cow && user_ok && fetch_ok && write_ok && table_ok {
            return true;
        }
    }

    // Lazy VMAs: a not-present fault inside a registered VMA fills the page on
    // demand. Anonymous VMAs (lazy stack, lazy mmap) are resolved from any CPL:
    // the kernel writing a signal frame onto an untouched stack page lands here
    let present = crate::mm::vmm::read_pte_raw(cr3, page_addr)
        .map_or(false, |p| p & crate::mm::vmm::PTE_PRESENT != 0);
    !present && crate::mm::mmap::handle_vma_fault(cr3, fault_addr, from_user)
}

/// One line about the page table entry behind a fault that nothing resolved
pub(crate) fn dump_pte(cr3: u64, fault_addr: u64) {
    let page_addr = fault_addr & !0xFFF;
    if page_addr == 0 || page_addr >= 0x0000_8000_0000_0000 {
        return;
    }
    if let Some(pte_raw) = crate::mm::vmm::read_pte_raw(cr3, page_addr) {
        let present  = pte_raw & 1 != 0;
        let writable = pte_raw & 2 != 0;
        let user     = pte_raw & 4 != 0;
        let nx       = pte_raw & (1 << 63) != 0;
        let cow      = pte_raw & crate::mm::vmm::PTE_COW != 0;
        let phys     = pte_raw & crate::mm::vmm::PTE_ADDR_MASK;
        crate::serial_println!(
            "[page fault] pte={:#x} phys={:#x} P={} W={} U={} NX={} COW={}",
            pte_raw, phys, present, writable, user, nx, cow
        );
    } else {
        crate::serial_println!("[page fault] pte=NOT_MAPPED for page {:#x}", page_addr);
    }
}

/// A general protection fault in ring 0; ring 3's goes to `trap::user_trap`
pub(crate) extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    // Ring 3 can leave RFLAGS.AC set; an interrupt does not clear it
    crate::arch::x86_64::cpu::clac();
    let saved_rbp = crate::process::crash::interrupted_rbp();
    // Pixel marker first, before anything that could deadlock: serial does not
    // exist on real HW
    pixel_mark(10, 255, 60, 60);

    use crate::process::crash::{self, Cause, Regs};
    let regs = Regs {
        rip:    stack_frame.instruction_pointer.as_u64(),
        rsp:    stack_frame.stack_pointer.as_u64(),
        rbp:    saved_rbp,
        rflags: stack_frame.cpu_flags,
        cs:     stack_frame.code_segment,
    };

    crate::cprintln!(255, 60, 60,
        "[gpf] code={} rip={:#x} rsp={:#x}",
        error_code,
        stack_frame.instruction_pointer.as_u64(),
        stack_frame.stack_pointer.as_u64());

    crash::report_kernel(Cause::GeneralProtection { error_code }, &regs);
    loop { x86_64::instructions::hlt(); }
}
