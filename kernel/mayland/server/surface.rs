// A surface's contents over time
//
// Commit, configure, and the two ends of a frame, under one rule:
//
//      attach      records what the next contents will be
//      commit      makes that current, and hands the buffer it replaces to
//                  `retired`; the frame callbacks are armed
//      frame_done  the callbacks fire, and the retired buffers go back, a
//                  frame after the commit that retired them, never during it
//
// A buffer stays the client's to read and not to write for as long as it is
// `current`, which is longer than the frame that first drew it: a later frame
// reads it again for new damage or when the pointer uncovers a pixel. It is
// released when a commit takes its place, not when a frame has been drawn from
// it.
//
// `frame_composed` is the hardware path's half of `frame_done`: there `present`
// submits copies rather than performing them, so the damage is answered at the
// end of the frame while the retired buffers wait for the engine's fence at the
// top of the next one.

use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::wire::{Arg};
use crate::mayland::shell::layout;

use super::{focus, iface};
use super::state::*;

/// Makes pending state current. Nothing is drawn here: the compositor picks up
/// committed surfaces at the next frame
pub(in crate::mayland) fn commit(s: &mut Server, si: usize) {
    let surf = &mut s.surfaces[si];
    // What this commit takes the place of. A second commit before the frame
    // that consumes the first leaves that buffer busy with nothing pointing at
    // it
    let superseded = surf.current.buffer;
    surf.current = surf.pending;
    // Unlike the buffer, a fence does not persist: it is about the pixels this
    // commit carries
    surf.pending.fence = 0;
    if let Some(old) = superseded {
        if Some(old) != surf.current.buffer {
            surf.retired.push(old);
        }
    }
    // Extended rather than replaced: two commits between two frames are two
    // lots of damage. Cleared when the frame is done
    surf.damage.extend(core::mem::take(&mut surf.pending_damage));
    let armed = core::mem::take(&mut surf.pending_frames);
    surf.frames.extend(armed);
    surf.commits += 1;
    surf.unconsumed = surf.current.buffer.is_some();
    // A surface's first frame is a surface arriving under a still pointer,
    // which is where a menu or a launcher opens. Crossings are otherwise
    // computed only when the mouse moves, so without this the click choosing a
    // menu item or closing aosh's launcher goes to the window underneath. A
    // layer's first commit carries no buffer (it asks for a size), so "first
    // frame" is the first commit with one
    let first = superseded.is_none() && surf.current.buffer.is_some()
                && matches!(surf.role, Role::Popup | Role::Layer | Role::Toplevel);
    // A layer that asked for the keyboard on demand takes it as it appears, as
    // in Hyprland: end4's overview and search open ready to be typed at, and
    // Escape shuts them. One that asks later takes it on a click, and an
    // exclusive one at `set_keyboard`
    let takes_keyboard = first && surf.role == Role::Layer
        && surf.layer.is_some_and(|l| l.keyboard == protocol::layer_surface::KEYBOARD_ON_DEMAND)
        && surf.focusable();
    // A new input region changes what is under a still pointer, as a surface
    // arriving does: a panel that stops taking the pointer where it is
    // see-through hands it to the window behind
    let reach = match surf.pending_input.take() {
        Some(input) if input != surf.input => {
            surf.input = input;
            true
        }
        _ => false,
    };
    let arrived = first || reach;

    // Pending buffer state does not clear on commit in Wayland: a client that
    // commits twice without attaching keeps showing the same buffer
    let bid = surf.current.buffer;
    if let Some(bid) = bid {
        let cid = surf.client;
        if let Some(b) = s.buffers.iter_mut().find(|b| b.client == cid && b.id == bid) {
            b.busy = true;
        }
    }

    // A floating window is placed at the commit that first gives it a size, not
    // at get_toplevel: until there is a buffer there is nothing to centre. A
    // tiled one was placed when it appeared, since the layout decides its size
    // too
    if s.surfaces[si].role == Role::Toplevel && !s.surfaces[si].placed
        && s.surfaces[si].current.buffer.is_some()
    {
        layout::place_window(s, si);
        s.surfaces[si].placed = true;
    }
    // A layer opens at its first frame, not at the arrangement that placed it
    if first && s.surfaces[si].role == Role::Layer {
        layout::layer_shown(s, si);
    }
    // Done last, when nothing is borrowed out of the server any more
    if takes_keyboard {
        let key = (s.surfaces[si].client, s.surfaces[si].id);
        focus::set_focus(s, Some(key));
    }
    if arrived {
        focus::repoint(s);
    }
}

