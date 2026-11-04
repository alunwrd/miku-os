// PS/2 8042 controller initialization
//
// On UEFI desktops the keyboard works one of two ways:
//  - a real PS/2 port wired to the LPC/eSPI 8042 (most desktop boards have one
//    even with no physical connector)
//  - "USB Legacy Support" SMM emulation: firmware presents a fake 8042 at ports
//    0x60/0x64 and synthesises scancodes from USB HID input
//
// Either way the 8042 interface must be configured before IRQ 1 fires.
// GRUB/UEFI does not leave it in a useful state on hand-off:
//  - port 1 may be disabled (controller blocks IRQ 1)
//  - keyboard may not be in scan-enabled mode
//  - config byte often has port-1 IRQ disabled
//
// Reference: https://wiki.osdev.org/%228042%22_PS/2_Controller

use core::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering};

use x86_64::instructions::port::Port;

const DATA:       u16 = 0x60;
const STATUS_CMD: u16 = 0x64;

const STATUS_OUT_FULL: u8 = 1 << 0; // data waiting for the CPU
const STATUS_IN_FULL:  u8 = 1 << 1; // controller still busy with prev write

const CMD_READ_CONFIG:    u8 = 0x20;
const CMD_WRITE_CONFIG:   u8 = 0x60;
const CMD_DISABLE_PORT2:  u8 = 0xA7;
const CMD_ENABLE_PORT2:   u8 = 0xA8;
const CMD_TEST_PORT2:     u8 = 0xA9;
const CMD_DISABLE_PORT1:  u8 = 0xAD;
const CMD_ENABLE_PORT1:   u8 = 0xAE;
const CMD_SELF_TEST:      u8 = 0xAA;
const CMD_TEST_PORT1:     u8 = 0xAB;
/// Send the next data byte to port 2. Every command to the mouse is this byte
/// followed by the command
const CMD_WRITE_PORT2:    u8 = 0xD4;

const KBD_ENABLE_SCAN:  u8 = 0xF4;

/// Status bit 5 tells the two ports apart on a read: set means the byte came
/// from the auxiliary device. A mouse packet otherwise reads as three scancodes
const STATUS_FROM_AUX: u8 = 1 << 5;

const ACK:               u8 = 0xFA;
/// The keyboard asking for the last byte again
const RESEND:            u8 = 0xFE;
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
    // Probe. An open bus reads as 0xFF; a real or emulated controller has at
    // least the system-flag bit set after POST
    let s = status();
    if s == 0xFF {
        return Err("no controller (status=0xFF)");
    }
    crate::serial_println!("[ps2] initial status={:#x}", s);

    // Quiesce both ports
    cmd(CMD_DISABLE_PORT1)?;
    cmd(CMD_DISABLE_PORT2)?;
    flush_output();

    // Read the current config to preserve unknown bits
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

    // Controller self-test. Some firmwares clobber the config byte during this
    // command, so it is re-written on success
    cmd(CMD_SELF_TEST)?;
    let st = read_data()?;
    if st != SELF_TEST_PASSED {
        crate::serial_println!("[ps2] self-test={:#x} (expected 0x55) - continuing", st);
    } else {
        cmd(CMD_WRITE_CONFIG)?;
        write_data(cfg_out)?;
    }

    // Port-1 interface test. Non-zero means the port is shorted/dead; it is
    // enabled anyway, as some emulated controllers return junk here
    cmd(CMD_TEST_PORT1)?;
    let pt = read_data()?;
    if pt != PORT_TEST_OK {
        crate::serial_println!("[ps2] port1 test={:#x} (expected 0x00) - continuing", pt);
    }

    // Activate port 1.
    cmd(CMD_ENABLE_PORT1)?;

    // Drain anything the controller buffered while ports were disabled
    flush_output();

    // Tell the keyboard to start sending scancodes. KBD_RESET (0xFF) is
    // skipped:
    //  - it triggers a 500ms+ Basic Assurance Test (BAT), with no
    //    scheduler-aware delay this early in init
    //  - it returns the keyboard to power-on defaults (scanning disabled)
    //  - UEFI/BIOS exercised reset during POST; the keyboard is in a known-good
    //    state on entry
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

