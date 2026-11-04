// Keyboard routing
//
// Both keyboards, the PS/2 controller's interrupt and the xHCI HID poller, go
// into `io::input::stdin::push` as Set-1 scancodes. This hooks that one place,
// so a client sees the same keys the shell would.
//
// Nothing here touches the display server. `push` runs inside the keyboard
// interrupt, and the server is behind an ordinary spin lock: a thread holding
// it, interrupted on the same core, would deadlock. So the interrupt puts a
// scancode into a lock-free ring and the compositor thread drains it once a
// frame, at a worst case of one frame of latency.
//
// The grab:
//
// While on, keys go to the focused surface and the shell does not see them. The
// way out is handled here, in the interrupt, on atomics alone: it works even if
// the compositor thread is wedged, the server lock is held, or the focused
// client has stopped reading.
//
// The way out is Ctrl+Alt+Escape, with F12 as a shortcut on keyboards having
// one: on a laptop F12 is often the second meaning of a media key.
//
// Ctrl+Alt+Tab moves the keyboard to the next window. Both chords are
// swallowed: they are the compositor's keys

#![allow(dead_code)]

// The mouse, with the same split between the interrupt and the frame
pub mod pointer;
// Who owns the pointer when it is not pointing: a window being carried, or an
// open menu
pub mod grab;
// The numbers a config sets about input: repeat, sensitivity, whether the
// keyboard follows the cursor
pub mod conf;
// Alt+Shift, moving the seat to its next keyboard layout. Watched here, where
// every key passes
pub mod switch;

use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicUsize, Ordering};
use x86_64::instructions::interrupts;

// Set-1 make codes we watch for
const K_ESC:      u8 = 0x01;
const K_LSHIFT:   u8 = 0x2A;
const K_RSHIFT:   u8 = 0x36;
const K_CTRL:     u8 = 0x1D; // right ctrl is the same code, 0xE0-prefixed
const K_ALT:      u8 = 0x38; // right alt likewise
const K_CAPSLOCK: u8 = 0x3A;
const K_LOGO:     u8 = 0x5B; // extended; right logo is 0x5C
const K_F12:      u8 = 0x58;
const K_TAB:      u8 = 0x0F;

/// Modifier bits, in xkb's conventional order. The keymap is NO_KEYMAP, so the
/// compositor sends the mask and the client decides the letter; the bits are
/// with the layout that reads them, in the file both sides compile
pub use crate::mayland::keymap::{MOD_ALT, MOD_CAPS, MOD_CTRL, MOD_LOGO, MOD_SHIFT};
/// What the letters after `bind` mean, shared with the config that wrote them
pub use crate::mayland::protocol::wm::chord;

static GRAB: AtomicBool = AtomicBool::new(false);
/// Set while the focused window is the console's
///
/// The console window is the shell, so keys go on the shell's ordinary input
/// path, not as protocol events to a client that never reads them. Maintained
/// by the drain, once a frame, and read in the interrupt, so atomic
static TO_SHELL: AtomicBool = AtomicBool::new(false);
/// Which modifiers are held, as a MOD_* mask. Maintained in the interrupt and
/// read by the drain, which forwards changes to the focused client
static MODS: AtomicU32 = AtomicU32::new(0);
/// The mask the client was last told about
static MODS_SENT: AtomicU32 = AtomicU32::new(0);
/// The layout group the client and the desktop were last told about. The group
/// itself is the seat's, in `io::input::layout`
static GROUP_SENT: AtomicU32 = AtomicU32::new(0);
/// Set when the previous byte was 0xE0, telling the extended key that follows
/// from the plain one with the same code
static EXTENDED: AtomicBool = AtomicBool::new(false);

const RING_SIZE: usize = 64;

/// Not a keycode: the compositor-level "give the keyboard to the next window"
/// chord, queued in the same ring as the keys around it
///
/// The keys typed before the chord belong to the window with the keyboard then,
/// and those after to the next, so the request goes through the queue, keeping
/// the order
const EV_SWITCH: u16 = 0x7fff;

