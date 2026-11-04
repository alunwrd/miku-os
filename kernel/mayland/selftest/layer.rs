// may_layer: a surface that is not a window
//
// Two things a layer surface decides: which band it is drawn in, which
// separates a panel from a window on top, and how much of an edge it keeps,
// which becomes the work area.

use super::*;

use crate::mayland::server::focus;

/// A client with a pool, a layer surface in `band`, anchored and reserving
pub(super) fn layer_client(band: u32, anchor: u32, exclusive: i32, want_h: i32)
    -> Option<(u32, ClientConn, u32, u32, u32, u32)>
{
    layer_client_on(band, anchor, exclusive, want_h).map(|(cl, conn, surface, ls, handle, buffer, _)| (cl, conn, surface, ls, handle, buffer))
}

/// The same, and the client's compositor, for a region
pub(super) fn layer_client_on(band: u32, anchor: u32, exclusive: i32, want_h: i32)
    -> Option<(u32, ClientConn, u32, u32, u32, u32, u32)>
{
    layer_client_asking(band, anchor, exclusive, want_h, None, None)
}

/// The same, saying what it is (`set_namespace`) before its first frame
pub(super) fn layer_client_named(band: u32, anchor: u32, exclusive: i32, want_h: i32, ns: &str)
    -> Option<(u32, ClientConn, u32, u32, u32, u32, u32)>
{
    layer_client_asking(band, anchor, exclusive, want_h, None, Some(ns))
}

