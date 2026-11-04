// What changed, and what therefore has to be copied
//
// Redrawing the screen every frame costs eight megabytes of copy per frame.
// This is the arithmetic that avoids it: a rectangle type, a list of the ones
// that moved, and two rules for when tracking stops being cheaper.
//
// Every rectangle costs a copy-engine round trip (the engine is waited on per
// submission), so past some count the whole screen in one copy is cheaper, and
// `MAX_RECTS` is that crossover. `push_rect` merges two that intersect: a copy
// of the union is one round trip.
//
// `LAST` and `OWED` are the memory: where each surface was last frame, so one
// that moved repaints what it left behind, and what each scanout surface is
// still owed, for a double buffer.

use alloc::vec::Vec;
use spin::Mutex;
use core::sync::atomic::{AtomicBool, Ordering};

use super::cursor::{CURSOR_LAST};
use super::effects::{EffectWin, effects};
use super::{raster, start, stop};

/// A rectangle in screen pixels, far edges exclusive
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect { x0: x, y0: y, x1: x + w, y1: y + h }
    }
    pub fn is_empty(&self) -> bool { self.x1 <= self.x0 || self.y1 <= self.y0 }
    pub fn area(&self) -> u64 {
        if self.is_empty() { 0 } else { (self.x1 - self.x0) as u64 * (self.y1 - self.y0) as u64 }
    }
    /// The smallest rectangle holding both
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            x0: self.x0.min(other.x0),
            y0: self.y0.min(other.y0),
            x1: self.x1.max(other.x1),
            y1: self.y1.max(other.y1),
        }
    }

    /// What is left of this rectangle when 'cut' is taken out: nothing, itself,
    /// or up to four pieces around the hole
    ///
    /// Top and bottom full width first and the sides only across the middle, so
    /// the pieces do not overlap: an overlap would put a pixel in two copies
    pub fn subtract(&self, cut: &Rect, out: &mut Vec<Rect>) {
        let Some(i) = self.intersect(cut) else { out.push(*self); return };
        if i.y0 > self.y0 {
            out.push(Rect { x0: self.x0, y0: self.y0, x1: self.x1, y1: i.y0 });
        }
        if i.y1 < self.y1 {
            out.push(Rect { x0: self.x0, y0: i.y1, x1: self.x1, y1: self.y1 });
        }
        if i.x0 > self.x0 {
            out.push(Rect { x0: self.x0, y0: i.y0, x1: i.x0, y1: i.y1 });
        }
        if i.x1 < self.x1 {
            out.push(Rect { x0: i.x1, y0: i.y0, x1: self.x1, y1: i.y1 });
        }
    }

    /// The part of this rectangle inside 'other', or None if they miss
    pub fn intersect(&self, other: &Rect) -> Option<Rect> {
        let r = Rect {
            x0: self.x0.max(other.x0),
            y0: self.y0.max(other.y0),
            x1: self.x1.min(other.x1),
            y1: self.y1.min(other.y1),
        };
        if r.is_empty() { None } else { Some(r) }
    }

    /// This rectangle with `n` pixels added on every side.
    ///
    /// Does this rectangle hold the whole of `other`? An empty `other` is
    /// inside anything: a piece with no pixels needs no copy
    pub fn contains(&self, other: Rect) -> bool {
        other.is_empty()
            || (self.x0 <= other.x0 && self.y0 <= other.y0
                && self.x1 >= other.x1 && self.y1 >= other.y1)
    }

    pub fn outset(&self, n: i32) -> Rect {
        Rect { x0: self.x0 - n, y0: self.y0 - n, x1: self.x1 + n, y1: self.y1 + n }
    }

    /// The band of `n` pixels just outside this rectangle, as four pieces
    ///
    /// Top and bottom run the full outset width and the sides reach between
    /// them, so the corners belong to the horizontal bands and no pixel is in
    /// two of the four, the cut `subtract` makes: a pixel written twice in one
    /// frame can be caught half way by the raster
    pub fn ring(&self, n: i32) -> [Rect; 4] {
        if n <= 0 || self.is_empty() {
            let none = Rect { x0: 0, y0: 0, x1: 0, y1: 0 };
            return [none; 4];
        }
        [
            Rect { x0: self.x0 - n, y0: self.y0 - n, x1: self.x1 + n, y1: self.y0 },
            Rect { x0: self.x0 - n, y0: self.y1,     x1: self.x1 + n, y1: self.y1 + n },
            Rect { x0: self.x0 - n, y0: self.y0,     x1: self.x0,     y1: self.y1 },
            Rect { x0: self.x1,     y0: self.y0,     x1: self.x1 + n, y1: self.y1 },
        ]
    }
}

/// How many damaged rectangles are worth tracking before redrawing the screen
///
/// Every rectangle costs a copy-engine round trip (the engine is waited on per
/// submission), so past some count the whole screen in one copy is cheaper.
/// Sixteen is well inside that, with three or four windows moving
pub(super) const MAX_RECTS: usize = 16;

/// Set when the next frame must redraw everything: the compositor has just
/// started, or something outside it has drawn on the screen
pub(super) static FORCE_FULL: AtomicBool = AtomicBool::new(true);