/// Chords the window manager asked for, queued in the same ring as the keys,
/// for the reason of `EV_SWITCH`: a chord switching workspaces and the keys
/// typed around it stay in order. The id is added to this base, so ids are
/// 0..0x7f, well clear of the switch above them
const EV_CHORD: u16 = 0x7f00;
pub const MAX_CHORD_ID: u32 = 0x7f;

/// How many chords can be bound at once
///
/// Sixty-four: with a `submap`, nine workspaces plus nine "send the window
/// there" plus focus, close and layout is thirty before any personal binding.
///
/// It matches `MAX_BINDS` in the window manager's config: the number is in the
/// protocol, as the window manager spends this budget and this file enforces
/// it. The cost is a linear scan of this many words per key press, in the
/// keyboard interrupt
const MAX_CHORDS: usize = crate::mayland::protocol::wm::MAX_CHORDS;

/// `mods << 16 | keycode` for each bound chord, zero when the slot is free,
/// with the id beside it. Two arrays, so they can be written from a system call
/// and read in an interrupt with no lock
static CHORD_KEY: [AtomicU32; MAX_CHORDS] = {
    const Z: AtomicU32 = AtomicU32::new(0);
    [Z; MAX_CHORDS]
};
static CHORD_ID: [AtomicU32; MAX_CHORDS] = {
    const Z: AtomicU32 = AtomicU32::new(0);
    [Z; MAX_CHORDS]
};
/// The letters after `bind` in the config line of this chord:
/// `protocol::wm::chord`. Read in the interrupt beside the key, since they
/// change what the interrupt does with it
static CHORD_FLAGS: [AtomicU32; MAX_CHORDS] = {
    const Z: AtomicU32 = AtomicU32::new(0);
    [Z; MAX_CHORDS]
};
/// Whether this chord's key is down now
///
/// A chord is matched on the make code, and a held key produces make codes
/// while held (the keyboard's typematic). `binde` asks for that repeat; plain
/// `bind` gets one firing per press
static CHORD_HELD: [AtomicBool; MAX_CHORDS] = {
    const Z: AtomicBool = AtomicBool::new(false);
    [Z; MAX_CHORDS]
};
/// Whether something else was pressed while this chord's key was down
///
/// A release binding on a modifier key (`bindr = SUPER, Super_L`) is a tap of
/// that key alone: Super pressed and released opens the search, Super+Tab does
/// not also open it on the way out. Set by any other key or a mouse button
/// going down while the chord is held, cleared when its key goes down afresh
/// (`spoil_held`)
static CHORD_SPOILT: [AtomicBool; MAX_CHORDS] = {
    const Z: AtomicBool = AtomicBool::new(false);
    [Z; MAX_CHORDS]
};

/// The keycodes of the modifier keys themselves: Ctrl, Shift, Alt, Super,
/// left and right
fn is_modifier_key(keycode: u32) -> bool {
    matches!(keycode, 29 | 97 | 42 | 54 | 56 | 100 | 125 | 126)
}

/// Something other than `keycode` went down: every held release chord on a
/// modifier key is no longer a tap of that key alone. 0 for a mouse button
pub fn spoil_held(keycode: u32) {
    for i in 0..MAX_CHORDS {
        let key = CHORD_KEY[i].load(Ordering::Relaxed) & 0xffff;
        if key != 0 && key != keycode && CHORD_HELD[i].load(Ordering::Relaxed)
            && CHORD_FLAGS[i].load(Ordering::Relaxed) & chord::RELEASE != 0 && is_modifier_key(key)
        {
            CHORD_SPOILT[i].store(true, Ordering::Relaxed);
        }
    }
}

/// Whether any bound chord asked to ignore the modifiers it did not name
///
/// `bindi` needs a second scan of the table on every key matching nothing
/// exactly, nearly every key. Almost no config has one, so the second scan is
/// skipped outright
static ANY_LOOSE: AtomicBool = AtomicBool::new(false);

