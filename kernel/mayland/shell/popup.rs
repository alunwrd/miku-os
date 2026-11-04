// Where a popup goes
//
// The arithmetic for menus, dropdowns and tooltips, in its own file so a
// selftest can call `place` with nine anchors and nine gravities without a
// client, a screen or the server lock.
//
// The recipe is xdg_positioner's. A client says which point of which rectangle
// the popup hangs off (`anchor`), which way it extends from there (`gravity`),
// how far to nudge it afterwards (`offset`), and what it accepts when the
// answer falls off the screen (`adjust`).
//
// The order matters: anchor point, then gravity, then offset, then the
// constraint. A constraint applied before the offset misplaces a menu near an
// edge.

use crate::mayland::protocol::positioner::{adjust, anchor, gravity};
use super::layout::Rect;

/// The recipe, as the client has built it up request by request
///
/// Defaults are xdg-shell's: no anchor, no gravity, no constraint, no offset. A
/// positioner used before `set_size` has a size of zero, which `get_popup`
/// refuses
#[derive(Clone, Copy)]
pub(in crate::mayland) struct Positioner {
    pub w: i32,
    pub h: i32,
    /// What is being hung off, in the parent surface's own coordinates
    pub anchor_rect: Rect,
    pub anchor: u32,
    pub gravity: u32,
    pub adjust: u32,
    pub off: (i32, i32),
}

impl Positioner {
    pub const NEW: Positioner = Positioner {
        w: 0,
        h: 0,
        anchor_rect: Rect::ZERO,
        anchor: anchor::NONE,
        gravity: gravity::NONE,
        adjust: adjust::NONE,
        off: (0, 0),
    };

    /// Whether this recipe can be used at all
    ///
    /// Checked when the popup is made, as in xdg-shell: a positioner is
    /// legitimately half-built between two requests
    pub fn usable(&self) -> bool {
        self.w > 0 && self.h > 0
            && self.anchor <= anchor::LAST
            && self.gravity <= gravity::LAST
            && self.adjust & !adjust::ALL == 0
    }
}

/// The point on the anchor rectangle that the popup hangs off
///
/// Every number came from a client, so every addition saturates: a huge
/// `anchor_rect` must not wrap the arithmetic
fn anchor_point(r: Rect, which: u32) -> (i32, i32) {
    let (mid_x, mid_y) = (r.x.saturating_add(r.w / 2), r.y.saturating_add(r.h / 2));
    let (right, bottom) = (r.x.saturating_add(r.w), r.y.saturating_add(r.h));
    match which {
        anchor::TOP          => (mid_x, r.y),
        anchor::BOTTOM       => (mid_x, bottom),
        anchor::LEFT         => (r.x,   mid_y),
        anchor::RIGHT        => (right, mid_y),
        anchor::TOP_LEFT     => (r.x,   r.y),
        anchor::BOTTOM_LEFT  => (r.x,   bottom),
        anchor::TOP_RIGHT    => (right, r.y),
        anchor::BOTTOM_RIGHT => (right, bottom),
        // NONE, and anything a client invented: the centre
        _ => (mid_x, mid_y),
    }
}

/// Where the popup's top-left ends up, given the point it hangs off
///
/// Gravity is which way the popup grows, so the corner meeting the anchor point
/// is the opposite one: with `BOTTOM_RIGHT` its top left sits on the point
fn top_left(point: (i32, i32), w: i32, h: i32, which: u32) -> (i32, i32) {
    let (x, y) = point;
    match which {
        gravity::TOP          => (x.saturating_sub(w / 2), y.saturating_sub(h)),
        gravity::BOTTOM       => (x.saturating_sub(w / 2), y),
        gravity::LEFT         => (x.saturating_sub(w),     y.saturating_sub(h / 2)),
        gravity::RIGHT        => (x,                       y.saturating_sub(h / 2)),
        gravity::TOP_LEFT     => (x.saturating_sub(w),     y.saturating_sub(h)),
        gravity::BOTTOM_LEFT  => (x.saturating_sub(w),     y),
        gravity::TOP_RIGHT    => (x,                       y.saturating_sub(h)),
        gravity::BOTTOM_RIGHT => (x,                       y),
        // NONE, and anything invented: centred on the point
        _ => (x.saturating_sub(w / 2), y.saturating_sub(h / 2)),
    }
}

