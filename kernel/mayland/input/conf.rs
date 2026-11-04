// The five numbers that say how the machine behaves under a hand
//
// Hyprland's `input {}` block, arriving down the param channel that carries
// every other number a config sets. One file, because three places read them:
// the pointer's drain scales movement and flips the wheel, the focus path asks
// whether the keyboard follows the cursor, and the seat tells a keyboard its
// repeat rate.
//
// Atomics, no locks. Two of the readers are on the compositor thread holding
// the server lock, and one, the keyboard's typematic, reaches a hardware
// controller.
//
// `kb_layout` and `kb_options` arrive here too, as numbers, and are handed to
// the seat's layouts in `io::input::layout`, which the keyboard interrupt
// reads.
//
// Not here: `kb_variant`, `accel_profile`, and the touchpad settings. There is
// no xkb to vary a layout with, no acceleration curve, and no touchpad driver.
// The window manager names them as unsupported; see `maywm`'s config
// dictionary.

use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

use crate::mayland::protocol::wm::param;

/// Key repeats per second, and the wait before the first repeat, for a machine
/// with no config
const DEFAULT_RATE: u32 = 25;
const DEFAULT_DELAY: u32 = 600;
static REPEAT_RATE: AtomicU32 = AtomicU32::new(DEFAULT_RATE);
static REPEAT_DELAY: AtomicU32 = AtomicU32::new(DEFAULT_DELAY);

static FOLLOW_MOUSE: AtomicBool = AtomicBool::new(false);
static NATURAL_SCROLL: AtomicBool = AtomicBool::new(false);

/// Pointer movement is multiplied by this over 256
///
/// Fixed point: read in the pointer's drain on every packet, and the remainder
/// below has to be exact. A factor of 0.5 on one-pixel movements is zero pixels
/// every time if the fraction is dropped
static SENSITIVITY: AtomicU32 = AtomicU32::new(UNIT);
const UNIT: u32 = 256;
/// The part of a scaled movement not spent yet, or a slow drag under a factor
/// below one would stand still
static REM_X: AtomicI32 = AtomicI32::new(0);
static REM_Y: AtomicI32 = AtomicI32::new(0);

/// One of `input {}`'s numbers, or not ours
///
/// Returns whether it was ours, so the caller can offer the param to others.
/// Unknown params are not an error: the window manager may be newer than the
/// compositor
pub fn set(p: u32, v: f32) -> bool {
    match p {
        param::REPEAT_RATE => {
            // A rate of zero is a keyboard that never repeats, which someone
            // may want; above the hardware's fastest is clamped
            let hz = v.clamp(0.0, 100.0) as u32;
            if REPEAT_RATE.swap(hz, Ordering::Relaxed) != hz {
                apply_typematic();
            }
            true
        }
        param::REPEAT_DELAY => {
            let ms = v.clamp(0.0, 4000.0) as u32;
            if REPEAT_DELAY.swap(ms, Ordering::Relaxed) != ms {
                apply_typematic();
            }
            true
        }
        param::FOLLOW_MOUSE => {
            // Hyprland has four settings; two concern which window the focus
            // follows with the pointer over a floating one, a case not
            // distinguished here. Non-zero means the keyboard follows the
            // cursor, as all three non-zero settings agree
            FOLLOW_MOUSE.store(v != 0.0, Ordering::Relaxed);
            true
        }
        param::SENSITIVITY => {
            // Hyprland writes -1..1 with 0 meaning as the device sends it. Read
            // here as a plain factor of 1 + value, clamped to a quarter and
            // four times: there is no acceleration curve
            let f = (1.0 + v).clamp(0.25, 4.0);
            SENSITIVITY.store((f * UNIT as f32) as u32, Ordering::Relaxed);
            // The leftovers belonged to the old factor
            REM_X.store(0, Ordering::Relaxed);
            REM_Y.store(0, Ordering::Relaxed);
            true
        }
        param::NATURAL_SCROLL => {
            NATURAL_SCROLL.store(v != 0.0, Ordering::Relaxed);
            true
        }
        param::KB_LAYOUTS => {
            // Exact: the packing is at most sixteen bits, and an f32 carries
            // twenty-four. A value the window manager should have refused is
            // refused here too, as it came from ring 3
            if v < 0.0 || v != (v as u32) as f32 || !crate::io::input::layout::set_layouts(v as u32) {
                crate::serial_println!("[mayland] kb_layout: {} names no layouts this machine has", v);
            }
            true
        }
        param::KB_SWITCH => {
            crate::io::input::layout::set_switch(v.clamp(0.0, 1.0) as u8);
            true
        }
        _ => false,
    }
}

/// The numbers, back to a machine with no config
///
/// Through `set`, so the side effects come with them: the typematic has to be
/// told
pub fn reset() {
    set(param::REPEAT_RATE, DEFAULT_RATE as f32);
    set(param::REPEAT_DELAY, DEFAULT_DELAY as f32);
    set(param::FOLLOW_MOUSE, 0.0);
    set(param::SENSITIVITY, 0.0);
    set(param::NATURAL_SCROLL, 0.0);
    crate::io::input::layout::reset();
}

/// What a keyboard is told when it binds: repeats per second, then the wait
pub fn repeat_info() -> (i32, i32) {
    (REPEAT_RATE.load(Ordering::Relaxed) as i32,
     REPEAT_DELAY.load(Ordering::Relaxed) as i32)
}

pub fn follow_mouse() -> bool {
    FOLLOW_MOUSE.load(Ordering::Relaxed)
}

/// Movement, with the sensitivity in it
///
/// The remainder is kept between calls, so a factor below one slows the cursor
/// without stopping it. Saturating throughout: the deltas are small from a
/// relative mouse and as wide as the screen from an absolute one
pub fn scale(dx: i32, dy: i32) -> (i32, i32) {
    let f = SENSITIVITY.load(Ordering::Relaxed) as i32;
    if f == UNIT as i32 {
        return (dx, dy);
    }
    (spend(&REM_X, dx, f), spend(&REM_Y, dy, f))
}

fn spend(rem: &AtomicI32, d: i32, f: i32) -> i32 {
    if d == 0 {
        return 0;
    }
    let total = d.saturating_mul(f).saturating_add(rem.load(Ordering::Relaxed));
    let whole = total / UNIT as i32;
    rem.store(total - whole * UNIT as i32, Ordering::Relaxed);
    whole
}

/// Wheel notches, the way round the config asked for
pub fn wheel(notches: i32) -> i32 {
    if NATURAL_SCROLL.load(Ordering::Relaxed) {
        notches.wrapping_neg()
    } else {
        notches
    }
}

/// Tell the keyboard controller
///
/// A held key repeats because the keyboard's own typematic generates make
/// codes, with no milliseconds counted in this kernel. `repeat_info` on the
/// wire tells a client what to expect; this makes it true
fn apply_typematic() {
    let (rate, delay) = repeat_info();
    crate::drivers::input::ps2::set_typematic(rate as u32, delay as u32);
}
