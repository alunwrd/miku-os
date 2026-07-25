use x86_64::instructions::port::Port;

pub fn reboot() -> ! {
    crate::serial_println!("[power] reboot via 0x64");

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

    unsafe {
        let mut port: Port<u8> = Port::new(0xCF9);
        port.write(0x06);
    }

    loop {
        x86_64::instructions::hlt();
    }
}

pub fn shutdown() -> ! {
    crate::serial_println!("[power] ACPI shutdown");

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
