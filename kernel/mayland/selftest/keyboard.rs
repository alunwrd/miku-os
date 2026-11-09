// The seat's keyboard layouts: the switch, the group on the wire, the keymap
//
// The tables themselves are tested on the host (`lib/gfx/mayland`,
// `cargo test-host`); here is what only a running compositor has: the switch in
// the keyboard interrupt, the group in `modifiers`, the bar's event, and a
// config's list reaching keymaps already sent.
//
// Every case leaves the seat as it found it: the list, the switch and the group
// are the machine's, and a running desktop is typing in them

extern crate alloc;
use alloc::format;
use alloc::string::ToString;
use alloc::vec::Vec;

use crate::io::input::layout;
use crate::mayland::client::Event;
use crate::mayland::input;
use crate::mayland::protocol::{self, Interface};
use crate::mayland::requests::Requests;
use crate::mayland::server::{self, shm};

use super::{exchange, one_window, Runner};

const ALT: u8 = 0x38;
const LSHIFT: u8 = 0x2A;
const RSHIFT: u8 = 0x36;
const UP: u8 = 0x80;

/// What the seat was, so a case can put it back
struct Saved { packed: u32, switch: u8, group: u32 }

fn save() -> Saved {
    Saved { packed: layout::packed_layouts(), switch: layout::switch(), group: layout::group() }
}

fn restore(s: Saved) {
    layout::set_layouts(s.packed);
    layout::set_switch(s.switch);
    layout::set_group(s.group);
    // Whatever the drain last told a window is now true again
    input::drain(0);
}

/// Alt+Shift moves the group once per press of the pair, in either order
///
/// The keyboard repeats make codes while a key is held, so the latch is what
/// makes a hold one switch rather than one per repeat
pub(super) fn test_layout_switch(t: &mut Runner) {
    const NAME: &str = "layout: Alt+Shift moves the group once per press, either order";
    let was = save();
    layout::set_layouts(0x21);
    layout::set_switch(layout::SWITCH_ALT_SHIFT);
    layout::set_group(0);

    input::intercept(ALT);
    input::intercept(LSHIFT);
    let first = layout::group();
    // Typematic: the second key's make code again, and again
    input::intercept(LSHIFT);
    input::intercept(LSHIFT);
    let held = layout::group();
    // Shift up and down again with Alt still held is a second press
    input::intercept(LSHIFT | UP);
    input::intercept(LSHIFT);
    let again = layout::group();
    input::intercept(LSHIFT | UP);
    input::intercept(ALT | UP);

    // The other order, and the right Shift
    input::intercept(RSHIFT);
    input::intercept(ALT);
    let other = layout::group();
    input::intercept(ALT | UP);
    input::intercept(RSHIFT | UP);

    // Shift alone, then a letter: nothing
    input::intercept(LSHIFT);
    input::intercept(0x10);
    input::intercept(0x10 | UP);
    input::intercept(LSHIFT | UP);
    let shift_alone = layout::group();

    t.check(NAME, first == 1 && held == 1 && again == 0 && other == 1 && shift_alone == 1,
            format!("after the pair {}, held {}, pressed again {}, the other order {}, \
                     Shift on its own {}", first, held, again, other, shift_alone));

    // One layout is nothing to switch between, and no switch is no switch
    layout::set_layouts(0x1);
    input::intercept(ALT);
    input::intercept(LSHIFT);
    input::intercept(LSHIFT | UP);
    input::intercept(ALT | UP);
    let one = layout::group();
    layout::set_layouts(0x21);
    layout::set_switch(layout::SWITCH_NONE);
    input::intercept(ALT);
    input::intercept(LSHIFT);
    input::intercept(LSHIFT | UP);
    input::intercept(ALT | UP);
    let off = layout::group();
    t.check("layout: one layout, or kb_options with no switch, never moves",
            one == 0 && off == 0, format!("one layout {}, no switch {}", one, off));

    restore(was);
}

/// The terminal decoder reads the same tables the windows do
pub(super) fn test_layout_terminal(t: &mut Runner) {
    use pc_keyboard::{DecodedKey, HandleControl, KeyCode, KeyboardLayout, Modifiers};
    let was = save();
    layout::set_layouts(0x21);
    let none = Modifiers {
        lshift: false, rshift: false, lctrl: false, rctrl: false,
        numlock: false, capslock: false, alt_gr: false, rctrl2: false,
    };
    let shift = Modifiers { lshift: true, ..none };
    let ctrl = Modifiers { lctrl: true, ..none };
    let map = |k: KeyCode, m: &Modifiers| {
        layout::MikuUs104.map_keycode(k, m, HandleControl::MapLettersToUnicode)
    };
    layout::set_group(1);
    let q = map(KeyCode::Q, &none);
    let yo = map(KeyCode::Oem8, &shift);
    let semi = map(KeyCode::Key4, &shift);
    let ctrl_c = map(KeyCode::C, &ctrl);
    let arrow = map(KeyCode::ArrowUp, &none);
    layout::set_group(0);
    let back = map(KeyCode::Q, &none);
    let bslash = map(KeyCode::Oem7, &shift);
    restore(was);

    t.check("layout: the terminal decoder types Russian through the shared table",
            q == DecodedKey::Unicode('й') && yo == DecodedKey::Unicode('Ё')
                && semi == DecodedKey::Unicode(';') && ctrl_c == DecodedKey::Unicode('\u{3}')
                && arrow == DecodedKey::RawKey(KeyCode::ArrowUp)
                && back == DecodedKey::Unicode('q') && bslash == DecodedKey::Unicode('|'),
            format!("Q {:?}, Shift+` {:?}, Shift+4 {:?}, Ctrl+C {:?}, up {:?}; in US Q {:?}, \
                     Shift+\\ {:?}", q, yo, semi, ctrl_c, arrow, back, bslash));
}

