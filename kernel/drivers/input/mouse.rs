// PS/2 mouse packet assembly, in the interrupt
//
// The controller delivers one byte at a time with no packet boundary. The only
// frame marker in the protocol is bit 3 of the first byte, always set, so a
// slipped stream shows there and nowhere else. A byte lost during port
// reconfiguration, or a controller that buffered part of a packet before
// reporting was enabled, slips it; uncorrected, every packet is two thirds of
// one and one third of the next, and the cursor runs off diagonally.
//
// Motion is a nine-bit signed value split between the flags byte and the
// movement byte, with the sign in the flags. Y counts upwards and the screen
// downwards; it is inverted here.
//
// Runs in the interrupt handler: atomics only, no locks, no allocation, no
// calls into anything that takes a lock

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU8, AtomicUsize, Ordering};

/// Buttons, in the order the packet has them
pub const BTN_LEFT: u8 = 1 << 0;
pub const BTN_RIGHT: u8 = 1 << 1;
pub const BTN_MIDDLE: u8 = 1 << 2;

/// Bit 3 of the flags byte, set on every first byte and on nothing else
const FLAG_ALWAYS_ONE: u8 = 1 << 3;
const FLAG_X_SIGN: u8 = 1 << 4;
const FLAG_Y_SIGN: u8 = 1 << 5;
/// Set when the movement did not fit in nine bits. The value is then
/// meaningless and the packet's motion is dropped
const FLAG_X_OVERFLOW: u8 = 1 << 6;
const FLAG_Y_OVERFLOW: u8 = 1 << 7;

/// Three bytes, or four when the wheel answered at init
static PACKET_LEN: AtomicUsize = AtomicUsize::new(3);
static BUF: [AtomicU8; 4] = [
    AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0), AtomicU8::new(0),
];
static HAVE: AtomicUsize = AtomicUsize::new(0);

/// Movement accumulated since the last drain, and the buttons as of the last
/// packet. The interrupt adds, a thread takes: the movement is a running total,
/// the buttons a level
static DX: AtomicI32 = AtomicI32::new(0);
static DY: AtomicI32 = AtomicI32::new(0);
static WHEEL: AtomicI32 = AtomicI32::new(0);
static BUTTONS: AtomicU8 = AtomicU8::new(0);
/// Bumped whenever a packet changes anything, so a poller can tell "no
/// movement" from "nothing arrived"
static SEQ: AtomicU32 = AtomicU32::new(0);
/// Packets dropped for arriving out of frame
static RESYNCS: AtomicU32 = AtomicU32::new(0);

/// The last position from an absolute device, normalised to 0..=0xFFFF, and
/// whether it is new since the last drain
static ABS_X: AtomicI32 = AtomicI32::new(0);
static ABS_Y: AtomicI32 = AtomicI32::new(0);
static ABS_NEW: AtomicBool = AtomicBool::new(false);
static ABS_SEEN: AtomicBool = AtomicBool::new(false);

/// Whether a mouse answered at init. Separates a mouse nobody moved from a
/// machine with no pointing device on the auxiliary port
static PRESENT: AtomicBool = AtomicBool::new(false);

pub fn set_packet_len(n: usize) {
    PACKET_LEN.store(n.clamp(3, 4), Ordering::Relaxed);
    HAVE.store(0, Ordering::Relaxed);
    PRESENT.store(true, Ordering::Relaxed);
}

pub fn present() -> bool {
    PRESENT.load(Ordering::Relaxed)
}

/// One byte from the auxiliary port. Called from the interrupt
pub fn feed(byte: u8) {
    let mut have = HAVE.load(Ordering::Relaxed);

    // A first byte without the marker bit: the stream has slipped. Bytes are
    // dropped until one could be a first byte
    if have == 0 && byte & FLAG_ALWAYS_ONE == 0 {
        RESYNCS.fetch_add(1, Ordering::Relaxed);
        return;
    }

    BUF[have].store(byte, Ordering::Relaxed);
    have += 1;
    if have < PACKET_LEN.load(Ordering::Relaxed) {
        HAVE.store(have, Ordering::Relaxed);
        return;
    }
    HAVE.store(0, Ordering::Relaxed);
    decode();
}

