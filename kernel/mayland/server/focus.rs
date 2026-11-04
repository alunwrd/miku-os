// Who gets the input, and what is in front
//
// The keyboard focus is where typing goes; the pointer focus is whatever the
// cursor is over; the stacking order decides what is drawn last and what a
// click lands on. They are together because each changes when another does: a
// click raises a window, which focuses it, which reconfigures two windows and
// tells the window manager.
//
// Two focuses
//
// The keyboard focus changes only when something decides it should; the pointer
// focus changes as the mouse moves. A window highlighting under the cursor
// while typing stays where it was is what a menu bar does.
//
// Coordinates in pointer events are surface-local and 24.8 fixed point, as the
// protocol says.
//
// Everything here takes `&mut Server` rather than reaching for the lock, except
// the entry points the input layer calls. A second `lock()` inside a function
// already holding it spins for ever.

use alloc::vec::Vec;

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{self, Arg};
use crate::mayland::shell::layout::{self, Rect};

use super::{iface, scene, surface};
use super::state::*;

/// Whether a lock is up and `to` is not the surface holding it
///
/// Every keyboard-moving path in this file asks this. A login screen that can
/// be focused away from has a root console behind it, so it is checked in
/// `set_focus`, which all of them funnel through, and again in the two entry
/// points a client can reach directly, so a refusal does not flicker the focus
pub(in crate::mayland) fn locked_away(s: &Server, to: Option<(u32, u32)>) -> bool {
    match s.lock {
        Some(lock) => to != Some(lock),
        None => false,
    }
}

/// The topmost toplevel other than `except`: who gets the keyboard when the
/// window holding it goes away
pub(in crate::mayland) fn next_focusable(s: &Server, except: Option<(u32, u32)>) -> Option<(u32, u32)> {
    // While something holds the lock it is the only answer, including when
    // `except` names it: the caller asks who takes over from a window that went
    // away, and under a lock nobody does
    if let Some(lock) = s.lock {
        return Some(lock);
    }
    // On this workspace, not merely on screen: `stack` keeps a window still
    // sliding off after a switch, and `move_to_workspace` has just sent the
    // focused window away
    let ws = *CURRENT_WS.lock();
    s.stack().into_iter().rev()
        .filter(|&i| s.surfaces[i].focusable() && s.surfaces[i].on_workspace(ws))
        .map(|i| (s.surfaces[i].client, s.surfaces[i].id))
        .find(|k| Some(*k) != except)
}

/// Puts a surface at the top of the stack
///
/// The compositor draws bottom to top, so "on top" is one number per surface
pub(in crate::mayland) fn raise(s: &mut Server, si: usize) {
    // Except the backdrop, which has nowhere to be raised to
    if s.surfaces[si].backdrop {
        return;
    }
    s.surfaces[si].z = s.next_z;
    s.next_z += 1;
}

/// Puts a surface behind everything
///
/// For the console, which is the backdrop. Counting down from where the first
/// surface started leaves room to do it as many times as there are surfaces, so
/// z does not begin at zero
pub fn lower_window(client: u32, surface: u32) -> bool {
    set_backdrop(client, surface, true)
}

/// Takes a surface out of the focus cycle for good: see `Surface::no_focus`
pub fn set_no_focus(client: u32, surface: u32) -> bool {
    let mut s = SERVER.lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    set_no_focus_at(&mut s, si, true);
    true
}

/// The same, for a caller that already holds the lock
///
/// A window rule reaches this (`windowrule = nofocus, class:splash`), and a
/// rule arrives inside `set_window_state`, which already has the server
pub(in crate::mayland) fn set_no_focus_at(s: &mut Server, si: usize, on: bool) {
    // Never the console or the wallpaper: a config line taking the keyboard
    // from the shell could lock the user out. `killactive` refuses them for the
    // same reason
    if on && s.surfaces[si].sticky {
        return;
    }
    s.surfaces[si].no_focus = on;
    if !on {
        return;
    }
    // It may already have the keyboard: the wallpaper is created first and, as
    // the only window then, is focused when it takes a toplevel role
    let key = (s.surfaces[si].client, s.surfaces[si].id);
    if s.focus == Some(key) {
        let next = next_focusable(s, Some(key));
        set_focus(s, next);
    }
}

