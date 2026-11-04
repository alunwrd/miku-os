// may_layer: a surface that is not a window
//
// Everything here decides one of two things: which band the surface is drawn
// in, which `Surface::band` reads and `stack()` sorts by, and how much of an
// edge it keeps, which becomes `layout::Reserved` and so the work area every
// algorithm measures itself against.
//
// A layer surface is sized by the compositor, like a tiled window: it is
// anchored to edges of a screen only the compositor knows the size of. It is
// told through the same configure handshake as a window.

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Arg, Reader};
use crate::mayland::shell::layout;

use crate::mayland::server::{dispatch, focus};
use crate::mayland::server::state::*;
use super::desktop;

pub(in crate::mayland) fn on_layer(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::layer::GET_LAYER_SURFACE => {
            let id = r.new_id().map_err(|_| "get_layer_surface: truncated")?;
            let surface = r.object().map_err(|_| "get_layer_surface: truncated")?;
            let band = r.uint().map_err(|_| "get_layer_surface: truncated")?;
            if band >= protocol::layer::BANDS {
                return Err("get_layer_surface: no such layer");
            }
            let cid = s.clients[idx].id;
            let Some(si) = s.surface_idx((cid, surface)) else {
                return Err("get_layer_surface: no such surface");
            };
            // One role to a surface: a panel that was also a window would be
            // placed by the tiling layout
            if s.surfaces[si].role != Role::None {
                return Err("get_layer_surface: the surface already has a role");
            }
            dispatch::bind_new(s, idx, id, Interface::LayerSurface, 1)?;
            s.surfaces[si].role = Role::Layer;
            s.surfaces[si].layer = Some(LayerState {
                band,
                anchor: 0,
                want_w: 0,
                want_h: 0,
                exclusive: -1,
                keyboard: protocol::layer_surface::KEYBOARD_NONE,
                namespace: libmay::conf::Text::EMPTY,
            });
            s.surfaces[si].xdg_id = id;
            // On every workspace
            s.surfaces[si].sticky = true;
            configure_layer(s, si);
            Ok(())
        }
        protocol::layer::DESTROY => {
            dispatch::destroy_object(s, idx, _sender);
            Ok(())
        }
        protocol::layer::GET_FOCUS_GRAB => {
            let id = r.new_id().map_err(|_| "get_focus_grab: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::FocusGrab, 1)?;
            s.clients[idx].focus_grabs.push((id, alloc::vec::Vec::new()));
            Ok(())
        }
        _ => Err("may_layer: unknown request"),
    }
}

/// may_focus_grab: the list a client builds, and the grab it puts in force
pub(in crate::mayland) fn on_focus_grab(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    use crate::mayland::input::grab;
    use protocol::focus_grab as fg;
    let cid = s.clients[idx].id;
    match opcode {
        fg::ADD_SURFACE | fg::REMOVE_SURFACE => {
            let surface = r.object().map_err(|_| "focus_grab: truncated")?;
            if s.surface_idx((cid, surface)).is_none() {
                return Err("focus_grab: not a surface of this client");
            }
            let Some((_, list)) = s.clients[idx].focus_grabs.iter_mut().find(|(id, _)| *id == sender)
            else { return Err("focus_grab: not a focus grab") };
            if opcode == fg::ADD_SURFACE {
                if !list.contains(&surface) {
                    if list.len() >= fg::MAX_SURFACES {
                        return Err("focus_grab: too many surfaces");
                    }
                    list.push(surface);
                }
            } else {
                list.retain(|&x| x != surface);
            }
            Ok(())
        }
        fg::COMMIT => {
            let list = s.clients[idx].focus_grabs.iter().find(|(id, _)| *id == sender)
                .map(|(_, l)| l.clone()).ok_or("focus_grab: not a focus grab")?;
            if list.is_empty() {
                grab::focus_release(cid, sender);
            } else if let Some((oc, oo)) = grab::focus_take(grab::FocusGrab { client: cid, object: sender, surfaces: list }) {
                // Another grab was in force: it is over
                if let Some(oi) = s.client_idx(oc) {
                    s.clients[oi].queue(oo, fg::CLEARED, &[]);
                }
            }
            // What is under a pointer that has not moved may be out of reach
            // now, or back in it
            super::super::focus::repoint(s);
            Ok(())
        }
        fg::DESTROY => {
            grab::focus_release(cid, sender);
            s.clients[idx].focus_grabs.retain(|(id, _)| *id != sender);
            dispatch::destroy_object(s, idx, sender);
            super::super::focus::repoint(s);
            Ok(())
        }
        _ => Err("may_focus_grab: unknown request"),
    }
}

