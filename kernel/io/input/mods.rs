// Which modifier keys are physically down
//
// `pc-keyboard` tracks modifiers of its own, in `Keyboard::modifiers`,
// answering whether a letter should be uppercase, while mapping it; the field
// is private with no accessor in 0.7. This answers what is held down now, as
// Shift+PageUp needs. The layout switch is the seat's
// (`mayland::input::switch`), since keys a window takes never reach the
// terminal decoder that feeds this.
//
// A layout is handed `Modifiers` on every `map_keycode`, but cannot answer
// this: `process_keyevent` handles Shift and Ctrl itself and returns early
// without calling the layout, so its copy updates only when an ordinary key is
// pressed, stale when only Alt and Shift are held. It never sees LAlt:
// `Modifiers` has
// `lshift, rshift, lctrl, rctrl, numlock, capslock, alt_gr, rctrl2`.
//
// Fed from `KeyEvent`, the crate's own decoded event, one step before
// `process_keyevent`, not from raw scancodes. The crate decodes; this records
// what it decoded.

use core::sync::atomic::{AtomicU8, Ordering};

use pc_keyboard::{KeyCode, KeyEvent, KeyState};

const LSHIFT: u8 = 1 << 0;
const RSHIFT: u8 = 1 << 1;
const LALT:   u8 = 1 << 2;
const RALT:   u8 = 1 << 3;
const LCTRL:  u8 = 1 << 4;
const RCTRL:  u8 = 1 << 5;

static HELD: AtomicU8 = AtomicU8::new(0);

/// Which bit, if this keycode is a modifier at all
fn bit(code: KeyCode) -> Option<u8> {
    Some(match code {
        KeyCode::LShift => LSHIFT,
        KeyCode::RShift => RSHIFT,
        KeyCode::LAlt => LALT,
        // The right Alt is `RAltGr` in this scancode set, and `RAlt2` is the
        // prefix half of the same physical key
        KeyCode::RAltGr | KeyCode::RAlt2 => RALT,
        KeyCode::LControl => LCTRL,
        KeyCode::RControl | KeyCode::RControl2 => RCTRL,
        _ => return None,
    })
}

/// Record one decoded key event. True when it was a modifier and nothing else
/// needs to look at it
pub fn observe(ev: &KeyEvent) -> bool {
    let Some(b) = bit(ev.code) else { return false };
    let held = HELD.load(Ordering::Relaxed);
    let next = match ev.state {
        KeyState::Down => held | b,
        KeyState::Up => held & !b,
        // A key that went down and up in one event holds nothing afterwards
        KeyState::SingleShot => held & !b,
    };
    HELD.store(next, Ordering::Relaxed);
    true
}

pub fn shift() -> bool { HELD.load(Ordering::Relaxed) & (LSHIFT | RSHIFT) != 0 }
pub fn alt()   -> bool { HELD.load(Ordering::Relaxed) & (LALT | RALT) != 0 }
pub fn ctrl()  -> bool { HELD.load(Ordering::Relaxed) & (LCTRL | RCTRL) != 0 }
