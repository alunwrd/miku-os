#![allow(
    dead_code,
    unused_imports,
    unused_variables,
    static_mut_refs,
    mismatched_lifetime_syntaxes,
    unused_assignments,
    unused_mut
)]
#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
extern crate alloc;
use core::panic::PanicInfo;
mod allocator;
mod ata;
mod block;
pub mod boot;
mod boot_entry;
mod color;
mod commands;
mod console;
mod font;
mod gdt;
mod grub;
mod interrupts;
mod miku_extfs;
mod ldso;
pub mod mkfs;
mod net;
mod pmm;
mod power;
mod process;
mod ring3;
mod scheduler;
mod syscall;
pub mod serial;
mod shell;
pub mod stdin;
pub mod timing;
mod vmm;
mod elf;
mod elf_loader;
mod exec_elf;
pub mod user_stdin;
mod vfs;
pub mod dynlink;
pub mod mmap;
mod gpt;
mod swap;
mod swap_map;
mod solib;
mod random;
mod reloc;
mod vfs_read;
pub mod signal;
pub mod mikud;
pub mod acpi;
pub mod apic;
pub mod percpu;
pub mod smp;
pub mod nvidia;
pub mod splash;
pub mod usb_handoff;
pub mod firmware;
pub mod firmware_probe;
pub mod fwload;
pub mod ps2;

unsafe extern "C" {
    static _kernel_end: u8;
}

fn kernel_end_phys() -> u64 {
    let virt = core::ptr::addr_of!(_kernel_end) as u64;
    virt - grub::KERNEL_VMA
}

#[no_mangle]
unsafe extern "C" fn kernel_main_grub(mb2_phys: u64) -> ! {
    grub::init(mb2_phys);
    kernel_main();
}