// The auxiliary port, and the mouse on it
//
// The same controller, a second device. `init` silences port 2 (clock disabled,
// interrupt off), since mouse packets read as scancodes to the keyboard
// decoder. This turns it back on.
//
// The device protocol is the plain PS/2 mouse one: three bytes a packet, with a
// fourth if the scroll wheel is unlocked by the sample-rate knock below. Byte 0
// is flags; its bit 3 is always set, the only frame marker and what a resync
// looks for.

const MOUSE_RESET:         u8 = 0xFF;
const MOUSE_SET_DEFAULTS:  u8 = 0xF6;
const MOUSE_ENABLE_REPORT: u8 = 0xF4;
const MOUSE_SET_RATE:      u8 = 0xF3;
const MOUSE_GET_ID:        u8 = 0xF2;

const MOUSE_SELF_TEST_OK: u8 = 0xAA;

/// Send one byte to the auxiliary device and read its acknowledgement
fn aux_cmd(b: u8) -> Result<u8, &'static str> {
    cmd(CMD_WRITE_PORT2)?;
    write_data(b)?;
    read_data()
}

/// Bring up the mouse on port 2
///
/// Returns the packet size: three bytes, four if the wheel answered. Separate
/// from `init` and called after it, since a machine with no mouse must still
/// get a keyboard: every failure here is reported and left alone
pub fn init_mouse() -> Result<usize, &'static str> {
    if status() == 0xFF {
        return Err("no controller");
    }
    cmd(CMD_ENABLE_PORT2)?;

    // The interface test is advisory, as on port 1
    cmd(CMD_TEST_PORT2)?;
    match read_data() {
        Ok(0) => {}
        Ok(v) => crate::serial_println!("[ps2] port2 test={:#x} - continuing", v),
        Err(e) => crate::serial_println!("[ps2] port2 test: {} - continuing", e),
    }

    // Undo what `init` did to keep port 2 quiet: clock on, interrupt on
    cmd(CMD_READ_CONFIG)?;
    let cfg = read_data()?;
    let cfg = (cfg | (1 << 1)) & !(1 << 5);
    cmd(CMD_WRITE_CONFIG)?;
    write_data(cfg)?;

    // Reset. The firmware may not have touched the mouse, and its power-on
    // state has reporting disabled. The answer is ACK, then a self-test byte,
    // then a device id; the self-test is slow, so the reads are long
    match aux_cmd(MOUSE_RESET) {
        Ok(ACK) => {}
        Ok(b) => crate::serial_println!("[ps2] mouse reset: got {:#x}", b),
        Err(e) => return Err(e),
    }
    match read_data() {
        Ok(MOUSE_SELF_TEST_OK) => {}
        Ok(b) => crate::serial_println!("[ps2] mouse self-test={:#x} - continuing", b),
        Err(_) => return Err("no mouse answered the reset"),
    }
    let _id = read_data().unwrap_or(0);

    let _ = aux_cmd(MOUSE_SET_DEFAULTS);

    // The knock that unlocks the scroll wheel: sample rates 200, 100, 80 in
    // that order, after which a wheel mouse reports device id 3 and sends a
    // fourth byte. A mouse without one keeps answering 0 and its three-byte
    // packets
    for rate in [200u8, 100, 80] {
        let _ = aux_cmd(MOUSE_SET_RATE);
        let _ = aux_cmd(rate);
    }
    let id = match aux_cmd(MOUSE_GET_ID) {
        Ok(ACK) => read_data().unwrap_or(0),
        _ => 0,
    };
    let packet = if id == 3 { 4 } else { 3 };

    match aux_cmd(MOUSE_ENABLE_REPORT) {
        Ok(ACK) => {}
        Ok(b) => crate::serial_println!("[ps2] mouse enable: got {:#x}", b),
        Err(e) => return Err(e),
    }

    crate::serial_println!(
        "[ps2] mouse ready - id {}, {} byte packets, cfg={:#x}", id, packet, cfg);
    Ok(packet)
}

