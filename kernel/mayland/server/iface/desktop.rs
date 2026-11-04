// may_desktop: what is on the screen, and what the user asked for
//
// The facts about the desktop, the four verbs that are somebody pressing a key
// rather than a shell deciding a policy, and a window's pixels copied for an
// overview (`capture`). Several clients hold it at once, because a panel
// showing the workspaces needs the same events the window manager acts on.
//
// Why this is a separate interface
//
// So a panel can draw a workspace pill without holding the interface that
// carries `bind_chord` and `set_layout`. Hyprland's answer to the same problem
// is `hyprctl` (anybody may ask, one program decides); this is that without a
// second socket.
//
// It is still privileged. A window list is every program's title and every
// window's geometry, which is why upstream Wayland files the same capability
// (`ext-foreign-toplevel-list`) among the protocols a compositor hides behind
// its global filter. Ours is hidden the same way: see `server::access`.

extern crate alloc;
use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::wire::{Arg, Reader};
use crate::mayland::shell::{layout, workspace};

use crate::mayland::server::{focus, scene, shm};
use crate::mayland::server::state::*;

/// Tells everyone listening to the desktop something
///
/// Every caller holds the server lock, so this takes the server. Silent when
/// nothing is bound
pub(in crate::mayland) fn event(s: &mut Server, opcode: u16, args: &[Arg]) {
    for l in leases(s) {
        let Some(idx) = s.client_idx(l.client) else { continue };
        s.clients[idx].queue(l.object, opcode, args);
    }
}

/// Everyone bound to may_desktop and still alive
///
/// A connection killed by a protocol error is dead at once, not when the
/// transport notices, so it stops being told anything then
pub(in crate::mayland::server) fn leases(s: &Server) -> Vec<Lease> {
    holders(s, &DESKTOP)
}

/// Where a window is and how big, as a shell is told: `x, y, w, h`
fn placed_as_told(s: &Server, si: usize) -> (i32, i32, i32, i32) {
    // Where it belongs rather than where an animation has it: on screen, the
    // layout's goal; on a workspace not looked at, its place there (`home`). A
    // switch slides windows in from off the screen, and a shell drawing the
    // workspaces (end4's overview) must not put them where the slide began
    let surf = &s.surfaces[si];
    let shown = surf.sticky || surf.workspace == *CURRENT_WS.lock();
    let (x, y) = match (shown, surf.goal.is_empty(), surf.home.is_empty()) {
        (true, false, _) | (false, false, true) => surf.goal.pos(),
        (false, _, false) => surf.home.pos(),
        _ => scene::placed(surf),
    };
    // The size the window has been told to be, or the size it is showing when
    // never told anything. A floating window that was never configured would
    // otherwise be reported as 0 by 0
    let (mut w, mut h) = (surf.last_w, surf.last_h);
    if w == 0 && h == 0 {
        if let Some((bw, bh)) = scene::size_of(s, si) {
            w = bw;
            h = bh;
        }
    }
    (x, y, w, h)
}

/// A `window` event for every window the shells were told of that a layout has
/// since moved or resized
///
/// A new window takes its half of the screen at its first commit, not when
/// announced: the window beside it is made smaller then, and nothing else says
/// so. Hyprland's shells re-read its client list on the `openwindow` it sends
/// after the layout; here the list says it itself. After every arrangement
/// (`layout::apply`)
pub(in crate::mayland) fn tell_moved(s: &mut Server) {
    for si in 0..s.surfaces.len() {
        let surf = &s.surfaces[si];
        if surf.role != Role::Toplevel || surf.ghost.is_some() {
            continue;
        }
        let Some(told) = surf.told else { continue };
        if told != placed_as_told(s, si) {
            send_window(s, si);
        }
    }
}

/// One `window` event describing a surface as a shell sees it
pub(in crate::mayland) fn send_window(s: &mut Server, si: usize) {
    let (client, surface) = (s.surfaces[si].client, s.surfaces[si].id);
    let (x, y, w, h) = placed_as_told(s, si);
    s.surfaces[si].told = Some((x, y, w, h));
    let ws = s.surfaces[si].workspace;
    let flags = s.surfaces[si].wm_flags(s.active == Some((client, surface)));
    // Cloned because `event` needs the server mutably and the strings live in
    // it. A window list is asked for on a key press, not per frame
    let title = s.surfaces[si].title.clone();
    let app_id = s.surfaces[si].app_id.clone();
    // Per listener, by the version that listener bound. A version 1 client is
    // told a version 1 window: one event and one field, or a program built for
    // version 1 would read a string where it expects a number
    for l in leases(s) {
        let Some(idx) = s.client_idx(l.client) else { continue };
        let args: &[Arg] = if l.version < 2 {
            &[
                Arg::Uint(client), Arg::Uint(surface), Arg::Str(Some(&title)),
                Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h),
                Arg::Uint(flags), Arg::Uint(ws),
            ]
        } else {
            &[
                Arg::Uint(client), Arg::Uint(surface),
                Arg::Str(Some(&title)), Arg::Str(Some(&app_id)),
                Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h),
                Arg::Uint(flags), Arg::Uint(ws),
            ]
        };
        s.clients[idx].queue(l.object, protocol::desktop::WINDOW, args);
    }
}