/// Makes a surface the backdrop, or stops it being one
///
/// The clearing half means a console that was the backdrop can be tiled again
pub fn set_backdrop(client: u32, surface: u32, on: bool) -> bool {
    let mut s = SERVER.lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    s.surfaces[si].backdrop = on;
    if on {
        let lowest = s.surfaces.iter().map(|x| x.z).min().unwrap_or(1);
        s.surfaces[si].z = lowest.saturating_sub(1);
        // It stays there. Focus and clicks raise a window, and the backdrop is
        // under the pointer whenever the pointer is over nothing else, so
        // without `backdrop` the first click on the desktop would bring it to
        // the front
        //
        // It is the screen, so it is at the origin. The backdrop is where
        // everything else is measured from
        layout::put(&mut s, si, 0, 0);
    } else {
        // Back into the layout, which will give it a position of its own
        s.surfaces[si].placed = false;
        s.surfaces[si].goal = Rect::ZERO;
        raise(&mut s, si);
    }
    // One window more or fewer in the layout
    layout::arrange(&mut s);
    true
}

/// Sends a window the size it already has, so the states array can change
///
/// Activation is carried in configure only, so telling a window it is no longer
/// active is a configure with the same geometry and one fewer state. A window
/// that has not had its first configure is skipped: its first one will carry
/// the right states
pub(in crate::mayland) fn reconfigure(s: &mut Server, si: usize) {
    if s.surfaces[si].toplevel_id == 0 || s.surfaces[si].configure_sent == 0 {
        return;
    }
    let (w, h) = (s.surfaces[si].last_w, s.surfaces[si].last_h);
    surface::configure(s, si, w, h);
}

/// Moves the keyboard to a surface, telling both ends
///
/// A client learns it has the keyboard from may_keyboard.enter and that it lost
/// it from leave. A window is also drawn differently when typed at, and a
/// client that never asked for a keyboard still needs to know: xdg-shell
/// carries it as the ACTIVATED state in configure, so both windows are
/// reconfigured here. The new one is raised at the same time
pub(in crate::mayland) fn set_focus(s: &mut Server, to: Option<(u32, u32)>) {
    if s.focus == to {
        return;
    }
    // Nothing takes the keyboard from a lock. Every other path here goes
    // through this function, so this line makes the greeter's grab hold
    if locked_away(s, to) {
        return;
    }
    let from = s.focus;
    s.focus = to;

    if let Some((cid, sid)) = from {
        let serial = s.next_serial();
        if let Some(idx) = s.client_idx(cid) {
            let kbds = s.clients[idx].objects.ids_of(Interface::Keyboard);
            for k in kbds {
                s.clients[idx].queue(k, protocol::keyboard::LEAVE, &[
                    Arg::Uint(serial), Arg::Object(sid),
                ]);
            }
        }
    }
    if let Some((cid, sid)) = to {
        let serial = s.next_serial();
        // Modifiers are tracked, and the window taking the keyboard has to be
        // told them: focus moves on a chord, so ctrl and alt are held as the
        // `enter` goes out. `input::drain` sends this only when the mask
        // changes, so without this the new window reads the first keystroke of
        // every chord as a plain key
        let mods = crate::mayland::input::modifiers();
        // The layout in use, which this window may never have been told: the
        // group moved while somebody else had the keyboard
        let group = crate::io::input::layout::group();
        if let Some(idx) = s.client_idx(cid) {
            let kbds = s.clients[idx].objects.ids_of(Interface::Keyboard);
            for k in kbds {
                // No keys held: key state is not tracked across a focus change,
                // and claiming a key is down when it might not be would leave
                // the new client with a stuck key
                s.clients[idx].queue(k, protocol::keyboard::ENTER, &[
                    Arg::Uint(serial), Arg::Object(sid), Arg::Array(&[]),
                ]);
                s.clients[idx].queue(k, protocol::keyboard::MODIFIERS, &[
                    Arg::Uint(serial), Arg::Uint(mods),
                    Arg::Uint(0), Arg::Uint(0), Arg::Uint(group),
                ]);
            }
        }
    }

    // Both windows have to hear about it, and the one gaining focus comes to
    // the front. Order matters only in that s.focus is already the new value,
    // so each configure carries the right states
    if let Some(si) = from.and_then(|k| s.surface_idx(k)) {
        reconfigure(s, si);
    }
    if let Some(si) = to.and_then(|k| s.surface_idx(k)) {
        raise(s, si);
        reconfigure(s, si);
    }

    // The active window follows the keyboard to a window and not to a layer
    // surface (see `Server::active`). The launcher taking the keyboard leaves
    // the terminal behind it active, and handing the keyboard back changes
    // nothing anyone has to hear about
    let layer = to.and_then(|k| s.surface_idx(k)).is_some_and(|si| s.surfaces[si].layer.is_some());
    if !layer {
        set_active(s, to);
    }
}