/// The same, asking for the keyboard before its first frame when `keyboard`
/// says how
fn layer_client_asking(band: u32, anchor: u32, exclusive: i32, want_h: i32, keyboard: Option<u32>,
                       ns: Option<&str>)
    -> Option<(u32, ClientConn, u32, u32, u32, u32, u32)>
{
    let (w, h, stride) = (64i32, 64i32, 256i32);
    let (cl, mut conn, comp, _xdg, handle, buffer) =
        client_with_pool((stride * h) as usize, w, h, stride)?;

    // may_layer is not in `client_with_pool`'s three globals, so it is found
    // the same way they were
    let registry = 2;
    let ls_name = server::globals().iter()
        .find(|g| g.iface == Interface::Layer).map(|g| g.name);
    let Some(ln) = ls_name else {
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return None;
    };
    let layer = conn.bind(registry, ln, Interface::Layer, 1);
    let surface = conn.create_surface(comp);
    let ls = conn.get_layer_surface(layer, surface, band);
    conn.layer_set_anchor(ls, anchor);
    conn.layer_set_size(ls, 0, want_h);
    conn.layer_set_exclusive(ls, exclusive);
    if let Some(mode) = keyboard {
        conn.layer_set_keyboard(ls, mode);
    }
    if let Some(ns) = ns {
        conn.layer_set_namespace(ls, ns);
    }
    let events = exchange(&mut conn, cl);
    if let Some((_, why)) = first_error(&events) {
        crate::serial_println!("[mayland] layer_client refused: {}", why);
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return None;
    }
    let serial = events.iter().rev().find_map(|e| match e {
        Event::LayerConfigure { serial, .. } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.layer_ack_configure(ls, serial);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    Some((cl, conn, surface, ls, handle, buffer, comp))
}

/// A panel keeps its edge, and everything measured against the work area
/// shrinks by exactly that much
pub(super) fn test_layer_reserves(t: &mut Runner) {
    const NAME: &str = "layer: a panel at the top takes its height off the work area";
    const BAR: i32 = 32;
    let before = layout::work_area_now();

    let Some((cl, _c, _surface, _ls, handle, _b)) = layer_client(
        protocol::layer::TOP, protocol::layer_surface::ANCHOR_TOP
            | protocol::layer_surface::ANCHOR_LEFT
            | protocol::layer_surface::ANCHOR_RIGHT,
        BAR, BAR)
    else { t.check(NAME, false, "setup failed".to_string()); return };

    let with = layout::work_area_now();
    let reserved = layout::reserved();

    server::disconnect(cl);
    let _ = shm::destroy(handle);
    let after = layout::work_area_now();

    let ok = with.y == before.y + BAR && with.h == before.h - BAR
             && with.w == before.w
             && reserved.top == BAR
             && after == before;
    t.check(NAME, ok,
            format!("work area {:?} -> {:?} -> {:?}, reserved top {}",
                    (before.x, before.y, before.w, before.h),
                    (with.x, with.y, with.w, with.h),
                    (after.x, after.y, after.w, after.h),
                    reserved.top));
}

/// A tiled window is laid out into what is left, not over the panel
pub(super) fn test_layer_pushes_windows(t: &mut Runner) {
    const NAME: &str = "layer: a tiled window is placed under the panel, not behind it";
    const BAR: i32 = 40;
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let Some((cl_p, _cp, _ps, _ls, hp, _bp)) = layer_client(
        protocol::layer::TOP, protocol::layer_surface::ANCHOR_TOP
            | protocol::layer_surface::ANCHOR_LEFT
            | protocol::layer_surface::ANCHOR_RIGHT,
        BAR, BAR)
    else {
        layout::set_kind(was);
        t.check(NAME, false, "the panel could not be set up".to_string());
        return;
    };

    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        server::disconnect(cl_p);
        let _ = shm::destroy(hp);
        layout::set_kind(was);
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };

    let area = layout::work_area_now();
    let win = server::window_box(cl_w, sw);
    let ok = win.map(|(_, y, _, h)| y >= BAR && y == area.y && h == area.h).unwrap_or(false);

    t.check(NAME, ok,
            format!("window {:?}, work area {:?}, bar {} tall",
                    win, (area.x, area.y, area.w, area.h), BAR));

    server::disconnect(cl_w);
    server::disconnect(cl_p);
    let _ = shm::destroy(hw);
    let _ = shm::destroy(hp);
    layout::set_kind(was);
}

/// The band decides the order, and nothing raises across it
pub(super) fn test_layer_bands(t: &mut Runner) {
    const NAME: &str = "layer: background is behind every window and overlay in front";

    let Some((cl_b, _cb, sb, _lb, hb, _)) = layer_client(
        protocol::layer::BACKGROUND, protocol::layer_surface::ANCHOR_ALL, -1, 0)
    else { t.check(NAME, false, "the background could not be set up".to_string()); return };
    let Some((cl_o, _co, so, _lo, ho, _)) = layer_client(
        protocol::layer::OVERLAY, protocol::layer_surface::ANCHOR_ALL, -1, 0)
    else {
        server::disconnect(cl_b);
        let _ = shm::destroy(hb);
        t.check(NAME, false, "the overlay could not be set up".to_string());
        return;
    };
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        server::disconnect(cl_b);
        server::disconnect(cl_o);
        let _ = shm::destroy(hb);
        let _ = shm::destroy(ho);
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };

    // The window was opened last and takes the front of the windows. It cannot
    // come out in front of the overlay or behind the background
    let order = mine(&[cl_b, cl_o, cl_w]);
    let ok = order.len() == 3
             && order[0].client == cl_b && order[0].surface == sb
             && order[1].client == cl_w && order[1].surface == sw
             && order[2].client == cl_o && order[2].surface == so;

    // The keyboard went to the window, not to either layer: neither asked for
    // it
    let focus_ok = server::focus().map(|(c, s, _)| (c, s)) == Some((cl_w, sw));

    t.check(NAME, ok && focus_ok,
            format!("bottom to top {:?}, focus {:?}",
                    order.iter().map(|p| (p.client, p.surface)).collect::<Vec<_>>(),
                    server::focus().map(|(c, s, _)| (c, s))));

    server::disconnect(cl_w);
    server::disconnect(cl_o);
    server::disconnect(cl_b);
    let _ = shm::destroy(hw);
    let _ = shm::destroy(ho);
    let _ = shm::destroy(hb);
}

/// Reserving nothing means reserving nothing, however anchored
pub(super) fn test_layer_no_exclusive(t: &mut Runner) {
    const NAME: &str = "layer: a surface that reserves nothing leaves the work area alone";
    let before = layout::work_area_now();

    let Some((cl, _c, _s, _ls, handle, _b)) = layer_client(
        protocol::layer::OVERLAY, protocol::layer_surface::ANCHOR_TOP
            | protocol::layer_surface::ANCHOR_LEFT
            | protocol::layer_surface::ANCHOR_RIGHT,
        -1, 64)
    else { t.check(NAME, false, "setup failed".to_string()); return };

    let with = layout::work_area_now();
    let reserved = layout::reserved();
    server::disconnect(cl);
    let _ = shm::destroy(handle);

    t.check(NAME, with == before && reserved == layout::Reserved::default(),
            format!("work area {:?} against {:?}, reserved {:?}",
                    (with.x, with.y, with.w, with.h),
                    (before.x, before.y, before.w, before.h), reserved));
}

