use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lazy_static::lazy_static;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

pub const PIC_1_OFFSET: u8  = 32;
pub const PIC_2_OFFSET: u8  = PIC_1_OFFSET + 8;
pub const PIT_HZ:       u32 = 250;

static TICK: AtomicU64 = AtomicU64::new(0);

pub static ATA_PRIMARY_IRQ:   AtomicBool = AtomicBool::new(false);
pub static ATA_SECONDARY_IRQ: AtomicBool = AtomicBool::new(false);

pub fn get_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

// Diagnostic pixel-marker. Disabled in normal builds - kept as a no-op so the
// call sites scattered through ISRs and early-boot diagnostics still compile.
// Re-enable by restoring the framebuffer-poking body when chasing an early
// hang where serial output isn't available yet
#[inline(always)]
pub fn pixel_mark(_slot: usize, _r: u8, _g: u8, _b: u8) {}

/// Drive scheduling from the timer interrupt.
///
/// HISTORY - read before flipping this back on if it ever gets turned off:
/// an earlier naked-asm timer entry worked under QEMU but wedged real
/// hardware on the very first hardware-delivered vector 0x20. A software
/// 'int 0x20' through the same IDT entry worked, which ruled out the gate
/// and IDT configuration and pointed at the entry code itself. The most
/// likely culprit is GS: a tick taken in ring 3 must 'swapgs' on entry and
/// again on exit, and getting that wrong leaves every 'percpu::current()'
/// reading userspace memory - which a mostly-kernel QEMU workload can hide
/// for a long time. The stub below makes both swaps conditional on the
/// saved CS
///
/// If a real machine hangs right after the first tick, set this to 'false':
/// the system falls back to the compiler-generated handler and cooperative
/// 'yield_now()' scheduling, which is slower but known to work everywhere
const PREEMPTIVE_TIMER: bool = true;

// Compiler-generated fallback used when PREEMPTIVE_TIMER is false. It counts
// ticks and does the per-tick bookkeeping, but cannot switch tasks: the
// compiler owns the prologue, so there is no way to resume on a different
// stack
extern "x86-interrupt" fn timer_interrupt_handler(_: InterruptStackFrame) {
    let tick = TICK.fetch_add(1, Ordering::Relaxed) + 1;
    crate::arch::x86_64::apic::eoi();
    if !crate::kcore::boot_state::is_done() {
        return;
    }
    crate::fs::procfs::tick();
    if tick % 250 == 0 {
        crate::mm::swap_map::request_reclaim_from_isr();
    }
}

// Preemptive timer interrupt
//
// The stub builds exactly the frame 'schedule_from_isr' expects (15 GPRs in
// push order, rax last, on top of the CPU's iret frame), hands it the stack
// pointer, and continues on whatever stack the scheduler returns. That is the
// whole trick: switching tasks is returning a different rsp
//
// Without it 'yield_now()' was the only way into the scheduler, so a CPU
// sitting in its idle 'hlt' loop could never pick up work and a task that
// never yielded kept its CPU indefinitely
#[unsafe(naked)]
unsafe extern "C" fn timer_interrupt_stub() {
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
        "mov rdi, rsp",
        "and rsp, -16",
        "call {entry}",
        "mov rsp, rax",
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
        // Decide on the frame we are actually returning to, which after a
        // switch belongs to a different task than the one we entered on
        "cmp qword ptr [rsp + 8], 0x08",
        "je 3f",
        "swapgs",
        "3:",
        "iretq",
        entry = sym timer_isr_entry,
    )
}

