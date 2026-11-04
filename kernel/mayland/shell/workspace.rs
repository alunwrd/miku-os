// Which workspace is being looked at
//
// A workspace is only which surfaces are presentable and laid out now: there is
// no second list and no per-workspace state. `Surface::on_workspace` is the
// model and `stack()` applies it.
//
// A switch changes the number and re-arranges. The rest of this file animates
// that: the windows leaving go off one edge, the windows arriving start off the
// other, and `layout::apply` walks the rest.
//
// `park_tiled` and `unpark_tiled` are here rather than in the selftest because
// they use floating, the existing way to be a window and not be tiled.

use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::wire::{Arg};
use crate::mayland::shell::layout;

use crate::mayland::server::{self, state::*};

/// Puts a window on another workspace, from outside the protocol
///
/// For the selftest: the request that does this is the window manager's
pub fn move_to_workspace_for_test(client: u32, surface: u32, ws: u32) -> bool {
    let mut s = server::lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    s.surfaces[si].workspace = ws;
    layout::arrange(&mut s);
    true
}

/// Sends a workspace's windows off the edge
///
/// They are in no layout now, so nothing gives them a goal, and a goal is what
/// the tick walks towards. Theirs is one screen to the left or right, and when
/// the curve ends `stack` stops keeping them
pub(in crate::mayland) fn slide_out(s: &mut Server, from: u32, forward: bool) {
    let spec = crate::mayland::shell::anim::spec(protocol::wm::anim_class::WORKSPACES_OUT);
    if !crate::mayland::shell::anim::enabled()
        || !spec.enabled || spec.style != crate::mayland::shell::anim::style::SLIDE
    {
        return;
    }
    let width = s.width;
    let now = layout::now_us();
    for si in 0..s.surfaces.len() {
        if s.surfaces[si].workspace != from
            || s.surfaces[si].sticky || s.surfaces[si].pinned
            || s.surfaces[si].ghost.is_some()
            || !s.surfaces[si].on_screen()
        {
            continue;
        }
        let pos = s.surfaces[si].pos;
        let dx = if forward { -width } else { width };
        let g = s.surfaces[si].goal;
        s.surfaces[si].goal = layout::Rect::new(pos.0 + dx, pos.1, g.w, g.h);
        let a = &mut s.surfaces[si].anim;
        *a = crate::mayland::shell::anim::Anim {
            running: true,
            begun_us: now,
            ms: spec.ms,
            curve: spec.curve,
            style: crate::mayland::shell::anim::style::SLIDE,
            from: (pos.0 as f32, pos.1 as f32),
            ..crate::mayland::shell::anim::Anim::NONE
        };
    }
}

/// Puts a workspace's windows off the far edge, ready to be brought in
///
/// Only the displacement is here. The curve carrying them back is started by
/// `arrange`, which knows where they belong, so this returns which surfaces it
/// moved, and the caller re-times them once they have goals
pub(in crate::mayland) fn slide_in(s: &mut Server, to: u32, forward: bool) -> Vec<usize> {
    let mut moved = Vec::new();
    let spec = crate::mayland::shell::anim::spec(protocol::wm::anim_class::WORKSPACES_IN);
    if !crate::mayland::shell::anim::enabled()
        || !spec.enabled || spec.style != crate::mayland::shell::anim::style::SLIDE
    {
        return moved;
    }
    let width = s.width;
    for si in 0..s.surfaces.len() {
        if s.surfaces[si].workspace != to
            || s.surfaces[si].sticky || s.surfaces[si].pinned
            || s.surfaces[si].ghost.is_some()
            || !s.surfaces[si].on_screen()
            || !s.surfaces[si].placed
        {
            continue;
        }
        // The opposite side from the one the leavers are going to
        let dx = if forward { width } else { -width };
        let p = s.surfaces[si].pos;
        s.surfaces[si].pos = (p.0 + dx, p.1);
        moved.push(si);
    }
    moved
}

/// Gives the arriving windows the workspace's duration and curve
///
/// A switch is one movement. `arrange` starts a curve for any window not where
/// it belongs, with the `windows` spec, which is wrong here: the two sides of
/// one slide would come apart in the middle. Hyprland animates one offset per
/// workspace, so it has one duration.
///
/// Re-stamped here because `arrange` knows where a window belongs. Nothing has
/// ticked in between (this is the same frame), so replacing the duration and
/// the curve is exact
fn retime_arrivals(s: &mut Server, arriving: &[usize]) {
    let spec = crate::mayland::shell::anim::spec(protocol::wm::anim_class::WORKSPACES_IN);
    if !spec.enabled {
        return;
    }
    for &si in arriving {
        let a = &mut s.surfaces[si].anim;
        if !a.running {
            continue;
        }
        a.ms = spec.ms;
        a.curve = spec.curve;
        a.style = crate::mayland::shell::anim::style::SLIDE;
    }
}