/// Packed key events: keycode in the low 15 bits, pressed in bit 15
static RING: [AtomicU16; RING_SIZE] = {
    const Z: AtomicU16 = AtomicU16::new(0);
    [Z; RING_SIZE]
};
static HEAD: AtomicUsize = AtomicUsize::new(0);
static TAIL: AtomicUsize = AtomicUsize::new(0);

/// Keys dropped because the ring was full: a client that stopped reading
static OVERRUNS: AtomicUsize = AtomicUsize::new(0);

pub fn grabbed() -> bool { GRAB.load(Ordering::Relaxed) }

/// Whether a pre-session screen holds the keyboard and must keep it
///
/// Read from the keyboard interrupt, so atomic, not a look at the server:
/// `intercept` runs with the server lock possibly held by the compositor
/// thread. Written by `iface::layer` on the compositor thread when the lock is
/// taken or released
static LOCKED: AtomicBool = AtomicBool::new(false);

/// Say whether the login screen is up. See `LOCKED`
pub fn set_locked(on: bool) {
    LOCKED.store(on, Ordering::Relaxed);
    crate::serial_println!("[mayland] keyboard lock {}", if on { "on" } else { "off" });
}

/// Whether the login screen is up
pub fn locked() -> bool {
    LOCKED.load(Ordering::Relaxed)
}

pub fn set_grab(on: bool) {
    GRAB.store(on, Ordering::Relaxed);
    EXTENDED.store(false, Ordering::Relaxed);
}

pub fn overruns() -> usize { OVERRUNS.load(Ordering::Relaxed) }

/// Ask for a chord. Binding an id that is already bound replaces it
pub fn bind_chord(id: u32, mods: u32, keycode: u32, flags: u32) -> Result<(), &'static str> {
    if id > MAX_CHORD_ID {
        return Err("bind_chord: the id is out of range");
    }
    if keycode == 0 || keycode > 0x7fff {
        return Err("bind_chord: not a keycode");
    }
    // Caps lock is a latch, not something held down, and caps on must not stop
    // a chord working
    let packed = ((mods & !MOD_CAPS) << 16) | (keycode & 0xffff);

    let mut free = None;
    for i in 0..MAX_CHORDS {
        if CHORD_KEY[i].load(Ordering::Relaxed) != 0 {
            if CHORD_ID[i].load(Ordering::Relaxed) == id {
                // Re-bound to a different key or letters. What was held was
                // held under the old chord, and the mark would leave the new
                // one refusing to fire
                CHORD_HELD[i].store(false, Ordering::Relaxed);
                CHORD_FLAGS[i].store(flags, Ordering::Relaxed);
                CHORD_KEY[i].store(packed, Ordering::Relaxed);
                note_loose();
                return Ok(());
            }
        } else if free.is_none() {
            free = Some(i);
        }
    }
    let Some(i) = free else { return Err("bind_chord: no room for another chord") };
    // The id and the flags first: a slot with a key and the wrong id beside it
    // would fire as the wrong chord, and the interrupt reads them in this order
    CHORD_ID[i].store(id, Ordering::Relaxed);
    CHORD_FLAGS[i].store(flags, Ordering::Relaxed);
    CHORD_HELD[i].store(false, Ordering::Relaxed);
    CHORD_KEY[i].store(packed, Ordering::Release);
    note_loose();
    Ok(())
}

/// Recompute whether the loose match is worth looking for
///
/// Read on every key matching nothing, written only when the window manager
/// changes a binding: at startup and on a submap switch
fn note_loose() {
    let any = (0..MAX_CHORDS).any(|i| {
        CHORD_KEY[i].load(Ordering::Relaxed) != 0
            && CHORD_FLAGS[i].load(Ordering::Relaxed) & chord::IGNORE_MODS != 0
    });
    ANY_LOOSE.store(any, Ordering::Relaxed);
}