/// Says a word to every listener, from inside the compositor
///
/// The same broadcast the `signal` request makes, reachable from the shell,
/// since under QEMU the host's window manager eats `$mod D`
pub fn signal(name: &str) -> usize {
    let mut s = super::super::state::SERVER.lock();
    let args = [Arg::Str(Some(name))];
    let mut told = 0;
    for l in leases(&s).into_iter().filter(|l| l.version >= 3) {
        let Some(idx) = s.client_idx(l.client) else { continue };
        s.clients[idx].queue(l.object, protocol::desktop::SIGNALLED, &args);
        told += 1;
    }
    told
}

/// Tells the listeners the rectangle they are reasoning about
///
/// On bind and whenever it changes: the screen resized, the gaps changed, a
/// panel reserved an edge. A stale work area would put a window under the panel
/// the shell drew
pub(in crate::mayland) fn send_work_area(s: &mut Server) {
    let area = layout::work_area(s, false);
    let (sw, sh) = (s.width, s.height);
    let args = [
        Arg::Int(area.x), Arg::Int(area.y), Arg::Int(area.w), Arg::Int(area.h),
        Arg::Int(sw), Arg::Int(sh),
    ];
    // Version 2 is where this event starts, and a version 1 listener would not
    // know it
    for l in leases(s).into_iter().filter(|l| l.version >= 2) {
        let Some(idx) = s.client_idx(l.client) else { continue };
        s.clients[idx].queue(l.object, protocol::desktop::WORK_AREA, &args);
    }
}

/// Tells the listeners which keyboard layout is in use
///
/// On bind and whenever it changes: the seat switched, or a config changed the
/// list. Version 4 is where this event starts
pub(in crate::mayland) fn send_keyboard_layout(s: &mut Server) {
    use crate::io::input::layout;
    let group = layout::group();
    let l = layout::layout_of(group as usize);
    let args = [
        Arg::Uint(group), Arg::Uint(layout::groups() as u32),
        Arg::Str(Some(l.code)), Arg::Str(Some(l.name)),
    ];
    for le in leases(s).into_iter().filter(|l| l.version >= 4) {
        let Some(idx) = s.client_idx(le.client) else { continue };
        s.clients[idx].queue(le.object, protocol::desktop::KEYBOARD_LAYOUT, &args);
    }
}

/// Asks a window to close, the only way a window is closed
///
/// Not `unmake_window`: taking the role away leaves the client drawing into a
/// buffer nobody presents and never tells it why. A window may ignore this, for
/// instance to ask about unsaved work
pub(in crate::mayland) fn ask_to_close(s: &mut Server, si: usize) {
    let (cid, top) = (s.surfaces[si].client, s.surfaces[si].toplevel_id);
    if top == 0 {
        return;
    }
    let Some(idx) = s.client_idx(cid) else { return };
    s.clients[idx].queue(top, protocol::toplevel::CLOSE, &[]);
}

