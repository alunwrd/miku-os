// PS/2 8042 controller initialization
//
// On UEFI desktops the keyboard works one of two ways:
//  real PS/2 port wired to the LPC/eSPI 8042 (most desktop boards still have one even when no physical connector is exposed) "USB Legacy Support" SMM emulation: firmware presents a fake
//  8042 at ports 0x60/0x64 and synthesises scancodes from USB HID input
//
// Either way the 8042 interface must be configured before IRQ 1 fires.
// GRUB/UEFI does not leave it in a useful state on hand-off:
// port 1 may be disabled (controller blocks IRQ 1)
// keyboard may not be in scan-enabled mode
// config byte often has port-1 IRQ disabled
//
// Taken from: https://wiki.osdev.org/%228042%22_PS/2_Controller

use x86_64::instructions::port::Port;

const DATA:       u16 = 0x60;
const STATUS_CMD: u16 = 0x64;

const STATUS_OUT_FULL: u8 = 1 << 0; // data waiting for the CPU
const STATUS_IN_FULL:  u8 = 1 << 1; // controller still busy with prev write

const CMD_READ_CONFIG:    u8 = 0x20;
const CMD_WRITE_CONFIG:   u8 = 0x60;
const CMD_DISABLE_PORT2:  u8 = 0xA7;
const CMD_DISABLE_PORT1:  u8 = 0xAD;
const CMD_ENABLE_PORT1:   u8 = 0xAE;
const CMD_SELF_TEST:      u8 = 0xAA;
const CMD_TEST_PORT1:     u8 = 0xAB;

const KBD_ENABLE_SCAN:  u8 = 0xF4;

const ACK:               u8 = 0xFA;
const SELF_TEST_PASSED:  u8 = 0x55;
const PORT_TEST_OK:      u8 = 0x00;

const SPIN_TIMEOUT: u32 = 1_000_000;

fn status() -> u8 {
    unsafe { Port::<u8>::new(STATUS_CMD).read() }
}

fn wait_input_clear() -> Result<(), &'static str> {
    for _ in 0..SPIN_TIMEOUT {
        if status() & STATUS_IN_FULL == 0 { return Ok(()); }
        core::hint::spin_loop();
    }
    Err("input buffer never cleared")
}

fn wait_output_full() -> Result<(), &'static str> {
    for _ in 0..SPIN_TIMEOUT {
        if status() & STATUS_OUT_FULL != 0 { return Ok(()); }
        core::hint::spin_loop();
    }
    Err("no response from controller")
}

fn cmd(b: u8) -> Result<(), &'static str> {
    wait_input_clear()?;
    unsafe { Port::<u8>::new(STATUS_CMD).write(b); }
    Ok(())
}

fn write_data(b: u8) -> Result<(), &'static str> {
    wait_input_clear()?;
    unsafe { Port::<u8>::new(DATA).write(b); }
    Ok(())
}

fn read_data() -> Result<u8, &'static str> {
    wait_output_full()?;
    Ok(unsafe { Port::<u8>::new(DATA).read() })
}

fn try_read_data() -> Option<u8> {
    if status() & STATUS_OUT_FULL != 0 {
        Some(unsafe { Port::<u8>::new(DATA).read() })
    } else {
        None
    }
}

fn flush_output() {
    for _ in 0..32 {
        if try_read_data().is_none() { break; }
    }
}

pub fn init() -> Result<(), &'static str> {
    // Probe. An open bus reads as 0xFF; a real (or emulated) controller
    // returns at minimum the system-flag bit set after POST
    let s = status();
    if s == 0xFF {
        return Err("no controller (status=0xFF)");
    }
    crate::serial_println!("[ps2] initial status={:#x}", s);

    // Quiesce both ports
    cmd(CMD_DISABLE_PORT1)?;
    cmd(CMD_DISABLE_PORT2)?;
    flush_output();

    // Read current config so we can preserve unknown bits
    cmd(CMD_READ_CONFIG)?;
    let cfg_in = read_data()?;
    crate::serial_println!("[ps2] cfg before={:#x}", cfg_in);

    // bit 0 = port-1 IRQ enable        -> 1
    // bit 1 = port-2 IRQ enable        -> 0
    // bit 4 = port-1 clock disable     -> 0
    // bit 5 = port-2 clock disable     -> 1 (keep port 2 silent)
    // bit 6 = port-1 translation       -> 1 (emit Set 1 codes; pc-keyboard expects Set 1)
    let cfg_out =
        (cfg_in | (1 << 0) | (1 << 5) | (1 << 6))
              & !((1 << 1) | (1 << 4));
    cmd(CMD_WRITE_CONFIG)?;
    write_data(cfg_out)?;

    // Controller self-test, Some firmwares clobber the config byte
    // during this command, so re-write it on success
    cmd(CMD_SELF_TEST)?;
    let st = read_data()?;
    if st != SELF_TEST_PASSED {
        crate::serial_println!("[ps2] self-test={:#x} (expected 0x55) - continuing", st);
    } else {
        cmd(CMD_WRITE_CONFIG)?;
        write_data(cfg_out)?;
    }

    // Port-1 interface test. Non-zero means the port is shorted/dead;
    // we still try to enable it because some emulated controllers, return junk here:
    cmd(CMD_TEST_PORT1)?;
    let pt = read_data()?;
    if pt != PORT_TEST_OK {
        crate::serial_println!("[ps2] port1 test={:#x} (expected 0x00) - continuing", pt);
    }

    // Activate port 1.
    cmd(CMD_ENABLE_PORT1)?;

    // Drain anything the controller buffered while ports were disabled
    flush_output();

    // Tell the keyboard to start sending scancodes. We deliberately skip
    // KBD_RESET (0xFF) because:
    //     reset triggers a 500ms+ Basic Assurance Test (BAT) and we have, no scheduler-aware delay during init
    //     reset returns the keyboard to power-on defaults, which include scanning DISABLED, so any sloppy timing here leaves the device silent
    //     UEFI/BIOS already exercised reset during POST; the keyboard is in a known-good state on entry
    write_data(KBD_ENABLE_SCAN)?;
    match read_data() {
        Ok(ACK) => {}
        Ok(b)   => crate::serial_println!("[ps2] enable-scan: got {:#x} (no ACK)", b),
        Err(e)  => crate::serial_println!("[ps2] enable-scan: {}", e),
    }
    flush_output();

    crate::serial_println!("[ps2] ready (cfg={:#x})", cfg_out);
    Ok(())
}