/// Proposes a size to a window
///
/// Zero for either dimension means "you choose", which a client gets the first
/// time. The client sends the serial back with ack_configure, so the buffer it
/// commits afterwards answers this proposal and not an older one
pub(in crate::mayland) fn configure(s: &mut Server, si: usize, width: i32, height: i32) {
    let serial = s.next_serial();
    let (cid, xdg_id, top_id) = (
        s.surfaces[si].client, s.surfaces[si].xdg_id, s.surfaces[si].toplevel_id,
    );
    let focused = s.focus == Some((cid, s.surfaces[si].id));
    s.surfaces[si].configure_sent = serial;
    s.surfaces[si].last_w = width;
    s.surfaces[si].last_h = height;

    let Some(idx) = s.client_idx(cid) else { return };
    let mut states: Vec<u8> = Vec::new();
    if focused {
        states.extend_from_slice(&protocol::toplevel::STATE_ACTIVATED.to_le_bytes());
    }
    let c = &mut s.clients[idx];
    if top_id != 0 {
        c.queue(top_id, protocol::toplevel::CONFIGURE, &[
            Arg::Int(width), Arg::Int(height), Arg::Array(&states),
        ]);
    }
    c.queue(xdg_id, protocol::xdg_surface::CONFIGURE, &[Arg::Uint(serial)]);
}

/// Strip the window role from a surface, moving the focus off it if needed
pub(in crate::mayland) fn unmake_window(s: &mut Server, si: usize) {
    let (cid, sid) = (s.surfaces[si].client, s.surfaces[si].id);
    // A menu belongs to the window it was opened from and goes with it. Left
    // behind it would hang off nothing, outside the layout and the focus cycle,
    // and only a click elsewhere could dismiss it
    close_children(s, (cid, sid));
    // A window being carried by the pointer stops being carried when it stops
    // being a window. Left behind, the grab would swallow every motion event
    // until the next button press
    if crate::mayland::input::grab::dragging() == Some((cid, sid)) {
        crate::mayland::input::grab::end_drag();
    }
    if s.surfaces[si].role == Role::Toplevel {
        iface::desktop::event(s, protocol::desktop::CLOSED, &[Arg::Uint(cid), Arg::Uint(sid)]);
        // The client is still here, so its buffer is readable, but it is about
        // to draw something else into it, so the copy is taken now
        crate::mayland::shell::ghost::ghost_window(s, si);
    }
    s.surfaces[si].role = Role::None;
    // The role objects' ids are not cleared here. An xdg teardown is three
    // requests (toplevel.destroy, xdg_surface.destroy, surface.destroy), and
    // each has to find its own object still resolvable. Each destroy handler
    // clears the one id it is destroying
    s.surfaces[si].configure_sent = 0;
    s.surfaces[si].configure_acked = 0;
    if s.focus == Some((cid, sid)) {
        let next = focus::next_focusable(s, Some((cid, sid)));
        focus::set_focus(s, next);
    }
    // The active window closing while something else has the keyboard: a
    // terminal exiting behind the launcher. The desktop has just been told it
    // closed; the keyboard coming back from the launcher makes the next window
    // active
    if s.active == Some((cid, sid)) {
        s.active = None;
    }
    // A surface with no role is out of `stack()`, and so out of the hit test:
    // the pointer cannot be over it any more
    focus::pointer_off(s, (cid, sid), true);
    // One window fewer is a different layout for every window that is left
    layout::arrange(s);
    // The cursor is now over whatever was behind it. `pointer_off` above only
    // says it is over nothing, and until the mouse moved the next click would
    // go to no one
    focus::repoint(s);
}

/// Closes every menu hanging off this surface, innermost first
///
/// Recursive through `unmake_popup`, bounded by the depth limit `get_popup`
/// enforces on the way in
pub(in crate::mayland) fn close_children(s: &mut Server, key: (u32, u32)) {
    let kids: Vec<(u32, u32)> = s.surfaces.iter()
        .filter(|x| x.parent == Some(key) && x.role == Role::Popup)
        .map(|x| (x.client, x.id))
        .collect();
    for k in kids {
        close_children(s, k);
        let closed = crate::mayland::input::grab::forget_surface(k);
        iface::xdg::dismiss(s, closed, true);
        if let Some(ki) = s.surface_idx(k) {
            let popup_id = s.surfaces[ki].popup_id;
            unmake_popup(s, ki);
            // The client still owns the popup object and has a destroy left to
            // send for it, so the id stays resolvable, as in the xdg teardown
            // below
            let _ = popup_id;
        }
    }
}

