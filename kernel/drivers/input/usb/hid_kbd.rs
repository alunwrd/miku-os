// HID boot-protocol keyboard: 8-byte reports -> PS/2 Set-1 scancodes
//
// The shell's input path is pc-keyboard decoding Set-1 scancodes popped from
// the stdin ring (fed by the PS/2 IRQ on machines that have one). Instead of
// teaching the shell a second input format, this module diffs consecutive
// HID boot reports and synthesizes the exact Set-1 make/break byte stream a
// PS/2 keyboard would have produced. stdin::push is lock-free and
// single-producer-friendly; the usbd thread is the only USB-side producer
//
// Boot report layout (HID 1.11 appendix B.1):
//   byte 0: modifier bitmap (LCtrl, LShift, LAlt, LGui, RCtrl, ...)
//   byte 1: reserved
//   bytes 2..7: up to 6 concurrently pressed keys as HID usage codes;
//               all 0x01 = phantom state (rollover), must be ignored

use crate::io::input::stdin;

/// Per-keyboard state: the previous report, for make/break diffing
pub struct KbdState {
    last: [u8; 8],
}

/// Set-1 scancode with an optional 0xE0 prefix, encoded as (extended, code)
type Scan = (bool, u8);

const NONE: Scan = (false, 0);

/// Modifier bit -> Set-1 scancode (byte 0 of the report, bit 0..7)
const MODIFIERS: [Scan; 8] = [
    (false, 0x1D), // LCtrl
    (false, 0x2A), // LShift
    (false, 0x38), // LAlt
    (true,  0x5B), // LGui
    (true,  0x1D), // RCtrl
    (false, 0x36), // RShift
    (true,  0x38), // RAlt
    (true,  0x5C), // RGui
];