/// A surface stretched over the screen that reserves nothing but asked to keep
/// clear (a zone of nought) starts under the bar, like end4's overview, and
/// moves when the bar goes; one that asked for -1 covers it all
pub(super) fn test_layer_keeps_clear(t: &mut Runner) {
    const NAME: &str = "layer: a zone of nought keeps clear of the panel, -1 does not";
    const BAR: i32 = 32;
    let all = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_BOTTOM
        | protocol::layer_surface::ANCHOR_LEFT | protocol::layer_surface::ANCHOR_RIGHT;
    let top = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_LEFT
        | protocol::layer_surface::ANCHOR_RIGHT;
    let (sw, sh) = { let s = server::lock(); (s.width, s.height) };

    let Some((cl_bar, _cb, _sb, _lb, hb, _)) = layer_client(protocol::layer::TOP, top, BAR, BAR)
    else { t.check(NAME, false, "the panel could not be set up".to_string()); return };
    let Some((cl_o, _co, so, _lo, ho, _)) = layer_client(protocol::layer::TOP, all, 0, 0)
    else {
        server::disconnect(cl_bar);
        let _ = shm::destroy(hb);
        t.check(NAME, false, "the overview could not be set up".to_string());
        return;
    };
    let Some((cl_f, _cf, sf, _lf, hf, _)) = layer_client(protocol::layer::OVERLAY, all, -1, 0)
    else {
        server::disconnect(cl_o);
        server::disconnect(cl_bar);
        let _ = shm::destroy(ho);
        let _ = shm::destroy(hb);
        t.check(NAME, false, "the cover could not be set up".to_string());
        return;
    };
    let goal = |cl: u32, surface: u32| {
        let s = server::lock();
        s.surface_idx((cl, surface)).map(|i| { let g = s.surfaces[i].goal; (g.x, g.y, g.w, g.h) })
    };
    let under = goal(cl_o, so);
    let cover = goal(cl_f, sf);
    // The panel goes: the overview has the screen back
    server::disconnect(cl_bar);
    let _ = shm::destroy(hb);
    let back = goal(cl_o, so);
    server::disconnect(cl_f);
    server::disconnect(cl_o);
    let _ = shm::destroy(hf);
    let _ = shm::destroy(ho);

    t.check(NAME,
            under == Some((0, BAR, sw, sh - BAR)) && cover == Some((0, 0, sw, sh)) && back == Some((0, 0, sw, sh)),
            format!("under the panel {under:?}, the cover {cover:?}, without the panel {back:?}, screen {sw}x{sh}"));
}

/// A panel whose input region is its top band takes the pointer there and
/// nowhere else: below it the window under the panel has it. end4's screen
/// corners are 250 px wide over the bar and take only a 5 px strip
pub(super) fn test_layer_input_region(t: &mut Runner) {
    const NAME: &str = "layer: the pointer reaches a surface only where its input region is";
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let Some((cl_w, mut cw, comp_w, xdg_w, hw, bw)) = client_with_pool((stride * h) as usize, w, h, stride)
    else { t.check(NAME, false, "the window could not be set up".to_string()); return };
    let (sw, _) = open_window(&mut cw, cl_w, comp_w, xdg_w, "under", bw, w, h);
    layout::set_window_pos(cl_w, sw, 0, 0);
    let top = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_LEFT;
    let Some((cl_l, mut cl, sl, _ls, hl, _b, comp_l)) = layer_client_on(protocol::layer::TOP, top, -1, 64)
    else {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        t.check(NAME, false, "the panel could not be set up".to_string());
        return;
    };
    let under = |x, y| { server::pointer_motion(x, y, 0); server::pointer_focus() };
    let whole = (under(10, 20), under(10, 4));
    // Only its top eight rows, a rectangle added and a corner of it taken out
    let region = cl.create_region(comp_l);
    cl.region_add(region, 0, 0, 64, 8);
    cl.region_subtract(region, 0, 0, 4, 4);
    cl.set_input_region(sl, region);
    cl.region_destroy(region);
    cl.commit(sl);
    let _ = exchange(&mut cl, cl_l);
    let banded = (under(10, 20), under(10, 4), under(2, 2));
    // And the whole of it again
    cl.set_input_region(sl, 0);
    cl.commit(sl);
    let _ = exchange(&mut cl, cl_l);
    let again = under(10, 20);
    server::disconnect(cl_l);
    server::disconnect(cl_w);
    let _ = shm::destroy(hl);
    let _ = shm::destroy(hw);

    let (panel, window) = (Some((cl_l, sl)), Some((cl_w, sw)));
    t.check(NAME,
            whole == (panel, panel) && banded == (window, panel, window) && again == panel,
            format!("whole {whole:?}, banded {banded:?}, whole again {again:?}; panel {panel:?}, window {window:?}"));
}

