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

mod arch;
mod kcore;
mod mm;
use crate::arch::x86_64::{acpi, apic, earlycon, gdt, grub, interrupts, smp};
use crate::kcore::{boot_state, firmware, time};
use crate::mm::{heap, pmm, vmm};
use core::panic::PanicInfo;
mod net;
mod process;
use crate::process::{ldso, solib};
mod drivers;
mod io;
mod sched;
mod syscall;
use crate::io::block;
use crate::io::console;
mod fs;
use crate::drivers::gpu::nvidia;
use crate::drivers::input::{ps2, usb};
use crate::fs::vfs;
pub mod mayland;
pub mod mikud;
mod shell;
// The installer's window. A client of the above, kept out of it: the display
// server does not know what an installer is
pub mod setup;

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
    // Before anything else: say on screen that the kernel got control. A hang
    // before init_framebuffer() is otherwise indistinguishable from the
    // bootloader never handing over
    earlycon::init();
    crate::kcore::stack_guard::init();
    serial_println!(
        "[kern] MikuOS starting (Release v{})",
        env!("CARGO_PKG_VERSION")
    );
    gdt::init();
    earlycon::step("gdt");
    unsafe {
        let cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        core::arch::asm!("mov cr0, {}", in(reg) (cr0 & !(1u64 << 2)) | (1u64 << 1));
        let cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        core::arch::asm!("mov cr4, {}", in(reg) cr4 | (1u64 << 9) | (1u64 << 10));
    }
    serial_println!("[sse] enabled (CR0.EM=0 CR0.MP=1 CR4.OSFXSR=1 CR4.OSXMMEXCPT=1)");
    earlycon::step("sse");
    // CR0.WP, SMEP and SMAP. Needs grub::init to have latched the command line,
    // since `nosmep`/`nosmap` are the escape hatch
    arch::x86_64::cpu::init_protections();
    arch::x86_64::cpu::init_tsc_aux(0);
    arch::x86_64::cpu::report();
    earlycon::step("cpu guards");
    // First thing that costs measurable time: ten milliseconds against PIT
    // channel 2 give a real TSC frequency, so every later udelay is a cycle
    // count. ACPI, PCI, the block probe, USB and PS/2 all time out against this
    kcore::time::calibrate_early();
    earlycon::step("tsc");
    // Before the first syscall: this decides whether each one writes a serial
    // line with interrupts off. `grub::init` has latched the boot line
    arch::x86_64::serial::set_trace(grub::cmdline_flag("trace"));
    syscall::init();
    earlycon::step("syscall MSRs");
    interrupts::init_idt();
    earlycon::step("idt");
    heap::init();
    earlycon::step("kernel heap");
    // Before anything maps an aperture: the write-combining memory type does
    // not exist until the PAT says so, and a mapping made under the old table
    // keeps the old meaning
    mm::vmm::init_pat();
    earlycon::step("pat (write-combining)");
    sched::reinit_scheduler();
    earlycon::step("scheduler");
    grub::set_kernel_address(grub::KERNEL_VMA + grub::KERNEL_PHYS, grub::KERNEL_PHYS);
    init_framebuffer();
    earlycon::step("framebuffer console");
    if let Some(mmap) = grub::memory_map() {
        for entry in mmap {
            let length = entry.length();
            let mem_type = entry.mem_type();
            let base = entry.base();
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
    serial_println!(
        "[kern] _kernel_end phys={:#x} ({}MB)",
        kend_aligned,
        kend_aligned / 1024 / 1024
    );

    // Everything below the end of the image, the first megabyte with it:
    // `smp::start_aps` takes its trampoline page from there
    pmm::reserve_region(grub::KERNEL_PHYS, kend_aligned);

    // What the bootloader left in memory
    //
    // GRUB loads modules into ordinary usable RAM above the kernel and gives
    // their place in the multiboot2 tags, so the frame allocator must not hand
    // those frames out. The information structure goes too: `grub::module()`
    // walks those tags long after boot.
    if let Some((base, len)) = grub::mb2_span() {
        pmm::reserve_region(base, len);
    }
    grub::for_each_module(&mut |start, end| {
        serial_println!(
            "[kern] GRUB module {:#x}..{:#x} ({} KiB) reserved",
            start,
            end,
            (end - start) / 1024
        );
        pmm::reserve_region(start, end - start);
    });
    // Claims its own frames and recovers the previous boot's log, so it runs
    // while the allocator still has everything
    crate::kcore::pstore::init(kend_aligned);
    // The whole log into the store too, for a run whose screen goes dark and
    // whose log is read after a warm reset (`klog::mirror`)
    if grub::cmdline_flag("klogpstore") && crate::kcore::pstore::available() {
        crate::kcore::klog::mirror_to_pstore(true);
        serial_println!("[pstore] 'klogpstore': the kernel log is mirrored into the region");
    }
    // Every reservation is named now, pstore's last, so the bitmap goes where
    // none of them is. From here frames can be handed out, the heap's slabs
    // among them
    pmm::layout();
    // Before the first user address space copies the kernel's half. From here
    // an object above 32 KiB is single frames mapped side by side
    mm::vmap::init();

    boot_step!("Physical memory manager", Ok(()));

    // Both allocate, so neither runs before the heap and the frame allocator.
    // The first reads the multiboot2 tags, safe from the allocator only after
    // `mb2_span` is reserved above. Anything that panics or faults from here on
    // can name its function
    process::symbols::init_kernel();
    earlycon::step("symbols");
    // Needs the TSC frequency, two frames and the heap. From here every exec
    // gets a vDSO
    process::vdso::init();
    earlycon::step("vdso");

    // Now that there are frames to build page tables out of
    remap_framebuffer_wc();
    // Latch the kernel page-table root while we are guaranteed to be on it;
    // vmm::kernel_cr3() serves this value from here on
    vmm::latch_kernel_cr3();
    boot_step!("ACPI (RSDP/MADT)", acpi::init());
    boot_step!("APIC", apic::init_bsp());
    boot_step!("IO-APIC", apic::ioapic_init());
    boot_step!("LAPIC timer", {
        apic::init_timer(apic::TIMER_HZ_DEFAULT);
        Ok::<(), &'static str>(())
    });
    // Seed the wall clock from the CMOS before anything can create a file, or
    // inodes written before DHCP finishes are stamped with time 0
    boot_step!("Real-time clock", {
        crate::kcore::clock::init_from_rtc();
        Ok::<(), &'static str>(())
    });
    boot_step!("IRQ routing", {
        let bsp_lapic = apic::lapic_id();
        let _ = apic::set_irq(1, apic::VEC_KEYBOARD, bsp_lapic);
        let _ = apic::set_irq(12, apic::VEC_MOUSE, bsp_lapic);
        let _ = apic::set_irq(14, apic::VEC_ATA_PRI, bsp_lapic);
        let _ = apic::set_irq(15, apic::VEC_ATA_SEC, bsp_lapic);
        Ok::<(), &'static str>(())
    });
    // One walk of config space for the whole kernel. Every driver below asks
    // this list what is on the bus
    boot_step!("PCI bus scan", {
        let n = drivers::bus::pci::enumerate();
        serial_println!("[pci] {} functions on the bus", n);
        Ok::<(), &'static str>(())
    });
    boot_step!("Virtual file system", vfs::core::init_vfs());
    boot_step!("Shared library cache", {
        crate::process::solib::init();
        let mut bytes = 0usize;
        for lib in crate::process::ldso::MIKU_LIBS {
            bytes += lib.bytes.len();
            crate::process::solib::preload(lib.name, lib.bytes.to_vec());
        }
        // One line for the lot. Which library and how big stays on the `trace`
        // flag: ten lines down a wire with interrupts off is a real cost
        serial_println!(
            "[solib] preloaded {} libraries, {} KB",
            crate::process::ldso::MIKU_LIBS.len(),
            bytes / 1024
        );
        crate::process::solib::ldconfig();
        Ok::<(), &'static str>(())
    });
    boot_step!("Block device probe", {
        block::probe();
        Ok::<(), &'static str>(())
    });
    boot_step!("Block device nodes (/dev)", {
        vfs::core::register_block_nodes();
        Ok::<(), &'static str>(())
    });
    boot_step!("Network subsystem", net::init());
    boot_step!("Firmware store", firmware::load::init());
    boot_step!("NVIDIA GPU probe", nvidia::init());
    boot_step!("Mayland protocol", {
        mayland::init();
        Ok::<(), &'static str>(())
    });
    boot_step!("Scheduler", {
        sched::init_main_thread();
        sched::init_workers(sched::default_worker_count());
        Ok::<(), &'static str>(())
    });
    boot_step!("Firmware SMI silence", {
        firmware::run();
        Ok::<(), &'static str>(())
    });

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

    let _ = apic::set_irq(1, apic::VEC_KEYBOARD, apic::lapic_id());
    let _ = apic::set_irq(12, apic::VEC_MOUSE, apic::lapic_id());
    let _ = apic::set_irq(14, apic::VEC_ATA_PRI, apic::lapic_id());
    let _ = apic::set_irq(15, apic::VEC_ATA_SEC, apic::lapic_id());

    boot_step!("PS/2 keyboard", ps2::init());

    // The mouse is on the same controller and comes up after the keyboard. A
    // machine with no mouse still gets a keyboard: this reports its own failure
    // and does not fail the boot
    match ps2::init_mouse() {
        Ok(len) => crate::drivers::input::mouse::set_packet_len(len),
        Err(e) => serial_println!("[ps2] no mouse: {}", e),
    }

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
    trace_println!("[debug] about to enable interrupts (sti #1 - test)");
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    trace_println!("[debug] interrupts enabled, disabling again");
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

    // Rearm the LAPIC timer, in the mode `init_timer` chose. Has to follow the
    // SVR re-enable above: while the local APIC is software-disabled (SVR bit 8
    // clear) the hardware forces every LVT mask bit to 1 and ignores attempts
    // to clear them, so a timer programmed then comes out masked and never
    // fires. mask_all_lvt() masked it too, hence the full reload
    apic::restart_timer();
    unsafe {
        let lvt = apic::lapic_read(apic::LAPIC_LVT_TIMER);
        serial_println!(
            "[apic] timer armed: vec={:#x} lvt={:#010x} masked={}",
            apic::VEC_TIMER,
            lvt,
            (lvt & (1 << 16)) != 0
        );
    }

    interrupts::dump_gate(apic::VEC_TIMER);
    interrupts::dump_gate(apic::VEC_KEYBOARD);
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    trace_println!("[debug] about to enable interrupts (sti #2 - final)");
    x86_64::instructions::interrupts::enable();
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    trace_println!("[debug] interrupts enabled, entering calibrate()");
    boot_step!("Interrupts", Ok(()));
    boot_step!("Timer calibration", {
        time::calibrate();
        Ok::<(), &'static str>(())
    });
    boot_step!("SMP (AP bringup)", {
        smp::start_aps();
        Ok::<(), &'static str>(())
    });
    // The card's own display, if this machine has one this driver can drive.
    //
    // After the timer: the bring-up measures its timeouts with `Stopwatch`, and
    // the timer is calibrated in the step above but one. Before the target
    // decision below, which asks `console::fb_size()`: the console should
    // already be on the surface the card is scanning.
    //
    // A failure is recorded and stepped over (`nvidia::autoboot`); `nogpu` on
    // the boot line skips it
    boot_step!("NVIDIA display", nvidia::autoboot());
    // Register services with mikuD
    {
        let mut svc = mikud::Service::empty();
        svc.name = "kbd";
        svc.description = "keyboard input handler";
        svc.entry = Some(crate::io::input::keyboard::kbd_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 2;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        svc.flags.critical = true;
        svc.on_restart = Some(shell::on_kbd_restart);
        mikud::register_service_ext(svc);
    }
    // The elevation broker, started by mikuD. Its environment is built here and
    // not inherited from a requester, so no LD_*, TZ or IFS reaches a process
    // about to become root for somebody.
    //
    // It runs as root with every capability, the one process on the machine
    // that does
    {
        let mut svc = mikud::Service::empty();
        svc.name = "mikudod";
        svc.description = "elevation broker (mikudo)";
        svc.exec_start_path = Some("/bin/mikudod");
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 4;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        // Not critical: a machine with no way to become root is usable, with a
        // loud message in the journal
        svc.flags.critical = false;
        mikud::register_service_ext(svc);
    }
    {
        let mut svc = mikud::Service::empty();
        svc.name = "shell";
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::MultiUser;
        svc.priority = 3;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        svc.flags.critical = true;
        svc.deps = &["kbd"];
        svc.description = "interactive shell (ring 0)";
        svc.entry = Some(shell::shell_thread);
        svc.on_restart = Some(shell::on_shell_restart);
        mikud::register_service_ext(svc);

        // `login` is how the console gets an identity; see kernel/shell's
        // `login` command
        serial_println!("[shell] the in-kernel shell is the console; 'login' starts a session");
    }
    // Who owns the card. `net=raw` stands the in-kernel stack down and sends
    // whole frames up through /dev/net0, for a program that wants to be the
    // stack.
    //
    // A boot-line choice: two stacks cannot drain one receive ring between
    // them.
    {
        let mut buf = [0u8; 16];
        let owner = grub::cmdline_arg("net", &mut buf)
            .and_then(|n| core::str::from_utf8(&buf[..n]).ok())
            .unwrap_or("kernel");
        if owner == "raw" {
            net::raw::set_enabled(true);
            serial_println!("[net] card handed to /dev/net0; in-kernel stack idle");
        }
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
    // claiming the controller kills the BIOS legacy PS/2 emulation; feeds the
    // same stdin ring the PS/2 IRQ uses.
    //
    // 'nousb' on the boot line leaves every USB controller with the firmware.
    // This stack does not descend into a keyboard behind a hub or dock yet, and
    // taking the controller away then leaves no input
    if grub::cmdline_flag("nousb") {
        serial_println!(
            "[usb] 'nousb' on the boot line - USB left to the firmware, \
             legacy PS/2 emulation stays up"
        );
    } else {
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
    // kswapd: page reclaim. Not in the timer ISR: the PMM/SWAP_MAP locks could
    // deadlock against a thread holding them, and the swap write is blocking
    // disk I/O. The tick only raises a flag for this thread
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

    // The graphical target
    //
    // Two services, neither started on a `multi-user` boot. Which boot this is
    // gets decided a few lines below; here is what `graphical` means.
    {
        let mut svc = mikud::Service::empty();
        svc.name = "compositor";
        svc.description = "mayland compositing";
        svc.entry = Some(mayland::compositor_thread);
        svc.restart = mikud::RestartPolicy::Always;
        svc.target = mikud::Target::Graphical;
        svc.priority = 3;
        svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
        mikud::register_service_ext(svc);
    }
    // The login screen
    //
    //   - `User=greeter`: it is a large program holding somebody's password, so
    //     it is neither root nor the user being logged in
    //   - one capability, `may_priv`, which lets it put a surface over
    //     everything and take the keyboard. The kernel takes it away when a
    //     session opens (kernel/process/session.rs)
    //   - it waits for mikudod: without the broker nothing can check a password
    //
    // The account is resolved here since this service is registered in code,
    // not from a file. A machine whose /etc has no `greeter` gets no greeter
    // service, since the alternative is a login screen running as root.
    match syscall::adapters::process::passwd_ids("greeter") {
        Some(ids) => {
            let mut svc = mikud::Service::empty();
            svc.name = "greeter";
            svc.description = "graphical login (maygreet)";
            svc.exec_start_path = Some("/bin/maygreet");
            // Always: a greeter that exits because somebody logged in comes
            // back when they log out. The kernel closes the session when its
            // leader exits
            svc.restart = mikud::RestartPolicy::Always;
            svc.target = mikud::Target::Graphical;
            svc.priority = 5;
            svc.restart_delay_ticks = mikud::service::DEFAULT_RESTART_DELAY;
            svc.deps = &["mikudod", "compositor"];
            // Only while nobody is logged in. `Restart=always` brings the login
            // screen back on logout; this stops it coming back over the desktop
            // on login
            svc.conditions[0] = Some(mikud::Condition {
                cond_type: mikud::ConditionType::SeatFree,
                arg: "0",
                negate: false,
            });
            svc.user = Some(ids);
            svc.caps = Some(1u64 << crate::process::cred::CAP_MAY_PRIV);
            mikud::register_service_ext(svc);
        }
        None => serial_println!("[mikud] no 'greeter' account in /etc/passwd, no graphical login"),
    }

    // Which target this boot is
    //
    // A screen, a greeter to put on it, and nobody asking for the developer's
    // console. Anything missing and the machine comes up multi-user, where the
    // console asks who you are: the same login through the same broker, in
    // text.
    //
    // `noauth` is the developer's escape hatch, a boot line flag. Anyone who
    // can type on the GRUB line can boot this machine into anything anyway.
    {
        let has_screen = console::fb_size().is_some();
        let has_greeter = vfs::core::with_vfs(|v| v.resolve_path(0, "/bin/maygreet").is_ok());
        let asked_off = grub::cmdline_flag("noauth");
        let graphical = has_screen && has_greeter && !asked_off;
        mikud::set_default_target(if graphical {
            mikud::Target::Graphical
        } else {
            mikud::Target::MultiUser
        });
        serial_println!(
            "[mikud] booting to {} (screen {}, greeter {}, noauth {})",
            if graphical { "graphical" } else { "multi-user" },
            has_screen,
            has_greeter,
            asked_off
        );
    }

    // Start mikuD (PID 1 init daemon)
    boot_step!("mikuD init daemon", {
        sched::spawn_named(mikud::mikud_main, "mikud", 1);
        Ok::<(), &'static str>(())
    });

    console::clear_screen();
    shell::init();

    // 'run=<command>[;<command>...]' on the boot line, executed once each as if
    // typed.
    //
    // For what should not need a keyboard, a display or a person: 'run=mayland
    // test' runs the protocol selftest under QEMU and prints the result to the
    // serial port. Underscores stand in for the spaces GRUB would split the
    // line on.
    //
    // Several, separated by semicolons: the root filesystem is mounted on
    // request, so starting a program from it is 'mount' and then the program.
    //
    // On a thread of their own, after the boot is done. Until `mark_done` the
    // timer preempts nothing, and a program whose threads spin on a lock whose
    // holder is waiting for a processor would never finish
    {
        let mut buf = [0u8; 1024];
        if let Some(n) = grub::cmdline_arg("run", &mut buf) {
            let line = core::str::from_utf8(&buf[..n])
                .unwrap_or("")
                .replace('_', " ");
            // A full buffer means the list was cut and the last command is a
            // fragment: running `may` for `mayland_test` is worse than nothing,
            // so it is dropped and the cut is reported
            let mut cmds: alloc::vec::Vec<alloc::string::String> =
                line.split(';').map(alloc::string::String::from).collect();
            if n == buf.len() {
                let lost = cmds.pop().unwrap_or_default();
                serial_println!(
                    "[shell] boot line run= is over {} bytes; dropped '{}' and anything after it",
                    buf.len(),
                    lost
                );
            }
            *BOOT_LINE.lock() = cmds;
            sched::spawn_named(run_boot_line, "bootline", 1);
        }
    }

    boot_state::mark_done();
    // BSP becomes the idle thread
    sched::idle::run()
}

/// The boot line's `run=` commands, for `run_boot_line`
static BOOT_LINE: spin::Mutex<alloc::vec::Vec<alloc::string::String>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// Each `run=` command in turn, as if typed, on a task the scheduler treats
/// like any other. Ends when the list does
fn run_boot_line() -> ! {
    let cmds = core::mem::take(&mut *BOOT_LINE.lock());
    for cmd in cmds {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            continue;
        }
        serial_println!("[shell] boot line asked for: '{}'", cmd);
        shell::dispatcher(cmd);
    }
    sched::kill(sched::current_pid());
    loop {
        sched::yield_now();
    }
}

/// Where the framebuffer is, latched by `init_framebuffer` for
/// `remap_framebuffer_wc` to use once there is an allocator
static FB_SPAN: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
static FB_BYTES: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// Map the framebuffer write-combining
///
/// It is reached through the boot HHDM, 1 GiB pages at the default write-back
/// type. Firmware leaves a PCI aperture uncacheable in the MTRRs, UC beats WB,
/// and every pixel written becomes its own bus transaction; write-combining
/// gathers a row into one burst.
///
/// Called after the physical memory manager has its regions, since splitting
/// the huge page covering the aperture needs a few hundred page-table frames.
/// `init_framebuffer` runs earlier, so a failure in between has a screen to be
/// reported on
fn remap_framebuffer_wc() {
    if FB_LATE.load(core::sync::atomic::Ordering::Relaxed) {
        if let Some(fb_info) = grub::framebuffer() {
            mm::vmm::map_mmio_wc(fb_info.addr, fb_info.pitch as u64 * fb_info.height as u64);
            console_on_framebuffer(fb_info);
        }
    }
    let phys = FB_SPAN.load(core::sync::atomic::Ordering::Relaxed);
    let bytes = FB_BYTES.load(core::sync::atomic::Ordering::Relaxed);
    if phys == 0 || bytes == 0 {
        return;
    }
    mm::vmm::map_mmio_wc(phys, bytes);
    serial_println!(
        "[kern] framebuffer {:#x} ({} KiB) mapped write-combining",
        phys,
        bytes / 1024
    );
    // Never read again: the console draws into a copy and stores what it drew.
    // Write-combining speeds stores, not loads, and a scroll that read the
    // aperture cost the GB206 0.78 s a line
    match console::shadow_panel() {
        Ok(n) => serial_println!(
            "[kern] console draws into a {} KiB shadow; the framebuffer is only written",
            n / 1024
        ),
        Err(why) => serial_println!("[kern] console draws on the framebuffer itself: {}", why),
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
    if !arch::x86_64::boot_entry::direct_mapped(
        fb_info.addr,
        fb_info.pitch as u64 * fb_info.height as u64,
    ) {
        FB_LATE.store(true, core::sync::atomic::Ordering::Relaxed);
        serial_println!(
            "[kern] framebuffer at {:#x} is above the boot direct map; the console waits for the frame allocator",
            fb_info.addr
        );
        return;
    }
    console_on_framebuffer(fb_info);
}

/// Set when the framebuffer lies above the entry stub's direct map: mapping it
/// takes page-table frames, so `remap_framebuffer_wc` builds the console
static FB_LATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn console_on_framebuffer(fb_info: grub::FbInfo) {
    // Before the console is built: ask the monitor what it can do and set its
    // best mode, so the console is created at the running size. On a machine
    // this cannot program (every real GPU), what comes back is what firmware
    // handed over
    let fb = crate::drivers::gpu::display::init(crate::drivers::gpu::display::Framebuffer {
        phys: fb_info.addr,
        width: fb_info.width as usize,
        height: fb_info.height as usize,
        pitch: fb_info.pitch as usize,
        bpp: fb_info.bpp as u32,
    });

    let bytes_per_pixel = (fb.bpp / 8) as usize;
    let pitch = fb.pitch;
    let width = fb.width;
    let height = fb.height;
    let fb_virt = fb.phys + grub::HHDM_OFFSET;
    if fb_virt == grub::HHDM_OFFSET {
        serial_println!("[kern] warn: framebuffer address is null");
        return;
    }
    // Where it is and how big, for `remap_framebuffer_wc` below. Splitting the
    // HHDM's huge page needs page-table frames, and the physical memory manager
    // has no regions yet
    FB_SPAN.store(fb.phys, core::sync::atomic::Ordering::Relaxed);
    FB_BYTES.store(
        (pitch * height) as u64,
        core::sync::atomic::Ordering::Relaxed,
    );
    let buffer = unsafe { core::slice::from_raw_parts_mut(fb_virt as *mut u8, pitch * height) };
    let config = console::FrameBufferConfig {
        width,
        height,
        stride: pitch / bytes_per_pixel,
        bytes_per_pixel,
        is_bgr: true,
    };
    *console::WRITER.lock() = Some(console::Console::new_limine(buffer, config));
    serial_println!(
        "[kern] framebuffer initialized {}x{} {}bpp",
        width,
        height,
        fb.bpp
    );
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    x86_64::instructions::interrupts::disable();
    apic::broadcast_halt();
    serial_println!("[panic] {}", info);
    // If the console is a Mayland window, the thread presenting it has just
    // been halted, and the message below would go into a buffer nobody copies.
    // Take the direct path back first, best effort, since a panic can happen
    // with any lock held
    crate::mayland::render::emergency();
    crate::cprintln!(255, 50, 50, "kernel panic: {}", info);
    crate::io::console::flush();
    loop {
        x86_64::instructions::hlt();
    }
}