/// Strips the popup role from a surface
///
/// Shorter than `unmake_window`: a popup has no ghost, is not in the layout,
/// and is not in the focus cycle. What it shares is the pointer: the cursor
/// must not be left inside it
pub(in crate::mayland) fn unmake_popup(s: &mut Server, si: usize) {
    let (cid, sid) = (s.surfaces[si].client, s.surfaces[si].id);
    s.surfaces[si].role = Role::None;
    s.surfaces[si].parent = None;
    s.surfaces[si].rel = (0, 0);
    s.surfaces[si].configure_sent = 0;
    s.surfaces[si].configure_acked = 0;
    focus::pointer_off(s, (cid, sid), true);
    // A menu closing puts the cursor back over whatever it covered. Otherwise
    // the compositor believes the pointer is over nothing, and a button with no
    // pointer focus is sent to nobody
    focus::repoint(s);
}

/// Hands one buffer back to its client
///
/// Idempotent, and silent about a buffer that is not there: a client may
/// destroy a buffer it has attached
pub(in crate::mayland) fn release_buffer(s: &mut Server, cid: u32, bid: u32) {
    let bi = match s.buffers.iter().position(|b| b.client == cid && b.id == bid) {
        Some(i) => i,
        None => return,
    };
    if !s.buffers[bi].busy {
        return;
    }
    s.buffers[bi].busy = false;
    if let Some(idx) = s.client_idx(cid) {
        s.clients[idx].queue(bid, protocol::buffer::RELEASE, &[]);
    }
}

/// Hand a surface's current buffer back to its client
pub(in crate::mayland) fn release_current(s: &mut Server, si: usize) {
    let (cid, bid) = (s.surfaces[si].client, s.surfaces[si].current.buffer);
    let bid = match bid { Some(b) => b, None => return };
    release_buffer(s, cid, bid);
}

/// Tells every client that a frame has been presented
///
/// 'time_ms' is the presentation time in milliseconds, in whatever timebase the
/// compositor uses consistently: clients only subtract two of them
///
/// The callbacks fire, and the buffers a later commit took the place of go
/// back. The one still `current` does not: see below
pub fn frame_done(time_ms: u32) {
    let mut s = SERVER.lock();
    let count = s.surfaces.len();
    for si in 0..count {
        // Damage is not cleared here. On the hardware path this runs at the top
        // of the frame, after the wait for the raster, which is when a client
        // commits. Clearing here would lose each commit landing in that wait
        // before `present` had read it, and the new picture would reach the
        // screen only where something else damaged it. Damage is answered by
        // `frame_composed`, on both paths, right after the frame that read it.
        //
        // Buffers a later commit took the place of. Before the early return
        // below, because a surface that committed a buffer and then committed
        // none is not `unconsumed` and still has one to give back
        if !s.surfaces[si].retired.is_empty() {
            let cid = s.surfaces[si].client;
            for bid in core::mem::take(&mut s.surfaces[si].retired) {
                release_buffer(&mut s, cid, bid);
            }
        }
        if !s.surfaces[si].unconsumed && s.surfaces[si].frames.is_empty() {
            continue;
        }
        // The buffer still `current` stays the client's to look at and not to
        // draw in, however many frames go by: later frames read it again for
        // whatever gets uncovered, so a client told it was free would paint
        // into the rectangle being repaired.
        //
        // A buffer goes back when a later commit takes its place: the `retired`
        // release above, after the engine's fence on the hardware path and
        // after the copies on the software one. A client with two buffers
        // alternates on it and never waits
        s.surfaces[si].unconsumed = false;

        let frames = core::mem::take(&mut s.surfaces[si].frames);
        let cid = s.surfaces[si].client;
        let idx = match s.client_idx(cid) { Some(i) => i, None => continue };
        for id in frames {
            let c = &mut s.clients[idx];
            c.queue(id, protocol::callback::DONE, &[Arg::Uint(time_ms)]);
            let _ = c.objects.remove(id);
            c.delete_id(id);
        }
    }
}

/// Retires the damage a frame has just read.
///
/// Called from both paths, right after `present`, and never at the top of a
/// frame: between frames the compositor is asleep on the raster, and a client
/// committing then must have its rectangle drawn.
///
/// What it read and no more: `damage_read` says how much went into the frame,
/// and whatever was committed while the frame was composing is the next
/// frame's. A surface the frame did not draw has all of its damage dropped: it
/// is drawn whole when it comes back
pub fn frame_composed() {
    let mut s = SERVER.lock();
    for surf in s.surfaces.iter_mut() {
        match surf.damage_read.take() {
            Some(n) => {
                let n = n.min(surf.damage.len());
                surf.damage.drain(..n);
            }
            None => surf.damage.clear(),
        }
    }
}