/// Per-tick bookkeeping, then the scheduler. Returns the stack pointer to
/// resume on - the same one when nothing switches
#[no_mangle]
unsafe extern "C" fn timer_isr_entry(frame_rsp: u64) -> u64 {
    let tick = TICK.fetch_add(1, Ordering::Relaxed) + 1;
    crate::arch::x86_64::apic::eoi();

    // Before the scheduler exists there is nothing to switch to
    if !crate::kcore::boot_state::is_done() {
        return frame_rsp;
    }

    crate::fs::procfs::tick();
    if tick % 250 == 0 {
        // Reclaim must not run here - see mm::swap_map. The tick only
        // raises a flag for kswapd
        crate::mm::swap_map::request_reclaim_from_isr();
    }

    crate::sched::schedule_from_isr(frame_rsp)
}

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer    = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
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
        idt.invalid_opcode.set_handler_fn(ud_handler);
        idt.device_not_available.set_handler_fn(nm_handler);
        // NMI (vector 2) MUST be handled. Real hardware delivers NMI from
        // sources we cannot mask: ACPI MADT LAPIC NMI Source (type 4) often
        // configures LINT0/LINT1 with delivery_mode=NMI, in which case the
        // LVT mask bit is irrelevant - the LAPIC will still forward NMI to
        // the CPU. Also: PMI, watchdog, and platform RAS sources. 
        // Without an IDT entry the very first sti causes #GP -> recursive fault  -> triple fault, which on this machine looks like a wedge
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
                .set_handler_fn(page_fault_handler)
                .set_stack_index(crate::arch::x86_64::gdt::PAGE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.general_protection_fault
                .set_handler_fn(gpf_handler)
                .set_stack_index(crate::arch::x86_64::gdt::PAGE_FAULT_IST_INDEX);
        }
        // The preemptive path needs a raw address: it is a naked stub, not
        // an `extern "x86-interrupt"` fn, because it has to be able to
        // return on a different task's stack
        if PREEMPTIVE_TIMER {
            unsafe {
                idt[InterruptIndex::Timer.as_usize()].set_handler_addr(
                    x86_64::VirtAddr::new(timer_interrupt_stub as usize as u64),
                );
            }
        } else {
            idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_interrupt_handler);
        }
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::AtaIrq14.as_usize()].set_handler_fn(ata_irq14_handler);
        idt[InterruptIndex::AtaIrq15.as_usize()].set_handler_fn(ata_irq15_handler);
        idt[crate::arch::x86_64::apic::VEC_APIC_ERR as usize].set_handler_fn(apic_error_handler);
        idt[crate::arch::x86_64::apic::VEC_SPURIOUS as usize].set_handler_fn(apic_spurious_handler);
        idt[crate::arch::x86_64::apic::VEC_IPI_RESCHED as usize].set_handler_fn(ipi_resched_handler);
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
    // No EOI for spurious
}

extern "x86-interrupt" fn apic_error_handler(_: InterruptStackFrame) {
    // Keep this minimal - it runs on real hardware where serial may not
    // exist, and taking any lock (like the framebuffer console) from an
    // interrupt context risks deadlock. Latch + clear ESR and EOI; count
    // errors via an atomic so it is visible from shell diagnostics
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
    crate::arch::x86_64::apic::eoi();
    // schedule hint - next timer tick will preempt if needed
}