/// A focus grab keeps the pointer to the surfaces it lists; a press beside them
/// ends it with `cleared` and goes no further: end4's overview shuts on a click
/// beside it, and the window behind is not clicked
pub(super) fn test_focus_grab(t: &mut Runner) {
    const NAME: &str = "layer: a focus grab keeps the pointer, and a press beside it ends it unheard";
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let Some((cl_w, mut cw, comp_w, xdg_w, hw, bw)) = client_with_pool((stride * h) as usize, w, h, stride)
    else { t.check(NAME, false, "the window could not be set up".to_string()); return };
    let has_pointer = super::pointer::bind_pointer(&mut cw, cl_w);
    let (sw, _) = open_window(&mut cw, cl_w, comp_w, xdg_w, "behind", bw, w, h);
    layout::set_window_pos(cl_w, sw, 300, 300);
    let top = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_LEFT;
    let Some((cl_l, mut cl, sl, _ls, hl, _b, _comp)) = layer_client_on(protocol::layer::TOP, top, -1, 64)
    else {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        t.check(NAME, false, "the overview could not be set up".to_string());
        return;
    };
    let name = server::globals().iter().find(|g| g.iface == Interface::Layer).map(|g| g.name).unwrap_or(0);
    let layer = cl.bind(2, name, Interface::Layer, 2);
    let grab = cl.get_focus_grab(layer);
    cl.focus_grab_add(grab, sl);
    cl.focus_grab_commit(grab);
    let refused = first_error(&exchange(&mut cl, cl_l)).map(|e| e.1);

    let under = |x, y| { server::pointer_motion(x, y, 0); server::pointer_focus() };
    let held = (under(310, 310), under(10, 10));
    let _ = drain_events(&mut cw, cl_w);
    under(310, 310);
    server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, true, 1);
    server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, false, 2);
    let cleared = drain_events(&mut cl, cl_l).iter().any(|e| matches!(e, Event::FocusGrabCleared { grab: g } if *g == grab));
    let clicked = drain_events(&mut cw, cl_w).iter().any(|e| matches!(e, Event::PointerButton { pressed: true, .. }));
    let after = under(311, 311);
    server::disconnect(cl_l);
    server::disconnect(cl_w);
    let _ = shm::destroy(hl);
    let _ = shm::destroy(hw);

    let (panel, window) = (Some((cl_l, sl)), Some((cl_w, sw)));
    t.check(NAME,
            refused.is_none() && has_pointer && held == (None, panel) && cleared && !clicked && after == window,
            format!("refused {refused:?}, pointer {has_pointer}, held {held:?}, cleared {cleared}, \
                     the window clicked {clicked}, after {after:?}; panel {panel:?}, window {window:?}"));
}