/// Makes `to` the active window, and tells the desktop if that is news
///
/// The desktop's `focused` is this and not the keyboard: what a bar shows the
/// title of and what a window manager's binds act on
pub(in crate::mayland) fn set_active(s: &mut Server, to: Option<(u32, u32)>) {
    if s.active == to {
        return;
    }
    s.active = to;
    // 0,0 for "nothing has it", which is a pair no window ever is
    let (c, sf) = to.unwrap_or((0, 0));
    iface::desktop::event(s, protocol::desktop::FOCUSED, &[Arg::Uint(c), Arg::Uint(sf)]);
}

/// Gives the keyboard to the next window, in the order they were created
///
/// Round-robin over creation order, not the stack: cycling by stacking order
/// visits the top two windows for ever, because raising the chosen window puts
/// it back where it started
pub fn focus_next() -> Option<(u32, u32)> {
    let mut s = SERVER.lock();
    // Creation order, but the same set `stack()` draws: a window on another
    // workspace is not on the screen, and keystrokes must not go there.
    // Afterimages are out by `no_focus`
    let ws = *CURRENT_WS.lock();
    let windows: Vec<(u32, u32)> = s.surfaces.iter()
        .filter(|x| x.role == Role::Toplevel && !x.no_focus && x.on_workspace(ws))
        .map(|x| (x.client, x.id))
        .collect();
    if windows.is_empty() {
        return None;
    }
    let next = match s.focus.and_then(|f| windows.iter().position(|&k| k == f)) {
        Some(i) => windows[(i + 1) % windows.len()],
        None => windows[0],
    };
    // Answering with a window the keyboard did not move to would have the
    // caller redraw a focus ring around it
    if locked_away(&s, Some(next)) {
        return None;
    }
    set_focus(&mut s, Some(next));
    Some(next)
}

/// Gives the keyboard to a named window. False if there is no such toplevel
///
/// Object ids belong to the connection that made them, so two clients can both
/// call a window 11. A surface id alone picks the first window that has it,
/// which only works with one client
pub fn focus_window(client: Option<u32>, surface: u32) -> bool {
    let mut s = SERVER.lock();
    let key = s.surfaces.iter()
        .find(|x| x.id == surface && x.role == Role::Toplevel && !x.no_focus
                  && client.map_or(true, |c| c == x.client))
        .map(|x| (x.client, x.id));
    match key {
        Some(k) if !locked_away(&s, Some(k)) => { set_focus(&mut s, Some(k)); true }
        _ => false,
    }
}

/// Delivers a key to whoever has the keyboard
///
/// Called from the compositor thread by mayland::input, never from the keyboard
/// interrupt (see the note at the top of `input`)
pub fn key(keycode: u32, pressed: bool, time_ms: u32) {
    let mut s = SERVER.lock();
    let Some((cid, _)) = s.focus else { return };
    // Remembered: one of the two serials a client may quote back to take the
    // pointer or set the selection
    let serial = s.input_serial();
    let Some(idx) = s.client_idx(cid) else { return };
    let state = if pressed {
        protocol::keyboard::KEY_PRESSED
    } else {
        protocol::keyboard::KEY_RELEASED
    };
    let kbds = s.clients[idx].objects.ids_of(Interface::Keyboard);
    for k in kbds {
        s.clients[idx].queue(k, protocol::keyboard::KEY, &[
            Arg::Uint(serial), Arg::Uint(time_ms), Arg::Uint(keycode), Arg::Uint(state),
        ]);
    }
}

/// Tells the focused client which modifiers are held, and which layout
///
/// Sent before the keys typed under them, so a client never sees a keystroke
/// before the state it was typed in
pub fn modifiers(mask: u32) {
    let group = crate::io::input::layout::group();
    let mut s = SERVER.lock();
    let Some((cid, _)) = s.focus else { return };
    let serial = s.next_serial();
    let Some(idx) = s.client_idx(cid) else { return };
    let kbds = s.clients[idx].objects.ids_of(Interface::Keyboard);
    for k in kbds {
        s.clients[idx].queue(k, protocol::keyboard::MODIFIERS, &[
            Arg::Uint(serial),
            Arg::Uint(mask),  // depressed
            Arg::Uint(0),     // latched
            Arg::Uint(0),     // locked
            Arg::Uint(group), // the layout, an index into the keymap's groups
        ]);
    }
}