pub fn unbind_chord(id: u32) {
    for i in 0..MAX_CHORDS {
        if CHORD_KEY[i].load(Ordering::Relaxed) != 0
            && CHORD_ID[i].load(Ordering::Relaxed) == id
        {
            CHORD_KEY[i].store(0, Ordering::Release);
            CHORD_HELD[i].store(false, Ordering::Relaxed);
        }
    }
    note_loose();
}

/// Forget every chord. The window manager holding them has gone away, and a
/// chord nobody acts on is a dead key
pub fn clear_chords() {
    for i in 0..MAX_CHORDS {
        CHORD_KEY[i].store(0, Ordering::Release);
        CHORD_HELD[i].store(false, Ordering::Relaxed);
    }
    ANY_LOOSE.store(false, Ordering::Relaxed);
}

pub fn chords_bound() -> usize {
    (0..MAX_CHORDS).filter(|&i| CHORD_KEY[i].load(Ordering::Relaxed) != 0).count()
}

/// The slot of the chord this key completes, if any
///
/// An exact match on the modifiers, not a subset: Super+2 must not fire on
/// Ctrl+Super+2. `bindi` asks for the other behaviour, looked for only after
/// every exact match failed, so one `bindi` cannot shadow exact chords above it
/// in the file
fn chord_slot(mods: u32, keycode: u16) -> Option<usize> {
    if keycode == 0 {
        return None;
    }
    let held = mods & !MOD_CAPS;
    let exact = ((held) << 16) | keycode as u32;
    if let Some(i) = (0..MAX_CHORDS)
        .find(|&i| CHORD_KEY[i].load(Ordering::Acquire) == exact)
    {
        return Some(i);
    }
    if !ANY_LOOSE.load(Ordering::Relaxed) {
        return None;
    }
    (0..MAX_CHORDS).find(|&i| {
        let packed = CHORD_KEY[i].load(Ordering::Acquire);
        if packed == 0 || (packed & 0xffff) as u16 != keycode {
            return false;
        }
        CHORD_FLAGS[i].load(Ordering::Relaxed) & chord::IGNORE_MODS != 0
            && held & (packed >> 16) == packed >> 16
    })
}

/// The slot whose key is this one and is down
///
/// What a release is matched against, ignoring the modifiers. Super is often
/// lifted before Q, so a release matched the ordinary way would not find its
/// chord, which would stay held for ever
fn held_slot(keycode: u16) -> Option<usize> {
    if keycode == 0 {
        return None;
    }
    (0..MAX_CHORDS).find(|&i| {
        CHORD_KEY[i].load(Ordering::Acquire) & 0xffff == keycode as u32
            && CHORD_HELD[i].load(Ordering::Relaxed)
    })
}

/// The chord a mouse button completes, if any
///
/// The table the keyboard uses. evdev numbers mouse buttons from 0x110 and
/// keyboard keys stop below 0x100, so `mouse:272` is a keycode no keyboard
/// produces, and a mouse binding is an ordinary chord with no second table, id
/// space or path through the window manager. So `bindm` in a config file is
/// `bind` with a different key name.
///
/// Called from the pointer's drain on the compositor thread, so it reads the
/// modifier mask directly
pub fn chord_for_button(code: u32) -> Option<u32> {
    if code > 0xffff {
        return None;
    }
    // On the press only. A button has no typematic, so `binde` means nothing to
    // one; `bindr` is not done here: the release of a mouse button ends a drag,
    // decided in the pointer's own drain
    let i = chord_slot(MODS.load(Ordering::Relaxed), code as u16)?;
    if CHORD_FLAGS[i].load(Ordering::Relaxed) & chord::RELEASE != 0 {
        return None;
    }
    Some(CHORD_ID[i].load(Ordering::Relaxed))
}