/// Two panels on the same edge stack rather than overlap
pub(super) fn test_layer_edges_add_up(t: &mut Runner) {
    const NAME: &str = "layer: two surfaces on one edge take their heights off in turn";
    let before = layout::work_area_now();
    let anchor = protocol::layer_surface::ANCHOR_TOP
        | protocol::layer_surface::ANCHOR_LEFT
        | protocol::layer_surface::ANCHOR_RIGHT;

    let Some((cl_a, _ca, _sa, _la, ha, _)) =
        layer_client(protocol::layer::TOP, anchor, 24, 24)
    else { t.check(NAME, false, "the first could not be set up".to_string()); return };
    let one = layout::reserved().top;

    let Some((cl_b, _cb, _sb, _lb, hb, _)) =
        layer_client(protocol::layer::TOP, anchor, 16, 16)
    else {
        server::disconnect(cl_a);
        let _ = shm::destroy(ha);
        t.check(NAME, false, "the second could not be set up".to_string());
        return;
    };
    let both = layout::reserved().top;

    // And the space comes back one at a time
    server::disconnect(cl_b);
    let _ = shm::destroy(hb);
    let left = layout::reserved().top;
    server::disconnect(cl_a);
    let _ = shm::destroy(ha);
    let after = layout::work_area_now();

    t.check(NAME,
            one == 24 && both == 40 && left == 24 && after == before,
            format!("one {}, both {}, one again {}, work area back {}",
                    one, both, left, after == before));
}

/// The same client, saying what it wants the keyboard for
///
/// `layer_client` never asks for the keyboard. The lock is armed by the
/// combination of overlay band, exclusive keyboard and nobody logged in, so the
/// request is made as a greeter makes it
fn locking_client(band: u32, keyboard: u32)
    -> Option<(u32, ClientConn, u32, u32, u32, u32)>
{
    let (cl, mut conn, surface, ls, handle, buffer) = layer_client(
        band, protocol::layer_surface::ANCHOR_ALL, -1, 0)?;
    conn.layer_set_keyboard(ls, keyboard);
    let _ = exchange(&mut conn, cl);
    Some((cl, conn, surface, ls, handle, buffer))
}

/// The login screen holds the keyboard
///
/// `set_focus` alone would be undone by anything that moves the focus
/// afterwards, including Ctrl+Alt+Esc, which reaches a ring-0 root console.
/// Fine for a launcher inside a session, a bypass for a login screen
pub(super) fn test_lock_pins_the_focus(t: &mut Runner) {
    const NAME: &str = "layer: the pre-session overlay keeps the keyboard against everything";

    if crate::process::session::seat_has_session(crate::process::session::SEAT0) {
        t.skip(NAME, "somebody is logged in; the lock is deliberately not armed".to_string());
        return;
    }

    let Some((cl_l, _cl, sl, _ls, hl, _)) = locking_client(
        protocol::layer::OVERLAY, protocol::layer_surface::KEYBOARD_EXCLUSIVE)
    else { t.check(NAME, false, "the lock surface could not be set up".to_string()); return };

    let armed = focus::focus_key() == Some((cl_l, sl)) && crate::mayland::input::locked();

    // A window opening behind it does not take the keyboard, and neither does
    // asking for it by name. Both are refusals: they answer false, so a caller
    // can report it
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        server::disconnect(cl_l);
        let _ = shm::destroy(hl);
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };

    let still_locked = focus::focus_key() == Some((cl_l, sl));
    let by_name = focus::focus_window(Some(cl_w), sw);
    let by_cycle = focus::focus_next();
    let held = focus::focus_key() == Some((cl_l, sl));

    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
    server::disconnect(cl_l);
    let _ = shm::destroy(hl);

    // The lock goes with the surface. A greeter that exited holding it would
    // leave the machine with no keyboard
    let released = !crate::mayland::input::locked() && focus::focus_key() != Some((cl_l, sl));

    t.check(NAME,
            armed && still_locked && !by_name && by_cycle.is_none() && held && released,
            format!("armed {}, a new window {}, by name {}, by cycle {:?}, released {}",
                    armed, still_locked, by_name, by_cycle, released));
}

