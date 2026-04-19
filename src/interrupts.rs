use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

pub const PIC_1_OFFSET: u8  = 32;
pub const PIC_2_OFFSET: u8  = PIC_1_OFFSET + 8;
pub const PIT_HZ:       u32 = 250;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static TICK: AtomicU64 = AtomicU64::new(0);

pub static ATA_PRIMARY_IRQ:   AtomicBool = AtomicBool::new(false);
pub static ATA_SECONDARY_IRQ: AtomicBool = AtomicBool::new(false);

pub fn get_tick() -> u64 {
    TICK.load(Ordering::Relaxed)
}

core::arch::global_asm!(
    ".global _timer_isr_naked",
    "_timer_isr_naked:",
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
    "call timer_handler_inner",
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
    "iretq",
);

extern "C" {
    fn _timer_isr_naked();
}

#[no_mangle]
unsafe extern "C" fn timer_handler_inner(old_rsp: u64) -> u64 {
    crate::vfs::procfs::tick();
    let tick = TICK.fetch_add(1, Ordering::Relaxed) + 1;

    if tick % 250 == 0 {
        crate::swap_map::age_all();
        crate::swap_map::refill_emergency_pool_tick();
    }

    PICS.lock().notify_end_of_interrupt(InterruptIndex::Timer.as_u8());

    if !crate::boot::is_done() {
        return old_rsp;
    }

    crate::scheduler::schedule_from_isr(old_rsp)
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
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(crate::gdt::DOUBLE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.page_fault
                .set_handler_fn(page_fault_handler)
                .set_stack_index(crate::gdt::PAGE_FAULT_IST_INDEX);
        }
        unsafe {
            idt.general_protection_fault
                .set_handler_fn(gpf_handler)
                .set_stack_index(crate::gdt::PAGE_FAULT_IST_INDEX);
        }
        unsafe {
            let timer_fn: extern "x86-interrupt" fn(InterruptStackFrame) =
                core::mem::transmute(_timer_isr_naked as *const ());
            idt[InterruptIndex::Timer.as_usize()].set_handler_fn(timer_fn);
        }
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::AtaIrq14.as_usize()].set_handler_fn(ata_irq14_handler);
        idt[InterruptIndex::AtaIrq15.as_usize()].set_handler_fn(ata_irq15_handler);
        idt
    };
}

pub fn init_idt() {
    crate::serial_println!("[int] loading idt");
    IDT.load();
    crate::serial_println!("[int] idt loaded");
}

pub fn init_pics() {
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        pics.write_masks(0b1111_1000, 0b0011_1111);
    }
    let masks = unsafe { PICS.lock().read_masks() };
    crate::serial_println!(
        "[int] PIC masks: PIC1=0b{:08b} PIC2=0b{:08b}",
        masks[0], masks[1]
    );
}

pub fn init_pit() {
    const PIT_FREQUENCY: u32 = 1_193_182;
    const DIVISOR: u16 = (PIT_FREQUENCY / PIT_HZ) as u16;
    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(0x43).write(0x36);
        Port::<u8>::new(0x40).write(DIVISOR as u8);
        Port::<u8>::new(0x40).write((DIVISOR >> 8) as u8);
    }
    crate::serial_println!("[pit] {} Hz (divisor={})", PIT_HZ, DIVISOR);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    crate::stdin::push(scancode);
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::Keyboard.as_u8()); }
}

extern "x86-interrupt" fn ata_irq14_handler(_: InterruptStackFrame) {
    ATA_PRIMARY_IRQ.store(true, Ordering::Release);
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::AtaIrq14.as_u8()); }
}

extern "x86-interrupt" fn ata_irq15_handler(_: InterruptStackFrame) {
    ATA_SECONDARY_IRQ.store(true, Ordering::Release);
    unsafe { PICS.lock().notify_end_of_interrupt(InterruptIndex::AtaIrq15.as_u8()); }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    crate::serial_println!("[int] breakpoint\n{:#?}", stack_frame);
}