pub(in crate::mayland) fn on_layer_surface(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let Some(si) = s.surfaces.iter().position(|x| x.client == cid && x.xdg_id == sender
                                                  && x.role == Role::Layer)
    else { return Err("may_layer_surface: not a layer surface") };

    match opcode {
        protocol::layer_surface::SET_ANCHOR => {
            let edges = r.uint().map_err(|_| "set_anchor: truncated")?;
            if edges & !protocol::layer_surface::ANCHOR_ALL != 0 {
                return Err("set_anchor: no such edge");
            }
            if let Some(l) = s.surfaces[si].layer.as_mut() { l.anchor = edges; }
            configure_layer(s, si);
            reserve_edges(s);
            Ok(())
        }
        protocol::layer_surface::SET_SIZE => {
            let w = r.int().map_err(|_| "set_size: truncated")?;
            let h = r.int().map_err(|_| "set_size: truncated")?;
            if w < 0 || h < 0 {
                return Err("set_size: a size is not negative");
            }
            if let Some(l) = s.surfaces[si].layer.as_mut() { l.want_w = w; l.want_h = h; }
            configure_layer(s, si);
            reserve_edges(s);
            Ok(())
        }
        protocol::layer_surface::SET_EXCLUSIVE => {
            let amount = r.int().map_err(|_| "set_exclusive: truncated")?;
            let was = s.surfaces[si].layer.map(|l| l.exclusive);
            if let Some(l) = s.surfaces[si].layer.as_mut() { l.exclusive = amount; }
            // Keeping clear of the reserved edges or not is where it is
            if was.is_some_and(|w| (w == 0) != (amount == 0)) {
                configure_layer(s, si);
            }
            reserve_edges(s);
            Ok(())
        }
        protocol::layer_surface::SET_KEYBOARD => {
            let mode = r.uint().map_err(|_| "set_keyboard: truncated")?;
            if mode > protocol::layer_surface::KEYBOARD_EXCLUSIVE {
                return Err("set_keyboard: no such mode");
            }
            let band = s.surfaces[si].layer.as_ref().map(|l| l.band).unwrap_or(0);
            if let Some(l) = s.surfaces[si].layer.as_mut() { l.keyboard = mode; }
            let key = (s.surfaces[si].client, s.surfaces[si].id);

            // A surface that wants the keyboard and is on screen takes it now;
            // one that has stopped wanting it hands it back
            if mode == protocol::layer_surface::KEYBOARD_EXCLUSIVE {
                // And if it is the pre-session screen, it keeps it (see
                // `may_lock`): an overlay asking for the keyboard while nobody
                // is logged in is the login screen, and every path moving the
                // keyboard away has to refuse
                if may_lock(s, idx, band) {
                    s.lock = Some(key);
                    crate::mayland::input::set_locked(true);
                    // Nothing reaches a client unless the compositor is
                    // forwarding keys, which it does not by default, so the one
                    // surface that must receive them turns it on
                    crate::mayland::input::set_grab(true);
                    crate::serial_println!(
                        "[mayland] client {} holds the keyboard: no session owns the seat", key.0
                    );
                }
                focus::set_focus(s, Some(key));
            } else {
                if s.lock == Some(key) {
                    release_lock(s, key);
                }
                if s.focus == Some(key) {
                    let next = focus::next_focusable(s, Some(key));
                    focus::set_focus(s, next);
                }
            }
            Ok(())
        }
        protocol::layer_surface::SET_NAMESPACE => {
            let ns = r.string().map_err(|_| "set_namespace: truncated")?.unwrap_or("");
            // Cut at 64 bytes on a character boundary (`Text::from`): a
            // namespace is a short name, and a long one still matches by its
            // beginning
            if let Some(l) = s.surfaces[si].layer.as_mut() { l.namespace = libmay::conf::Text::from(ns); }
            Ok(())
        }
        protocol::layer_surface::ACK_CONFIGURE => {
            let serial = r.uint().map_err(|_| "ack_configure: truncated")?;
            if serial > s.surfaces[si].configure_sent {
                return Err("ack_configure: acknowledging a configure never sent");
            }
            s.surfaces[si].configure_acked = serial;
            Ok(())
        }
        protocol::layer_surface::DESTROY => {
            let key = (s.surfaces[si].client, s.surfaces[si].id);
            if s.lock == Some(key) {
                release_lock(s, key);
            }
            s.surfaces[si].role = Role::None;
            s.surfaces[si].layer = None;
            // Its id is forgotten with it, or a later object reusing the number
            // would resolve to this surface. `xdg_id` is where a layer surface
            // keeps its role object, as a window keeps its xdg_surface there
            s.surfaces[si].xdg_id = 0;
            s.surfaces[si].configure_sent = 0;
            s.surfaces[si].configure_acked = 0;
            // And the keyboard, if it had it: a lock screen leaving `s.focus`
            // on a surface with no role would send every keystroke nowhere.
            // `unmake_window` does the same for windows
            if s.focus == Some(key) {
                let next = focus::next_focusable(s, Some(key));
                focus::set_focus(s, next);
            }
            focus::pointer_off(s, key, true);
            dispatch::destroy_object(s, idx, sender);
            reserve_edges(s);
            Ok(())
        }
        _ => Err("may_layer_surface: unknown request"),
    }
}