/// HID usage (keyboard page) -> Set-1 scancode, usages 0x00..=0x65
const USAGE_TO_SET1: [Scan; 0x66] = [
    NONE, NONE, NONE, NONE,     // 00-03: none, rollover, POST fail, undefined
    (false, 0x1E),              // 04 a
    (false, 0x30),              // 05 b
    (false, 0x2E),              // 06 c
    (false, 0x20),              // 07 d
    (false, 0x12),              // 08 e
    (false, 0x21),              // 09 f
    (false, 0x22),              // 0A g
    (false, 0x23),              // 0B h
    (false, 0x17),              // 0C i
    (false, 0x24),              // 0D j
    (false, 0x25),              // 0E k
    (false, 0x26),              // 0F l
    (false, 0x32),              // 10 m
    (false, 0x31),              // 11 n
    (false, 0x18),              // 12 o
    (false, 0x19),              // 13 p
    (false, 0x10),              // 14 q
    (false, 0x13),              // 15 r
    (false, 0x1F),              // 16 s
    (false, 0x14),              // 17 t
    (false, 0x16),              // 18 u
    (false, 0x2F),              // 19 v
    (false, 0x11),              // 1A w
    (false, 0x2D),              // 1B x
    (false, 0x15),              // 1C y
    (false, 0x2C),              // 1D z
    (false, 0x02),              // 1E 1
    (false, 0x03),              // 1F 2
    (false, 0x04),              // 20 3
    (false, 0x05),              // 21 4
    (false, 0x06),              // 22 5
    (false, 0x07),              // 23 6
    (false, 0x08),              // 24 7
    (false, 0x09),              // 25 8
    (false, 0x0A),              // 26 9
    (false, 0x0B),              // 27 0
    (false, 0x1C),              // 28 Enter
    (false, 0x01),              // 29 Escape
    (false, 0x0E),              // 2A Backspace
    (false, 0x0F),              // 2B Tab
    (false, 0x39),              // 2C Space
    (false, 0x0C),              // 2D -
    (false, 0x0D),              // 2E =
    (false, 0x1A),              // 2F [
    (false, 0x1B),              // 30 ]
    (false, 0x2B),              // 31 backslash
    (false, 0x2B),              // 32 Europe-1 (same position)
    (false, 0x27),              // 33 ;
    (false, 0x28),              // 34 '
    (false, 0x29),              // 35 `
    (false, 0x33),              // 36 ,
    (false, 0x34),              // 37 .
    (false, 0x35),              // 38 /
    (false, 0x3A),              // 39 CapsLock
    (false, 0x3B),              // 3A F1
    (false, 0x3C),              // 3B F2
    (false, 0x3D),              // 3C F3
    (false, 0x3E),              // 3D F4
    (false, 0x3F),              // 3E F5
    (false, 0x40),              // 3F F6
    (false, 0x41),              // 40 F7
    (false, 0x42),              // 41 F8
    (false, 0x43),              // 42 F9
    (false, 0x44),              // 43 F10
    (false, 0x57),              // 44 F11
    (false, 0x58),              // 45 F12
    NONE,                       // 46 PrintScreen (multi-byte, skip)
    (false, 0x46),              // 47 ScrollLock
    NONE,                       // 48 Pause (multi-byte, skip)
    (true,  0x52),              // 49 Insert
    (true,  0x47),              // 4A Home
    (true,  0x49),              // 4B PageUp
    (true,  0x53),              // 4C Delete
    (true,  0x4F),              // 4D End
    (true,  0x51),              // 4E PageDown
    (true,  0x4D),              // 4F Right
    (true,  0x4B),              // 50 Left
    (true,  0x50),              // 51 Down
    (true,  0x48),              // 52 Up
    (false, 0x45),              // 53 NumLock
    (true,  0x35),              // 54 KP /
    (false, 0x37),              // 55 KP *
    (false, 0x4A),              // 56 KP -
    (false, 0x4E),              // 57 KP +
    (true,  0x1C),              // 58 KP Enter
    (false, 0x4F),              // 59 KP 1
    (false, 0x50),              // 5A KP 2
    (false, 0x51),              // 5B KP 3
    (false, 0x4B),              // 5C KP 4
    (false, 0x4C),              // 5D KP 5
    (false, 0x4D),              // 5E KP 6
    (false, 0x47),              // 5F KP 7
    (false, 0x48),              // 60 KP 8
    (false, 0x49),              // 61 KP 9
    (false, 0x52),              // 62 KP 0
    (false, 0x53),              // 63 KP .
    (false, 0x56),              // 64 Europe-2 (102nd key)
    (true,  0x5D),              // 65 App/Menu
];

fn push_key(usage: u8, make: bool) {
    let (ext, code) = if (usage as usize) < USAGE_TO_SET1.len() {
        USAGE_TO_SET1[usage as usize]
    } else {
        NONE
    };
    if code == 0 {
        return;
    }
    if ext {
        stdin::push(0xE0);
    }
    stdin::push(if make { code } else { code | 0x80 });
}

fn push_modifier(bit: u8, make: bool) {
    let (ext, code) = MODIFIERS[bit as usize];
    if ext {
        stdin::push(0xE0);
    }
    stdin::push(if make { code } else { code | 0x80 });
}

impl KbdState {
    pub fn new() -> Self {
        KbdState { last: [0; 8] }
    }

    /// Diff a fresh boot report against the previous one and emit Set-1
    /// make/break codes for every change
    pub fn process_report(&mut self, cur: &[u8; 8]) {
        // phantom state: every key slot reads 0x01 when the keyboard cannot
        // resolve the chord; keep the old state, emit nothing
        if cur[2..8].iter().all(|&k| k == 0x01) {
            return;
        }

        // modifiers
        let changed = self.last[0] ^ cur[0];
        for bit in 0..8u8 {
            if changed & (1 << bit) != 0 {
                push_modifier(bit, cur[0] & (1 << bit) != 0);
            }
        }

        // releases: keys in last but not in cur
        for &k in &self.last[2..8] {
            if k > 0x03 && !cur[2..8].contains(&k) {
                push_key(k, false);
            }
        }
        // presses: keys in cur but not in last
        for &k in &cur[2..8] {
            if k > 0x03 && !self.last[2..8].contains(&k) {
                push_key(k, true);
            }
        }

        self.last = *cur;
    }
}