/// Read one byte if the controller has one and it came from the mouse
///
/// The interrupt handler's read: the source bit keeps a scancode out of the
/// packet assembler and a packet byte out of the shell
pub fn read_aux() -> Option<u8> {
    let s = status();
    if s & STATUS_OUT_FULL == 0 || s & STATUS_FROM_AUX == 0 {
        return None;
    }
    Some(unsafe { Port::<u8>::new(DATA).read() })
}

/// Keyboard LED command (0xED + mask; bit0 scroll, bit1 num, bit2 caps)
const KBD_SET_LEDS: u8 = 0xED;

/// Set the keyboard LEDs, over a real 8042 or USB legacy emulation. An
/// out-of-band status channel for when the framebuffer console may be dead
/// (GSP-RM display init can kill GOP scanout). ACK bytes are drained
/// best-effort; a missing ACK is not an error
pub fn set_leds(mask: u8) {
    let _ = write_data(KBD_SET_LEDS);
    crate::kcore::time::udelay(2_000);
    let _ = try_read_data(); // ACK
    let _ = write_data(mask & 0x07);
    crate::kcore::time::udelay(2_000);
    let _ = try_read_data(); // ACK
}

/// Keyboard typematic command (0xF3 + one byte: rate in bits 0-4, delay in 5-6)
const KBD_SET_TYPEMATIC: u8 = 0xF3;

/// How fast a held key repeats, and how long before it starts
///
/// The kernel counts no milliseconds for a held key: the keyboard's own
/// typematic generates repeat make codes, and every layer above (the shell, the
/// compositor, a client told `repeat_info`) reacts to them.
/// `input { repeat_rate }` has effect only through this byte.
///
/// `rate` is repeats per second and `delay_ms` the wait before the first one.
/// The controller has 32 rates from 2 Hz to 30 Hz and four delays; both are
/// matched to the nearest.
///
/// A rate of zero asks for no repeat, which the hardware cannot do; the slowest
/// is 2 Hz.
///
/// Nothing here waits. The command goes out if the controller can take it now,
/// and the argument follows from the keyboard's interrupt on the ACK
/// (`on_reply`). This is called from a window manager's `set_param` under the
/// compositor's server lock, and on a USB keyboard the 8042 is a slow firmware
/// emulation
pub fn set_typematic(rate: u32, delay_ms: u32) {
    // Rate: the byte's low five bits are two fields, and the period is
    // `(8 + A) * 2^B * 4.17 ms`: index 0 is 30 Hz, index 31 is 2 Hz. Searched,
    // since the table is short
    let want = rate.clamp(2, 30);
    let mut best = 31u8;
    let mut best_err = u32::MAX;
    for i in 0..32u32 {
        let a = i & 7;
        let b = (i >> 3) & 3;
        // In microseconds, so the division below stays in integers
        let period_us = (8 + a) * (1 << b) * 4_170;
        let hz = 1_000_000 / period_us;
        let err = hz.abs_diff(want);
        if err < best_err {
            best_err = err;
            best = i as u8;
        }
    }
    // Delay: 250, 500, 750 or 1000 ms
    let delay = match delay_ms {
        0..=374 => 0u8,
        375..=624 => 1,
        625..=874 => 2,
        _ => 3,
    };
    let byte = (delay << 5) | (best & 0x1f);
    TYPEMATIC_WANT.store(WANT_PENDING | byte as u32, Ordering::Release);
    kick_typematic();
}

/// The typematic byte for the keyboard, with `WANT_PENDING` set until it has
/// gone out. A newer one replaces an older one still unsent
static TYPEMATIC_WANT: AtomicU32 = AtomicU32::new(0);
const WANT_PENDING: u32 = 1 << 8;
/// Exchange state: `STEP_IDLE`, or which byte is waiting for its ACK
static TYPEMATIC_STEP: AtomicU8 = AtomicU8::new(STEP_IDLE);
const STEP_IDLE: u8 = 0;
const STEP_COMMAND: u8 = 1;
const STEP_ARGUMENT: u8 = 2;
/// The argument byte in flight, for a RESEND
static TYPEMATIC_SENT: AtomicU8 = AtomicU8::new(0);
/// When the byte in flight went out. A keyboard that never answers (there may
/// be none behind the controller) must not hold the exchange for ever
static TYPEMATIC_AT_MS: AtomicU64 = AtomicU64::new(0);
/// Past this without an ACK the exchange is given up and may start again
const REPLY_TIMEOUT_MS: u64 = 100;

