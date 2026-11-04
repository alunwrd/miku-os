use x86_64::instructions::port::Port;

/// Reset the machine, trying each mechanism in turn and ending with one that
/// cannot fail.
///
/// The keyboard-controller pulse is first as the gentlest, but does nothing on
/// a board with no i8042, and a single 0x06 write to the reset control register
/// is ignored unless RST_CPU is armed by a preceding 0x02. The triple fault at
/// the end is unconditional: an interrupt taken with no IDT resolves only by
/// the CPU resetting
pub fn reboot() -> ! {
    crate::serial_println!("[power] reboot");

    release_devices();
    x86_64::instructions::interrupts::disable();

    unsafe {
        let mut port: Port<u8> = Port::new(0x64);
        let mut timeout = 0u32;
        loop {
            let status = port.read();
            if status & 0x02 == 0 {
                break;
            }
            timeout += 1;
            if timeout > 100_000 {
                break;
            }
        }
        port.write(0xFE);
    }

    spin_a_little();

    unsafe {
        // Arm RST_CPU, then pulse it together with SYS_RST. 0x0e asks for a
        // full reset, not a warm one, as the GPU may be half-configured
        let mut port: Port<u8> = Port::new(0xCF9);
        port.write(0x02);
        spin_a_little();
        port.write(0x06);
        spin_a_little();
        port.write(0x0E);
    }

    spin_a_little();

    triple_fault();
}

/// Hand the hardware back before the machine stops
///
/// Both ways out of this kernel come through here: the one place to tell a
/// device. A booted GSP-RM whose host vanishes leaves the card in a state the
/// next boot inherits (`gfw progress before boot`).
///
/// Called before interrupts are disabled: talking to GSP-RM means a message
/// queue and a doorbell. Every step is bounded and may fail: a machine must
/// always be able to turn off
fn release_devices() {
    crate::drivers::gpu::nvidia::shutdown();
}

/// A delay for a reset request to act before the next is tried, not needing the
/// timer subsystem
fn spin_a_little() {
    for _ in 0..1_000_000 {
        core::hint::spin_loop();
    }
}

/// Load an empty IDT and take an exception: the CPU cannot deliver it, cannot
/// deliver the resulting #DF either, and resets
fn triple_fault() -> ! {
    use x86_64::structures::DescriptorTablePointer;
    use x86_64::VirtAddr;

    crate::serial_println!("[power] falling back to triple fault");
    let null_idt = DescriptorTablePointer { limit: 0, base: VirtAddr::new(0) };
    unsafe {
        x86_64::instructions::tables::lidt(&null_idt);
        core::arch::asm!("int3", options(noreturn));
    }
}

pub fn shutdown() -> ! {
    crate::serial_println!("[power] ACPI shutdown");

    release_devices();
    x86_64::instructions::interrupts::disable();

    unsafe {
        let mut port: Port<u16> = Port::new(0x604);
        port.write(0x2000);
    }

    unsafe {
        let mut port: Port<u16> = Port::new(0xB004);
        port.write(0x2000);
    }

    unsafe {
        let mut port: Port<u16> = Port::new(0x4004);
        port.write(0x3400);
    }

    loop {
        x86_64::instructions::hlt();
    }
}
