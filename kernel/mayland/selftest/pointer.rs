// The pointer, the cursor and the keymap
//
// Two focuses, so the crossing cases matter most: a leave and an enter in that
// order, with their own serials. `set_cursor`: the client under the pointer
// says what it should look like, and only that client may. The keymap is the
// same question for the keyboard: the compositor decides the layout and the
// client reads it from shared memory.

use super::*;

/// Where the cursor is, what it is over, and what a click does
///
/// Two windows side by side with known sizes, so a hit is arithmetic. This is
/// the pointer path short of the mouse itself, and none of it needs a screen
pub(super) fn test_pointer(t: &mut Runner) {
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let bytes = (stride * h) as usize;
    let (Some((cl_a, mut a, comp_a, xdg_a, ha, ba)),
         Some((cl_b, mut b, comp_b, xdg_b, hb, bb))) =
        (client_with_pool(bytes, w, h, stride), client_with_pool(bytes, w, h, stride))
    else {
        t.check("pointer: the cursor enters the window under it", false,
                "setup failed".to_string());
        return;
    };

    // A pointer object each, so the events have somewhere to go. A second
    // registry rather than the one `client_with_pool` kept: asking twice is
    // legal
    if !bind_pointer(&mut a, cl_a) || !bind_pointer(&mut b, cl_b) {
        t.check("pointer: the cursor enters the window under it, in its own coordinates",
                false, "no seat to get a pointer from".to_string());
        server::disconnect(cl_a);
        server::disconnect(cl_b);
        let _ = shm::destroy(ha);
        let _ = shm::destroy(hb);
        return;
    }

    let (sa, _) = open_window(&mut a, cl_a, comp_a, xdg_a, "left", ba, w, h);
    let (sb, _) = open_window(&mut b, cl_b, comp_b, xdg_b, "right", bb, w, h);
    // Both are at the origin; moves the second one clear of the first. The
    // client is not asked
    layout::set_window_pos(cl_b, sb, 100, 50);
    // The cursor on the second, and what that said forgotten: a window opening
    // under a still pointer has it at once (a first frame is a crossing), and
    // this case is about the crossing motion makes
    server::pointer_motion(120, 60, 0);
    drain_events(&mut a, cl_a);
    drain_events(&mut b, cl_b);

    // Into the middle of the first window
    server::pointer_motion(10, 10, 1);
    let ea = drain_events(&mut a, cl_a);
    let entered = ea.iter().find_map(|e| match e {
        Event::PointerEnter { surface, x, y } => Some((*surface, *x, *y)),
        _ => None,
    });
    t.check("pointer: the cursor enters the window under it, in its own coordinates",
            entered == Some((sa, 10, 10)),
            format!("{:?}, expected surface {} at 10,10", entered, sa));

    // Across to the second. A leave for the first and an enter for the second,
    // with coordinates relative to where that window is
    server::pointer_motion(120, 60, 2);
    let left = drain_events(&mut a, cl_a).iter()
        .any(|e| matches!(e, Event::PointerLeave { surface } if *surface == sa));
    let arrived = drain_events(&mut b, cl_b).iter().find_map(|e| match e {
        Event::PointerEnter { surface, x, y } => Some((*surface, *x, *y)),
        _ => None,
    });
    t.check("pointer: crossing between windows leaves one and enters the other",
            left && arrived == Some((sb, 20, 10)),
            format!("left {}, arrived {:?}, expected surface {} at 20,10", left, arrived, sb));

    // Nothing is under the cursor out here, and nobody should hear about it
    server::pointer_motion(900, 900, 3);
    let nothing = server::pointer_focus().is_none();
    t.check("pointer: bare screen is not a surface", nothing,
            format!("focus {:?}", server::pointer_focus()));

    // A click gives the window under the cursor the keyboard, whichever one
    // had it before
    server::focus_window(Some(cl_a), sa);
    server::pointer_motion(120, 60, 4);
    let _ = drain_events(&mut b, cl_b);
    server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, true, 5);
    let got_button = drain_events(&mut b, cl_b).iter().any(|e| matches!(
        e, Event::PointerButton { button, pressed: true, .. } if *button == crate::mayland::input::pointer::BTN_LEFT));
    let moved_focus = server::focus_key() == Some((cl_b, sb));
    t.check("pointer: clicking a window gives it the keyboard",
            got_button && moved_focus,
            format!("button delivered {}, keyboard now {:?}", got_button, server::focus_key()));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// `input { follow_mouse = 1 }`: the keyboard goes where the pointer went