/// Works out how big a layer surface is and tells it
///
/// Anchored to two opposite edges means stretch between them: a panel is as
/// wide as the screen without knowing how wide that is. Anywhere not stretched
/// it gets the size it asked for.
///
/// What it is measured against is layer-shell's: an exclusive zone of nought
/// keeps clear of the edges other surfaces keep (an overview stretched over the
/// screen starts under the bar, as on Hyprland); anything else, a zone of its
/// own or -1, is measured against the whole screen
pub(in crate::mayland) fn configure_layer(s: &mut Server, si: usize) {
    use protocol::layer_surface as ls;
    let Some(l) = s.surfaces[si].layer else { return };
    let (ox, oy, sw, sh) = if l.exclusive == 0 {
        let r = layout::reserved();
        (r.left, r.top, (s.width - r.left - r.right).max(0), (s.height - r.top - r.bottom).max(0))
    } else {
        (0, 0, s.width.max(0), s.height.max(0))
    };

    let stretch_x = l.anchor & ls::ANCHOR_LEFT != 0 && l.anchor & ls::ANCHOR_RIGHT != 0;
    let stretch_y = l.anchor & ls::ANCHOR_TOP != 0 && l.anchor & ls::ANCHOR_BOTTOM != 0;
    let w = if stretch_x || l.want_w == 0 { sw } else { l.want_w.min(sw) };
    let h = if stretch_y || l.want_h == 0 { sh } else { l.want_h.min(sh) };

    // Where it sits follows from what it is anchored to: an edge it is stuck
    // to holds it there, and an axis it is stuck to neither end of centres it
    let x = ox + if l.anchor & ls::ANCHOR_LEFT != 0 || stretch_x { 0 }
            else if l.anchor & ls::ANCHOR_RIGHT != 0 { sw - w }
            else { (sw - w) / 2 };
    let y = oy + if l.anchor & ls::ANCHOR_TOP != 0 || stretch_y { 0 }
            else if l.anchor & ls::ANCHOR_BOTTOM != 0 { sh - h }
            else { (sh - h) / 2 };

    s.surfaces[si].pos = (x, y);
    s.surfaces[si].goal = layout::Rect::new(x, y, w, h);
    s.surfaces[si].goal_sized = true;
    s.surfaces[si].placed = true;

    let serial = s.next_serial();
    s.surfaces[si].configure_sent = serial;
    s.surfaces[si].last_w = w;
    s.surfaces[si].last_h = h;
    let (cid, obj) = (s.surfaces[si].client, s.surfaces[si].xdg_id);
    let Some(ci) = s.client_idx(cid) else { return };
    s.clients[ci].queue(obj, ls::CONFIGURE, &[
        Arg::Uint(serial), Arg::Int(w), Arg::Int(h),
    ]);
}