/// Everything that is not all three things does not lock
///
/// The lock answers whether this is the login screen, from the band, the
/// keyboard mode and whether anybody is logged in. With two of them alone every
/// launcher would become something that cannot be dismissed
pub(super) fn test_lock_needs_all_three(t: &mut Runner) {
    const NAME: &str = "layer: an overlay that did not ask for the keyboard does not lock";

    if crate::process::session::seat_has_session(crate::process::session::SEAT0) {
        t.skip(NAME, "somebody is logged in".to_string());
        return;
    }

    // Overlay, but on demand rather than exclusive
    let Some((cl_a, _ca, _sa, _la, ha, _)) = locking_client(
        protocol::layer::OVERLAY, protocol::layer_surface::KEYBOARD_ON_DEMAND)
    else { t.check(NAME, false, "setup failed".to_string()); return };
    let on_demand = crate::mayland::input::locked();
    server::disconnect(cl_a);
    let _ = shm::destroy(ha);

    // Exclusive, but in the band a panel lives in. A bar taking the keyboard
    // for ever would leave a desktop nobody could type at
    let Some((cl_b, _cb, _sb, _lb, hb, _)) = locking_client(
        protocol::layer::TOP, protocol::layer_surface::KEYBOARD_EXCLUSIVE)
    else { t.check(NAME, false, "the panel could not be set up".to_string()); return };
    let top_band = crate::mayland::input::locked();
    server::disconnect(cl_b);
    let _ = shm::destroy(hb);

    t.check(NAME, !on_demand && !top_band,
            format!("on-demand overlay locked {}, exclusive panel locked {}",
                    on_demand, top_band));
}

/// A click on a panel does not take the keyboard; a click on a layer that asked
/// for it does
///
/// The bar is `keyboard = none` and is clicked all the time, for its workspace
/// buttons. If the click gave it the keyboard, a panel on every workspace would
/// keep it across a switch, and the terminal on the workspace switched to could
/// not be typed at until clicked
pub(super) fn test_layer_click_keeps_the_keyboard(t: &mut Runner) {
    const NAME: &str = "layer: clicking a panel leaves the keyboard where it was";
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let Some((cl_w, mut win, comp_w, xdg_w, hw, bw)) = client_with_pool((stride * h) as usize, w, h, stride)
    else {
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    let (sw, _) = open_window(&mut win, cl_w, comp_w, xdg_w, "typing", bw, w, h);
    layout::set_window_pos(cl_w, sw, 0, 0);
    let anchor = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_LEFT;
    let Some((cl_l, _l, sl, _ls, hl, _)) = layer_client(protocol::layer::TOP, anchor, 0, 64) else {
        t.check(NAME, false, "no layer surface".to_string());
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        return;
    };

    server::focus_window(Some(cl_w), sw);
    server::pointer_motion(5, 5, 1);
    let over_panel = server::pointer_focus() == Some((cl_l, sl));
    server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, true, 2);
    server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, false, 3);
    let kept = focus::focus_key() == Some((cl_w, sw));
    t.check(NAME, over_panel && kept,
            format!("pointer on the panel {}, keyboard now {:?}, expected {:?}",
                    over_panel, focus::focus_key(), (cl_w, sw)));
    server::disconnect(cl_l);
    let _ = shm::destroy(hl);

    // The launcher's case: `on_demand` is asking for it, and a click gives it
    if let Some((cl_d, _d, sd, _ld, hd, _)) =
        locking_client(protocol::layer::TOP, protocol::layer_surface::KEYBOARD_ON_DEMAND)
    {
        server::focus_window(Some(cl_w), sw);
        server::pointer_motion(6, 6, 4);
        server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, true, 5);
        server::pointer_button(crate::mayland::input::pointer::BTN_LEFT, false, 6);
        t.check("layer: clicking a layer that asked for the keyboard gives it the keyboard",
                focus::focus_key() == Some((cl_d, sd)),
                format!("keyboard now {:?}, expected {:?}", focus::focus_key(), (cl_d, sd)));
        server::disconnect(cl_d);
        let _ = shm::destroy(hd);
    }
    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
}

/// A layer taking the keyboard leaves the window behind it the active one
///
/// Hyprland's arrangement: `rawSurfaceFocus` moves the keyboard and leaves
/// `m_focusWindow`, so the border stays lit and the bar keeps the title
pub(super) fn test_layer_keyboard_leaves_the_window_active(t: &mut Runner) {
    const NAME: &str = "layer: the keyboard going to a layer leaves the window active";
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };
    let win = Some((cl_w, sw));
    server::focus_window(Some(cl_w), sw);
    let before = (focus::focus_key(), server::active_key());

    // In the band a panel lives in: an exclusive overlay with nobody logged in
    // is the login screen, which is `test_lock_pins_the_focus`'s
    let Some((cl_l, _l, sl, _ls, hl, _)) =
        locking_client(protocol::layer::TOP, protocol::layer_surface::KEYBOARD_EXCLUSIVE)
    else {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        t.check(NAME, false, "the layer could not be set up".to_string());
        return;
    };
    let up = (focus::focus_key(), server::active_key());
    server::disconnect(cl_l);
    let _ = shm::destroy(hl);
    let back = (focus::focus_key(), server::active_key());

    // A window that closes is no longer the active one
    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
    let gone = server::active_key() != win;

    t.check(NAME,
            before == (win, win) && up == (Some((cl_l, sl)), win) && back == (win, win) && gone,
            format!("(keyboard, active): before {:?}, with the layer up {:?}, after it {:?}; \
                     the window gone and still active {}",
                    before, up, back, !gone));
}