/// Offered every scancode before the shell sees it. True means consumed
///
/// Called from the keyboard interrupt. Atomics only; see the note above
pub fn intercept(scancode: u8) -> bool {
    // The window manager's chords are watched for whether or not a client has
    // the keyboard: Super+2 has to switch workspaces while the shell is being
    // typed at.
    //
    // Everything below runs unconditionally, and the grab decides only what
    // happens to keys that are not chords
    let grabbed = GRAB.load(Ordering::Relaxed);
    // The focused window is the console, so the keys are the shell's. The
    // prefix byte is passed on too: the shell has its own state machine for
    // extended keys, and half a two-byte sequence would corrupt its arrow keys
    let to_shell = TO_SHELL.load(Ordering::Relaxed);
    if scancode == 0xE0 {
        EXTENDED.store(true, Ordering::Relaxed);
        return grabbed && !to_shell;
    }
    let extended = EXTENDED.swap(false, Ordering::Relaxed);
    let pressed = scancode & 0x80 == 0;
    let make = scancode & 0x7f;

    track_modifier(make, extended, pressed);
    // Before anything can return: the switch is the seat's, so it fires whoever
    // has the keyboard (a window, the console, the login screen)
    let alt_or_shift = (!extended && matches!(make, K_LSHIFT | K_RSHIFT)) || make == K_ALT;
    switch::observe(MODS.load(Ordering::Relaxed), alt_or_shift, pressed);

    let keycode = to_keycode(make, extended);

    // The way out, before anything else can go wrong.
    //
    // Ctrl+Alt+Escape, not a single key: on a laptop F12 is often the secondary
    // meaning of a media key. Escape, ctrl and alt exist everywhere, and the
    // three together are not a chord a client wants.
    //
    // Only while the grab is on: these take a key away from whoever is typing,
    // and the shell may see F12 when the compositor is not holding the
    // keyboard.
    //
    // Except while the login screen is up. Before anybody has logged in, what
    // is behind the screen is a ring-0 console that is root, and a two-key way
    // into it is not a rescue. The chord is swallowed, so nothing happens
    let mods = MODS.load(Ordering::Relaxed);
    let chord = mods & MOD_CTRL != 0 && mods & MOD_ALT != 0;
    let locked = LOCKED.load(Ordering::Relaxed);
    if grabbed && pressed && ((!extended && make == K_ESC && chord)
                              || (!extended && make == K_F12))
    {
        if locked {
            return true;
        }
        GRAB.store(false, Ordering::Relaxed);
        // The client must not be left believing keys are down
        MODS.store(0, Ordering::Relaxed);
        switch::rearm();
        return true;
    }

    // Ctrl+Alt+Tab moves the keyboard to the next window, under the same two
    // modifiers as the way out. The key is swallowed: the window losing focus
    // should not see a tab
    if grabbed && pressed && !extended && make == K_TAB && chord {
        // Under the lock there is one surface and nothing to move to
        if !locked {
            push_event(EV_SWITCH, true);
        }
        return true;
    }

    // The window manager's chords, after the compositor's own two: the way out
    // of a grab must not be something a window manager can bind over. `bindn`
    // does not change that: it lets a key through to the window as well, and
    // the two chords above have returned by now.
    //
    // A press is matched on modifiers and key together; a release only on the
    // key, as `held_slot` explains.
    //
    // Under the lock they do not fire: `close`, `workspace` and `exec` would be
    // a bound key reaching past the login screen
    if pressed && keycode != 0 {
        spoil_held(keycode as u32);
    }
    let slot = if locked { None }
               else if pressed { chord_slot(mods, keycode) } else { held_slot(keycode) };
    if let Some(i) = slot {
        let flags = CHORD_FLAGS[i].load(Ordering::Relaxed);
        let id = CHORD_ID[i].load(Ordering::Relaxed);
        let was_held = CHORD_HELD[i].swap(pressed, Ordering::Relaxed);
        if pressed && !was_held {
            CHORD_SPOILT[i].store(false, Ordering::Relaxed);
        }
        let fire = if pressed {
            // Once per press unless `binde` asked for more. `bindr` waits
            flags & chord::RELEASE == 0
                && (!was_held || flags & chord::REPEAT != 0)
        } else {
            // Only if this chord swallowed the press and, for a modifier key,
            // only if nothing else was pressed meanwhile
            flags & chord::RELEASE != 0 && was_held && !CHORD_SPOILT[i].load(Ordering::Relaxed)
        };
        if fire {
            push_event(EV_CHORD + (id as u16 & 0x7f), true);
        }
        // Swallowed whole, press and release, unless `bindn` asked for it to go
        // through as well. A client seeing the release of a key whose press it
        // never got would have that key stuck down
        if flags & chord::PASSTHROUGH == 0 {
            return true;
        }
    }

    // Past the chords, and either nothing has taken the keyboard or the window
    // that has is the console's: the key is the shell's, untouched.
    //
    // Never while the lock is up. Neither condition should hold there (the lock
    // forces the grab on and pins the focus away from the console), and this
    // path ends in a root prompt
    if !locked && (!grabbed || to_shell) {
        return false;
    }

    if keycode != 0 {
        push_event(keycode, pressed);
    }
    true
}