/// One byte to the keyboard if the controller can take it now, with no wait
fn put_now(b: u8) -> bool {
    let st = status();
    if st == 0xFF || st & STATUS_IN_FULL != 0 {
        return false;
    }
    unsafe { Port::<u8>::new(DATA).write(b); }
    true
}

/// Start the exchange if one is owed and none is in flight
fn kick_typematic() {
    if TYPEMATIC_WANT.load(Ordering::Acquire) & WANT_PENDING == 0 {
        return;
    }
    let now = crate::kcore::time::since_boot_ms();
    let step = TYPEMATIC_STEP.load(Ordering::Acquire);
    if step != STEP_IDLE
        && now.saturating_sub(TYPEMATIC_AT_MS.load(Ordering::Relaxed)) < REPLY_TIMEOUT_MS
    {
        // In flight: the ACK continues from here, and picks up the new byte
        // when the old one is done
        return;
    }
    if TYPEMATIC_STEP.compare_exchange(step, STEP_COMMAND, Ordering::AcqRel, Ordering::Acquire).is_err() {
        return;
    }
    TYPEMATIC_AT_MS.store(now, Ordering::Relaxed);
    if !put_now(KBD_SET_TYPEMATIC) {
        // The controller is busy. Left owed: the next key the keyboard sends,
        // or the next rate set, starts it again
        TYPEMATIC_STEP.store(STEP_IDLE, Ordering::Release);
    }
}

/// A reply byte from the keyboard, from its interrupt
/// (`keyboard_interrupt_handler`). ACKs and RESENDs move the typematic exchange
/// on; other bytes are ignored. No lock and no wait: interrupt context
pub fn on_reply(b: u8) {
    let step = TYPEMATIC_STEP.load(Ordering::Acquire);
    match (b, step) {
        (ACK, STEP_COMMAND) => {
            // The command is taken: the argument is the newest byte asked for,
            // and it is no longer owed
            let want = TYPEMATIC_WANT.fetch_and(!WANT_PENDING, Ordering::AcqRel);
            let byte = (want & 0xFF) as u8;
            TYPEMATIC_SENT.store(byte, Ordering::Relaxed);
            TYPEMATIC_STEP.store(STEP_ARGUMENT, Ordering::Release);
            TYPEMATIC_AT_MS.store(crate::kcore::time::since_boot_ms(), Ordering::Relaxed);
            if !put_now(byte) {
                TYPEMATIC_WANT.fetch_or(WANT_PENDING, Ordering::AcqRel);
                TYPEMATIC_STEP.store(STEP_IDLE, Ordering::Release);
            }
        }
        (ACK, STEP_ARGUMENT) => {
            TYPEMATIC_STEP.store(STEP_IDLE, Ordering::Release);
            // A rate set while this one was going out
            kick_typematic();
        }
        (RESEND, STEP_COMMAND) => { let _ = put_now(KBD_SET_TYPEMATIC); }
        (RESEND, STEP_ARGUMENT) => { let _ = put_now(TYPEMATIC_SENT.load(Ordering::Relaxed)); }
        _ => {}
    }
}

/// A key arrived, so the controller is free and a typematic byte it could not
/// take earlier can go. One atomic load when none is owed
pub fn on_key() {
    if TYPEMATIC_WANT.load(Ordering::Relaxed) & WANT_PENDING != 0 {
        kick_typematic();
    }
}

/// Blink all three LEDs 'times' times (~300 ms period), then leave them in
/// 'final_mask'. A heartbeat visible without a working screen
pub fn blink_leds(times: u32, final_mask: u8) {
    for _ in 0..times {
        set_leds(0x07);
        crate::kcore::time::mdelay(150);
        set_leds(0x00);
        crate::kcore::time::mdelay(150);
    }
    set_leds(final_mask & 0x07);
}
