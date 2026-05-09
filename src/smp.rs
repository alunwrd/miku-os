// SMP bring-up: AP trampoline+BSP starter. Per-CPU state lives in 'percpu'

use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use crate::acpi;
use crate::apic;
use crate::grub;
use crate::percpu;
use crate::pmm;

const TRAMPOLINE_PHYS: u32   = 0x8000;
const AP_STACK_PAGES:  usize = 16;  // 64KB per AP

extern "C" {
    fn _ap_trampoline_start();
    fn _ap_trampoline_end();
}

/// Parameter block written by BSP before firing SIPIs, read by the trampoline
/// Layout must match offsets referenced in the trampoline assembly below
#[repr(C)]
struct ApParams {
    page_table: u64,  // +0x00 - CR3 for new AP
    stack_top:  u64,  // +0x08 - kernel stack top
    entry:      u64,  // +0x10 - Rust entry fn
    cpu_index:  u64,  // +0x18 - logical CPU number
    ready:      u64,  // +0x20 - AP sets this to 1 when in long mode
    gdt_ptr:    u64,  // +0x28 - unused (AP reloads kernel's later)
    idt_ptr:    u64,  // +0x30 - unused
    _padding:   [u64; 9],
}

const PARAM_OFFSET: u32 = 0xF00;

// AP entry //