/// The seat moved to another layout: tell the bar
///
/// The focused window has already been told, in `modifiers`, which goes out
/// first. Every other window learns the group when it next gets the keyboard
pub fn keyboard_layout_changed() {
    let mut s = SERVER.lock();
    super::iface::desktop::send_keyboard_layout(&mut s);
}

/// Which connection has the keyboard, without the title. Cheap enough for the
/// input drain to ask every frame
pub fn focus_client() -> Option<u32> {
    SERVER.lock().focus.map(|(c, _)| c)
}

/// The focused window as (client, surface), for comparing against a known one
pub fn focus_key() -> Option<(u32, u32)> {
    SERVER.lock().focus
}

/// Which window is the active one (`Server::active`). Not always the one with
/// the keyboard: with the launcher up it is the window behind it
pub fn active_key() -> Option<(u32, u32)> {
    SERVER.lock().active
}

/// Who has the keyboard, and what the window is called
pub fn focus() -> Option<(u32, u32, alloc::string::String)> {
    let s = SERVER.lock();
    let (cid, sid) = s.focus?;
    let title = s.surfaces.iter()
        .find(|x| x.client == cid && x.id == sid)
        .map(|x| x.title.clone())
        .unwrap_or_default();
    Some((cid, sid, title))
}

/// Which surface is under a screen point, and where in it
///
/// Topmost first, so `stack` is walked backwards: the window in front gets the
/// cursor as it gets the pixels
pub(in crate::mayland) fn surface_at(s: &Server, x: i32, y: i32) -> Option<(u32, u32, i32, i32)> {
    for si in s.stack().into_iter().rev() {
        let surf = &s.surfaces[si];
        let Some(bid) = surf.current.buffer else { continue };
        let Some(b) = s.buffers.iter().find(|b| b.client == surf.client && b.id == bid)
        else { continue };
        let (ox, oy) = scene::placed(surf);
        // On it, and where its input region takes the pointer: elsewhere it
        // is over whatever is under
        let reaches = surf.input.as_ref().is_none_or(|steps| region_holds(steps, x - ox, y - oy));
        if x >= ox && y >= oy && x < ox + b.width && y < oy + b.height && reaches {
            return Some((surf.client, surf.id, x - ox, y - oy));
        }
    }
    None
}

/// What is under the pointer, as far as a focus grab lets it reach: while one
/// is in force, only the surfaces it lists and the menus its client opens from
/// them
fn reachable(s: &Server, hit: Option<(u32, u32, i32, i32)>) -> Option<(u32, u32, i32, i32)> {
    use crate::mayland::input::grab;
    let Some(owner) = grab::focus_client() else { return hit };
    hit.filter(|&(c, sid, _, _)| {
        grab::focus_holds((c, sid))
            || (c == owner && s.surface_idx((c, sid)).is_some_and(|i| s.surfaces[i].role == Role::Popup))
    })
}

/// Send one message to every pointer object a client has bound
pub(in crate::mayland) fn to_pointers(s: &mut Server, client: u32, opcode: u16, args: &[Arg]) {
    let Some(idx) = s.client_idx(client) else { return };
    let ptrs = s.clients[idx].objects.ids_of(Interface::Pointer);
    for p in ptrs {
        s.clients[idx].queue(p, opcode, args);
    }
}

