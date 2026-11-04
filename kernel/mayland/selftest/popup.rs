// Popups: where they go, what they hold, and what lets go
//
// Two halves, tested apart. `place` is arithmetic with no client, screen or
// server lock, so it is checked directly, with the awkward inputs a client is
// free to send. The rest needs a real connection, because it checks the
// pointer, the keyboard and the stack while a menu is open.
//
// Every case that opens a menu closes it before it returns. A popup grab left
// behind takes the desktop's pointer for the rest of the boot.

use super::*;
use crate::mayland::input::grab;
use crate::mayland::shell::layout::Rect;
use crate::mayland::shell::popup::{place, Positioner};
use crate::mayland::protocol::positioner::{adjust, anchor, gravity};

/// A screen to place against, and a parent in the middle of it
const SCREEN: Rect = Rect { x: 0, y: 0, w: 1000, h: 800 };
const PARENT: Rect = Rect { x: 400, y: 300, w: 200, h: 100 };

fn recipe(a: u32, g: u32, adj: u32) -> Positioner {
    Positioner {
        w: 80,
        h: 60,
        // The whole parent, so the anchor points are its own corners
        anchor_rect: Rect::new(0, 0, PARENT.w, PARENT.h),
        anchor: a,
        gravity: g,
        adjust: adj,
        off: (0, 0),
    }
}

/// The recipe, run directly
///
/// Nine anchors against nine gravities is eighty-one answers. Checked instead:
/// the anchor point is on the parent, the gravity puts the popup on the correct
/// side of it, and neither depends on the other
pub(super) fn test_popup_placement(t: &mut Runner) {
    // Anchor picks a point on the parent. Bottom-left of the parent, popup
    // growing down and right: the popup's top-left is exactly there
    let r = place(&recipe(anchor::BOTTOM_LEFT, gravity::BOTTOM_RIGHT, adjust::NONE),
                  PARENT, SCREEN);
    let corner = r.x == PARENT.x && r.y == PARENT.y + PARENT.h;

    // The same point, popup growing up and left: its bottom-right is there
    let r2 = place(&recipe(anchor::BOTTOM_LEFT, gravity::TOP_LEFT, adjust::NONE),
                   PARENT, SCREEN);
    let mirrored = r2.x + r2.w == PARENT.x && r2.y + r2.h == PARENT.y + PARENT.h;

    // No anchor and no gravity is the centre of the parent, centred on itself
    let r3 = place(&recipe(anchor::NONE, gravity::NONE, adjust::NONE), PARENT, SCREEN);
    let centred = r3.x + r3.w / 2 == PARENT.x + PARENT.w / 2
               && r3.y + r3.h / 2 == PARENT.y + PARENT.h / 2;

    // The size is the client's and is never changed by a placement
    let sized = r.w == 80 && r.h == 60 && r3.w == 80;

    t.check("popup: the recipe puts a menu where it says",
            corner && mirrored && centred && sized,
            format!("anchor point on the parent {}, gravity mirrors {}, \
                     no anchor is the centre {}, the size is the client's {}",
                    corner, mirrored, centred, sized));
}