/// The window being typed at is told the group with the modifiers, on `enter`
/// and again when it changes
pub(super) fn test_layout_group_on_the_wire(t: &mut Runner) {
    const NAME: &str = "layout: the group goes out in modifiers, on enter and on a switch";
    let was = save();
    let Some((cl, mut conn, sw, handle, _)) = one_window() else {
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };
    if super::pointer::bind_keyboard(&mut conn, cl).is_none() {
        t.check(NAME, false, "no keyboard".to_string());
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return;
    }
    let groups = |ev: &[Event]| -> Vec<u32> {
        ev.iter().filter_map(|e| match e {
            Event::Modifiers { group, .. } => Some(*group), _ => None,
        }).collect()
    };

    layout::set_layouts(0x21);
    layout::set_group(1);
    // Off and on again, so there is an `enter` to look at
    server::focus_window(None, 0);
    let _ = exchange(&mut conn, cl);
    server::focus_window(Some(cl), sw);
    input::drain(0);
    let on_enter = groups(&exchange(&mut conn, cl));

    layout::set_group(0);
    input::drain(0);
    let on_switch = groups(&exchange(&mut conn, cl));
    // Nothing changed, so nothing is sent
    input::drain(0);
    let quiet = groups(&exchange(&mut conn, cl));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
    restore(was);
    t.check(NAME, on_enter.first() == Some(&1) && on_switch == [0] && quiet.is_empty(),
            format!("on enter {:?}, after the switch {:?}, with nothing new {:?}",
                    on_enter, on_switch, quiet));
}

/// A bar hears which layout is in use, and a config's list reaches the
/// keymaps already sent
pub(super) fn test_layout_desktop_and_config(t: &mut Runner) {
    const NAME: &str = "layout: the bar is told the layout, and kb_layout rewrites the keymaps";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = save();
    layout::set_layouts(0x21);
    layout::set_group(0);
    let Some((cl, mut conn, _desk, wm, _, _)) = super::wm::wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        restore(was);
        return;
    };
    // A keyboard of this client's own, to read the keymap it is sent
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let seat_name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_seat" => Some(*name),
        _ => None,
    });
    let mut keymap = None;
    if let Some(n) = seat_name {
        let seat = conn.bind(registry, n, Interface::Seat, 1);
        conn.get_keyboard(seat);
        keymap = exchange(&mut conn, cl).iter().find_map(|e| match e {
            Event::KeyboardKeymap { format, handle, size }
                if *format == protocol::keyboard::KEYMAP_MAY_V2 => Some((*handle, *size as usize)),
            _ => None,
        });
    }
    let read = |k: Option<(u32, usize)>| -> (usize, alloc::string::String) {
        let Some((h, n)) = k else { return (0, "".to_string()) };
        shm::with_bytes(h, 0, n, |b| {
            crate::mayland::keymap::Table::new(b)
                .map(|t| (t.groups(), format!("{},{}", t.code(0), t.code(1))))
                .unwrap_or((0, "".to_string()))
        }).unwrap_or((0, "".to_string()))
    };
    let sent = read(keymap);

    // The switch, as the drain tells the bar about it
    layout::set_group(1);
    input::drain(0);
    let told = exchange(&mut conn, cl).iter().find_map(|e| match e {
        Event::DeskKeyboardLayout { group, groups, code, name } =>
            Some((*group, *groups, code.clone(), name.clone())),
        _ => None,
    });

    // `kb_layout = ru` from a config: one group, rewritten in place
    use protocol::wm::param;
    conn.wm_set_param(wm, param::KB_LAYOUTS, 0x2 as f32);
    let after_ev = exchange(&mut conn, cl);
    let rewritten = read(keymap);
    let told_again = after_ev.iter().any(|e| matches!(e,
        Event::DeskKeyboardLayout { group: 0, groups: 1, code, .. } if code == "ru"));
    // A list naming a layout this machine does not have is refused
    conn.wm_set_param(wm, param::KB_LAYOUTS, 0x9 as f32);
    let _ = exchange(&mut conn, cl);
    let refused = layout::packed_layouts() == 0x2;

    server::disconnect(cl);
    restore(was);
    let told_ok = matches!(&told, Some((1, 2, c, n)) if c == "ru" && n == "Russian");
    t.check(NAME,
            sent == (2, "us,ru".to_string()) && told_ok
                && rewritten == (1, "ru,".to_string()) && told_again && refused,
            format!("keymap at bind {:?}, the bar told {:?}, after kb_layout = ru {:?} \
                     (told {}), a list naming layout 9 refused {}",
                    sent, told, rewritten, told_again, refused));
}