/// The cursor is at (x, y) on the screen
///
/// Crossing from one surface to another is a leave and an enter, in that order,
/// each with its own serial: a client seeing the enter first would believe the
/// cursor was in two places
pub fn pointer_motion(x: i32, y: i32, time_ms: u32) {
    let mut s = SERVER.lock();
    let hit = reachable(&s, surface_at(&s, x, y));
    // While a menu holds the pointer, only its own client hears about it.
    // Wayland calls this an owner-events grab; it stops a window highlighting
    // its buttons under an open menu. The menu's client still gets motion over
    // its parent, so a menu bar can slide from one item to the next
    let hit = match crate::mayland::input::grab::popup_client() {
        Some(owner) => hit.filter(|&(c, _, _, _)| c == owner),
        None => hit,
    };
    let now = hit.map(|(c, sid, _, _)| (c, sid));

    if s.pointer_focus != now {
        if let Some((c, sid)) = s.pointer_focus {
            let serial = s.next_serial();
            to_pointers(&mut s, c, protocol::pointer::LEAVE,
                        &[Arg::Uint(serial), Arg::Object(sid)]);
        }
        if let Some((c, sid, lx, ly)) = hit {
            let serial = s.next_serial();
            to_pointers(&mut s, c, protocol::pointer::ENTER, &[
                Arg::Uint(serial), Arg::Object(sid),
                Arg::Fixed(wire::fixed_from_int(lx)), Arg::Fixed(wire::fixed_from_int(ly)),
            ]);
        }
        s.pointer_focus = now;
        // The cursor belonged to the surface being left. The client now under
        // the pointer has said nothing, so it is the compositor's until that
        // client asks
        s.cursor = None;
        follow_mouse(&mut s, now);
        return;
    }

    if let Some((c, _, lx, ly)) = hit {
        to_pointers(&mut s, c, protocol::pointer::MOTION, &[
            Arg::Uint(time_ms),
            Arg::Fixed(wire::fixed_from_int(lx)), Arg::Fixed(wire::fixed_from_int(ly)),
        ]);
    }
}

/// `input { follow_mouse = 1 }`: the keyboard goes where the pointer went
///
/// Called from the one place that decides the pointer has crossed into
/// something else. It must not act:
///
/// - while anything holds the pointer. A menu is open or a window is
///   being carried; the crossing is already filtered to the owner, and moving
///   the keyboard would take it from what was being typed at
/// - to a surface that may not have it: a panel, the wallpaper, a window a
///   rule said `nofocus` about. `focusable` answers that
/// - off a window onto nothing. Leaving a window for the desktop background
///   is not a request to stop typing
fn follow_mouse(s: &mut Server, now: Option<(u32, u32)>) {
    if !crate::mayland::input::conf::follow_mouse() {
        return;
    }
    use crate::mayland::input::grab;
    if grab::popup_client().is_some() || grab::dragging().is_some() {
        return;
    }
    let Some(key) = now else { return };
    if s.focus == Some(key) {
        return;
    }
    let Some(si) = s.surface_idx(key) else { return };
    if !s.surfaces[si].focusable() {
        return;
    }
    set_focus(s, Some(key));
}

/// Works out what the pointer is over again, without it having moved
///
/// Crossings are computed in `pointer_motion`, but a surface can arrive under a
/// still pointer, and a menu opens where the cursor already is. Without this
/// the
/// click that chose a menu item goes to the window underneath and dismisses the
/// menu as a click outside it.
///
/// `pointer_off` is the other half: a surface leaving from under the pointer
pub(in crate::mayland) fn repoint(s: &mut Server) {
    let (x, y) = crate::mayland::input::pointer::position();
    let hit = reachable(s, surface_at(s, x, y));
    let hit = match crate::mayland::input::grab::popup_client() {
        Some(owner) => hit.filter(|&(c, _, _, _)| c == owner),
        None => hit,
    };
    let now = hit.map(|(c, sid, _, _)| (c, sid));
    if s.pointer_focus == now {
        return;
    }
    if let Some((c, sid)) = s.pointer_focus {
        let serial = s.next_serial();
        to_pointers(s, c, protocol::pointer::LEAVE,
                    &[Arg::Uint(serial), Arg::Object(sid)]);
    }
    if let Some((c, sid, lx, ly)) = hit {
        let serial = s.next_serial();
        to_pointers(s, c, protocol::pointer::ENTER, &[
            Arg::Uint(serial), Arg::Object(sid),
            Arg::Fixed(wire::fixed_from_int(lx)), Arg::Fixed(wire::fixed_from_int(ly)),
        ]);
    }
    s.pointer_focus = now;
    // The image belonged to whoever is being left, as on any other crossing
    s.cursor = None;
    follow_mouse(s, now);
}