/// A layer that asks for the keyboard on demand before it appears takes it as
/// it appears, and the window has it back when the layer goes; one that asks
/// once it is up waits for a click. Hyprland's arrangement: end4's overview
/// opens on demand, and Escape and the search must reach it, not the terminal
/// behind
pub(super) fn test_layer_on_demand_takes_the_keyboard_as_it_appears(t: &mut Runner) {
    const NAME: &str = "layer: on demand takes the keyboard as it appears, and gives it back";
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };
    let win = Some((cl_w, sw));
    server::focus_window(Some(cl_w), sw);
    let Some((cl_l, _l, sl, _ls, hl, _, _)) = layer_client_asking(protocol::layer::TOP,
        protocol::layer_surface::ANCHOR_ALL, -1, 0, Some(protocol::layer_surface::KEYBOARD_ON_DEMAND), None)
    else {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        t.check(NAME, false, "the layer could not be set up".to_string());
        return;
    };
    let up = focus::focus_key();
    server::disconnect(cl_l);
    let _ = shm::destroy(hl);
    let back = focus::focus_key();
    // Asked for once it is up: the keyboard stays with the window
    let later = locking_client(protocol::layer::TOP, protocol::layer_surface::KEYBOARD_ON_DEMAND);
    let waits = focus::focus_key();
    if let Some((cl_d, _, _, _, hd, _)) = later {
        server::disconnect(cl_d);
        let _ = shm::destroy(hd);
    }
    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
    t.check(NAME, up == Some((cl_l, sl)) && back == win && waits == win,
            format!("with the layer up {:?}, after it {:?}, asked later {:?}; the window {:?}",
                    up, back, waits, win));
}

/// A layer that arrives under a pointer nobody moved has the pointer, and gives
/// it back when it goes. aosh's launcher opens under the cursor, and the click
/// that closes it must reach the launcher, not the window behind
pub(super) fn test_layer_arriving_under_a_still_pointer(t: &mut Runner) {
    const NAME: &str = "layer: one that arrives under a still pointer has it";
    let (w, h, stride) = (40i32, 30i32, 160i32);
    let Some((cl_w, mut win, comp_w, xdg_w, hw, bw)) = client_with_pool((stride * h) as usize, w, h, stride)
    else {
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    let (sw, _) = open_window(&mut win, cl_w, comp_w, xdg_w, "under", bw, w, h);
    layout::set_window_pos(cl_w, sw, 0, 0);
    server::pointer_motion(5, 5, 1);
    let before = server::pointer_focus();
    let anchor = protocol::layer_surface::ANCHOR_TOP | protocol::layer_surface::ANCHOR_LEFT;
    let Some((cl_l, _l, sl, _ls, hl, _)) = layer_client(protocol::layer::OVERLAY, anchor, -1, 64) else {
        t.check(NAME, false, "no layer surface".to_string());
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        return;
    };
    t.check(NAME, before == Some((cl_w, sw)) && server::pointer_focus() == Some((cl_l, sl)),
            format!("before {:?}, now {:?}, expected {:?}", before, server::pointer_focus(), (cl_l, sl)));
    server::disconnect(cl_l);
    let _ = shm::destroy(hl);
    // On whatever is under it now. Not necessarily the window: the panel going
    // re-tiles it, and it may have moved out from under the cursor
    let under = focus::under_pointer();
    t.check("layer: and when it goes the pointer is on what is under it",
            server::pointer_focus() == under && under != Some((cl_l, sl)),
            format!("now {:?}, under it {:?}", server::pointer_focus(), under));
    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
}