extern "x86-interrupt" fn ipi_tlb_handler(_: InterruptStackFrame) {
    x86_64::instructions::tlb::flush_all();
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ipi_halt_handler(_: InterruptStackFrame) {
    crate::arch::x86_64::apic::eoi();
    x86_64::instructions::interrupts::disable();
    loop { x86_64::instructions::hlt(); }
}

pub fn init_idt() {
    crate::serial_println!("[int] loading idt");
    IDT.load();
    crate::serial_println!("[int] idt loaded");
}

/// Early-boot diagnostic: print the raw IDT gate the CPU will use.  Keeping
/// this next to the table makes it possible to distinguish a bad gate from an
/// interrupt source problem before IF is enabled
pub fn dump_gate(vector: u8) {
    let idt = &*IDT as *const InterruptDescriptorTable as *const u64;
    unsafe {
        crate::serial_println!(
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
    use x86_64::instructions::port::Port;
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    // Filter out controller-reply bytes (ACK 0xFA, RESEND 0xFE, ERROR 0xFF,
    // BAT_FAIL 0xFC etc.). These are responses to commands we send, not key
    // events; pc-keyboard's Set-1 decoder hangs on them
    if scancode < 0xFA {
        crate::io::input::stdin::push(scancode);
    }
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ata_irq14_handler(_: InterruptStackFrame) {
    ATA_PRIMARY_IRQ.store(true, Ordering::Release);
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn ata_irq15_handler(_: InterruptStackFrame) {
    ATA_SECONDARY_IRQ.store(true, Ordering::Release);
    crate::arch::x86_64::apic::eoi();
}

extern "x86-interrupt" fn nmi_handler(_stack_frame: InterruptStackFrame) {
    // Paint a marker so we can SEE that NMI fired. White slot so it stands
    // out from the timer ISR pixels. NMI is rare and we cannot EOI it
    pixel_mark(9, 255, 255, 255);
    crate::serial_println!("[#NMI] received");
}

extern "x86-interrupt" fn mce_handler(_stack_frame: InterruptStackFrame) -> ! {
    pixel_mark(9, 255, 0, 255);
    crate::serial_println!("[#MC] machine check - halting");
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    crate::serial_println!("[int] breakpoint\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn ud_handler(stack_frame: InterruptStackFrame) {
    pixel_mark(12, 0, 255, 255);
    crate::serial_println!("[#UD] invalid opcode\n{:#?}", stack_frame);
    let pid = crate::sched::current_pid();
    if pid != 0 { crate::sched::kill(pid); crate::sched::yield_now(); }
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn nm_handler(stack_frame: InterruptStackFrame) {
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

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: x86_64::structures::idt::PageFaultErrorCode,
) {
    // Pixel marker FIRST. On real HW serial does not exist; this lets us
    // visually confirm whether #PF fired during the sti diagnostic
    pixel_mark(11, 255, 165, 0);
    use x86_64::registers::control::{Cr2, Cr3};

    let fault_addr = Cr2::read().as_u64();
    let page_addr  = fault_addr & !0xFFF;
    let (cr3_frame, _) = Cr3::read();
    let cr3 = cr3_frame.start_address().as_u64();

    // A fault in the kernel half that lands on a stack guard page means a
    // thread ran off the bottom of its kernel stack. Say so by name instead
    // of leaving a bare "page fault at <address>" to be decoded by hand
    if page_addr >= 0xFFFF_8000_0000_0000 {
        if let Some((pid, name)) = crate::mm::kstack::guard_fault_owner(fault_addr) {
            crate::serial_println!(
                "[stack] kernel stack overflow: pid={} '{}' hit its guard page at {:#x} (rip={:#x})",
                pid, name, fault_addr, stack_frame.instruction_pointer.as_u64()
            );
            panic!("kernel stack overflow in pid {} '{}'", pid, name);
        }
    }

    // guard: only walk page tables for canonical user-range addresses
    let safe_to_walk = page_addr != 0 && page_addr < 0x0000_8000_0000_0000;

    if safe_to_walk {
        if let Some(pte_raw) = crate::mm::vmm::read_pte_raw(cr3, page_addr) {
            if crate::mm::swap_map::is_swap_pte(pte_raw) {
                let slot = crate::mm::swap_map::slot_from_pte(pte_raw);
                if crate::mm::swap_map::try_swapin(cr3, page_addr, slot, pte_raw) {
                    return;
                }
            }

            // COW: write fault on present page with COW bit
            let is_write = error_code.contains(
                x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE
            );
            let is_present = pte_raw & crate::mm::vmm::PTE_PRESENT != 0;
            let is_cow = pte_raw & crate::mm::vmm::PTE_COW != 0;

            if is_write && is_present && is_cow {
                let old_phys = pte_raw & crate::mm::vmm::PTE_ADDR_MASK;
                let hhdm = crate::arch::x86_64::grub::hhdm();
                let rc = crate::mm::pmm::ref_get(old_phys);

                let aspace = crate::mm::vmm::AddressSpace { cr3 };
                if rc > 1 {
                    if let Some(new_phys) = crate::mm::pmm::alloc_frame() {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                (old_phys + hhdm) as *const u8,
                                (new_phys + hhdm) as *mut u8,
                                4096,
                            );
                        }
                        let remaining = crate::mm::pmm::ref_dec(old_phys);
                        if remaining <= 1 {
                            crate::mm::swap_map::set_pinned(old_phys, false);
                        }
                        let new_pte = (new_phys & crate::mm::vmm::PTE_ADDR_MASK)
                            | (pte_raw & !crate::mm::vmm::PTE_ADDR_MASK & !crate::mm::vmm::PTE_COW)
                            | crate::mm::vmm::PTE_WRITABLE;
                        unsafe { aspace.write_pte_raw(page_addr, new_pte); }
                        crate::mm::swap_map::track(new_phys, cr3, page_addr, false);
                        x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(page_addr));
                        core::mem::forget(aspace);
                        return;
                    }
                } else {
                    // Last COW reference - just make writable and unpin
                    let new_pte = (pte_raw & !crate::mm::vmm::PTE_COW) | crate::mm::vmm::PTE_WRITABLE;
                    unsafe { aspace.write_pte_raw(page_addr, new_pte); }
                    crate::mm::swap_map::set_pinned(old_phys, false);
                    x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(page_addr));
                    core::mem::forget(aspace);
                    return;
                }
                core::mem::forget(aspace);
            }
        }
    }

    let from_user = error_code.contains(
        x86_64::structures::idt::PageFaultErrorCode::USER_MODE
    );

    // Lazy VMAs: a not-present fault inside a registered VMA fills the page
    // on demand. Anonymous VMAs (lazy stack, lazy mmap) are resolved from
    // any CPL - the kernel writing a signal frame onto an untouched stack
    // page lands here. File-backed VMAs are user-mode only, so the brief
    // VFS lock taken to read the file can't deadlock against kernel code
    // already holding it
    if safe_to_walk {
        let present = crate::mm::vmm::read_pte_raw(cr3, page_addr)
            .map_or(false, |p| p & crate::mm::vmm::PTE_PRESENT != 0);
        if !present && crate::mm::mmap::handle_vma_fault(cr3, fault_addr, from_user) {
            return;
        }
    }

    crate::serial_println!(
        "[page fault] addr={:#x} code={:?} user={}",
        fault_addr, error_code, from_user
    );

    // dump PTE info for debugging
    if safe_to_walk {
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

    crate::serial_println!("[page fault] rip={:#x}", stack_frame.instruction_pointer.as_u64());

    if from_user {
        let pid = crate::sched::current_pid();
        crate::serial_println!("[page fault] killing pid={}", pid);
        crate::sched::kill(pid);
        crate::sched::yield_now();
        return;
    }

    crate::serial_println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    // Pixel marker FIRST - before anything that could deadlock. On real HW
    // serial doesn't exist, so the only way to know #GP fired is the pixel
    pixel_mark(10, 255, 60, 60);
    let from_user = stack_frame.code_segment != 0x08;

    crate::serial_println!(
        "[gpf] code={} user={}\n{:#?}",
        error_code, from_user, stack_frame
    );
    crate::cprintln!(255, 60, 60,
        "[gpf] code={} rip={:#x} rsp={:#x}",
        error_code,
        stack_frame.instruction_pointer.as_u64(),
        stack_frame.stack_pointer.as_u64());

    if from_user {
        let pid = crate::sched::current_pid();
        crate::serial_println!("[gpf] killing pid={}", pid);
        crate::sched::kill(pid);
        crate::sched::yield_now();
        return;
    }

    loop { x86_64::instructions::hlt(); }
}
