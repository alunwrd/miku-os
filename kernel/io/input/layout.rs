// What a key means, before anything decides what to do about it
//
// US 104-key under the crate's decoder, and the seat's layouts beside it: the
// group in use, the list a config asked for, and the chord that moves between
// them. The tables are libmay's (`keymap::LAYOUTS`), since every window reads
// them too; see `mayland-architecture.md`, "Keyboard layouts".
//
// The key the crate's US layout drops:
//
// `pc-keyboard` 0.7 decodes set-1 scancode `0x2B` to `KeyCode::Oem7`
// (`scancodes/set1.rs:68`), while its `Us104Key` layout has an arm only for
// `Oem5` (`layouts/us104.rs:217`), where `'\'` and `'|'` live. So the key
// decodes to a `DecodedKey::RawKey(Oem7)` with no meaning, and the character is
// lost.
//
// Of everything set 1 can produce and the US layout does not map, this is the
// only printable key. The rest are modifiers, the function row, media keys, and
// the Japanese `Oem9`..`Oem13` at scancodes `0x70`..`0x7D`.
//
// A layout and not a special case in the keyboard thread:
//
//   * Shift. `Keyboard::modifiers` is a private field with no accessor in
//     0.7, so a fix in the decode loop would track shift make/break scancodes
//     itself, a second record of the keyboard's state. A layout is handed
//     the modifiers.
//   * It has to be a character. `\` arrives through `tty::feed_char`, as every
//     other character. `key_sequence` turns non-character keys into escape
//     sequences through `tty::feed_key`, which passes them only in raw mode,
//     so a `\` routed that way would vanish in canonical mode.
//   * It fixes the key for every consumer of the decoder.

use core::sync::atomic::{AtomicU32, AtomicU8, Ordering};

use pc_keyboard::{
    layouts::Us104Key, DecodedKey, HandleControl, KeyCode, KeyboardLayout, Modifiers,
};

use crate::mayland::keymap::{self, Layout, MAX_GROUPS, MOD_CAPS, MOD_SHIFT};

/// US 104-key, with the one key the crate's own version loses, and the other
/// layouts through libmay's tables
pub struct MikuUs104;

/// Which of the seat's layouts is in use, an index into `LAYOUTS`
///
/// A mode, not a modifier: Cyrillic is a whole keyboard's worth of letters.
/// Read in the keyboard interrupt and by the terminal decoder, so atomic
static GROUP: AtomicU8 = AtomicU8::new(0);

/// The layouts the seat moves between, as `param::KB_LAYOUTS` packs them: four
/// bits per group, each an index into `keymap::LAYOUTS` plus one, zero past the
/// last. `us,ru` until a config says otherwise
static LAYOUTS: AtomicU32 = AtomicU32::new(DEFAULT_LAYOUTS);
const DEFAULT_LAYOUTS: u32 = 0x21;

/// Which chord moves between them: `param::KB_SWITCH`'s numbers
static SWITCH: AtomicU8 = AtomicU8::new(SWITCH_ALT_SHIFT);
pub const SWITCH_NONE: u8 = 0;
pub const SWITCH_ALT_SHIFT: u8 = 1;

/// How many layouts the seat has
pub fn groups() -> usize {
    let packed = LAYOUTS.load(Ordering::Relaxed);
    (0..MAX_GROUPS).take_while(|&g| (packed >> (4 * g)) & 0xf != 0).count().max(1)
}

/// The layout group `g` is, or US for a group past the end
pub fn layout_of(g: usize) -> &'static Layout {
    let i = (LAYOUTS.load(Ordering::Relaxed) >> (4 * g.min(MAX_GROUPS - 1))) & 0xf;
    keymap::LAYOUTS.get((i as usize).wrapping_sub(1)).unwrap_or(&keymap::LAYOUTS[0])
}

