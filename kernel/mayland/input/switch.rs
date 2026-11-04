// The chord that moves the seat to its next keyboard layout
//
// Hyprland's `kb_options = grp:alt_shift_toggle`, xkb's: the group moves on
// when the second of Alt and Shift goes down while the first is held, in either
// order and on either side. Here, not in the terminal decoder: `intercept` is
// the one place every key passes, and a key a window takes never reaches
// `kbd_thread`. See `mayland-architecture.md`, "Keyboard layouts".
//
// In the keyboard interrupt, so atomics only. The keys are not swallowed: a
// window may see Alt and Shift go down, and an Alt+Shift+key binding has both
// held when the key arrives.

use core::sync::atomic::{AtomicBool, Ordering};

use crate::io::input::layout;

use super::{MOD_ALT, MOD_SHIFT};

/// Whether the pair has been released since it last switched
///
/// The keyboard repeats make codes while a key is down, at the hardware's
/// typematic rate, so without this holding Alt+Shift for half a second would
/// switch the layout twenty times
static ARMED: AtomicBool = AtomicBool::new(true);

/// Look at one key after the modifier mask has taken it. True when the group
/// moved, so the drain can tell the window and the bar
pub fn observe(mods: u32, is_alt_or_shift: bool, pressed: bool) -> bool {
    let both = mods & MOD_ALT != 0 && mods & MOD_SHIFT != 0;
    if !both {
        ARMED.store(true, Ordering::Relaxed);
        return false;
    }
    if !pressed || !is_alt_or_shift || layout::switch() != layout::SWITCH_ALT_SHIFT {
        return false;
    }
    if !ARMED.swap(false, Ordering::Relaxed) {
        return false;
    }
    // One layout has nothing to move between, and every window would get a
    // group change that changed nothing
    if layout::groups() < 2 {
        return false;
    }
    layout::next_group();
    true
}

/// Forget a half-pressed pair. The grab going away clears the modifier mask
/// under the keys, and a latch left disarmed would eat the next real switch
pub fn rearm() {
    ARMED.store(true, Ordering::Relaxed);
}