extern "x86-interrupt" fn ud_handler(stack_frame: InterruptStackFrame) {
    crate::serial_println!("[#UD] invalid opcode\n{:#?}", stack_frame);
    let pid = crate::scheduler::current_pid();
    if pid != 0 { crate::scheduler::kill(pid); crate::scheduler::yield_now(); }
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn nm_handler(stack_frame: InterruptStackFrame) {
    crate::serial_println!("[#NM] device not available (SSE/FPU)\n{:#?}", stack_frame);
    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        crate::serial_println!("[#NM] cr0={:#x} cr4={:#x}", cr0, cr4);
    }
    let pid = crate::scheduler::current_pid();
    if pid != 0 { crate::scheduler::kill(pid); crate::scheduler::yield_now(); }
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn double_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) -> ! {
    let cr2 = x86_64::registers::control::Cr2::read().as_u64();
    let (cr3f, _) = x86_64::registers::control::Cr3::read();
    let cr3 = cr3f.start_address().as_u64();
    unsafe {
        let tss = &*crate::gdt::tss_ptr();
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
    use x86_64::registers::control::{Cr2, Cr3};

    let fault_addr = Cr2::read().as_u64();
    let page_addr  = fault_addr & !0xFFF;
    let (cr3_frame, _) = Cr3::read();
    let cr3 = cr3_frame.start_address().as_u64();

    // guard: only walk page tables for canonical user-range addresses
    let safe_to_walk = page_addr != 0 && page_addr < 0x0000_8000_0000_0000;

    if safe_to_walk {
        if let Some(pte_raw) = crate::vmm::read_pte_raw(cr3, page_addr) {
            if crate::swap_map::is_swap_pte(pte_raw) {
                let slot = crate::swap_map::slot_from_pte(pte_raw);
                if crate::swap_map::try_swapin(cr3, page_addr, slot, pte_raw) {
                    return;
                }
            }

            // COW: write fault on present page with COW bit
            let is_write = error_code.contains(
                x86_64::structures::idt::PageFaultErrorCode::CAUSED_BY_WRITE
            );
            let is_present = pte_raw & crate::vmm::PTE_PRESENT != 0;
            let is_cow = pte_raw & crate::vmm::PTE_COW != 0;

            if is_write && is_present && is_cow {
                let old_phys = pte_raw & crate::vmm::PTE_ADDR_MASK;
                let hhdm = crate::grub::hhdm();
                let rc = crate::pmm::ref_get(old_phys);

                let aspace = crate::vmm::AddressSpace { cr3 };
                if rc > 1 {
                    if let Some(new_phys) = crate::pmm::alloc_frame() {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                (old_phys + hhdm) as *const u8,
                                (new_phys + hhdm) as *mut u8,
                                4096,
                            );
                        }
                        let remaining = crate::pmm::ref_dec(old_phys);
                        if remaining <= 1 {
                            crate::swap_map::set_pinned(old_phys, false);
                        }
                        let new_pte = (new_phys & crate::vmm::PTE_ADDR_MASK)
                            | (pte_raw & !crate::vmm::PTE_ADDR_MASK & !crate::vmm::PTE_COW)
                            | crate::vmm::PTE_WRITABLE;
                        unsafe { aspace.write_pte_raw(page_addr, new_pte); }
                        crate::swap_map::track(new_phys, cr3, page_addr, false);
                        x86_64::instructions::tlb::flush(x86_64::VirtAddr::new(page_addr));
                        core::mem::forget(aspace);
                        return;
                    }
                } else {
                    // Last COW reference - just make writable and unpin
                    let new_pte = (pte_raw & !crate::vmm::PTE_COW) | crate::vmm::PTE_WRITABLE;
                    unsafe { aspace.write_pte_raw(page_addr, new_pte); }
                    crate::swap_map::set_pinned(old_phys, false);
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

    crate::serial_println!(
        "[page fault] addr={:#x} code={:?} user={}",
        fault_addr, error_code, from_user
    );

    // dump PTE info for debugging
    if safe_to_walk {
        if let Some(pte_raw) = crate::vmm::read_pte_raw(cr3, page_addr) {
            let present  = pte_raw & 1 != 0;
            let writable = pte_raw & 2 != 0;
            let user     = pte_raw & 4 != 0;
            let nx       = pte_raw & (1 << 63) != 0;
            let cow      = pte_raw & crate::vmm::PTE_COW != 0;
            let phys     = pte_raw & crate::vmm::PTE_ADDR_MASK;
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
        let pid = crate::scheduler::current_pid();
        crate::serial_println!("[page fault] killing pid={}", pid);
        crate::scheduler::kill(pid);
        crate::scheduler::yield_now();
        return;
    }

    crate::serial_println!("{:#?}", stack_frame);
    loop { x86_64::instructions::hlt(); }
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    let from_user = stack_frame.code_segment != 0x08;

    crate::serial_println!(
        "[gpf] code={} user={}\n{:#?}",
        error_code, from_user, stack_frame
    );

    if from_user {
        let pid = crate::scheduler::current_pid();
        crate::serial_println!("[gpf] killing pid={}", pid);
        crate::scheduler::kill(pid);
        crate::scheduler::yield_now();
        return;
    }

    loop { x86_64::instructions::hlt(); }
}