/// Which window had the keyboard at the last frame, for damage, as `LAST` and
/// `CURSOR_LAST`
pub(super) static LAST_FOCUS: Mutex<Option<(u32, u32)>> = Mutex::new(None);

pub fn force_full_frame() {
    FORCE_FULL.store(true, Ordering::Relaxed);
    // This repairs the backdrop too. Everything calling it (a modeset, a
    // resize, a pool that would not grow, compositing handed back) is also a
    // reason to doubt that buffer
    super::effects::BACKDROP_DIRTY.store(true, Ordering::Relaxed);
}

/// Where each surface was last frame, to notice that one moved
pub(super) static LAST: Mutex<Vec<(u32, u32, Rect)>> = Mutex::new(Vec::new());

/// Damage each surface still owes, indexed as `scanout::draw_slot` indexes them
///
/// One list per surface. Assuming the two surfaces take turns strictly every
/// frame fails on a frame with nothing to do or a flip the window channel
/// refuses: one frame of damage is owed to a surface that never receives it,
/// and a cursor stays on one of the two, flickering along the pointer's path.
///
/// So every frame's damage is added to the debt of both surfaces, and the one
/// drawn into pays its whole debt and clears it. As
/// `gl_renderer_update_renderbuffers` in weston, which unions each frame's
/// damage into every renderbuffer not stale
pub(super) static OWED: Mutex<[Vec<Rect>; 2]> = Mutex::new([Vec::new(), Vec::new()]);

/// Which surfaces had a curve running last frame
///
/// A `popin` changes no rectangle and sends no client damage: the window sits
/// where the layout put it and only the fraction drawn moves. So a surface with
/// a curve running is damaged whole, every frame.
///
/// And one frame after: `layout::tick` stops the curve on the same step that
/// sets the final size and alpha, and the frame drawing the window in full
/// would otherwise never be painted
pub(super) static ANIMATED: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// What one copy-engine submission is worth, in pixels
///
/// The engine is waited on per submission, so a copy costs a fixed round trip
/// plus its bytes. Measured on the GB206: four copies moving 80k pixels took
/// 273 us. At about 500 megapixels a second the pixels account for 160 of that,
/// leaving 28 us a submission, worth roughly 14k pixels of transfer.
///
/// Why rectangles are merged: a window moving every frame produces two
/// rectangles, where it was and where it is, overlapping almost completely, and
/// every surface underneath pays a second round trip to redraw what it just
/// redrew
pub(super) const MERGE_SLACK: u64 = 15_000;

/// How many pieces a surface may be cut into before drawing it whole is
/// cheaper, with the surfaces above overwriting part of it
///
/// Cutting one rectangle around n windows gives about 2n+1 pieces, so this is
/// room for ten windows over one damaged area. Generous: falling back draws
/// under a window again, which can show black bands across it
pub(super) const MAX_PIECES: usize = 24;

/// What is left of `area` once every opaque window above `i` has been cut out
///
/// The answer is in `pieces`, and `next` is the caller's scratch, so a frame
/// does not allocate two vectors per rectangle. A window and the ring around it
/// are both drawn through this, as the same windows cover them
pub(super) fn uncovered(
    area: Rect,
    i: usize,
    now: &[(u32, u32, Rect)],
    effects: &[EffectWin],
    pieces: &mut Vec<Rect>,
    next: &mut Vec<Rect>,
) {
    pieces.clear();
    pieces.push(area);
    for (j, above) in now.iter().enumerate().skip(i + 1) {
        // What is under an effect window still has to be drawn: the backdrop
        // the blur reads. Only opaque windows above cut anything away
        if effects.iter().any(|w| w.index == j) {
            continue;
        }
        next.clear();
        for piece in pieces.iter() {
            piece.subtract(&above.2, next);
        }
        core::mem::swap(pieces, next);
        if pieces.len() > MAX_PIECES {
            // Cut too finely. Draw the lot, and the windows above paint over
            // some of it
            pieces.clear();
            pieces.push(area);
            return;
        }
    }
}

/// Add a rectangle to the damage list, clipped to the screen and merged with
/// anything cheaper to merge with than to submit separately
///
/// Past MAX_RECTS the whole screen in one copy beats the bookkeeping, and
/// 'full' tells the caller
pub(super) fn push_rect(regions: &mut Vec<Rect>, r: Rect, screen: &Rect, full: &mut bool) {
    if *full {
        return;
    }
    let Some(mut r) = r.intersect(screen) else { return };

    let mut i = 0;
    while i < regions.len() {
        let merged = regions[i].union(&r);
        // Overlapping regions are merged whatever the union costs. Two thin
        // rectangles crossing in an L fail the area test, and left apart their
        // intersection is copied once per region. The frame is written straight
        // down the beam, so a rectangle written twice can be caught by the
        // raster between the writes
        if regions[i].intersect(&r).is_some()
            || merged.area() <= regions[i].area() + r.area() + MERGE_SLACK
        {
            r = merged;
            regions.remove(i);
            // The wider rectangle may now merge with one too far away before,
            // so start again
            i = 0;
            continue;
        }
        i += 1;
    }

    if regions.len() >= MAX_RECTS {
        *full = true;
        return;
    }
    regions.push(r);
}