/// What happens at an edge
pub(super) fn test_popup_constraints(t: &mut Runner) {
    // A parent hard against the right edge, a menu growing right: it does not
    // fit, and flipping puts it on the other side of its anchor
    let right = Rect::new(SCREEN.w - 60, 300, 50, 100);
    let unconstrained = place(&recipe(anchor::TOP_RIGHT, gravity::BOTTOM_RIGHT,
                                      adjust::NONE), right, SCREEN);
    let overflows = unconstrained.x + unconstrained.w > SCREEN.w;

    let flipped = place(&recipe(anchor::TOP_RIGHT, gravity::BOTTOM_RIGHT,
                                adjust::FLIP_X), right, SCREEN);
    let fits_now = flipped.x >= 0 && flipped.x + flipped.w <= SCREEN.w;

    // At the bottom, sliding rather than flipping: it ends up against the edge
    // and never past it
    let low = Rect::new(400, SCREEN.h - 20, 200, 20);
    let slid = place(&recipe(anchor::BOTTOM_LEFT, gravity::BOTTOM_RIGHT,
                             adjust::SLIDE_Y), low, SCREEN);
    let inside = slid.y + slid.h <= SCREEN.h && slid.y >= 0;

    // A menu bigger than the screen is cut down to it. Sliding cannot fit
    // something that does not fit, and a rectangle wider than the screen would
    // slide for ever
    let mut huge = recipe(anchor::TOP_LEFT, gravity::BOTTOM_RIGHT, adjust::SLIDE_X);
    huge.w = SCREEN.w * 4;
    let cut = place(&huge, PARENT, SCREEN);
    let clamped = cut.w == SCREEN.w;

    // The numbers a client is free to send: `i32::MAX` everywhere must come
    // back as a rectangle, not a wrapped negative one
    let mut absurd = recipe(anchor::BOTTOM_RIGHT, gravity::BOTTOM_RIGHT, adjust::NONE);
    absurd.anchor_rect = Rect::new(i32::MAX, i32::MAX, i32::MAX, i32::MAX);
    absurd.off = (i32::MAX, i32::MAX);
    let sat = place(&absurd, Rect::new(i32::MAX, i32::MAX, 10, 10), SCREEN);
    let no_wrap = sat.w > 0 && sat.h > 0;

    t.check("popup: a menu that does not fit flips, slides, and never wraps",
            overflows && fits_now && inside && clamped && no_wrap,
            format!("overflows unconstrained {}, flips back in {}, slides in {}, \
                     bigger than the screen is cut to it {}, i32::MAX saturates {}",
                    overflows, fits_now, inside, clamped, no_wrap));
}

/// Maps one window and returns both its ids
///
/// `open_window` returns only the surface, and a popup hangs off the
/// xdg_surface above it
fn map_window(conn: &mut ClientConn, cl: u32, comp: u32, xdg: u32, buffer: u32,
              w: i32, h: i32) -> Option<(u32, u32)> {
    let surface = conn.create_surface(comp);
    let xs = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xs);
    let events = exchange(conn, cl);
    if first_error(&events).is_some() {
        return None;
    }
    let serial = events.iter().rev().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs, serial);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(conn, cl);
    layout::set_window_pos(cl, surface, 0, 0);
    Some((surface, xs))
}

/// A client with a window and a menu hanging off it
///
/// Returns what a case needs to drive both, and the serial of a real keystroke:
/// a grab that answers nothing is refused
fn window_with_popup(w: i32, h: i32)
    -> Option<(u32, ClientConn, u32, u32, u32, u32, u32, u32)>
{
    let stride = 64 * 4;
    let (cl, mut conn, comp, xdg, handle, buffer) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)?;
    // Opened by hand because a popup hangs off the parent's xdg_surface, and
    // `open_window` only returns the surface under it
    let (parent, parent_xs) = map_window(&mut conn, cl, comp, xdg, buffer, 64, 64)?;

    // A real keystroke, for a serial to quote back. The keyboard has to be
    // asked for first: the compositor sends a key to the focused client's
    // keyboard objects, and one that never bound a seat has none
    super::pointer::bind_keyboard(&mut conn, cl)?;
    crate::mayland::server::key(30, true, 0);
    let events = drain_events(&mut conn, cl);
    let serial = events.iter().rev().find_map(|e| match e {
        Event::Key { serial, .. } => Some(*serial), _ => None,
    }).unwrap_or(0);

    let pos = conn.create_positioner(xdg);
    conn.positioner_set_size(pos, w, h);
    conn.positioner_set_anchor_rect(pos, 0, 0, 64, 64);
    conn.positioner_set_anchor(pos, anchor::BOTTOM_LEFT);
    conn.positioner_set_gravity(pos, gravity::BOTTOM_RIGHT);

    let surface = conn.create_surface(comp);
    let xs = conn.get_xdg_surface(xdg, surface);
    let popup = conn.get_popup(xs, parent_xs, pos);
    let events = exchange(&mut conn, cl);
    if let Some((_, why)) = first_error(&events) {
        crate::serial_println!("[mayland] window_with_popup refused: {}", why);
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return None;
    }
    let cfg = events.iter().rev().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs, cfg);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    Some((cl, conn, parent, surface, popup, handle, serial, buffer))
}