/// Takes the pointer off a surface that is going away
///
/// Crossings are otherwise computed only in `pointer_motion`, so a window
/// closing under a still cursor would produce no `leave` and leave
/// `pointer_focus` naming a dead surface, and the next button or wheel notch
/// would go to a client told the pointer was still inside.
///
/// `notify` is false when the client itself went away: there is nobody to tell,
/// and the connection's queue is about to be dropped
pub(in crate::mayland) fn pointer_off(s: &mut Server, key: (u32, u32), notify: bool) {
    if s.pointer_focus == Some(key) {
        if notify {
            let serial = s.next_serial();
            to_pointers(s, key.0, protocol::pointer::LEAVE,
                        &[Arg::Uint(serial), Arg::Object(key.1)]);
        }
        s.pointer_focus = None;
        // The image belonged to the client being left, as on any other
        // crossing: the compositor's arrow until somebody asks
        s.cursor = None;
    }
    // If the surface going away was the cursor, whoever is under the pointer
    // has no image at all
    if matches!(s.cursor, Some(ref c) if c.client == key.0 && c.surface == key.1) {
        s.cursor = None;
    }
}

/// A button went down or came up over whatever the pointer is on
///
/// Pressing gives that window the keyboard and raises it. Click to focus is the
/// compositor's to decide: a client that could take focus by asking could steal
/// it
pub fn pointer_button(button: u32, pressed: bool, time_ms: u32) {
    let mut s = SERVER.lock();
    // A press outside an open menu dismisses it and goes no further: the menu
    // closes, and the click does not also press what is underneath.
    //
    // Checked before `pointer_focus` is read, because the press may be over
    // nothing: clicking the wallpaper is a common way to dismiss a menu
    if pressed && crate::mayland::input::grab::popup_client().is_some() {
        let inside = s.pointer_focus
            .is_some_and(|k| crate::mayland::input::grab::popup_holds(k));
        if !inside {
            let closed = crate::mayland::input::grab::popup_close_to(None);
            super::iface::xdg::dismiss(&mut s, closed, true);
            return;
        }
    }
    // A press beside a focus grab's surfaces ends it and goes no further, as a
    // press beside a menu does: the overview it kept open shuts, and the click
    // does not land on what is behind
    if pressed && crate::mayland::input::grab::focus_client().is_some() && s.pointer_focus.is_none() {
        if let Some((c, object)) = crate::mayland::input::grab::focus_clear() {
            if let Some(ci) = s.client_idx(c) {
                s.clients[ci].queue(object, protocol::focus_grab::CLEARED, &[]);
            }
            repoint(&mut s);
        }
        return;
    }
    let Some((c, sid)) = s.pointer_focus else { return };
    let serial = s.input_serial();
    let state = if pressed {
        protocol::pointer::BUTTON_PRESSED
    } else {
        protocol::pointer::BUTTON_RELEASED
    };
    to_pointers(&mut s, c, protocol::pointer::BUTTON, &[
        Arg::Uint(serial), Arg::Uint(time_ms), Arg::Uint(button), Arg::Uint(state),
    ]);

    // A layer surface only if it asked for the keyboard (`focusable`). A bar is
    // `keyboard = none` and is clicked all the time. It is on every workspace,
    // so `switch_workspace` would let it keep the keyboard, and typing on a
    // workspace with a terminal would go nowhere until the terminal was
    // clicked. Windows and menus use `no_focus` alone
    if pressed && s.focus != Some((c, sid)) {
        let takes = |x: &Surface| !x.no_focus && (x.layer.is_none() || x.focusable());
        if let Some(si) = s.surface_idx((c, sid)).filter(|&si| takes(&s.surfaces[si])) {
            raise(&mut s, si);
            set_focus(&mut s, Some((c, sid)));
        }
    }
}

/// The wheel turned. One notch is fifteen units, which is what Wayland's
/// clients divide by
pub fn pointer_axis(notches: i32, time_ms: u32) {
    let mut s = SERVER.lock();
    let Some((c, _)) = s.pointer_focus else { return };
    // Vertical scroll is axis 0. Down the page is positive, and a wheel
    // reports up as positive, so the sign turns over here
    to_pointers(&mut s, c, protocol::pointer::AXIS, &[
        Arg::Uint(time_ms), Arg::Uint(0),
        Arg::Fixed(wire::fixed_from_int(-notches * 15)),
    ]);
}

/// Which surface the pointer is over, for the shell to report
pub fn pointer_focus() -> Option<(u32, u32)> {
    SERVER.lock().pointer_focus
}

/// Which surface is under the cursor now, worked out afresh: what
/// `pointer_focus` must agree with once nothing is moving (the selftest)
pub fn under_pointer() -> Option<(u32, u32)> {
    let s = SERVER.lock();
    let (x, y) = crate::mayland::input::pointer::position();
    surface_at(&s, x, y).map(|(c, sid, _, _)| (c, sid))
}