/// Look at a different workspace
pub(in crate::mayland) fn switch_workspace(s: &mut Server, n: u32) {
    let from = *CURRENT_WS.lock();
    if n == 0 || n > protocol::wm::WORKSPACES || n == from {
        return;
    }
    *CURRENT_WS.lock() = n;
    // Where the leaving windows are on their workspace, kept before the slide
    // takes their goals off the screen
    for si in 0..s.surfaces.len() {
        if s.surfaces[si].workspace == from && !s.surfaces[si].sticky && s.surfaces[si].ghost.is_none() {
            s.surfaces[si].home = s.surfaces[si].goal;
        }
    }
    // The windows on screen leave the way the new ones arrive: to the side, and
    // the side is which way the number went. Started before the arrangement
    // below, because `arrange` gives the newcomers goals and these have none
    slide_out(s, from, n > from);
    // The ones arriving start off the opposite edge, so the arrangement below
    // has somewhere to bring them from. `apply` finds a window not where it
    // belongs and starts a curve
    let arriving = slide_in(s, n, n > from);
    // Everything on screen changed, and none of it by being drawn over
    crate::mayland::render::force_full_frame();
    layout::arrange(s);
    // The curves `arrange` just started belong to the switch: both halves of
    // one movement, one duration
    retime_arrivals(s, &arriving);
    // The keyboard cannot stay with a window nobody can see
    let keep = s.focus.and_then(|k| s.surface_idx(k))
        .map(|si| s.surfaces[si].sticky || s.surfaces[si].workspace == n)
        .unwrap_or(false);
    if !keep {
        let next = crate::mayland::server::focus::next_focusable(s, None);
        crate::mayland::server::focus::set_focus(s, next);
    }
    // Nor can the active window, when what has the keyboard is on every
    // workspace: the launcher, up across the switch. The keyboard coming back
    // from it makes a window on this workspace active
    let seen = s.active.and_then(|k| s.surface_idx(k))
        .map(|si| s.surfaces[si].sticky || s.surfaces[si].workspace == n)
        .unwrap_or(true);
    if !seen {
        crate::mayland::server::focus::set_active(s, None);
    }
    crate::mayland::server::iface::desktop::event(
        s, protocol::desktop::WORKSPACE_CHANGED, &[Arg::Uint(n)]);
}

/// Looks at a workspace, from outside the protocol
///
/// For the shell command and for a compositor putting the screen back. The
/// window manager's own switch goes through `may_wm`
pub fn set_workspace(n: u32) {
    let mut s = server::lock();
    switch_workspace(&mut s, n);
}

/// Make a surface visible on every workspace, and immovable between them
pub fn set_sticky(client: u32, surface: u32, on: bool) -> bool {
    let mut s = server::lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    s.surfaces[si].sticky = on;
    layout::arrange(&mut s);
    true
}

/// Takes every window tiled now out of the tiling, and returns the list to put
/// back
///
/// For the selftest. Its cases measure the rectangle the layout gave their
/// windows, and with the desktop running the console is in the same layout
pub fn park_tiled() -> Vec<(u32, u32)> {
    let mut s = server::lock();
    let parked: Vec<(u32, u32)> = (0..s.surfaces.len())
        .filter(|&i| s.surfaces[i].role == Role::Toplevel
                     && !s.surfaces[i].floating
                     // A backdrop is out of the tiling already
                     && !s.surfaces[i].backdrop)
        .map(|i| (s.surfaces[i].client, s.surfaces[i].id))
        .collect();
    for &(c, id) in &parked {
        if let Some(si) = s.surface_idx((c, id)) {
            s.surfaces[si].floating = true;
        }
    }
    layout::arrange(&mut s);
    parked
}

/// Puts back what `park_tiled` took out. A window gone in the meantime is
/// skipped
pub fn unpark_tiled(list: &[(u32, u32)]) {
    let mut s = server::lock();
    for &(c, id) in list {
        if let Some(si) = s.surface_idx((c, id)) {
            s.surfaces[si].floating = false;
        }
    }
    layout::arrange(&mut s);
}