/// The layouts in group order, for serialising a keymap
pub fn layouts(out: &mut [&'static Layout; MAX_GROUPS]) -> usize {
    let n = groups();
    for (g, slot) in out.iter_mut().enumerate().take(n) {
        *slot = layout_of(g);
    }
    n
}

/// The group in use
pub fn group() -> u32 {
    GROUP.load(Ordering::Relaxed) as u32
}

/// Move to the next layout, and answer with the group now in use. Called from
/// the keyboard interrupt: atomics only
pub fn next_group() -> u32 {
    let n = groups() as u8;
    let g = (GROUP.load(Ordering::Relaxed) + 1) % n;
    GROUP.store(g, Ordering::Relaxed);
    g as u32
}

/// Put the seat in group `g`, or the last one there is. The keyboard's own
/// way there is `next_group`; this is for a program that names the layout
pub fn set_group(g: u32) {
    let last = groups() as u32 - 1;
    GROUP.store(g.min(last) as u8, Ordering::Relaxed);
}

/// The packing in use, as `set_layouts` takes it
pub fn packed_layouts() -> u32 {
    LAYOUTS.load(Ordering::Relaxed)
}

/// Take a config's `kb_layout`. False when the packing names a layout this
/// machine does not have, which the window manager has already refused; checked
/// here too, as the value comes from ring 3
pub fn set_layouts(packed: u32) -> bool {
    let mut seen_end = false;
    for g in 0..MAX_GROUPS {
        let i = (packed >> (4 * g)) & 0xf;
        if i == 0 {
            seen_end = true;
        } else if seen_end || i as usize > keymap::LAYOUTS.len() {
            return false;
        }
    }
    if packed & 0xf == 0 || packed >> (4 * MAX_GROUPS) != 0 {
        return false;
    }
    LAYOUTS.store(packed, Ordering::Relaxed);
    // A list that got shorter has no group where the old one was
    if GROUP.load(Ordering::Relaxed) as usize >= groups() {
        GROUP.store(0, Ordering::Relaxed);
    }
    true
}

pub fn set_switch(v: u8) {
    SWITCH.store(v, Ordering::Relaxed);
}

pub fn switch() -> u8 {
    SWITCH.load(Ordering::Relaxed)
}

/// Back to a machine with no config: `us,ru` and Alt+Shift
///
/// The group in use stays if the list still has it: a config is reset and
/// re-applied on every save, and someone typing Russian keeps typing Russian
pub fn reset() {
    set_layouts(DEFAULT_LAYOUTS);
    set_switch(SWITCH_ALT_SHIFT);
}

/// The evdev number of a key the layouts may change, from the crate's name for
/// it
///
/// The alphanumeric block: everything a layout in `keymap::LAYOUTS` replaces
/// except the keypad's decimal key, which stays the crate's since the crate
/// knows num lock (with it off the key is Delete). A key outside the block is
/// the crate's in every layout: Enter, the arrows, the function row
fn evdev(code: KeyCode) -> Option<u16> {
    use KeyCode as K;
    Some(match code {
        K::Oem8 => 41,
        K::Key1 => 2, K::Key2 => 3, K::Key3 => 4, K::Key4 => 5, K::Key5 => 6,
        K::Key6 => 7, K::Key7 => 8, K::Key8 => 9, K::Key9 => 10, K::Key0 => 11,
        K::OemMinus => 12, K::OemPlus => 13,
        K::Q => 16, K::W => 17, K::E => 18, K::R => 19, K::T => 20,
        K::Y => 21, K::U => 22, K::I => 23, K::O => 24, K::P => 25,
        K::Oem4 => 26, K::Oem6 => 27,
        K::A => 30, K::S => 31, K::D => 32, K::F => 33, K::G => 34,
        K::H => 35, K::J => 36, K::K => 37, K::L => 38,
        K::Oem1 => 39, K::Oem3 => 40,
        // `0x2B`, the backslash key; see the note above
        K::Oem7 => 43,
        K::Z => 44, K::X => 45, K::C => 46, K::V => 47, K::B => 48,
        K::N => 49, K::M => 50,
        K::OemComma => 51, K::OemPeriod => 52, K::Oem2 => 53,
        _ => return None,
    })
}

impl KeyboardLayout for MikuUs104 {
    fn map_keycode(
        &self,
        keycode: KeyCode,
        modifiers: &Modifiers,
        handle_ctrl: HandleControl,
    ) -> DecodedKey {
        // Another layout, through the table every window reads. Not under Ctrl:
        // Ctrl-C stays Ctrl-C under any alphabet, and the crate below makes
        // control characters out of the Latin letters
        let g = group() as usize;
        if g != 0 && !modifiers.is_ctrl() {
            if let Some(kc) = evdev(keycode) {
                let mut mods = 0;
                if modifiers.is_shifted() {
                    mods |= MOD_SHIFT;
                }
                if modifiers.capslock {
                    mods |= MOD_CAPS;
                }
                if let Some(c) = keymap::to_char_in(layout_of(g), kc, mods) {
                    return DecodedKey::Unicode(c);
                }
            }
        }
        match keycode {
            // The `\` and `|` key. The two characters and the shift test are
            // the crate's own `Oem5` arm, reaching a keycode the scancode table
            // produces
            KeyCode::Oem7 => {
                if modifiers.is_shifted() {
                    DecodedKey::Unicode('|')
                } else {
                    DecodedKey::Unicode('\\')
                }
            }
            // Everything else is the crate's. A copied table could drift from
            // the scancode set it is paired with
            other => Us104Key.map_keycode(other, modifiers, handle_ctrl),
        }
    }
}