fn decode() {
    let flags = BUF[0].load(Ordering::Relaxed);
    let bx = BUF[1].load(Ordering::Relaxed);
    let by = BUF[2].load(Ordering::Relaxed);

    // The ninth bit is the sign, in the flags. Sign-extended by hand: the byte
    // is the low eight bits of a nine-bit two's complement number, not an i8
    let extend = |v: u8, negative: bool| -> i32 {
        if negative { v as i32 - 256 } else { v as i32 }
    };
    let mut dx = extend(bx, flags & FLAG_X_SIGN != 0);
    // Y counts up the screen and the framebuffer counts down it
    let mut dy = -extend(by, flags & FLAG_Y_SIGN != 0);
    if flags & FLAG_X_OVERFLOW != 0 {
        dx = 0;
    }
    if flags & FLAG_Y_OVERFLOW != 0 {
        dy = 0;
    }

    if PACKET_LEN.load(Ordering::Relaxed) == 4 {
        // The wheel is a four-bit signed value in the low nibble; the high
        // nibble carries the fourth and fifth buttons, unused here
        let z = BUF[3].load(Ordering::Relaxed) & 0x0f;
        let z = if z & 0x08 != 0 { z as i32 - 16 } else { z as i32 };
        if z != 0 {
            WHEEL.fetch_add(z, Ordering::Relaxed);
        }
    }

    if dx != 0 {
        DX.fetch_add(dx, Ordering::Relaxed);
    }
    if dy != 0 {
        DY.fetch_add(dy, Ordering::Relaxed);
    }
    BUTTONS.store(flags & 0x07, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

/// A HID boot-protocol mouse report from the USB stack. USB delivers whole
/// reports or nothing, so no framing; the movement goes into the same
/// accumulators.
///
/// HID counts Y downwards like the screen, so no inversion; HID counts the
/// wheel positive away from the user, opposite to the PS/2 nibble, so it is
/// inverted
pub fn feed_hid(buttons: u8, dx: i32, dy: i32, wheel: i32) {
    if dx != 0 {
        DX.fetch_add(dx, Ordering::Relaxed);
    }
    if dy != 0 {
        DY.fetch_add(dy, Ordering::Relaxed);
    }
    if wheel != 0 {
        WHEEL.fetch_add(-wheel, Ordering::Relaxed);
    }
    BUTTONS.store(buttons & 0x07, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

/// A report from a device stating where the pointer is, not how far it moved: a
/// drawing tablet, a touch screen, QEMU's tablet.
///
/// The position is normalised to 0..=0xFFFF on both axes here, where the
/// device's logical maximum is known; everything above scales that range to the
/// screen.
///
/// Absolute and relative are not merged. With one of each plugged in, adding a
/// delta to a position the other just stated would make them fight; the last
/// statement wins
pub fn feed_abs(buttons: u8, x: u32, y: u32, wheel: i32) {
    ABS_X.store(x.min(0xFFFF) as i32, Ordering::Relaxed);
    ABS_Y.store(y.min(0xFFFF) as i32, Ordering::Relaxed);
    ABS_NEW.store(true, Ordering::Relaxed);
    if wheel != 0 {
        WHEEL.fetch_add(-wheel, Ordering::Relaxed);
    }
    BUTTONS.store(buttons & 0x07, Ordering::Relaxed);
    SEQ.fetch_add(1, Ordering::Relaxed);
}

/// The last absolute position stated, if one has been stated since the last
/// call. Reading clears the flag and keeps the position: a tablet reports only
/// on change, and the pointer stays between reports
pub fn take_abs() -> Option<(i32, i32)> {
    if !ABS_NEW.swap(false, Ordering::Relaxed) {
        return None;
    }
    Some((ABS_X.load(Ordering::Relaxed), ABS_Y.load(Ordering::Relaxed)))
}

/// Whether anything absolute has ever reported. An absolute device makes the
/// host's pointer and the guest's the same pointer, which a compositor can
/// report
pub fn absolute() -> bool {
    ABS_SEEN.load(Ordering::Relaxed)
}

/// A HID mouse was configured, whether or not it has moved; recorded separately
/// from the packet count
pub fn hid_attached() {
    PRESENT.store(true, Ordering::Relaxed);
}

/// An absolute pointing device was configured
pub fn abs_attached() {
    PRESENT.store(true, Ordering::Relaxed);
    ABS_SEEN.store(true, Ordering::Relaxed);
}

/// Everything since the last call: movement, wheel, and the current buttons.
/// Taking clears the movement; the buttons are a level and stay
pub fn take() -> (i32, i32, i32, u8) {
    (
        DX.swap(0, Ordering::Relaxed),
        DY.swap(0, Ordering::Relaxed),
        WHEEL.swap(0, Ordering::Relaxed),
        BUTTONS.load(Ordering::Relaxed),
    )
}

/// Packets seen and packets dropped for arriving out of frame
pub fn counts() -> (u32, u32) {
    (SEQ.load(Ordering::Relaxed), RESYNCS.load(Ordering::Relaxed))
}
