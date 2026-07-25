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

mod kcore;
mod mm;
mod arch;
use crate::arch::x86_64::{acpi, apic, gdt, grub, interrupts, smp};
use crate::mm::{allocator, pmm, vmm};
use crate::kcore::{boot_state, firmware, time};
use core::panic::PanicInfo;
mod net;
mod process;
use crate::process::{solib, ldso};
mod sched;
mod syscall;
mod drivers;
mod io;
use crate::io::block;
use crate::io::console;
mod fs;
use crate::fs::vfs;
use crate::drivers::gpu::nvidia;
use crate::drivers::input::{ps2, usb};
mod shell;
pub mod mikud;

/// Ring-3 shell on the root filesystem. When present it replaces the
/// in-kernel shell (see the "shell" service registration below)
const USERSPACE_SHELL: &str = "/bin/msh";

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
    crate::kcore::stack_guard::init();
    serial_println!("[kern] MikuOS starting (Release v{})", env!("CARGO_PKG_VERSION"));
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
    sched::reinit_scheduler();
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
    // Latch the kernel page-table root while we are guaranteed to be on it;
    // vmm::kernel_cr3() serves this value from here on
    vmm::latch_kernel_cr3();
    boot_step!("ACPI (RSDP/MADT)",         acpi::init());
    boot_step!("APIC", apic::init_bsp());
    boot_step!("IO-APIC",                  apic::ioapic_init());
    apic::init_timer(apic::TIMER_HZ_DEFAULT);
    boot_step!("LAPIC timer",              Ok(()));
    // Seed the wall clock from the CMOS before anything can create a file.
    // Until this existed the only source was NTP, so every inode written
    // before DHCP finished was stamped with time 0
    crate::kcore::clock::init_from_rtc();
    boot_step!("Real-time clock",          Ok(()));
    let bsp_lapic = apic::lapic_id();
    let _ = apic::set_irq(1,  apic::VEC_KEYBOARD, bsp_lapic);
    let _ = apic::set_irq(14, apic::VEC_ATA_PRI,  bsp_lapic);
    let _ = apic::set_irq(15, apic::VEC_ATA_SEC,  bsp_lapic);
    boot_step!("IRQ routing", Ok(()));
    boot_step!("Virtual file system",       vfs::core::init_vfs());
    crate::process::solib::init();
    for lib in crate::process::ldso::MIKU_LIBS {
        crate::process::solib::preload(lib.name, lib.bytes.to_vec());
    }
    crate::process::solib::ldconfig();
    boot_step!("Shared library cache",      Ok(()));
    boot_step!("Block device probe",        { block::probe(); Ok::<(), &'static str>(()) });
    boot_step!("Block device nodes (/dev)", { vfs::core::register_block_nodes(); Ok::<(), &'static str>(()) });
    boot_step!("Network subsystem",         net::init());
    boot_step!("Firmware store",            firmware::load::init());
    boot_step!("NVIDIA GPU probe",          nvidia::init());
    sched::init_main_thread();
    let workers = sched::default_worker_count();
    sched::init_workers(workers);
    boot_step!("Scheduler",   Ok(()));
    boot_step!("Firmware SMI silence",    { firmware::run(); Ok::<(), &'static str>(()) });

    let _ = firmware::probe::dump;

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
    crate::arch::x86_64::interrupts::pixel_mark(4, 255, 0, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    unsafe {
        use x86_64::instructions::port::Port;
        Port::<u8>::new(0x70).write(0x80);
        let _ = Port::<u8>::new(0x71).read();
    }
    crate::arch::x86_64::interrupts::pixel_mark(15, 255, 255, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    serial_println!("[debug] about to enable interrupts (sti #1 - test)");
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    serial_println!("[debug] interrupts enabled, disabling again");
    crate::arch::x86_64::interrupts::pixel_mark(5, 0, 255, 0);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    x86_64::instructions::interrupts::disable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    crate::arch::x86_64::interrupts::pixel_mark(7, 0, 80, 220);

    unsafe {
        apic::lapic_write(apic::LAPIC_SVR, saved_svr | 0x100);
        apic::lapic_write(apic::LAPIC_TPR, saved_tpr);
    }
    crate::arch::x86_64::interrupts::pixel_mark(8, 220, 220, 0);

    // Rearm the LAPIC timer for periodic mode. This has to happen *after* the
    // SVR re-enable above: while the local APIC is software-disabled (SVR
    // bit 8 clear) the hardware forces every LVT mask bit to 1 and ignores
    // attempts to clear them, so a timer programmed inside that window comes
    // out masked and never fires. mask_all_lvt() earlier masked it too, hence
    // the full DIV/INIT_CNT/LVT reload rather than just an unmask
    unsafe {
        let ticks = apic::bsp_ticks_per_hz().max(10_000);
        apic::lapic_write(apic::LAPIC_DIV_CONF, 0x3);  // divide by 16
        apic::lapic_write(apic::LAPIC_LVT_TIMER, (1 << 17) | apic::VEC_TIMER as u32);
        apic::lapic_write(apic::LAPIC_INIT_CNT, ticks);
        let lvt = apic::lapic_read(apic::LAPIC_LVT_TIMER);
        serial_println!(
            "[apic] timer armed: init_cnt={} vec={:#x} lvt={:#010x} masked={}",
            ticks, apic::VEC_TIMER, lvt, (lvt & (1 << 16)) != 0
        );
    }

    interrupts::dump_gate(apic::VEC_TIMER);
    interrupts::dump_gate(apic::VEC_KEYBOARD);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    serial_println!("[debug] about to enable interrupts (sti #2 - final)");
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    serial_println!("[debug] interrupts enabled, entering calibrate()");
    boot_step!("Interrupts",              Ok(()));
    time::calibrate();
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
        let userspace_shell = vfs::core::with_vfs(|v| v.resolve_path(0, USERSPACE_SHELL).is_ok());
        let mut svc = mikud::Service::empty();
        svc.name = "shell";
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 3;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        svc.flags.critical = true;
        svc.deps = &["kbd"];
        if userspace_shell {
            svc.description = "interactive shell (ring 3)";
            svc.exec_start_path = Some(USERSPACE_SHELL);
            serial_println!("[shell] using userspace shell {}", USERSPACE_SHELL);
        } else {
            svc.description = "interactive shell (in-kernel fallback)";
            svc.entry = Some(shell::shell_thread);
            svc.on_restart = Some(shell::on_shell_restart);
            serial_println!("[shell] {} not present, using in-kernel shell", USERSPACE_SHELL);
        }
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
    // usbd: native xHCI + USB HID keyboard. Required on real hardware where
    // usb_handoff kills the BIOS legacy PS/2 emulation; feeds the same stdin
    // ring the PS/2 IRQ uses
    {
        let mut svc = mikud::Service::empty();
        svc.name = "usbd";
        svc.description = "xHCI host + USB HID input";
        svc.entry = Some(usb::usbd_thread);
        svc.restart = mikud::RestartPolicy::OnFailure;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 2;
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
    // kswapd: page reclaim. This work used to run inside the timer ISR,
    // where taking the PMM/SWAP_MAP locks could deadlock the CPU against a
    // thread already holding them (and the swap write was blocking disk I/O
    // in interrupt context). The tick now only raises a flag for this thread
    {
        let mut svc = mikud::Service::empty();
        svc.name = "kswapd";
        svc.description = "page reclaim / swap daemon";
        svc.entry = Some(crate::mm::swap_map::kswapd_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 1;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        mikud::register_service_ext(svc);
    }

    // Start mikuD (PID 1 init daemon)
    sched::spawn_named(mikud::mikud_main, "mikud", 1);
    boot_step!("mikuD init daemon",        Ok(()));

    console::clear_screen();
    shell::init();

    boot_state::mark_done();
    // BSP becomes the idle thread. Timer ISR does not preempt (see
    // comment in interrupts.rs::timer_interrupt_handler), so we MUST
    // cooperatively yield each loop iteration - otherwise no other
    // spawned thread (mikud, workers, kbd_thread, shell_thread) ever
    // gets the CPU, and the system "boots" to a frozen prompt
    loop {
        sched::yield_now();
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