fn kernel_main() -> ! {
    serial_println!("[kern] MikuOS starting (Release v0.2.9-rc)");
    gdt::init();
    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        core::arch::asm!("mov cr0, {}", in(reg) (cr0 & !(1u64 << 2)) | (1u64 << 1));
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1u64 << 9) | (1u64 << 10));
    }
    serial_println!("[sse] enabled (CR0.EM=0 CR0.MP=1 CR4.OSFXSR=1 CR4.OSXMMEXCPT=1)");
    syscall::init();
    interrupts::init_idt();
    allocator::init();
    scheduler::reinit_scheduler();
    grub::set_kernel_address(
        grub::KERNEL_VMA + grub::KERNEL_PHYS,
        grub::KERNEL_PHYS,
    );
    init_framebuffer();
    if let Some(mmap) = grub::memory_map() {
        for entry in mmap {
            let length   = entry.length();
            let mem_type = entry.mem_type();
            let base     = entry.base();
            pmm::register_total_ram(length);
            if mem_type == grub::MMAP_USABLE {
                pmm::add_region(base, length);
            }
        }
    } else {
        serial_println!("[kern] warn: no memory map from GRUB");
    }

    let kend = kernel_end_phys();
    let kend_aligned = (kend + 0xFFF) & !0xFFF;
    serial_println!("[kern] _kernel_end phys={:#x} ({}MB)", kend_aligned, kend_aligned / 1024 / 1024);

    pmm::reserve_region(0x0, 0x6000);
    // Reserve AP trampoline page (used once by smp::start_aps, never returned to pool)
    pmm::reserve_region(0x8000, 0x9000);
    pmm::reserve_region(grub::KERNEL_PHYS, kend_aligned);

    boot_step!("Physical memory manager", Ok(()));
    boot_step!("ACPI (RSDP/MADT)",         acpi::init());
    boot_step!("APIC", apic::init_bsp());
    boot_step!("IO-APIC",                  apic::ioapic_init());
    apic::init_timer(apic::TIMER_HZ_DEFAULT);
    boot_step!("LAPIC timer",              Ok(()));
    let bsp_lapic = apic::lapic_id();
    let _ = apic::set_irq(1,  apic::VEC_KEYBOARD, bsp_lapic);  // keyboard
    let _ = apic::set_irq(14, apic::VEC_ATA_PRI,  bsp_lapic);
    let _ = apic::set_irq(15, apic::VEC_ATA_SEC,  bsp_lapic);
    boot_step!("IRQ routing", Ok(()));
    boot_step!("Virtual file system",       vfs::core::init_vfs());
    crate::solib::init();
    crate::solib::preload("libmiku.so", crate::ldso::LIBMIKU_BYTES.to_vec());
    crate::solib::ldconfig();
    boot_step!("Shared library cache",      Ok(()));
    boot_step!("Block device probe",        { block::probe(); Ok::<(), &'static str>(()) });
    boot_step!("Block device nodes (/dev)", { vfs::core::register_block_nodes(); Ok::<(), &'static str>(()) });
    boot_step!("Network subsystem",         net::init());
    boot_step!("Firmware store",            fwload::init());
    boot_step!("NVIDIA GPU probe",          nvidia::init());
    scheduler::init_main_thread();
    scheduler::init_workers(4);
    boot_step!("Scheduler (4 workers)",   Ok(()));
    boot_step!("Firmware SMI silence",    { firmware::run(); Ok::<(), &'static str>(()) });

    let _ = firmware_probe::dump;

    apic::mask_all_lvt();
    unsafe {
        apic::lapic_write(apic::LAPIC_LVT_TIMER, (1 << 16) | apic::VEC_SPURIOUS as u32);
        apic::lapic_write(apic::LAPIC_INIT_CNT, 0);
        apic::lapic_write(apic::LAPIC_TPR, 0);
        let svr = apic::lapic_read(apic::LAPIC_SVR);
        if svr & 0x100 == 0 {
            apic::lapic_write(apic::LAPIC_SVR, 0x100 | apic::VEC_SPURIOUS as u32);
        }
        for _ in 0..16 {
            apic::lapic_write(apic::LAPIC_EOI, 0);
        }
    }

    let _ = apic::set_irq(1,  apic::VEC_KEYBOARD, apic::lapic_id());
    let _ = apic::set_irq(14, apic::VEC_ATA_PRI,  apic::lapic_id());
    let _ = apic::set_irq(15, apic::VEC_ATA_SEC,  apic::lapic_id());

    boot_step!("PS/2 keyboard", ps2::init());

    let saved_tpr: u32;
    let saved_svr: u32;
    unsafe {
        saved_tpr = apic::lapic_read(apic::LAPIC_TPR);
        saved_svr = apic::lapic_read(apic::LAPIC_SVR);
        apic::lapic_write(apic::LAPIC_TPR, 0xFF);
        apic::lapic_write(apic::LAPIC_SVR, saved_svr & !0x100);
        for _ in 0..16 {
            apic::lapic_write(apic::LAPIC_EOI, 0);
        }
    }
    crate::interrupts::pixel_mark(4, 255, 0, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(0x70).write(0x80);
        let _ = Port::<u8>::new(0x71).read();
    }
    crate::interrupts::pixel_mark(15, 255, 255, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    crate::interrupts::pixel_mark(5, 0, 255, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    x86_64::instructions::interrupts::disable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    crate::interrupts::pixel_mark(7, 0, 80, 220);

    unsafe {
        apic::lapic_write(apic::LAPIC_SVR, saved_svr | 0x100);
        apic::lapic_write(apic::LAPIC_TPR, saved_tpr);
    }
    crate::interrupts::pixel_mark(8, 220, 220, 0);

    unsafe {
        let ticks = apic::bsp_ticks_per_hz().max(10_000);
        apic::lapic_write(apic::LAPIC_INIT_CNT, ticks);
        apic::lapic_write(apic::LAPIC_LVT_TIMER, (1 << 17) | apic::VEC_TIMER as u32);
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    boot_step!("Interrupts",              Ok(()));
    timing::calibrate();
    boot_step!("Timer calibration",       Ok(()));
    smp::start_aps();
    boot_step!("SMP (AP bringup)",         Ok(()));
    // Register services with mikuD
    {
        let mut svc = mikud::Service::empty();
        svc.name = "kbd";
        svc.description = "keyboard input handler";
        svc.entry = Some(shell::kbd_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 2;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        svc.flags.critical = true;
        svc.on_restart = Some(shell::on_kbd_restart);
        mikud::register_service_ext(svc);
    }
    {
        let mut svc = mikud::Service::empty();
        svc.name = "shell";
        svc.description = "interactive shell";
        svc.entry = Some(shell::shell_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 3;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        svc.flags.critical = true;
        svc.deps = &["kbd"];
        svc.on_restart = Some(shell::on_shell_restart);
        mikud::register_service_ext(svc);
    }
    // netd: NetworkManager/systemd-networkd equivalent. Auto-runs DHCP
    // after the NIC comes up, in its own thread so boot is never blocked
    {
        let mut svc = mikud::Service::empty();
        svc.name = "netd";
        svc.description = "DHCP auto-configuration";
        svc.entry = Some(net::netd_thread);
        svc.restart = mikud::RestartPolicy::OnFailure;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 1;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        mikud::register_service_ext(svc);
    }
    // bdflush: background writeback for the block-layer cache (the
    // flusher-thread half of write-back caching)
    {
        let mut svc = mikud::Service::empty();
        svc.name = "bdflush";
        svc.description = "block cache writeback daemon";
        svc.entry = Some(block::writeback_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 1;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        mikud::register_service_ext(svc);
    }

    // Start mikuD (PID 1 init daemon)
    scheduler::spawn_named(mikud::mikud_main, "mikud", 1);
    boot_step!("mikuD init daemon",        Ok(()));

    console::clear_screen();
    shell::init();

    boot::mark_done();
    // BSP becomes the idle thread. Timer ISR does not preempt (see
    // comment in interrupts.rs::timer_interrupt_handler), so we MUST
    // cooperatively yield each loop iteration - otherwise no other
    // spawned thread (mikud, workers, kbd_thread, shell_thread) ever
    // gets the CPU, and the system "boots" to a frozen prompt
    loop {
        scheduler::yield_now();
        x86_64::instructions::interrupts::enable_and_hlt();
    }
}

fn init_framebuffer() {
    let fb_info = match grub::framebuffer() {
        Some(f) => f,
        None => {
            serial_println!("[kern] warn: no framebuffer from GRUB");
            return;
        }
    };
    if fb_info.bpp == 0 || fb_info.pitch == 0 || fb_info.width == 0 || fb_info.height == 0 {
        serial_println!("[kern] warn: invalid framebuffer params");
        return;
    }
    let bytes_per_pixel = (fb_info.bpp / 8) as usize;
    let pitch           = fb_info.pitch as usize;
    let width           = fb_info.width as usize;
    let height          = fb_info.height as usize;
    let fb_virt = fb_info.addr + grub::HHDM_OFFSET;
    if fb_virt == grub::HHDM_OFFSET {
        serial_println!("[kern] warn: framebuffer address is null");
        return;
    }
    let buffer = unsafe {
        core::slice::from_raw_parts_mut(fb_virt as *mut u8, pitch * height)
    };
    let config = console::FrameBufferConfig {
        width,
        height,
        stride: pitch / bytes_per_pixel,
        bytes_per_pixel,
        is_bgr: true,
    };
    *console::WRITER.lock() = Some(console::Console::new_limine(buffer, config));
    serial_println!("[kern] framebuffer initialized {}x{} {}bpp", width, height, fb_info.bpp);
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    apic::broadcast_halt();
    serial_println!("[panic] {}", info);
    crate::cprintln!(255, 50, 50, "kernel panic: {}", info);
    loop { x86_64::instructions::hlt(); }
}