/// Keep the modifier mask current. Called for every key, pressed or released,
/// and the key is still delivered: a client needs to see ctrl go down
fn track_modifier(make: u8, extended: bool, pressed: bool) {
    let bit = match (make, extended) {
        (K_LSHIFT, false) | (K_RSHIFT, false) => MOD_SHIFT,
        (K_CTRL, _) => MOD_CTRL,
        (K_ALT, _)  => MOD_ALT,
        (K_LOGO, true) | (0x5C, true) => MOD_LOGO,
        (K_CAPSLOCK, false) => {
            // Caps is a latch, not a modifier held down: it flips on press and
            // the release means nothing
            if pressed {
                MODS.fetch_xor(MOD_CAPS, Ordering::Relaxed);
            }
            return;
        }
        _ => return,
    };
    if pressed {
        MODS.fetch_or(bit, Ordering::Relaxed);
    } else {
        MODS.fetch_and(!bit, Ordering::Relaxed);
    }
}

pub fn modifiers() -> u32 { MODS.load(Ordering::Relaxed) }

/// Serialises the two producers. See `push_event`
static PUSH_LOCK: AtomicBool = AtomicBool::new(false);

fn push_event(keycode: u16, pressed: bool) {
    let packed = (keycode & 0x7fff) | if pressed { 0x8000 } else { 0 };

    // Two producers.
    //
    // The ring is drained by one thread, so the consumer side needs nothing.
    // The producer side is `stdin::push`, reached by the PS/2 keyboard
    // interrupt and the usbd thread carrying xHCI HID reports. On several cores
    // they run at the same instant, and "load the tail, write the slot, store
    // the tail" run twice loses one event and writes one slot twice.
    //
    // Interrupts off before the lock, not merely around the write: the usbd
    // thread taking it can be interrupted by the keyboard interrupt wanting it,
    // on its own core
    interrupts::without_interrupts(|| {
        while PUSH_LOCK
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            core::hint::spin_loop();
        }
        let tail = TAIL.load(Ordering::Relaxed);
        let next = (tail + 1) % RING_SIZE;
        if next == HEAD.load(Ordering::Acquire) {
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        } else {
            RING[tail].store(packed, Ordering::Relaxed);
            TAIL.store(next, Ordering::Release);
        }
        PUSH_LOCK.store(false, Ordering::Release);
    });
}

fn pop_event() -> Option<(u16, bool)> {
    let head = HEAD.load(Ordering::Relaxed);
    if head == TAIL.load(Ordering::Acquire) {
        return None;
    }
    let packed = RING[head].load(Ordering::Relaxed);
    HEAD.store((head + 1) % RING_SIZE, Ordering::Release);
    Some((packed & 0x7fff, packed & 0x8000 != 0))
}