/// Adds up what the layer surfaces are keeping, and hands it to the layout
///
/// Two surfaces on the same edge stack: a panel and a dock both at the top take
/// their heights off in turn. A surface anchored to opposite edges reserves
/// nothing
pub(in crate::mayland) fn reserve_edges(s: &mut Server) {
    use protocol::layer_surface as ls;
    let mut r = layout::Reserved::default();
    for x in s.surfaces.iter() {
        let Some(l) = x.layer else { continue };
        if l.exclusive <= 0 {
            continue;
        }
        let n = l.exclusive;
        let horizontal = l.anchor & ls::ANCHOR_LEFT != 0 && l.anchor & ls::ANCHOR_RIGHT != 0;
        let vertical = l.anchor & ls::ANCHOR_TOP != 0 && l.anchor & ls::ANCHOR_BOTTOM != 0;
        if l.anchor & ls::ANCHOR_TOP != 0 && !vertical { r.top += n; }
        else if l.anchor & ls::ANCHOR_BOTTOM != 0 && !vertical { r.bottom += n; }
        else if l.anchor & ls::ANCHOR_LEFT != 0 && !horizontal { r.left += n; }
        else if l.anchor & ls::ANCHOR_RIGHT != 0 && !horizontal { r.right += n; }
    }
    if layout::set_reserved(r) {
        // The work area moved, so everything measured against it is measured
        // again (the layer surfaces keeping clear of the edges, the windows),
        // and the window manager is told
        let clear: alloc::vec::Vec<usize> = (0..s.surfaces.len())
            .filter(|&i| s.surfaces[i].layer.is_some_and(|l| l.exclusive == 0))
            .collect();
        for si in clear {
            configure_layer(s, si);
        }
        layout::arrange(s);
        desktop::send_work_area(s);
    }
}

/// Whether this client asking for the keyboard means "I am the login screen"
///
/// Three conditions:
///
///   - an overlay band. A surface not in front of everything is not a screen
///     that has to be got past
///   - CAP_MAY_PRIV. `may_layer` is already gated on it, but this decides
///     whether to stop Ctrl+Alt+Esc reaching a root console, so it is asked again
///   - no session owns the seat. Inside a session, `keyboard = exclusive` is
///     what
///     the launcher asks for and it must stay interruptible; before a session,
///     the same request is a login screen and must not be
fn may_lock(s: &Server, idx: usize, band: u32) -> bool {
    if band != protocol::layer::OVERLAY {
        return false;
    }
    if crate::process::session::seat_has_session(crate::process::session::SEAT0) {
        return false;
    }
    let pid = s.clients[idx].pid;
    if pid == 0 {
        // The compositor's own in-kernel clients. They are the compositor, so
        // they are allowed what it is allowed
        return true;
    }
    match crate::sched::cred_checked(pid) {
        Some(c) => c.has(crate::process::cred::CAP_MAY_PRIV),
        None => false,
    }
}