///
/// Off, the pointer must not move the keyboard at all. On, leaving a window for
/// the bare screen must not take the keyboard with it. Turning it off again has
/// to stop it
pub(super) fn test_follow_mouse(t: &mut Runner) {
    use crate::mayland::protocol::wm::param;
    const NAME: &str = "input: follow_mouse moves the keyboard, and only when it is on";

    let (w, h, stride) = (40i32, 30i32, 160i32);
    let bytes = (stride * h) as usize;
    let (Some((cl_a, mut a, comp_a, xdg_a, ha, ba)),
         Some((cl_b, mut b, comp_b, xdg_b, hb, bb))) =
        (client_with_pool(bytes, w, h, stride), client_with_pool(bytes, w, h, stride))
    else {
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    let (sa, _) = open_window(&mut a, cl_a, comp_a, xdg_a, "left", ba, w, h);
    let (sb, _) = open_window(&mut b, cl_b, comp_b, xdg_b, "right", bb, w, h);
    layout::set_window_pos(cl_b, sb, 100, 50);

    // Off, unless a config says otherwise
    server::focus_window(Some(cl_a), sa);
    server::pointer_motion(120, 60, 1);
    let stayed = server::focus_key() == Some((cl_a, sa));

    crate::mayland::input::conf::set(param::FOLLOW_MOUSE, 1.0);
    // Out onto the bare screen. The keyboard stays where it was
    server::pointer_motion(900, 900, 2);
    let not_lost = server::focus_key() == Some((cl_a, sa));
    // And onto the other window
    server::pointer_motion(120, 60, 3);
    let followed = server::focus_key() == Some((cl_b, sb));

    crate::mayland::input::conf::set(param::FOLLOW_MOUSE, 0.0);
    server::pointer_motion(10, 10, 4);
    let stopped = server::focus_key() == Some((cl_b, sb));

    t.check(NAME, stayed && not_lost && followed && stopped,
            format!("off: the keyboard stayed {}; on: kept over bare screen {},                      followed onto the other window {}; off again: stopped {}",
                    stayed, not_lost, followed, stopped));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// Bind the seat and ask it for a pointer. None if the registry has no seat
pub(super) fn bind_pointer_obj(conn: &mut ClientConn, cl: u32) -> Option<u32> {
    let registry = conn.get_registry();
    let events = exchange(conn, cl);
    let name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_seat" => Some(*name),
        _ => None,
    })?;
    let seat = conn.bind(registry, name, Interface::Seat, 1);
    let ptr = conn.get_pointer(seat);
    let events = exchange(conn, cl);
    if first_error(&events).is_some() { None } else { Some(ptr) }
}

/// Binds the seat and asks it for a keyboard
///
/// Otherwise the compositor queues a key to nobody: `focus::key` sends to the
/// focused client's keyboard objects, and a client that never asked has none.
/// Grabs and copies need the serial a key carries
pub(super) fn bind_keyboard(conn: &mut ClientConn, cl: u32) -> Option<u32> {
    let registry = conn.get_registry();
    let events = exchange(conn, cl);
    let name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_seat" => Some(*name),
        _ => None,
    })?;
    let seat = conn.bind(registry, name, Interface::Seat, 1);
    let kbd = conn.get_keyboard(seat);
    let events = exchange(conn, cl);
    if first_error(&events).is_some() { None } else { Some(kbd) }
}

pub(super) fn bind_pointer(conn: &mut ClientConn, cl: u32) -> bool {
    bind_pointer_obj(conn, cl).is_some()
}