#[no_mangle]
pub extern "C" fn ap_entry(cpu_index: u64) -> ! {
    let idx = cpu_index as usize;

    unsafe {
        // 1) Establish per-CPU GS base so current_index() works
        percpu::install_gs_base(idx);

        // 2) Load this CPU's GDT + TSS
        crate::gdt::init_cpu(idx);

        // 3) Load the shared IDT on this CPU
        crate::interrupts::init_idt();

        // 4) Enable SSE (same bits as BSP)
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        core::arch::asm!("mov cr0, {}", in(reg) (cr0 & !(1u64 << 2)) | (1u64 << 1));
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1u64 << 9) | (1u64 << 10));

        // 5) Program syscall MSRs (STAR/LSTAR/SFMASK) on this CPU
        crate::syscall::init();
    }

    // 6) Bring the LAPIC online
    apic::init_ap();
    let lid = apic::lapic_id();
    percpu::mark_online(idx, lid);

    // 7) Register the per-CPU idle thread so the scheduler has a fallback
    crate::scheduler::init_ap_idle(idx);

    // 8) Start this CPU's LAPIC timer using the BSP calibration
    apic::start_ap_timer(apic::bsp_ticks_per_hz());

    crate::serial_println!("[smp] AP#{} online lapic_id={}", idx, lid);

    // 9) Enable interrupts and idle. The timer will drive scheduler entry
    x86_64::instructions::interrupts::enable();
    loop {
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

// BSP startup //

// Start all APs found in ACPI MADT. Must run after acpi::init(), apic::init_bsp(),           //
// apic::init_timer(), and scheduler::init_main_thread() have completed                       //
pub fn start_aps() {
    let (cpus, ticks_per_hz) = {
        let topo_lock = acpi::topology();
        let topo = match topo_lock.as_ref() {
            Some(t) => t,
            None => {
                crate::serial_println!("[smp] no acpi topology, skipping AP startup");
                return;
            }
        };
        (topo.cpus.clone(), apic::bsp_ticks_per_hz())
    };

    let bsp_lapic = apic::lapic_id();

    // BSP is cpu 0 - already registered by gdt::init
    percpu::mark_online(0, bsp_lapic);

    let _ = ticks_per_hz; // APs fetch their own via apic::bsp_ticks_per_hz()

    // Copy trampoline to 0x8000
    let tramp_start = _ap_trampoline_start as *const () as u64;
    let tramp_end   = _ap_trampoline_end   as *const () as u64;
    let tramp_len   = (tramp_end - tramp_start) as usize;
    let hhdm        = grub::hhdm();

    if tramp_len > PARAM_OFFSET as usize {
        crate::serial_println!("[smp] trampoline too large: {} bytes", tramp_len);
        return;
    }

    unsafe {
        core::ptr::copy_nonoverlapping(
            tramp_start as *const u8,
            (TRAMPOLINE_PHYS as u64 + hhdm) as *mut u8,
            tramp_len,
        );
    }

    let kernel_cr3 = crate::vmm::kernel_cr3();

    let mut cpu_idx: u64 = 1;  // BSP is 0
    let mut started = 0usize;

    for cpu in &cpus {
        if !cpu.enabled || cpu.is_x2apic { continue; }
        if cpu.lapic_id == bsp_lapic { continue; }
        if cpu_idx as usize >= percpu::MAX_CPUS { break; }

        // Allocate kernel stack for this AP
        let stack_phys = match pmm::alloc_frames(AP_STACK_PAGES) {
            Some(p) => p,
            None => {
                crate::serial_println!("[smp] no memory for AP stack");
                break;
            }
        };
        let stack_top = stack_phys + hhdm + (AP_STACK_PAGES * 4096) as u64;

        unsafe {
            // Fill ApParams at TRAMPOLINE_PHYS + PARAM_OFFSET
            let params = (TRAMPOLINE_PHYS as u64 + PARAM_OFFSET as u64 + hhdm) as *mut ApParams;
            (*params).page_table = kernel_cr3;
            (*params).stack_top  = stack_top;
            (*params).entry      = ap_entry as *const () as u64;
            (*params).cpu_index  = cpu_idx;
            (*params).ready      = 0;
            (*params).gdt_ptr    = 0;
            (*params).idt_ptr    = 0;

            core::sync::atomic::fence(Ordering::SeqCst);
        }

        // Pre-register the AP so percpu::iter_online is consistent once it
        // flips the online flag from ap_entry
        percpu::register(cpu_idx as usize);

        crate::serial_println!("[smp] starting AP lapic_id={} cpu_idx={}", cpu.lapic_id, cpu_idx);

        apic::send_init_sipi(cpu.lapic_id, TRAMPOLINE_PHYS);

        // Wait up to 100ms for AP to signal ready
        let params = (TRAMPOLINE_PHYS as u64 + PARAM_OFFSET as u64 + hhdm) as *mut ApParams;
        let mut waited_us = 0u64;
        let mut ok = false;
        while waited_us < 100_000 {
            let ready = unsafe { core::ptr::read_volatile(&(*params).ready) };
            if ready != 0 { ok = true; break; }
            crate::timing::udelay(100);
            waited_us += 100;
        }

        if !ok {
            crate::serial_println!("[smp] AP lapic_id={} did not come online", cpu.lapic_id);
            continue;
        }

        cpu_idx += 1;
        started += 1;
    }

    // Wait for all APs to flip their online bit via percpu::mark_online
    let target = started + 1;
    let mut spins = 0;
    while percpu::online_cpus() < target && spins < 100 {
        crate::timing::mdelay(1);
        spins += 1;
    }

    crate::serial_println!(
        "[smp] bringup complete: {}/{} cpus online",
        percpu::online_cpus(), target
    );
}

//////////////////////////////////////////////////////////////////////////////////////////////
//                                  AP trampoline                                           // 
//                                                                                          //
// The trampoline lives in its own ELF section. Build emits a blob of                       //
// position-independent real-mode -> long-mode bootstrap code followed by an                //
// ApParams block at offset 0xF00. We memcpy the whole blob to physical 0x8000              //
// before firing the SIPIs - APs start at CS:IP = 0x800:0.                                  //
//////////////////////////////////////////////////////////////////////////////////////////////
core::arch::global_asm!(r#"
.section .ap_trampoline, "ax"
.code16
.balign 4096
.globl _ap_trampoline_start
_ap_trampoline_start:
    cli
    cld
    xor    %ax, %ax
    mov    %ax, %ds
    mov    %ax, %es
    mov    %ax, %ss
    mov    %ax, %fs
    mov    %ax, %gs

    // Load 16-bit GDT (flat code/data for pmode)
    lgdtl  ap_gdtr_16 - _ap_trampoline_start + 0x8000

    // Enable PE in CR0
    mov    %cr0, %eax
    or     $1, %eax
    mov    %eax, %cr0

    // Far jump to 32-bit code - absolute physical address 0x8000 + offset
    .byte  0x66, 0xEA
    .long  ap_pm32 - _ap_trampoline_start + 0x8000
    .word  0x08

.code32
ap_pm32:
    mov    $0x10, %ax
    mov    %ax, %ds
    mov    %ax, %es
    mov    %ax, %ss
    mov    %ax, %fs
    mov    %ax, %gs

    // Enable PAE + PSE
    mov    %cr4, %eax
    or     $(1 << 5) | (1 << 4), %eax
    mov    %eax, %cr4

    // Load CR3 from ApParams.page_table (at 0x8F00 + 0)
    mov    (0x8F00), %eax
    mov    %eax, %cr3

    // Enable LM in EFER MSR
    mov    $0xC0000080, %ecx
    rdmsr
    or     $(1 << 8), %eax
    wrmsr

    // Enable paging (CR0.PG=1, PE already set)
    mov    %cr0, %eax
    or     $(1 << 31), %eax
    mov    %eax, %cr0

    // Load 64-bit GDT and far jump to 64-bit code
    lgdt   ap_gdtr_64 - _ap_trampoline_start + 0x8000

    ljmpl  $0x18, $(ap_pm64 - _ap_trampoline_start + 0x8000)

.code64
ap_pm64:
    mov    $0x20, %ax
    mov    %ax, %ds
    mov    %ax, %es
    mov    %ax, %ss
    mov    %ax, %fs
    mov    %ax, %gs

    // Load kernel stack pointer from ApParams.stack_top
    mov    (0x8F08), %rsp

    // Mark ready
    movq   $1, (0x8F20)

    // Load cpu_index into rdi (arg 0)
    mov    (0x8F18), %rdi

    // Jump to ap_entry (virtual address in kernel)
    mov    (0x8F10), %rax
    jmp    *%rax

.balign 8
ap_gdt_16:
    .quad 0x0000000000000000
    .quad 0x00CF9A000000FFFF  // 32-bit code
    .quad 0x00CF92000000FFFF  // 32-bit data
    .quad 0x00AF9A000000FFFF  // 64-bit code
    .quad 0x00AF92000000FFFF  // 64-bit data
ap_gdt_end:

ap_gdtr_16:
    .word ap_gdt_end - ap_gdt_16 - 1
    .long ap_gdt_16 - _ap_trampoline_start + 0x8000

ap_gdtr_64:
    .word ap_gdt_end - ap_gdt_16 - 1
    .quad ap_gdt_16 - _ap_trampoline_start + 0x8000

.balign 64
.globl _ap_trampoline_end
_ap_trampoline_end:
"#, options(att_syntax));