/// How long a surface may hold the keyboard without ever having drawn
///
/// Long enough for a slow greeter to reach its first frame (reading
/// `/etc/mayland`, laying out a font), short enough that a black screen is not
/// taken for a broken machine
const LOCK_FIRST_FRAME_MS: u32 = 10_000;

/// When the current lock was taken, in the frame loop's own milliseconds
///
/// Zero for no lock
static LOCK_SINCE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// The keyboard lock, checked once a frame against the client that holds it
///
/// With the lock up, `input::intercept` swallows every scancode including
/// Ctrl+Alt+Escape, because behind the login screen is a ring-0 root console. A
/// live greeter must be unbypassable, but a dead one must not keep the lock.
/// The lock is dropped in `server::disconnect`, reached through zombie reaping,
/// so a killed greeter would leave the machine with no keyboard and no chord to
/// rescue it. Two conditions, neither of which fires on a working greeter
/// sitting still:
///
///   - the process is gone. Nothing is going to draw or type again
///   - it has never committed a frame. It took the screen and put nothing on
///     it.
///     A greeter that drew its box once has a non-zero commit count from then on
///
/// Releasing the lock alone would give back the chord for a console that is not
/// being painted. So the screen goes back too, by asking the watcher thread to
/// stop compositing: `render::stop` cannot be called from the frame loop, where
/// this runs
pub(in crate::mayland) fn watchdog(s: &mut Server, now_ms: u32) {
    use core::sync::atomic::Ordering;

    let Some(key) = s.lock else {
        LOCK_SINCE.store(0, Ordering::Relaxed);
        return;
    };
    let since = LOCK_SINCE.load(Ordering::Relaxed);
    if since == 0 {
        LOCK_SINCE.store(now_ms.max(1), Ordering::Relaxed);
        return;
    }

    let gone = match s.surface_idx(key) {
        None => Some("its surface is gone"),
        Some(si) => {
            let pid = s.client_idx(key.0).map(|ci| s.clients[ci].pid).unwrap_or(0);
            // pid 0 is one of the compositor's own in-kernel clients, which
            // cannot exit without it
            if pid != 0 && !crate::sched::process_alive(pid) {
                Some("the process that held it has exited")
            } else if s.surfaces[si].commits == 0
                && now_ms.wrapping_sub(since) > LOCK_FIRST_FRAME_MS {
                Some("it never drew a frame")
            } else {
                None
            }
        }
    };

    let Some(why) = gone else { return };
    crate::serial_println!("[mayland] the client holding the keyboard: {}", why);
    // Says which of the two cases this is, in one place, and undoes the lock
    release_lock(s, key);
    LOCK_SINCE.store(0, Ordering::Relaxed);

    // A session owns the seat, so this is the ordinary end of a login screen:
    // the greeter exits once the session it asked for exists. The keyboard
    // belongs to the session
    if crate::process::session::seat_has_session(crate::process::session::SEAT0) {
        return;
    }

    // Nobody is logged in and nothing is drawing the screen. Releasing the lock
    // alone gives back Ctrl+Alt+Escape at a console that is not being painted,
    // so the screen goes back as well
    crate::mayland::input::set_grab(false);
    crate::mayland::render::request_stop();
}

/// Gives the keyboard back to the rest of the machine
///
/// The greeter is expected to reach here: it destroys its surface once the
/// session it asked for exists. If the greeter crashed, with nobody logged in
/// and a root console behind it, this says so loudly, and mikuD's restart
/// policy puts the screen back
fn release_lock(s: &mut Server, key: (u32, u32)) {
    s.lock = None;
    crate::mayland::input::set_locked(false);
    let seat = crate::process::session::SEAT0;
    if crate::process::session::seat_has_session(seat) {
        crate::serial_println!(
            "[mayland] client {} released the keyboard; a session owns the seat", key.0
        );
    } else {
        crate::serial_println!(
            "[mayland] client {} released the keyboard with nobody logged in", key.0
        );
    }
}
