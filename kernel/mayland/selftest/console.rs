// may_console: the kernel's text, drawn by somebody else
//
// What crosses this interface is a shared-memory handle onto the character grid
// and which rows of it moved. The cases check that the grid arrives, its shape
// follows what the client asked for, the damage is the rows that changed, and
// only one client at a time may draw it.

use super::*;

/// Connects a client and gives it `may_console`, along with whatever it was
/// told on binding
pub(super) fn console_client() -> Option<(u32, ClientConn, u32, Vec<Event>)> {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_console" => Some(*name),
        _ => None,
    })?;
    let obj = conn.bind(registry, name, Interface::Console, 1);
    // And which terminal, which is what `may_console.bind` is for. A client
    // asking for anything else first is a protocol error. Zero is the machine's
    // own console
    conn.console_bind(obj, 0);
    let events = exchange(&mut conn, cl);
    if first_error(&events).is_some() {
        server::disconnect(cl);
        return None;
    }
    Some((cl, conn, obj, events))
}

pub(super) fn grid_of(events: &[Event]) -> Option<(u32, u32, u32, u32)> {
    events.iter().find_map(|e| match e {
        Event::ConsoleGrid { handle, size, cols, rows } => Some((*handle, *size, *cols, *rows)),
        _ => None,
    })
}

/// One client draws the console, and the grid it is given is the one the kernel
/// writes into
///
/// The grid arrives on the first frame, not at bind: handing it over allocates
/// and reads the console, and bind runs inside the server's lock (see the rule
/// at the top of `server::iface::console`)
pub(super) fn test_console_grid(t: &mut Runner) {
    const NAME: &str = "console: taking may_console hands over the character grid";
    if t.needs(NAME, Interface::Console) { return; }
    let Some((cl, mut conn, _obj, _)) = console_client() else {
        t.check(NAME, false, "could not bind may_console".to_string());
        return;
    };
    server::console_pump();
    let events = exchange(&mut conn, cl);
    let got = grid_of(&events);
    let (cols, rows) = crate::io::console::text_size();
    let (cap_c, cap_r) = crate::io::console::grid_capacity();
    let ok = match got {
        Some((handle, size, c, r)) => {
            // The shape is the console's, the pool is the whole ceiling
            // (allocated once and never moved), and the handle names something
            // that exists
            c as usize == cols && r as usize == rows
                && size as usize == cap_c * cap_r * protocol::console::CELL_BYTES
                && shm::size(handle).is_ok()
        }
        None => false,
    };
    t.check(NAME, ok,
            format!("grid {:?}, console is {}x{}, ceiling {}x{}",
                    got, cols, rows, cap_c, cap_r));
    give_back(cl);
}

/// Takes the console back as a dying terminal would, and lets the kernel's own
/// window return
pub(super) fn give_back(cl: u32) {
    server::disconnect(cl);
    server::console_pump();
}

/// Only one at a time, and the lease outlives nothing
pub(super) fn test_console_is_exclusive(t: &mut Runner) {
    const NAME: &str = "console: one client draws it, and letting go gives it back";
    if t.needs(NAME, Interface::Console) { return; }
    let Some((cl_a, _ca, _obj, _)) = console_client() else {
        t.check(NAME, false, "the first client could not bind it".to_string());
        return;
    };
    let refused = match console_client() {
        Some((cl, _, _, _)) => { server::disconnect(cl); false }
        None => true,
    };
    server::disconnect(cl_a);
    let regained = match console_client() {
        Some((cl, _, _, _)) => { server::disconnect(cl); true }
        None => false,
    };
    t.check(NAME, refused && regained,
            format!("second refused {}, regained after the first left {}",
                    refused, regained));
}

/// The client owns the font, so it says how many characters fit, and the kernel
/// re-flows its text to match
///
/// The re-flow happens on the next frame and not in the request, for the same
/// reason the grid does
pub(super) fn test_console_set_size(t: &mut Runner) {
    const NAME: &str = "console: set_size re-flows the text and re-cuts the grid";
    if t.needs(NAME, Interface::Console) { return; }
    let (cols0, rows0) = crate::io::console::text_size();
    let Some((cl, mut conn, obj, _)) = console_client() else {
        t.check(NAME, false, "could not bind may_console".to_string());
        return;
    };
    server::console_pump();
    let _ = exchange(&mut conn, cl);

    // Narrower and shorter than it is now, so the number has to change
    let (want_c, want_r) = (cols0.max(20) / 2, rows0.max(8) / 2);
    conn.console_set_size(obj, want_c as u32, want_r as u32);
    let _ = exchange(&mut conn, cl);
    server::console_pump();
    let events = exchange(&mut conn, cl);
    let after = crate::io::console::text_size();
    let regrid = grid_of(&events);

    // A size with no room for a character is ignored, because a window
    // mid-resize must not cost the text
    conn.console_set_size(obj, 0, 0);
    let _ = exchange(&mut conn, cl);
    server::console_pump();
    let _ = exchange(&mut conn, cl);
    let held = crate::io::console::text_size();

    let ok = after == (want_c, want_r)
             && matches!(regrid, Some((_, _, c, r)) if c as usize == want_c && r as usize == want_r)
             && held == after;

    t.check(NAME, ok,
            format!("asked {}x{}, got {:?}, regrid {:?}, after an empty size {:?}",
                    want_c, want_r, after, regrid, held));

    give_back(cl);
    crate::io::console::set_grid(cols0, rows0);
}

/// Printing moves rows, and the client is told which ones, once and not once
/// per character
pub(super) fn test_console_damage(t: &mut Runner) {
    const NAME: &str = "console: what was printed arrives as a range of rows";
    if t.needs(NAME, Interface::Console) { return; }
    let (cols0, rows0) = crate::io::console::text_size();
    let was_headless = crate::io::console::is_headless();
    let Some((cl, mut conn, _obj, _)) = console_client() else {
        t.check(NAME, false, "could not bind may_console".to_string());
        return;
    };

    // The frame the compositor runs, where the console's work is coalesced.
    // Holding the console does not take the screen: the compositor does that
    // when it puts up its first frame (`took_screen`). A client holding the
    // grid with no compositor running, as on this boot, must leave the kernel
    // painting
    server::console_pump();
    let headless = crate::io::console::is_headless();
    let _ = exchange(&mut conn, cl);

    crate::print!("selftest\n");
    server::console_pump();
    let events = exchange(&mut conn, cl);
    let damage = events.iter().find_map(|e| match e {
        Event::ConsoleDamage { row0, row1, .. } => Some((*row0, *row1)),
        _ => None,
    });

    // Given back as a dying terminal would: the lease goes with the connection,
    // and who paints the screen is what it was before
    give_back(cl);
    let restored = !crate::io::console::is_headless();
    crate::io::console::set_grid(cols0, rows0);

    let ok = headless == was_headless && damage.map(|(a, b)| a < b).unwrap_or(false)
        && restored == !was_headless;
    t.check(NAME, ok,
            format!("headless while held {} (before: {}), damage {:?}, painting after {}",
                    headless, was_headless, damage, restored));
}