/// The mirror image of a gravity, for a flip
fn flip_x(g: u32) -> u32 {
    match g {
        gravity::LEFT         => gravity::RIGHT,
        gravity::RIGHT        => gravity::LEFT,
        gravity::TOP_LEFT     => gravity::TOP_RIGHT,
        gravity::TOP_RIGHT    => gravity::TOP_LEFT,
        gravity::BOTTOM_LEFT  => gravity::BOTTOM_RIGHT,
        gravity::BOTTOM_RIGHT => gravity::BOTTOM_LEFT,
        other => other,
    }
}

fn flip_y(g: u32) -> u32 {
    match g {
        gravity::TOP          => gravity::BOTTOM,
        gravity::BOTTOM       => gravity::TOP,
        gravity::TOP_LEFT     => gravity::BOTTOM_LEFT,
        gravity::BOTTOM_LEFT  => gravity::TOP_LEFT,
        gravity::TOP_RIGHT    => gravity::BOTTOM_RIGHT,
        gravity::BOTTOM_RIGHT => gravity::TOP_RIGHT,
        other => other,
    }
}

/// The same, for the anchor point: a flip mirrors both halves of the recipe
fn flip_anchor_x(a: u32) -> u32 {
    match a {
        anchor::LEFT         => anchor::RIGHT,
        anchor::RIGHT        => anchor::LEFT,
        anchor::TOP_LEFT     => anchor::TOP_RIGHT,
        anchor::TOP_RIGHT    => anchor::TOP_LEFT,
        anchor::BOTTOM_LEFT  => anchor::BOTTOM_RIGHT,
        anchor::BOTTOM_RIGHT => anchor::BOTTOM_LEFT,
        other => other,
    }
}

fn flip_anchor_y(a: u32) -> u32 {
    match a {
        anchor::TOP          => anchor::BOTTOM,
        anchor::BOTTOM       => anchor::TOP,
        anchor::TOP_LEFT     => anchor::BOTTOM_LEFT,
        anchor::BOTTOM_LEFT  => anchor::TOP_LEFT,
        anchor::TOP_RIGHT    => anchor::BOTTOM_RIGHT,
        anchor::BOTTOM_RIGHT => anchor::TOP_RIGHT,
        other => other,
    }
}

/// Where this popup goes on the screen
///
/// `parent` is where the surface it hangs off is, in screen pixels; `screen` is
/// what it must stay inside: the whole output, not the work area, because a
/// menu may cover a panel
pub(in crate::mayland) fn place(p: &Positioner, parent: Rect, screen: Rect) -> Rect {
    // The size is the client's, clamped to something drawable. A popup larger
    // than the screen cannot be constrained by sliding, so it is cut down here
    let w = p.w.clamp(1, screen.w.max(1));
    let h = p.h.clamp(1, screen.h.max(1));

    let put = |a: u32, g: u32| {
        let (ax, ay) = anchor_point(p.anchor_rect, a);
        let (tx, ty) = top_left(
            (parent.x.saturating_add(ax), parent.y.saturating_add(ay)), w, h, g);
        (tx.saturating_add(p.off.0), ty.saturating_add(p.off.1))
    };

    let (mut x, mut y) = put(p.anchor, p.gravity);

    // Then the constraint. Flipping first, and sliding only if the flip did not
    // help, which is xdg-shell's order of preference
    if p.adjust & adjust::FLIP_X != 0 && (x < screen.x || x + w > screen.x + screen.w) {
        let (fx, _) = put(flip_anchor_x(p.anchor), flip_x(p.gravity));
        if fx >= screen.x && fx + w <= screen.x + screen.w {
            x = fx;
        }
    }
    if p.adjust & adjust::FLIP_Y != 0 && (y < screen.y || y + h > screen.y + screen.h) {
        let (_, fy) = put(flip_anchor_y(p.anchor), flip_y(p.gravity));
        if fy >= screen.y && fy + h <= screen.y + screen.h {
            y = fy;
        }
    }
    if p.adjust & adjust::SLIDE_X != 0 {
        x = x.min(screen.x.saturating_add(screen.w).saturating_sub(w)).max(screen.x);
    }
    if p.adjust & adjust::SLIDE_Y != 0 {
        y = y.min(screen.y.saturating_add(screen.h).saturating_sub(h)).max(screen.y);
    }

    Rect::new(x, y, w, h)
}