pub(in crate::mayland) fn on_desktop(s: &mut Server, idx: usize, sender: u32,
                                     opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    // The pair every request but two names its window by
    let mut window = || -> Result<(u32, u32), &'static str> {
        let c = r.uint().map_err(|_| "may_desktop: truncated")?;
        let sf = r.uint().map_err(|_| "may_desktop: truncated")?;
        Ok((c, sf))
    };

    match opcode {
        protocol::desktop::GET_WINDOWS => {
            let serial = r.uint().map_err(|_| "get_windows: truncated")?;
            // In creation order, the order the layout uses, so the list reads
            // as the screen does. Afterimages are not windows: they have no
            // client behind them, and listing one would offer to focus or close
            // something gone
            let all: Vec<usize> = (0..s.surfaces.len())
                .filter(|&i| s.surfaces[i].role == Role::Toplevel
                             && s.surfaces[i].ghost.is_none())
                .collect();
            for si in all {
                send_window(s, si);
            }
            event(s, protocol::desktop::WINDOWS_DONE, &[Arg::Uint(serial)]);
            Ok(())
        }
        protocol::desktop::FOCUS => {
            let (c, sf) = window()?;
            let key = s.surfaces.iter()
                .find(|x| x.client == c && x.id == sf && x.focusable())
                .map(|x| (x.client, x.id));
            if let Some(k) = key {
                focus::set_focus(s, Some(k));
            }
            Ok(())
        }
        protocol::desktop::CLOSE => {
            let (c, sf) = window()?;
            if let Some(si) = s.surface_idx((c, sf)) {
                ask_to_close(s, si);
            }
            Ok(())
        }
        protocol::desktop::WORKSPACE => {
            let n = r.uint().map_err(|_| "workspace: truncated")?;
            workspace::switch_workspace(s, n);
            Ok(())
        }
        protocol::desktop::SIGNAL => {
            // Carried and not read. The compositor has no table of what words
            // mean: `launcher` is a layer in some client's document.
            //
            // Sent to version 3 and above only, and to the sender as well: a
            // shell with its window manager and bar on two connections is still
            // one shell
            let name = r.string().map_err(|_| "signal: truncated")?.unwrap_or("");
            if name.is_empty() {
                return Err("signal: a word with no letters in it");
            }
            if name.len() > 31 {
                // What a client can decode: `conn::copy_string` keeps
                // thirty-two bytes, so a longer word would arrive cut in half
                // and match nothing at the other end
                return Err("signal: that word is too long to carry");
            }
            let args = [Arg::Str(Some(name))];
            for l in leases(s).into_iter().filter(|l| l.version >= 3) {
                let Some(idx) = s.client_idx(l.client) else { continue };
                s.clients[idx].queue(l.object, protocol::desktop::SIGNALLED, &args);
            }
            Ok(())
        }
        protocol::desktop::MOVE_TO_WORKSPACE => {
            let (c, sf) = window()?;
            let n = r.uint().map_err(|_| "move_to_workspace: truncated")?;
            if n == 0 || n > protocol::desktop::WORKSPACES {
                return Err("move_to_workspace: no such workspace");
            }
            let Some(si) = s.surface_idx((c, sf)) else { return Ok(()) };
            // The console and the wallpaper stay where they are. A machine's
            // only shell must not be sent to a workspace nobody is looking at
            if s.surfaces[si].sticky {
                return Err("move_to_workspace: that window is on every workspace");
            }
            // Its place where it was, kept as the place it goes with: nothing
            // arranges a workspace nobody is looking at
            if s.surfaces[si].workspace == *CURRENT_WS.lock() {
                s.surfaces[si].home = s.surfaces[si].goal;
            }
            s.surfaces[si].workspace = n;
            // It has left the screen, so the focus and the layout both change
            if !s.surfaces.iter().any(|x| x.client == c && x.id == sf
                                          && x.workspace == *CURRENT_WS.lock()) {
                let next = focus::next_focusable(s, None);
                focus::set_focus(s, next);
            }
            layout::arrange(s);
            Ok(())
        }
        protocol::desktop::CAPTURE => {
            let (c, sf) = window()?;
            let pool = r.uint().map_err(|_| "capture: truncated")?;
            let offset = r.uint().map_err(|_| "capture: truncated")? as usize;
            let serial = r.uint().map_err(|_| "capture: truncated")?;
            let (copied, w, h, format) = capture(s, idx, (c, sf), pool, offset)?;
            s.clients[idx].queue(sender, protocol::desktop::CAPTURED, &[
                Arg::Uint(serial), Arg::Uint(copied as u32), Arg::Int(w), Arg::Int(h), Arg::Uint(format),
            ]);
            Ok(())
        }
        _ => Err("may_desktop: unknown request"),
    }
}

/// A window's pixels, what it last committed, into the asking client's pool
/// `pool` at `offset`, rows `w * 4` apart: whether they were copied, and the
/// window's size and format. Nothing to copy (no such window, no buffer) is
/// `(false, 0, 0, 0)`; a pool too small from that offset is `false` with the
/// size, for the asker to grow it. A window whose buffer is video memory comes
/// down on the copy engine, into a pool of system memory only
fn capture(s: &mut Server, idx: usize, window: (u32, u32), pool: u32, offset: usize)
    -> Result<(bool, i32, i32, u32), &'static str>
{
    let me = s.clients[idx].id;
    let to = s.pools.iter().find(|p| p.client == me && p.id == pool)
        .map(|p| (p.handle, p.size))
        .ok_or("capture: no such pool")?;
    let nothing = Ok((false, 0, 0, 0));
    // A window, not a layer or a popup: what the window list names
    let Some(si) = s.surface_idx(window) else { return nothing };
    let surf = &s.surfaces[si];
    if surf.role != Role::Toplevel || surf.ghost.is_some() {
        return nothing;
    }
    let Some(bid) = surf.current.buffer else { return nothing };
    let Some(b) = s.buffers.iter().find(|b| b.client == surf.client && b.id == bid) else { return nothing };
    let (w, h) = (b.width, b.height);
    if w <= 0 || h <= 0 {
        return nothing;
    }
    let row = w as usize * 4;
    let need = row * h as usize;
    if offset.checked_add(need).is_none_or(|end| end > to.1) {
        return Ok((false, w, h, b.format));
    }
    if let Ok((src, shm::Memory::Video)) = shm::location(b.pool) {
        // The destination is the asker's own pool, checked above to hold
        // `need` bytes from `offset`; one in video memory is refused here
        let Ok(dst) = shm::phys(to.0) else { return nothing };
        let ok = crate::mayland::render::texture::download(
            src + b.offset as u64, b.stride, dst + offset as u64, row, w, h);
        return if ok { Ok((true, w, h, b.format)) } else { nothing };
    }
    match shm::copy_rows(b.pool, b.offset, b.stride, to.0, offset, row, row, h as usize) {
        Ok(()) => Ok((true, w, h, b.format)),
        Err(_) => nothing,
    }
}