/// A client saying what the cursor should look like over it
///
/// Only the client being pointed at may set it, the surface stops being an
/// ordinary surface, and crossing out of that window gives the cursor back to
/// the compositor
pub(super) fn test_set_cursor(t: &mut Runner) {
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let win_bytes = (stride * h) as usize;
    let Some((cl, mut conn, comp, xdg, handle, buffer)) =
        client_with_pool(win_bytes, w, h, stride)
    else {
        t.check("set_cursor: the client under the pointer sets the cursor",
                false, "setup failed".to_string());
        return;
    };
    let Some(ptr) = bind_pointer_obj(&mut conn, cl) else {
        t.check("set_cursor: the client under the pointer sets the cursor",
                false, "no seat to get a pointer from".to_string());
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return;
    };

    let (surface, _) = open_window(&mut conn, cl, comp, xdg, "window", buffer, w, h);
    // The same buffer serves as the cursor image: a buffer may be attached to
    // more than one surface, and the case is about the rules around the cursor
    let (cw, ch) = (w, h);
    let cursor = conn.create_surface(comp);
    conn.attach(cursor, buffer, 0, 0);
    conn.commit(cursor);
    let _ = exchange(&mut conn, cl);

    // Nothing asked for yet, so the compositor's own arrow
    server::pointer_motion(10, 10, 1);
    let before = server::cursor_image().is_none();

    conn.set_cursor(ptr, cursor, 4, 4);
    let errs = exchange(&mut conn, cl);
    let img = server::cursor_image();
    let set_ok = first_error(&errs).is_none() && matches!(&img, Some(Some(i))
        if i.width == cw && i.height == ch && i.hot_x == 4 && i.hot_y == 4);
    t.check("set_cursor: the client under the pointer sets the cursor",
            before && set_ok,
            format!("before {}, after {:?}", before,
                    img.as_ref().map(|i| i.as_ref().map(|i| (i.width, i.height, i.hot_x)))));

    // The cursor surface is not a window: it is not in the stack, and composing
    // it would draw the cursor twice
    let composed = mine(&[cl]).iter().any(|p| p.surface == cursor);
    t.check("set_cursor: a cursor surface is not composed as a window", !composed,
            format!("ours on screen {:?}",
                    mine(&[cl]).iter().map(|p| p.surface).collect::<Vec<_>>()));

    // The hot spot: the image hangs up and left of where the pointer is
    let was_visible = crate::mayland::input::pointer::visible();
    crate::mayland::input::pointer::start(0, 0);
    let (px, py) = crate::mayland::input::pointer::position();
    let r = crate::mayland::input::pointer::rect();
    t.check("set_cursor: the image is placed by its hot spot",
            r == Some((px - 4, py - 4, cw, ch)),
            format!("{:?}, pointer at {},{}", r, px, py));

    // Off the window: the cursor goes back to the compositor's
    server::pointer_motion(900, 900, 2);
    let cleared = server::cursor_image().is_none();
    // A client the pointer is not over cannot set one
    conn.set_cursor(ptr, cursor, 1, 1);
    let _ = exchange(&mut conn, cl);
    let still_clear = server::cursor_image().is_none();
    t.check("set_cursor: leaving gives the cursor back, and only the pointed-at client may set it",
            cleared && still_clear,
            format!("cleared {}, after a stranger asked {}", cleared, still_clear));

    // A null surface is a client asking for no cursor at all, which is not the
    // same answer as not having asked
    server::pointer_motion(10, 10, 3);
    conn.set_cursor(ptr, 0, 0, 0);
    let _ = exchange(&mut conn, cl);
    let hidden = matches!(server::cursor_image(), Some(None))
        && crate::mayland::input::pointer::rect().is_none();
    t.check("set_cursor: a null surface means no cursor, not the default one",
            hidden, format!("rect {:?}", crate::mayland::input::pointer::rect()));

    if !was_visible {
        crate::mayland::input::pointer::stop();
    }
    server::pointer_motion(900, 900, 4);
    let _ = surface;
    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// The layout a client reads keycodes through
///
/// The compositor picks the keycodes and the modifier bits, and this file
/// decides what letter they are, so the two have to agree
pub(super) fn test_keymap(t: &mut Runner) {
    use crate::mayland::keymap::{self, Named, MOD_CAPS, MOD_CTRL, MOD_SHIFT};

    // Keycode 30 is 'a' in evdev and in set 1 alike, which is why the
    // compositor sends these numbers
    let plain = keymap::to_char(30, 0);
    let shifted = keymap::to_char(30, MOD_SHIFT);
    // Shift and caps each flip the case, so both together are lower again
    let both = keymap::to_char(30, MOD_SHIFT | MOD_CAPS);
    let ctrl = keymap::to_char(46, MOD_CTRL);
    let digit = keymap::to_char(2, 0);
    let bang = keymap::to_char(2, MOD_SHIFT);

    let letters_ok = plain == Some('a') && shifted == Some('A') && both == Some('a')
        && ctrl == Some('\u{3}') && digit == Some('1') && bang == Some('!');
    t.check("keymap: shift and caps cancel, ctrl makes a control character",
            letters_ok,
            format!("a={:?} A={:?} both={:?} ctrl+c={:?} 1={:?} !={:?}",
                    plain, shifted, both, ctrl, digit, bang));

    // An arrow is not a character, and a client must be able to tell that from
    // a key it simply has no glyph for
    let named_ok = keymap::to_named(103) == Named::Up
        && keymap::to_named(28) == Named::Enter
        && keymap::to_named(88) == Named::Function(12)
        && keymap::to_char(103, 0).is_none()
        && keymap::to_char(42, 0).is_none()
        && keymap::to_char(28, 0) == Some('\n');
    t.check("keymap: keys with no character are named instead", named_ok,
            format!("up={:?} enter={:?} f12={:?}",
                    keymap::to_named(103), keymap::to_named(28), keymap::to_named(88)));

    // The layouts the compositor sends have to be the layouts the seat has:
    // every keycode in every group, under every combination of the modifiers
    // that change what a key produces. The host tests in `lib/mayland` go
    // through the same; this is the kernel's build of it, against the seat's
    // own list
    let mut list = [&keymap::LAYOUTS[0]; keymap::MAX_GROUPS];
    let n = crate::io::input::layout::layouts(&mut list);
    let mut bytes = alloc::vec![0u8; keymap::SIZE_MAX];
    let wrote = keymap::serialise(&mut bytes, &list[..n]);
    let table = keymap::Table::new(&bytes);
    let mut mismatch: Option<(usize, u16, u32)> = None;
    if let Some(tab) = table {
        'all: for (g, l) in list[..n].iter().enumerate() {
            for kc in 0..keymap::COUNT as u16 {
                for mods in [0, MOD_SHIFT, MOD_CAPS, MOD_SHIFT | MOD_CAPS, MOD_CTRL] {
                    if tab.to_char_in(kc, mods, g as u32) != keymap::to_char_in(l, kc, mods) {
                        mismatch = Some((g, kc, mods));
                        break 'all;
                    }
                }
                if tab.to_named(kc) != keymap::to_named(kc) {
                    mismatch = Some((g, kc, 0));
                    break 'all;
                }
            }
        }
    }
    t.check("keymap: the table the compositor sends is the layouts the seat has",
            wrote == Some(keymap::size(n)) && table.is_some_and(|t| t.groups() == n) && mismatch.is_none(),
            match mismatch {
                Some((g, kc, mods)) => format!(
                    "group {} keycode {} mods {:#x}: sent {:?}, meant {:?}",
                    g, kc, mods, table.and_then(|t| t.to_char_in(kc, mods, g as u32)),
                    keymap::to_char_in(list[g], kc, mods)),
                None => format!("{} groups, {} bytes, {} keycodes agree",
                                n, keymap::size(n), keymap::COUNT),
            });

    // A client handed something it does not understand reads nothing
    let mut wrong = bytes.clone();
    wrong[4] = 9; // version; 1, the single-group format, is not accepted either
    t.check("keymap: an unknown version is refused, not parsed",
            keymap::Table::new(&wrong).is_none() && keymap::Table::new(&bytes[..8]).is_none(),
            "version 9 and a truncated table both refused".to_string());
}