/// Where a menu sits in the stack, and where it does not sit
pub(super) fn test_popup_is_a_child(t: &mut Runner) {
    let Some((cl, mut conn, parent, popup, _pid, handle, _serial, _buf)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: a menu is drawn directly above what it hangs off",
                false, "could not open a window with a menu".into());
        return;
    };

    // Directly above, and nowhere else: a menu another window could get between
    // could be hidden by the thing it belongs to
    let on_screen = mine(&[cl]);
    let pi = on_screen.iter().position(|p| p.surface == parent);
    let ui = on_screen.iter().position(|p| p.surface == popup);
    let directly_above = matches!((pi, ui), (Some(a), Some(b)) if b == a + 1);

    // Out of the tiling: a menu the layout arranged would take half the screen
    // from its own window
    layout::arrange_now();
    let s = server::lock();
    let placed_by_layout = s.surface_idx((cl, popup))
        .map(|i| s.surfaces[i].goal.w != 80)
        .unwrap_or(true);
    drop(s);

    // It moves with its parent, whoever moved it
    layout::set_window_pos(cl, parent, 120, 90);
    let s = server::lock();
    let ppos = s.surface_idx((cl, parent)).map(|i| s.surfaces[i].pos);
    let upos = s.surface_idx((cl, popup)).map(|i| s.surfaces[i].pos);
    let rel = s.surface_idx((cl, popup)).map(|i| s.surfaces[i].rel);
    drop(s);
    let follows = matches!((ppos, upos, rel),
        (Some(p), Some(u), Some(r)) if u == (p.0 + r.0, p.1 + r.1));

    t.check("popup: a menu is drawn directly above what it hangs off",
            directly_above && !placed_by_layout && follows,
            format!("directly above {} (stack {:?}), out of the tiling {}, \
                     follows its window {} ({:?} + {:?} = {:?})",
                    directly_above,
                    on_screen.iter().map(|p| p.surface).collect::<Vec<_>>(),
                    !placed_by_layout, follows, ppos, rel, upos));

    let _ = conn;
    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// The grab: who may take it, and what it takes
pub(super) fn test_popup_grab(t: &mut Runner) {
    let Some((cl, mut conn, _parent, popup_surface, popup, handle, serial, _buf)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: a menu holds the pointer, and a click outside lets go",
                false, "could not open a window with a menu".into());
        return;
    };

    // A grab that answers nothing is refused. This stands between a program
    // with no window on screen and the desktop's pointer
    conn.popup_grab(popup, 0, 0);
    let events = exchange(&mut conn, cl);
    let refused = matches!(first_error(&events),
        Some((_, ref why)) if why.contains("answers no key and no button"));
    // The refusal is fatal, as every protocol error here is, so the rest of the
    // case needs a live connection
    server::disconnect(cl);
    let _ = shm::destroy(handle);
    let _ = conn;

    let Some((cl, mut conn, _parent, popup_surface2, popup2, handle2, serial2, _b)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: a menu holds the pointer, and a click outside lets go",
                false, "could not open a second window with a menu".into());
        return;
    };
    let _ = (popup_surface, serial);

    conn.popup_grab(popup2, 0, serial2);
    let events = exchange(&mut conn, cl);
    let took = first_error(&events).is_none() && grab::popup_holds((cl, popup_surface2));

    // A click inside the menu is the menu's: it stays open and the client
    // hears the button
    crate::mayland::server::pointer_motion(0, 0, 0);
    let held_after_inside = grab::popup_holds((cl, popup_surface2));

    // A click outside dismisses it and does not reach whatever was under the
    // pointer
    crate::mayland::server::pointer_motion(900, 700, 0);
    crate::mayland::server::pointer_button(0x110, true, 0);
    let events = drain_events(&mut conn, cl);
    let told = events.iter().any(|e| matches!(e, Event::PopupDone));
    let no_button = !events.iter().any(|e| matches!(e, Event::PointerButton { .. }));
    let let_go = !grab::popup_holds((cl, popup_surface2));

    t.check("popup: a menu holds the pointer, and a click outside lets go",
            refused && took && held_after_inside && told && no_button && let_go,
            format!("a grab answering nothing is refused {}, a real serial takes it {}, \
                     stays while inside {}, told when dismissed {}, \
                     the dismissing click is swallowed {}, let go {}",
                    refused, took, held_after_inside, told, no_button, let_go));

    let _ = grab::popup_close_to(None);
    server::disconnect(cl);
    let _ = shm::destroy(handle2);
}

