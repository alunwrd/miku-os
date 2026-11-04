// may_xdg, may_xdg_surface, may_toplevel: the window role
//
// The handshake that turns a surface into a window: a window may not show
// pixels until it has agreed a size. The compositor proposes with a configure
// carrying a serial, the client acknowledges that serial, and the buffer it
// commits afterwards answers that proposal and not an older one.
//
// Teardown is three requests (toplevel, xdg_surface, surface), and each must
// find its own object still resolvable, so each destroy handler clears only the
// one id it is destroying.

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Arg, Reader};
use crate::mayland::shell::layout;

use crate::mayland::server::{dispatch, focus, surface};
use crate::mayland::server::state::*;
use crate::mayland::shell::popup::{self, Positioner};
use crate::mayland::input::grab;
use super::{desktop, wm};

pub(in crate::mayland) fn on_xdg(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::xdg::GET_XDG_SURFACE => {
            let id = r.new_id().map_err(|_| "get_xdg_surface: truncated")?;
            let surf = r.object().map_err(|_| "get_xdg_surface: truncated")?;
            let cid = s.clients[idx].id;
            let si = s.surfaces.iter().position(|x| x.client == cid && x.id == surf)
                .ok_or("get_xdg_surface: not a may_surface")?;
            if s.surfaces[si].role != Role::None {
                return Err("get_xdg_surface: the surface already has a role");
            }
            // Wayland is strict about this and so is this: a surface that has
            // shown content cannot become a window afterwards, because the
            // window's first configure would arrive after pixels
            if s.surfaces[si].commits > 0 {
                return Err("get_xdg_surface: the surface has already been committed");
            }
            dispatch::bind_new(s, idx, id, Interface::XdgSurface, 1)?;
            s.surfaces[si].role = Role::Xdg;
            s.surfaces[si].xdg_id = id;
            Ok(())
        }
        protocol::xdg::PONG => {
            let _ = r.uint().map_err(|_| "pong: truncated")?;
            Ok(())
        }
        protocol::xdg::DESTROY => {
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        protocol::xdg::CREATE_POSITIONER => {
            let id = r.new_id().map_err(|_| "create_positioner: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Positioner, 1)?;
            s.clients[idx].positioners.push((id, Positioner::NEW));
            Ok(())
        }
        _ => Err("may_xdg: unknown request"),
    }
}

pub(in crate::mayland) fn on_xdg_surface(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let si = s.surfaces.iter().position(|x| x.client == cid && x.xdg_id == sender)
        .ok_or("may_xdg_surface: no surface behind it")?;

    match opcode {
        protocol::xdg_surface::GET_TOPLEVEL => {
            let id = r.new_id().map_err(|_| "get_toplevel: truncated")?;
            if s.surfaces[si].role != Role::Xdg {
                return Err("get_toplevel: the surface already has a role");
            }
            dispatch::bind_new(s, idx, id, Interface::Toplevel, 1)?;
            s.surfaces[si].role = Role::Toplevel;
            s.surfaces[si].toplevel_id = id;

            // A new window opens on top and takes the keyboard. Focus first,
            // configure second: set_focus tells the window it is active in the
            // same configure that proposes its size, so a client that has just
            // mapped is not sent two
            let sid = s.surfaces[si].id;
            focus::raise(s, si);
            focus::set_focus(s, Some((cid, sid)));
            // A tiling layout has an opinion about every window's size as soon
            // as a new one appears, and says so here. When it has none
            // (floating) the window is told 0x0, which is xdg-shell for "you
            // choose", and it is placed at its first commit
            layout::arrange(s);
            desktop::event(s, protocol::desktop::OPENED, &[Arg::Uint(cid), Arg::Uint(sid)]);
            if s.surfaces[si].configure_sent == 0 {
                surface::configure(s, si, 0, 0);
            }
            Ok(())
        }
        protocol::xdg_surface::ACK_CONFIGURE => {
            let serial = r.uint().map_err(|_| "ack_configure: truncated")?;
            if serial > s.surfaces[si].configure_sent {
                return Err("ack_configure: acknowledging a configure never sent");
            }
            s.surfaces[si].configure_acked = serial;
            Ok(())
        }
        protocol::xdg_surface::SET_WINDOW_GEOMETRY => {
            for _ in 0..4 {
                let _ = r.int().map_err(|_| "set_window_geometry: truncated")?;
            }
            // Accepted and ignored: it describes where the window's visible
            // part sits inside the surface
            Ok(())
        }
        protocol::xdg_surface::DESTROY => {
            surface::unmake_window(s, si);
            s.surfaces[si].xdg_id = 0;
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        protocol::xdg_surface::GET_POPUP => {
            let id = r.new_id().map_err(|_| "get_popup: truncated")?;
            let parent_xdg = r.object().map_err(|_| "get_popup: truncated")?;
            let pos_id = r.object().map_err(|_| "get_popup: truncated")?;
            get_popup(s, idx, si, id, parent_xdg, pos_id)
        }
        _ => Err("may_xdg_surface: unknown request"),
    }
}

pub(in crate::mayland) fn on_toplevel(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let si = s.surfaces.iter().position(|x| x.client == cid && x.toplevel_id == sender)
        .ok_or("may_toplevel: no surface behind it")?;

    match opcode {
        protocol::toplevel::SET_TITLE => {
            let title = r.string().map_err(|_| "set_title: bad string")?.unwrap_or("");
            s.surfaces[si].title.clear();
            s.surfaces[si].title.push_str(title);
            Ok(())
        }
        protocol::toplevel::SET_APP_ID => {
            let id = r.string().map_err(|_| "set_app_id: bad string")?;
            s.surfaces[si].app_id.clear();
            if let Some(id) = id {
                s.surfaces[si].app_id.push_str(id);
            }
            // The window manager matches rules on this, and a rule taking
            // effect only at the next window list would let a window flash
            // undecorated first
            desktop::send_window(s, si);
            Ok(())
        }
        protocol::toplevel::SET_MAXIMIZED | protocol::toplevel::SET_FULLSCREEN => {
            // Both mean the same here and go through the state the window
            // manager sets, or the layout would not know a window is fullscreen
            if opcode == protocol::toplevel::SET_FULLSCREEN {
                let _ = r.object();
            }
            wm::set_fullscreen(s, si, true);
            Ok(())
        }
        protocol::toplevel::UNSET_MAXIMIZED | protocol::toplevel::UNSET_FULLSCREEN => {
            wm::set_fullscreen(s, si, false);
            Ok(())
        }
        protocol::toplevel::SET_MAX_SIZE | protocol::toplevel::SET_MIN_SIZE => {
            let _ = r.int().map_err(|_| "size hint: truncated")?;
            let _ = r.int().map_err(|_| "size hint: truncated")?;
            Ok(())
        }
        protocol::toplevel::SET_PARENT => {
            let _ = r.object().map_err(|_| "set_parent: truncated")?;
            Ok(())
        }
        protocol::toplevel::SET_MINIMIZED => Ok(()),
        // A client asking to be moved or stretched by its own decoration: the
        // same grab a mouse binding starts. The ways in are here and in
        // `iface::wm`
        protocol::toplevel::MOVE => {
            let serial = r.uint().map_err(|_| "move: truncated")?;
            // The serial makes this a user's action rather than the program's
            // own idea. Without it any client could call `move` unprompted and
            // take the pointer, and the grab is only given back when a button
            // comes up (`input/pointer.rs`). The popup grab below and the
            // selection ask the same
            if !s.is_input_serial(serial) {
                return Ok(());
            }
            let key = (s.surfaces[si].client, s.surfaces[si].id);
            if s.surfaces[si].role == Role::Toplevel {
                focus::raise(s, si);
                crate::mayland::input::grab::begin_move(key.0, key.1);
            }
            Ok(())
        }
        protocol::toplevel::RESIZE => {
            let serial = r.uint().map_err(|_| "resize: truncated")?;
            let edges = r.uint().map_err(|_| "resize: truncated")?;
            // A malformed request is still malformed, so the edges are checked
            // before the serial: a client bug should not be hidden by a stale
            // serial being dropped
            if edges & !protocol::wm::edge::ALL != 0 {
                return Err("resize: an edge this compositor does not have");
            }
            // Same door, same question as `MOVE` above
            if !s.is_input_serial(serial) {
                return Ok(());
            }
            if s.surfaces[si].role != Role::Toplevel {
                return Ok(());
            }
            let key = (s.surfaces[si].client, s.surfaces[si].id);
            let g = s.surfaces[si].goal;
            let edges = if edges != 0 { edges } else { wm::edges_from_pointer(g) };
            focus::raise(s, si);
            crate::mayland::input::grab::begin_resize(key.0, key.1, edges, g.w, g.h);
            Ok(())
        }
        // A menu the compositor would have to draw, and it draws no
        // decorations. Nothing rather than fatal: a toolkit offering a
        // right-click menu on its title bar should lose the menu, not the
        // connection
        protocol::toplevel::SHOW_WINDOW_MENU => Ok(()),
        protocol::toplevel::DESTROY => {
            surface::unmake_window(s, si);
            // Only this object's id. The client still owns the xdg_surface and
            // the surface under it and has a destroy left to send for each, and
            // an id forgotten while its object lives resolves to the wrong
            // surface
            s.surfaces[si].toplevel_id = 0;
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_toplevel: unknown request"),
    }
}

// may_positioner: the recipe
//
// Seven setters. Each is accepted whatever the positioner already says:
// xdg-shell lets a client build a recipe in any order and only checks it when a
// popup is made from it (`Positioner::usable`).

pub(in crate::mayland) fn on_positioner(s: &mut Server, idx: usize, sender: u32,
                                        opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    // Read from the reader first, then find the positioner: borrowing the
    // client mutably while the reader is still being asked for arguments would
    // be two mutable paths into `s`
    let mut set = |f: &mut dyn FnMut(&mut Positioner)| -> Result<(), &'static str> {
        let p = s.clients[idx].positioners.iter_mut()
            .find(|(id, _)| *id == sender)
            .ok_or("may_positioner: no positioner behind it")?;
        f(&mut p.1);
        Ok(())
    };

    match opcode {
        protocol::positioner::SET_SIZE => {
            let w = r.int().map_err(|_| "set_size: truncated")?;
            let h = r.int().map_err(|_| "set_size: truncated")?;
            if w <= 0 || h <= 0 {
                return Err("may_positioner: a popup with no width or no height");
            }
            set(&mut |p| { p.w = w; p.h = h; })
        }
        protocol::positioner::SET_ANCHOR_RECT => {
            let x = r.int().map_err(|_| "set_anchor_rect: truncated")?;
            let y = r.int().map_err(|_| "set_anchor_rect: truncated")?;
            let w = r.int().map_err(|_| "set_anchor_rect: truncated")?;
            let h = r.int().map_err(|_| "set_anchor_rect: truncated")?;
            if w < 0 || h < 0 {
                return Err("may_positioner: an anchor rectangle with a negative side");
            }
            set(&mut |p| p.anchor_rect = layout::Rect::new(x, y, w, h))
        }
        protocol::positioner::SET_ANCHOR => {
            let a = r.uint().map_err(|_| "set_anchor: truncated")?;
            if a > protocol::positioner::anchor::LAST {
                return Err("may_positioner: an anchor this compositor does not have");
            }
            set(&mut |p| p.anchor = a)
        }
        protocol::positioner::SET_GRAVITY => {
            let g = r.uint().map_err(|_| "set_gravity: truncated")?;
            if g > protocol::positioner::gravity::LAST {
                return Err("may_positioner: a gravity this compositor does not have");
            }
            set(&mut |p| p.gravity = g)
        }
        protocol::positioner::SET_CONSTRAINT_ADJUSTMENT => {
            let a = r.uint().map_err(|_| "set_constraint_adjustment: truncated")?;
            if a & !protocol::positioner::adjust::ALL != 0 {
                return Err("may_positioner: a constraint this compositor does not have");
            }
            set(&mut |p| p.adjust = a)
        }
        protocol::positioner::SET_OFFSET => {
            let x = r.int().map_err(|_| "set_offset: truncated")?;
            let y = r.int().map_err(|_| "set_offset: truncated")?;
            set(&mut |p| p.off = (x, y))
        }
        // Accepted and not acted on: see the note in `protocol::positioner`.
        // The arguments are still drained, since a message half-read is one the
        // next starts inside
        protocol::positioner::SET_REACTIVE => Ok(()),
        protocol::positioner::SET_PARENT_SIZE => {
            let _ = r.int().map_err(|_| "set_parent_size: truncated")?;
            let _ = r.int().map_err(|_| "set_parent_size: truncated")?;
            Ok(())
        }
        protocol::positioner::SET_PARENT_CONFIGURE => {
            let _ = r.uint().map_err(|_| "set_parent_configure: truncated")?;
            Ok(())
        }
        protocol::positioner::DESTROY => {
            s.clients[idx].positioners.retain(|(id, _)| *id != sender);
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_positioner: unknown request"),
    }
}

// may_popup: the menu itself

/// Turns an xdg_surface into a popup hung off another one
///
/// Four refusals before anything is built. Nothing is recorded until all have
/// passed, so a client failing here has changed nothing
fn get_popup(s: &mut Server, idx: usize, si: usize, id: u32,
             parent_xdg: u32, pos_id: u32) -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    if s.surfaces[si].role != Role::Xdg {
        return Err("get_popup: the surface already has a role");
    }
    if s.surfaces[si].commits > 0 {
        return Err("get_popup: the surface has already been committed");
    }
    // A popup with no parent would be drawn wherever the origin is and outlive
    // whatever opened it. xdg-shell allows a null parent only for a popup about
    // to get one from another protocol, which does not exist here
    let pi = s.surfaces.iter().position(|x| x.client == cid && x.xdg_id == parent_xdg)
        .ok_or("get_popup: no parent to hang off")?;
    if pi == si {
        return Err("get_popup: a popup cannot hang off itself");
    }
    let positioner = s.clients[idx].positioners.iter()
        .find(|(pid, _)| *pid == pos_id)
        .map(|(_, p)| *p)
        .ok_or("get_popup: no positioner behind that id")?;
    if !positioner.usable() {
        return Err("get_popup: the positioner has no size yet");
    }
    // How deep the parent already is. Counted by walking up, which also proves
    // the walk terminates: a chain longer than the limit is refused, deep or
    // circular
    let depth = ancestry(s, pi)?;
    if depth + 1 >= grab::MAX_POPUP_DEPTH {
        return Err("get_popup: menus are nested deeper than this compositor goes");
    }

    dispatch::bind_new(s, idx, id, Interface::Popup, 1)?;

    let key = (cid, s.surfaces[pi].id);
    s.surfaces[si].role = Role::Popup;
    s.surfaces[si].popup_id = id;
    s.surfaces[si].parent = Some(key);
    // A popup is never tiled and never placed by the layout, so it is placed
    // once, here, and counts as placed from the start
    s.surfaces[si].placed = true;
    s.surfaces[si].floating = true;
    s.surfaces[si].fresh = false;
    // Above whatever is already open. Only the order within its parent's
    // subtree matters (`stack` puts the whole subtree above the parent), but
    // two sibling menus have to be told apart
    s.surfaces[si].z = s.next_z;
    s.next_z += 1;

    reposition(s, si, &positioner);
    Ok(())
}

/// How many surfaces this one hangs off, following the chain upwards
///
/// Bounded, and the bound is the check: a cycle cannot be counted, so it is
/// reported as too deep and refused. `stack` bounds its walk too, but must
/// never be what discovers a problem
fn ancestry(s: &Server, si: usize) -> Result<usize, &'static str> {
    let mut n = 0;
    let mut at = si;
    while let Some(parent) = s.surfaces[at].parent {
        n += 1;
        if n >= grab::MAX_POPUP_DEPTH {
            return Err("get_popup: this popup hangs off a chain that does not end");
        }
        at = s.surface_idx(parent)
            .ok_or("get_popup: this popup hangs off a surface that is gone")?;
    }
    Ok(n)
}

/// Runs a positioner and tells the client where its popup landed
///
/// The two configures go out in xdg-shell's order, the popup's geometry first
/// and the xdg_surface's serial after it, because the serial acknowledges
/// everything said before it
fn reposition(s: &mut Server, si: usize, p: &Positioner) {
    let Some(parent) = s.surfaces[si].parent else { return };
    let Some(pi) = s.surface_idx(parent) else { return };
    let (px, py) = s.surfaces[pi].pos;
    let screen = layout::Rect::new(0, 0, s.width, s.height);
    let r = crate::mayland::shell::popup::place(
        p, layout::Rect::new(px, py, s.surfaces[pi].last_w, s.surfaces[pi].last_h), screen);

    s.surfaces[si].rel = (r.x - px, r.y - py);
    s.surfaces[si].pos = (r.x, r.y);
    s.surfaces[si].goal = r;
    s.surfaces[si].goal_sized = true;

    let serial = s.next_serial();
    let (cid, xdg_id, popup_id) =
        (s.surfaces[si].client, s.surfaces[si].xdg_id, s.surfaces[si].popup_id);
    s.surfaces[si].configure_sent = serial;
    s.surfaces[si].last_w = r.w;
    s.surfaces[si].last_h = r.h;

    let Some(idx) = s.client_idx(cid) else { return };
    let c = &mut s.clients[idx];
    c.queue(popup_id, protocol::popup::CONFIGURE, &[
        Arg::Int(r.x - px), Arg::Int(r.y - py), Arg::Int(r.w), Arg::Int(r.h),
    ]);
    c.queue(xdg_id, protocol::xdg_surface::CONFIGURE, &[Arg::Uint(serial)]);
}

pub(in crate::mayland) fn on_popup(s: &mut Server, idx: usize, sender: u32,
                                   opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let si = s.surfaces.iter().position(|x| x.client == cid && x.popup_id == sender)
        .ok_or("may_popup: no surface behind it")?;

    match opcode {
        protocol::popup::GRAB => {
            let _seat = r.object().map_err(|_| "grab: truncated")?;
            let serial = r.uint().map_err(|_| "grab: truncated")?;
            grab_popup(s, si, serial)
        }
        protocol::popup::REPOSITION => {
            let pos_id = r.object().map_err(|_| "reposition: truncated")?;
            let token = r.uint().map_err(|_| "reposition: truncated")?;
            let positioner = s.clients[idx].positioners.iter()
                .find(|(pid, _)| *pid == pos_id)
                .map(|(_, p)| *p)
                .ok_or("reposition: no positioner behind that id")?;
            if !positioner.usable() {
                return Err("reposition: the positioner has no size yet");
            }
            reposition(s, si, &positioner);
            let popup_id = s.surfaces[si].popup_id;
            if let Some(i) = s.client_idx(cid) {
                s.clients[i].queue(popup_id, protocol::popup::REPOSITIONED,
                                   &[Arg::Uint(token)]);
            }
            Ok(())
        }
        protocol::popup::DESTROY => {
            // Everything this popup was holding goes with it, including any
            // submenu it opened: a menu whose parent is gone cannot be closed
            dismiss(s, grab::forget_surface((cid, s.surfaces[si].id)), false);
            surface::unmake_popup(s, si);
            s.surfaces[si].popup_id = 0;
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_popup: unknown request"),
    }
}

/// Take the pointer and the keyboard for an open menu
fn grab_popup(s: &mut Server, si: usize, serial: u32) -> Result<(), &'static str> {
    // A grab is a consequence of somebody pressing something. Without this a
    // program with no window on screen could take the desktop's pointer (see
    // `is_input_serial`)
    if !s.is_input_serial(serial) {
        return Err("may_popup: a grab that answers no key and no button");
    }
    if s.surfaces[si].role != Role::Popup {
        return Err("may_popup: not a popup");
    }
    let key = (s.surfaces[si].client, s.surfaces[si].id);
    // Only on the innermost menu. A grab further out would leave the submenu
    // between the pointer and its parent with nothing dismissing it, which is
    // xdg-shell's `invalid_grab`
    let parent = s.surfaces[si].parent;
    let chain = grab::popup_chain();
    if let Some(&top) = chain.last() {
        if parent != Some(top) {
            return Err("may_popup: a grab on a popup that is not the innermost one open");
        }
    }
    grab::popup_open(key)?;
    // The menu types, and its parent stops being the window with the keyboard
    // until the menu is gone
    focus::set_focus(s, Some(key));
    Ok(())
}

/// Tells a client its menus have been dismissed, innermost first
///
/// `notify` is false when the client itself went away, as for
/// `focus::pointer_off`
pub(in crate::mayland) fn dismiss(s: &mut Server, closed: alloc::vec::Vec<(u32, u32)>,
                                  notify: bool) {
    for (cid, sid) in closed {
        let Some(si) = s.surface_idx((cid, sid)) else { continue };
        let popup_id = s.surfaces[si].popup_id;
        if notify && popup_id != 0 {
            if let Some(idx) = s.client_idx(cid) {
                s.clients[idx].queue(popup_id, protocol::popup::POPUP_DONE, &[]);
            }
        }
    }
    // Whatever the menu was covering gets the keyboard back. Asked for once at
    // the end rather than per popup: a chain closing is one focus change
    if grab::popup_client().is_none() {
        let next = focus::next_focusable(s, None);
        focus::set_focus(s, next);
    }
}