/// Hand everything queued to the focused surface. Called once a frame from the
/// compositor thread, where taking the server lock is safe
pub fn drain(time_ms: u32) -> usize {
    // Which terminal the focused window is showing, if any: the interrupt only
    // reads a flag, asked once a frame
    let focus_tty = crate::mayland::server::focused_terminal();
    if let Some(t) = focus_tty {
        crate::io::tty::set_active(t);
    } else {
        crate::io::tty::set_active(crate::io::tty::SYSTEM);
    }
    // The keys go past the display protocol into the terminal when a terminal
    // has the focus: for the console the kernel shell, for a pty whatever is
    // reading it
    TO_SHELL.store(focus_tty.is_some(), Ordering::Relaxed);

    // Nobody to type at. A client dying while holding the keyboard is the
    // likeliest way to be left unable to type, so the grab goes with it.
    //
    // Not while a session owns the seat. There "no focus" is an empty
    // workspace, and the next window to open takes the keyboard, which needs
    // the grab: nothing turns it back on inside a session
    let seat = crate::process::session::SEAT0;
    if GRAB.load(Ordering::Relaxed)
        && crate::mayland::server::focus_client().is_none()
        && !crate::process::session::seat_has_session(seat)
    {
        set_grab(false);
        MODS.store(0, Ordering::Relaxed);
        crate::serial_println!("[mayland] no window has focus - keyboard back to the shell");
        return 0;
    }

    // Modifiers first: a client receiving the key before the state it was typed
    // in would misread the first keystroke of every chord. The group goes in
    // the same event, and to the desktop as well for a bar to show
    let mods = MODS.load(Ordering::Relaxed);
    let group = crate::io::input::layout::group();
    let mods_moved = mods != MODS_SENT.swap(mods, Ordering::Relaxed);
    let group_moved = group != GROUP_SENT.swap(group, Ordering::Relaxed);
    if mods_moved || group_moved {
        crate::mayland::server::modifiers(mods);
    }
    if group_moved {
        crate::mayland::server::keyboard_layout_changed();
    }
    let mut n = 0;
    while let Some((keycode, pressed)) = pop_event() {
        if keycode == EV_SWITCH {
            // The modifier state the new window is owed goes out with the
            // `enter`, in `set_focus`: every focus change needs it
            if let Some((c, s)) = crate::mayland::server::focus_next() {
                crate::serial_println!(
                    "[mayland] keyboard moved to client {} surface {}", c, s);
            }
            continue;
        }
        if keycode >= EV_CHORD && keycode < EV_SWITCH {
            crate::mayland::server::wm_chord((keycode - EV_CHORD) as u32);
            continue;
        }
        crate::mayland::server::key(keycode as u32, pressed, time_ms);
        n += 1;
    }
    n
}

/// Set-1 make code to the keycode a client sees
///
/// For the first eighty-odd keys these are the same number: Linux's evdev codes
/// were taken from the AT set, so ESC is 1, the digit row is 2..11, and F12 is
/// 88 in both. Kept for an eventual xkb keymap expecting ordinary keycodes.
///
/// The extended (0xE0-prefixed) keys differ and need the table below. Anything
/// not in it returns 0 and is dropped: a key a client cannot name must not be
/// sent as another key
fn to_keycode(make: u8, extended: bool) -> u16 {
    if !extended {
        // 0x59 is the last code in the shared range; above it the AT set has
        // keys evdev numbers elsewhere
        return if make > 0 && make <= 0x58 { make as u16 } else { 0 };
    }
    match make {
        0x1C => 96,  // KEY_KPENTER
        0x1D => 97,  // KEY_RIGHTCTRL
        0x35 => 98,  // KEY_KPSLASH
        0x38 => 100, // KEY_RIGHTALT
        0x47 => 102, // KEY_HOME
        0x48 => 103, // KEY_UP
        0x49 => 104, // KEY_PAGEUP
        0x4B => 105, // KEY_LEFT
        0x4D => 106, // KEY_RIGHT
        0x4F => 107, // KEY_END
        0x50 => 108, // KEY_DOWN
        0x51 => 109, // KEY_PAGEDOWN
        0x52 => 110, // KEY_INSERT
        0x53 => 111, // KEY_DELETE
        0x5B => 125, // KEY_LEFTMETA
        0x5C => 126, // KEY_RIGHTMETA
        _ => 0,
    }
}