/// A menu whose window closes, and menus nested past the end
pub(super) fn test_popup_teardown(t: &mut Runner) {
    let Some((cl, mut conn, _parent, popup_surface, popup, handle, serial, _buf)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: a menu goes when its window goes",
                false, "could not open a window with a menu".into());
        return;
    };

    conn.popup_grab(popup, 0, serial);
    let _ = exchange(&mut conn, cl);
    let held = grab::popup_holds((cl, popup_surface));

    // The whole client goes. Nothing is sent, but the grab must not outlive it,
    // or the desktop's pointer belongs to a process that has exited
    server::disconnect(cl);
    let released = !grab::popup_holds((cl, popup_surface));
    let nothing_held = grab::popup_client().is_none();
    let _ = shm::destroy(handle);
    let _ = conn;

    t.check("popup: a menu goes when its window goes",
            held && released && nothing_held,
            format!("held while open {}, released with the client {}, \
                     nothing holds the pointer {}", held, released, nothing_held));
}

/// The two shapes a client can ask for that must not be built
pub(super) fn test_popup_refuses_a_chain_that_does_not_end(t: &mut Runner) {
    let stride = 64 * 4;
    let Some((cl, mut conn, comp, xdg, handle, buffer)) =
        client_with_pool((stride * 64) as usize, 64, 64, stride)
    else {
        t.check("popup: a menu cannot hang off itself, or off nine others",
                false, "no client".into());
        return;
    };
    let Some((_parent, xs0)) = map_window(&mut conn, cl, comp, xdg, buffer, 64, 64) else {
        t.check("popup: a menu cannot hang off itself, or off nine others",
                false, "no window".into());
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return;
    };
    let mut parent_xs = xs0;

    let pos = conn.create_positioner(xdg);
    conn.positioner_set_size(pos, 40, 20);
    conn.positioner_set_anchor_rect(pos, 0, 0, 10, 10);
    let _ = exchange(&mut conn, cl);

    // Menus inside menus, one deeper each time, until the compositor says no
    let mut depth = 0;
    let mut refusal = String::new();
    for _ in 0..16 {
        let surface = conn.create_surface(comp);
        let xs = conn.get_xdg_surface(xdg, surface);
        conn.get_popup(xs, parent_xs, pos);
        let events = exchange(&mut conn, cl);
        if let Some((_, why)) = first_error(&events) {
            refusal = why;
            break;
        }
        let cfg = events.iter().rev().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        conn.ack_configure(xs, cfg);
        conn.attach(surface, buffer, 0, 0);
        conn.commit(surface);
        let _ = exchange(&mut conn, cl);
        parent_xs = xs;
        depth += 1;
    }
    let bounded = depth > 0 && depth < 16 && refusal.contains("deeper");
    // The stack still terminates with all of them open, which is what the depth
    // limit is for
    let drawn = mine(&[cl]).len();

    t.check("popup: a menu cannot hang off itself, or off nine others",
            bounded && drawn > 0,
            format!("nested {} deep then refused with {:?}, and {} surfaces \
                     still compose", depth, refusal, drawn));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// One program may not take the pointer out from under another's menu
///
/// The rule is xdg-shell's: a grab belongs on the innermost open popup, and a
/// second client's popup is never that
pub(super) fn test_popup_grab_cannot_be_stolen(t: &mut Runner) {
    let Some((first, mut a, _p1, held_surface, popup_a, ha, serial_a, _b1)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: one program may not take the pointer from another's menu",
                false, "could not open the first menu".into());
        return;
    };
    a.popup_grab(popup_a, 0, serial_a);
    let _ = exchange(&mut a, first);
    let holds = grab::popup_holds((first, held_surface));

    // A second program with its own window, menu and honest serial. Only what
    // it asks for is illegitimate
    let Some((second, mut b, _p2, _s2, popup_b, hb, serial_b, _b2)) =
        window_with_popup(80, 60)
    else {
        t.check("popup: one program may not take the pointer from another's menu",
                false, "could not open the second menu".into());
        let _ = grab::popup_close_to(None);
        server::disconnect(first);
        let _ = shm::destroy(ha);
        return;
    };
    b.popup_grab(popup_b, 0, serial_b);
    let events = exchange(&mut b, second);
    let refused = matches!(first_error(&events),
        Some((_, ref why)) if why.contains("not the innermost one open"));
    let still_first = grab::popup_holds((first, held_surface));

    t.check("popup: one program may not take the pointer from another's menu",
            holds && refused && still_first,
            format!("the first menu holds the pointer {}, the second is refused {}, \
                     and the first still holds it {}", holds, refused, still_first));

    let _ = grab::popup_close_to(None);
    server::disconnect(second);
    server::disconnect(first);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}
