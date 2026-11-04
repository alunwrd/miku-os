// Where every window goes
//
// The arithmetic of tiling, in `shell` and not in `server` because the server
// changes when a client asks for something new, and this changes when the
// desktop does.
//
// Three rules:
//
//   - A layout writes a goal and nothing else. `apply` is the only thing in
//     the tree that moves a window on screen or sends it a configure. The
//     curve is in `apply` and `tick`, and no algorithm knows about it: an
//     algorithm says where a window belongs, not when it gets there.
//
//   - The work area is asked for, never assumed: the screen less the outer
//     gap and the edges panels reserve (`reserved`).
//
//   - Numbers live in `Params`, not in the algorithms. The window manager
//     sets them at run time from a config file; an algorithm reads them. A
//     new knob is a field here and a `param` const in the protocol, never a
//     new request.
//
// Not here: which layout to use, which windows are exempt, and what the keys
// do. That is policy, in `/bin/maywm`, and it reaches this file through
// `may_wm`.

use spin::Mutex;

use alloc::vec::Vec;

use super::anim;
use super::ghost;
use crate::mayland::server::{self, Role, Server};

// Rectangles

/// A rectangle in screen pixels, as a layout thinks of one
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const ZERO: Rect = Rect { x: 0, y: 0, w: 0, h: 0 };

    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub const fn pos(&self) -> (i32, i32) { (self.x, self.y) }
    pub const fn size(&self) -> (i32, i32) { (self.w, self.h) }

    /// Nothing to draw. A rectangle with no width is an arrangement that failed
    pub const fn is_empty(&self) -> bool { self.w <= 0 || self.h <= 0 }

    /// The same rectangle with `by` pixels taken off every side
    pub const fn shrink(&self, by: i32) -> Rect {
        Rect { x: self.x + by, y: self.y + by, w: self.w - 2 * by, h: self.h - 2 * by }
    }
}

// What the desktop is set to

/// How the compositor arranges windows
///
/// Hyprland's word for this is a layout and it has three; here it is a kind
/// because `layout` is the module. Everything else about an arrangement is a
/// number in `Params`
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// Windows keep whatever size they drew themselves and are placed out of
    /// each other's way
    Float,
    /// One window on the left, the rest stacked down the right. The layout the
    /// master plugin gives Hyprland
    Master,
    /// Every new window splits the one it opened on, along that window's longer
    /// side. Hyprland's default. It needs a tree: where a window goes depends
    /// on every split made before it, so the arrangement is state and not a
    /// function of the window list
    Dwindle,
}

static KIND: Mutex<Kind> = Mutex::new(Kind::Master);

pub fn kind() -> Kind { *KIND.lock() }

pub fn set_kind(k: Kind) {
    let mut s = server::lock();
    set_kind_in(&mut s, k);
}

/// The same, for a caller that already holds the server
///
/// Every `may_wm` request arrives with the lock held, and a setter taking it
/// again would deadlock the desktop. Two names rather than one that sometimes
/// locks: which one a caller wants is decided when the code is written
pub(in crate::mayland) fn set_kind_in(s: &mut Server, k: Kind) {
    *KIND.lock() = k;
    arrange(s);
}

/// The numbers an arrangement is made of
///
/// One struct because they are read together: an arrangement from an old outer
/// gap and a new inner one never existed. Every field is something a config
/// file sets and `may_wm.set_param` carries
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Params {
    /// Gaps, in pixels: around the screen and between windows
    ///
    /// Hyprland's gaps_out and gaps_in. Without the outer one a tiled window's
    /// shadow has nothing to fall on
    pub gaps_out: i32,
    pub gaps_in: i32,
    /// How a new dwindle split divides the box it is made in, as Hyprland
    /// counts it: 1.0 is even, and the first child gets `half * ratio`. The
    /// range is 0.1 to 1.9: a ratio of 0 or 2 is a window with no pixels
    pub split_ratio: f32,
    /// How much of the work area the master window takes, 0.1 to 0.9
    pub master_factor: f32,
}

impl Params {
    const DEFAULT: Params = Params {
        gaps_out: 16, gaps_in: 12, split_ratio: 1.0, master_factor: 0.5,
    };
}

static PARAMS: Mutex<Params> = Mutex::new(Params::DEFAULT);

pub fn params() -> Params { *PARAMS.lock() }

pub fn gaps() -> (i32, i32) {
    let p = params();
    (p.gaps_out, p.gaps_in)
}

pub fn set_gaps(outer: i32, inner: i32) {
    let mut s = server::lock();
    set_gaps_in(&mut s, outer, inner);
}

/// One of the layout's own numbers, by the protocol's numbering
///
/// The mirror of `compositor::set_param`. `false` means the param belongs to
/// somebody else
pub(in crate::mayland) fn set_param(s: &mut Server, param: u32, value: f32) -> bool {
    use crate::mayland::protocol::wm::param;
    {
        let mut p = PARAMS.lock();
        match param {
            param::SPLIT_RATIO   => p.split_ratio = value.clamp(0.1, 1.9),
            param::MASTER_FACTOR => p.master_factor = value.clamp(0.1, 0.9),
            _ => return false,
        }
    }
    arrange(s);
    true
}

/// The layout's numbers, back to what this compositor starts with
///
/// Through `set_gaps_in` rather than writing `PARAMS` and calling `arrange`
/// here, because the outer gap is part of the work area and that function
/// re-sends it
pub(in crate::mayland) fn reset(s: &mut Server) {
    {
        let mut p = PARAMS.lock();
        p.split_ratio = Params::DEFAULT.split_ratio;
        p.master_factor = Params::DEFAULT.master_factor;
    }
    set_gaps_in(s, Params::DEFAULT.gaps_out, Params::DEFAULT.gaps_in);
}

pub(in crate::mayland) fn set_gaps_in(s: &mut Server, outer: i32, inner: i32) {
    {
        let mut p = PARAMS.lock();
        // Negative gaps would overlap windows, and a gap wider than the screen
        // leaves a work area with no room in it
        p.gaps_out = outer.clamp(0, 200);
        p.gaps_in = inner.clamp(0, 200);
    }
    arrange(s);
    // The outer gap is part of the work area, so a window manager that set it
    // has to hear the new rectangle
    server::iface::desktop::send_work_area(s);
}

// The work area

/// Edges something has asked the layout to keep off
///
/// A panel at the top of the screen reserves height, and a tiling layout must
/// not draw under it. Every algorithm and every maximise reads `work_area`
/// instead of the screen
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Reserved {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

static RESERVED: Mutex<Reserved> = Mutex::new(Reserved { top: 0, bottom: 0, left: 0, right: 0 });

pub fn reserved() -> Reserved { *RESERVED.lock() }

/// What the layer surfaces are keeping off the screen
///
/// The only writer is `server::reserve_edges`. True when the number moved, so
/// the caller knows whether to re-arrange: a panel committing a frame every
/// second must not cost a re-layout every second
pub fn set_reserved(r: Reserved) -> bool {
    let mut held = RESERVED.lock();
    if *held == r {
        return false;
    }
    *held = r;
    true
}

/// The screen a layout may use, in screen pixels
///
/// Two of them, as in Hyprland: the tiled one has the outer gap taken off and
/// is what an algorithm divides; the floating one has not and is what a window
/// placed by hand or maximised is measured against
pub fn work_area(s: &Server, floating: bool) -> Rect {
    let r = reserved();
    let (sw, sh) = (s.width.max(0), s.height.max(0));
    let area = Rect {
        x: r.left,
        y: r.top,
        w: sw - r.left - r.right,
        h: sh - r.top - r.bottom,
    };
    if floating { area } else { area.shrink(params().gaps_out) }
}

/// The tiled work area, for a caller that does not hold the server:
/// `mayland status` and anything else only reading the number
pub fn work_area_now() -> Rect {
    let s = server::lock();
    work_area(&s, false)
}

/// The floating one, which is what a fullscreen window is measured against
pub fn work_area_now_floating() -> Rect {
    let s = server::lock();
    work_area(&s, true)
}

// Goals, and the one place they are applied

/// Records where a window belongs, without moving it
///
/// The size is the layout's when it has one to give, and the client's own when
/// it does not, which is what `sized` distinguishes. A floating window whose
/// goal claimed a size would be sent a configure for the size it drew, and a
/// client answering a configure redraws
pub(in crate::mayland) fn set_goal(s: &mut Server, si: usize, r: Rect, sized: bool) {
    s.surfaces[si].goal = r;
    s.surfaces[si].goal_sized = sized;
    s.surfaces[si].placed = true;
}

/// Puts a window somewhere, now, by somebody's direct decision
///
/// A drag and the backdrop's corner: the goal is where the pointer put it, so
/// goal and position are set together. Everything a layout decides goes through
/// `set_goal`
pub(in crate::mayland) fn put(s: &mut Server, si: usize, x: i32, y: i32) {
    let (w, h) = size_or_goal(s, si);
    s.surfaces[si].pos = (x, y);
    s.surfaces[si].goal = Rect::new(x, y, w, h);
    s.surfaces[si].placed = true;
}

/// Moves every window to the goal it was given, and tells it its size
///
/// The only place in the compositor that writes a window's position as a
/// consequence of a layout, and the only place a configure is sent for one.
/// This assignment is the first step of a curve, and `tick` walks the rest
pub(in crate::mayland) fn apply(s: &mut Server) {
    for si in 0..s.surfaces.len() {
        if !s.surfaces[si].placed {
            continue;
        }
        let g = s.surfaces[si].goal;
        if s.surfaces[si].goal_sized
            && !g.is_empty()
            && (s.surfaces[si].last_w, s.surfaces[si].last_h) != (g.w, g.h)
        {
            // Only when it changed. A configure proposing the size a window has
            // still costs the client a redraw, and there is one of these for
            // every window each time any window opens
            server::surface::configure(s, si, g.w, g.h);
        }
        // The first arrangement a window ever gets is its opening. A layer's
        // is its first frame instead (`layer_shown`): it is placed when it
        // asks for a size, long before it has anything to show
        if s.surfaces[si].fresh {
            if s.surfaces[si].layer.is_some() && !s.surfaces[si].has_buffer() {
                continue;
            }
            s.surfaces[si].fresh = false;
            s.surfaces[si].pos = g.pos();
            start_open(s, si);
            continue;
        }
        if s.surfaces[si].pos == g.pos() {
            continue;
        }
        // A window already where it belongs is not animated, and one that is
        // moving carries on from where it has got to
        if !start_move(s, si) {
            s.surfaces[si].pos = g.pos();
        }
    }
    // Whatever hangs off a window that moved moves with it in the same pass: a
    // menu lagging by one frame visibly slides
    follow_parents(s);
    // And the shells hear where the windows they know of are now
    crate::mayland::server::iface::desktop::tell_moved(s);
}

/// Begins a move animation, or says there is not going to be one
///
/// False means the caller assigns: with animations off, and for a window never
/// placed, which has nowhere to come from
fn start_move(s: &mut Server, si: usize) -> bool {
    let spec = anim::spec(anim::class::WINDOWS);
    if !animating() || !spec.enabled || spec.style == anim::style::NONE {
        return false;
    }
    let from = s.surfaces[si].pos;
    let a = &mut s.surfaces[si].anim;
    a.running = true;
    a.begun_us = now_us();
    a.ms = spec.ms;
    a.curve = spec.curve;
    // A move is a move whatever the class is styled as: `popin` describes
    // opening, and a window being re-tiled has opened
    a.style = anim::style::SLIDE;
    a.from = (from.0 as f32, from.1 as f32);
    // A window re-tiled while still opening carries on fading and growing from
    // where it has got to. The clock above starts again, and `tick` reads the
    // alpha and the scale off `from_*`, so left at the opening values a move
    // would restart the fade. At login that is the console: the bar's exclusive
    // zone moves it 48 px while it fades in. A window not opening has both at 1
    // already
    a.from_alpha = a.alpha;
    a.from_scale = a.scale;
    true
}

/// Begins the animation a window gets for existing
///
/// Two classes at once: `windows` says how it arrives (`popin 80%` is a scale)
/// and `fadeIn` says whether it appears out of nothing. As in Hyprland they run
/// on their own clocks: layers can pop in over 270 ms with their alpha up in 50
/// (`dev/mayland-layers-above.md`, section 4).
///
/// A layer a `noanim` rule names gets neither
fn start_open(s: &mut Server, si: usize) {
    if !animating() {
        return;
    }
    let is_layer = s.surfaces[si].layer.is_some();
    if s.surfaces[si].layer.is_some_and(|l| anim::layer_has(&l.namespace, crate::mayland::protocol::wm::layer_rule::NOANIM)) {
        return;
    }
    let (shape, fade) = if is_layer {
        (anim::spec(anim::class::LAYERS_IN), anim::spec(anim::class::FADE_LAYERS_IN))
    } else {
        (anim::spec(anim::class::WINDOWS_IN), anim::spec(anim::class::FADE_IN))
    };
    if !shape.enabled && !fade.enabled {
        return;
    }

    let at = (s.surfaces[si].goal.x as f32, s.surfaces[si].goal.y as f32);
    let a = &mut s.surfaces[si].anim;
    *a = anim::Anim {
        running: true,
        begun_us: now_us(),
        ms: if shape.enabled { shape.ms } else { 0 },
        curve: shape.curve,
        style: shape.style,
        from: (0.0, 0.0),
        // Its own clock, or none: a 0 here would hand the alpha to the
        // movement's, and a fade that is off must not move at all
        alpha_ms: if fade.enabled { fade.ms.max(1) } else { 0 },
        alpha_curve: fade.curve,
        ..anim::Anim::NONE
    };
    // Where the pixels start from. A window not fading in starts opaque, and
    // one not popping in starts full size
    a.from_alpha = if fade.enabled { 0.0 } else { 1.0 };
    a.to_alpha = 1.0;
    a.alpha = a.from_alpha;
    a.from_scale = if shape.enabled && shape.style == anim::style::POPIN {
        shape.arg.clamp(0.05, 1.0)
    } else {
        1.0
    };
    a.to_scale = 1.0;
    a.scale = a.from_scale;
    // The position stays: an opening window is where the layout put it, and
    // only its size and its alpha move
    a.from = at;
}

/// A layer's first frame: its opening starts now, when there is something to
/// see
///
/// A layer is placed at `configure_layer`, when it asks for a size, before it
/// has anything to show. Starting the curve at the next arrangement would spend
/// part of it on nothing for a panel and open a notification a second time
pub(in crate::mayland) fn layer_shown(s: &mut Server, si: usize) {
    if s.surfaces[si].fresh && s.surfaces[si].placed {
        s.surfaces[si].fresh = false;
        s.surfaces[si].pos = s.surfaces[si].goal.pos();
        start_open(s, si);
    }
}

/// Whether anything should animate at all right now
///
/// On the software path as well: `render::compose` already composes blended
/// regions on the processor every frame the bar is up, and a desktop behaving
/// differently per machine is two desktops. `animations { enabled = false }`
/// turns it all off
fn animating() -> bool {
    anim::enabled()
}

/// The clock animations are measured against, in microseconds
///
/// The timestamp counter rather than the timer tick: a tick is 4 ms, which
/// visibly steps a fade over a 300 ms curve
pub fn now_us() -> u64 {
    let khz = crate::kcore::time::tsc_khz().max(1) as u64;
    crate::kcore::time::rdtsc() / (khz / 1000).max(1)
}

/// Puts every child where its parent went
///
/// One place, run after positions have settled, rather than a hook in each of
/// the three things writing a position: the layout, a drag, and the animation
/// walking a curve.
///
/// Parents before children, which `stack` guarantees: it emits a subtree
/// parent-first, so a submenu reads a parent already placed this frame
pub(in crate::mayland) fn follow_parents(s: &mut Server) {
    let order = s.stack();
    for si in order {
        let Some(parent) = s.surfaces[si].parent else { continue };
        let Some(pi) = s.surface_idx(parent) else { continue };
        let (px, py) = s.surfaces[pi].pos;
        let rel = s.surfaces[si].rel;
        let at = (px + rel.0, py + rel.1);
        if s.surfaces[si].pos != at {
            s.surfaces[si].pos = at;
            let (w, h) = (s.surfaces[si].goal.w, s.surfaces[si].goal.h);
            s.surfaces[si].goal = Rect::new(at.0, at.1, w, h);
        }
    }
}

/// Walks every running curve one frame
///
/// Called from the frame loop before the frame is composed. It moves `pos`,
/// never `goal`, and never sends a configure: a client is told its size once,
/// when the layout decides it
pub fn tick(now_us: u64) {
    let mut s = server::lock();
    for si in 0..s.surfaces.len() {
        if !s.surfaces[si].anim.running {
            continue;
        }
        let goal = s.surfaces[si].goal;
        let a = s.surfaces[si].anim;
        let x = a.progress(now_us);
        let y = anim::curve_y(a.curve, x);
        let (xa, alpha_curve) = a.alpha_progress(now_us);
        let ya = anim::curve_y(alpha_curve, xa);

        let nx = a.from.0 + (goal.x as f32 - a.from.0) * y;
        let ny = a.from.1 + (goal.y as f32 - a.from.1) * y;
        s.surfaces[si].pos = (nx as i32, ny as i32);
        s.surfaces[si].anim.alpha = a.from_alpha + (a.to_alpha - a.from_alpha) * ya;
        s.surfaces[si].anim.scale = a.from_scale + (a.to_scale - a.from_scale) * y;

        if x >= 1.0 && xa >= 1.0 {
            // Landed, exactly on the goal and not on what the last step rounded
            // to: a window a pixel short of its tile has a seam down one side
            s.surfaces[si].pos = goal.pos();
            s.surfaces[si].anim.running = false;
            s.surfaces[si].anim.alpha = a.to_alpha;
            s.surfaces[si].anim.scale = a.to_scale;
        }
    }
    // And whatever hangs off something that just moved moves with it
    follow_parents(&mut s);
    // An afterimage whose curve has ended is memory nobody is drawing from
    let freed = reap(&mut s);
    drop(s);
    for h in freed {
        let _ = crate::mayland::server::shm::destroy(h);
    }
}

/// Arrange the desktop from outside the compositor
///
/// For the selftest and the shell: everything inside already holds the server
/// and calls `arrange` directly
pub fn arrange_now() {
    let mut s = server::lock();
    arrange(&mut s);
}

/// Starts a curve by hand, from a position to a goal, for the selftest
pub fn begin_anim_for_test(client: u32, surface: u32, from: (i32, i32), goal: Rect,
                           now_us: u64, ms: u32, curve: u32, style: u32) {
    let mut s = server::lock();
    let Some(si) = s.surface_idx((client, surface)) else { return };
    s.surfaces[si].pos = from;
    s.surfaces[si].goal = goal;
    s.surfaces[si].placed = true;
    s.surfaces[si].anim = anim::Anim {
        running: true,
        begun_us: now_us,
        ms,
        curve,
        style,
        from: (from.0 as f32, from.1 as f32),
        ..anim::Anim::NONE
    };
}

/// Lets go of anything whose curve has ended
///
/// Separate from `tick` because freeing shared memory takes a second lock, and
/// nothing holds the server while reaching for another
fn reap(s: &mut Server) -> alloc::vec::Vec<u32> {
    ghost::reap_ghosts(s)
}

/// How many curves are running, for `mayland status`
pub fn running() -> usize {
    let s = server::lock();
    s.surfaces.iter().filter(|x| x.anim.running).count()
}

/// How big a window is, by the layout's reckoning
///
/// The goal when it has one: a window just told to be 900 wide counts as 900
/// wide, and the client catches up. The buffer otherwise
fn size_or_goal(s: &Server, si: usize) -> (i32, i32) {
    let g = s.surfaces[si].goal;
    if !g.is_empty() {
        return g.size();
    }
    server::scene::size_of(s, si).unwrap_or((0, 0))
}

// Arranging

/// Arranges every window on the workspace being looked at
///
/// Called when the set of windows changes (opened, closed, moved to another
/// workspace, made the backdrop) and when the layout or its numbers change. Not
/// per frame: a configure every frame would make a client redraw for ever. The
/// per-frame half of this file is `tick`
pub(in crate::mayland) fn arrange(s: &mut Server) {
    if s.width <= 0 || s.height <= 0 {
        return;
    }
    match kind() {
        // Nothing to compute: a floating window is wherever it was put, and
        // `apply` will find its goal already where its position is
        Kind::Float => {}
        Kind::Master => tile_master(s),
        Kind::Dwindle => tile_dwindle(s),
    }
    // After the algorithm: a window can be fullscreen in any layout. Last, so
    // it overwrites what the algorithm gave; leaving fullscreen is then free,
    // since the next arrangement does not do this
    fullscreen_windows(s);
    apply(s);
}

/// Every fullscreen window on this workspace fills the work area
///
/// The floating one (reserved edges off, gaps not), because a fullscreen window
/// with a gap around it is not fullscreen. It does not cover panels
fn fullscreen_windows(s: &mut Server) {
    let area = work_area(s, true);
    if area.is_empty() {
        return;
    }
    let ws = server::workspace();
    for si in 0..s.surfaces.len() {
        if s.surfaces[si].role != Role::Toplevel || !s.surfaces[si].fullscreen {
            continue;
        }
        if !s.surfaces[si].on_workspace(ws) {
            continue;
        }
        set_goal(s, si, area, true);
    }
}

/// Which windows an algorithm divides the screen between
///
/// In the order the windows were opened, not stacking order: `raise` runs on
/// every click and focus change, so a layout ranked by z would make the last
/// clicked window the master. `surfaces` keeps insertion order, so the oldest
/// window is master.
///
/// Only what is on screen: a window on another workspace is not in the layout,
/// and the backdrop is the screen. A floating window is out by its own state,
/// and a fullscreen one because it gets the whole work area
fn tiled_windows(s: &Server) -> Vec<usize> {
    let ws = server::workspace();
    (0..s.surfaces.len())
        .filter(|&i| s.surfaces[i].role == Role::Toplevel
                     && !s.surfaces[i].backdrop
                     && !s.surfaces[i].floating
                     && !s.surfaces[i].fullscreen
                     && s.surfaces[i].on_workspace(ws))
        .collect()
}

fn tile_master(s: &mut Server) {
    let tiled = tiled_windows(s);
    if tiled.is_empty() {
        return;
    }
    let area = work_area(s, false);
    if area.is_empty() {
        return;
    }
    let p = params();
    let boxes = master_stack(area, tiled.len(), p.gaps_in, p.master_factor);
    for (&si, &b) in tiled.iter().zip(boxes.iter()) {
        set_goal(s, si, b, true);
    }
}

/// One window on the left, the rest down the right
///
/// Arithmetic only, so a test can hold it still. A single window fills the work
/// area
pub fn master_stack(area: Rect, n: usize, gap: i32, factor: f32) -> Vec<Rect> {
    let mut out = Vec::with_capacity(n);
    if n == 0 {
        return out;
    }
    if n == 1 {
        out.push(area);
        return out;
    }

    let master_w = ((area.w - gap) as f32 * factor.clamp(0.1, 0.9)) as i32;
    out.push(Rect { x: area.x, y: area.y, w: master_w, h: area.h });

    let stack_x = area.x + master_w + gap;
    let stack_w = area.w - master_w - gap;
    let rest = n - 1;
    // Divided by remainder rather than by rounding, so the pieces add back up
    // to the whole and the last one does not fall a pixel short of the edge
    let each = (area.h - gap * (rest as i32 - 1)) / rest as i32;
    for k in 0..rest {
        let y = area.y + k as i32 * (each + gap);
        let h = if k == rest - 1 { area.y + area.h - y } else { each };
        out.push(Rect { x: stack_x, y, w: stack_w, h });
    }
    out
}

// Placing and moving one window

/// Keeps a window somewhere it can be got hold of again
///
/// Not clamped fully on screen: dragging a window half past an edge is
/// ordinary. Losing it entirely is refused: enough of it stays for the pointer
/// to reach.
///
/// Enough is `KEEP`, or the whole window when it is smaller than that; a window
/// forty pixels wide can then still sit at the left edge
pub(in crate::mayland) fn clamp_pos(s: &Server, pos: (i32, i32), size: (i32, i32)) -> (i32, i32) {
    const KEEP: i32 = 48;
    // Read off the server rather than through `output_size`, which takes the
    // lock every caller here is already holding
    let (sw, sh) = (s.width.max(1), s.height.max(1));
    let (kx, ky) = (KEEP.min(size.0), KEEP.min(size.1));
    (pos.0.clamp(kx - size.0, sw - kx), pos.1.clamp(0, sh - ky))
}

/// Where a window goes when it opens and nothing is tiling it
///
/// Centred if the screen is empty, and stepped down and right from the last one
/// otherwise, so two windows are never exactly on top of each other. A default:
/// the window manager overrules it with a rule or a move
pub(in crate::mayland) fn place_window(s: &mut Server, si: usize) {
    let Some((w, h)) = server::scene::size_of(s, si) else { return };
    let area = work_area(s, true);
    if area.is_empty() {
        return;
    }

    // Where the last window that is not this one and not the backdrop went.
    // `stack` is toplevels only and in drawing order, so this is the topmost
    // window somebody opened
    let last = s.stack().into_iter().rev()
        .find(|&i| i != si && !s.surfaces[i].backdrop)
        .map(|i| s.surfaces[i].pos);

    const STEP: i32 = 40;
    let pos = match last {
        None => (area.x + (area.w - w) / 2, area.y + (area.h - h) / 2),
        Some((x, y)) => {
            let (mut nx, mut ny) = (x + STEP, y + STEP);
            // Off the bottom or the right, and it starts again near the top
            // left
            if nx + w > area.x + area.w || ny + h > area.y + area.h {
                nx = area.x + STEP;
                ny = area.y + STEP;
            }
            (nx, ny)
        }
    };
    let (x, y) = clamp_pos(s, pos, (w, h));
    s.surfaces[si].pos = (x, y);
    s.surfaces[si].goal = Rect::new(x, y, w, h);
    s.surfaces[si].goal_sized = false;
}

/// Moves a window, by the compositor's own decision (dragging)
///
/// The position is the compositor's and the client cannot answer back
pub fn move_window(client: u32, surface: u32, dx: i32, dy: i32) -> bool {
    let mut s = server::lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    let size = size_or_goal(&s, si);
    let p = s.surfaces[si].pos;
    let (x, y) = clamp_pos(&s, (p.0 + dx, p.1 + dy), size);
    put(&mut s, si, x, y);
    follow_parents(&mut s);
    true
}

/// Put a window at a given place, for the shell to drive
pub fn set_window_pos(client: u32, surface: u32, x: i32, y: i32) -> bool {
    let mut s = server::lock();
    let Some(si) = s.surface_idx((client, surface)) else { return false };
    let size = size_or_goal(&s, si);
    let (x, y) = clamp_pos(&s, (x, y), size);
    put(&mut s, si, x, y);
    follow_parents(&mut s);
    true
}

// Dwindle
//
// Hyprland's default layout, which needs memory. Master is a function of the
// window list, but dwindle's answer depends on the order the windows arrived
// and on every split dragged since. So the arrangement is a tree that lives
// between arrangements, one per workspace, and `sync` keeps it matching the
// windows that exist.
//
// The shape is `DwindleAlgorithm.cpp`'s: a node is a leaf holding one window or
// a split holding two children, a split divides its box along the longer side,
// and `ratio` says where. Left out is what needs a mouse (smart splits, split
// bias, the direction overrides): the shell has no way to ask for them yet.

/// One node of a workspace's split tree
///
/// One type because a split becomes a leaf when its sibling closes, and a leaf
/// becomes a split when a window opens on it
#[derive(Clone, Copy)]
struct Node {
    parent: Option<usize>,
    /// None for a leaf
    kids: Option<[usize; 2]>,
    /// The window, for a leaf. Named by the pair everything else names one by
    win: Option<(u32, u32)>,
    /// Top and bottom rather than left and right
    vertical: bool,
    /// Where the split falls: 1.0 is even, as Hyprland counts it
    ratio: f32,
    /// What this node was last given, so a drag can ask how big this split is
    /// without walking from the root
    rect: Rect,
}

impl Node {
    const fn leaf(win: (u32, u32), parent: Option<usize>) -> Node {
        Node {
            parent, kids: None, win: Some(win),
            vertical: false, ratio: 1.0, rect: Rect::ZERO,
        }
    }
}

/// One workspace's tree
struct Tree {
    nodes: Vec<Node>,
    /// Slots `remove` gave back. The vector is indexed by slot, so nothing may
    /// shift, and it must not grow with each window of a session
    free: Vec<usize>,
    root: Option<usize>,
}

impl Tree {
    const fn new() -> Tree {
        Tree { nodes: Vec::new(), free: Vec::new(), root: None }
    }

    fn alloc(&mut self, n: Node) -> usize {
        if let Some(i) = self.free.pop() {
            self.nodes[i] = n;
            return i;
        }
        self.nodes.push(n);
        self.nodes.len() - 1
    }

    fn release(&mut self, i: usize) {
        self.nodes[i].win = None;
        self.nodes[i].kids = None;
        self.nodes[i].parent = None;
        self.free.push(i);
    }

    fn leaf_of(&self, win: (u32, u32)) -> Option<usize> {
        (0..self.nodes.len()).find(|&i| !self.free.contains(&i)
                                        && self.nodes[i].win == Some(win))
    }

    /// Every leaf, in the order a walk from the root finds them
    fn leaves(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(root) = self.root else { return out };
        let mut stack = alloc::vec![root];
        while let Some(i) = stack.pop() {
            match self.nodes[i].kids {
                // Pushed in reverse so the walk comes out left to right
                Some([a, b]) => { stack.push(b); stack.push(a); }
                None => out.push(i),
            }
        }
        out
    }

    /// Puts a window in, splitting `at` if there is anything there to split
    ///
    /// `at` is the focused window's leaf: the window being looked at gives up
    /// half its space
    fn insert(&mut self, win: (u32, u32), at: Option<usize>, ratio: f32) {
        let Some(root) = self.root else {
            let n = self.alloc(Node::leaf(win, None));
            self.root = Some(n);
            return;
        };
        let target = match at {
            Some(t) if !self.free.contains(&t) && self.nodes[t].kids.is_none() => t,
            // Nothing focused here, so the newest window splits the deepest
            // leaf, which is the last one dwindle made
            _ => *self.leaves().last().unwrap_or(&root),
        };

        let old_parent = self.nodes[target].parent;
        let rect = self.nodes[target].rect;
        let moved = self.alloc(self.nodes[target]);
        let fresh = self.alloc(Node::leaf(win, None));
        // The target's slot becomes the split, so nothing above it is
        // repointed, and the root stays the root
        self.nodes[target] = Node {
            parent: old_parent,
            kids: Some([moved, fresh]),
            win: None,
            // Along the longer side
            vertical: rect.h > rect.w,
            ratio,
            rect,
        };
        self.nodes[moved].parent = Some(target);
        self.nodes[fresh].parent = Some(target);
    }

    /// Take a window out, and let its sibling have the space back
    fn remove(&mut self, win: (u32, u32)) {
        let Some(leaf) = self.leaf_of(win) else { return };
        let Some(parent) = self.nodes[leaf].parent else {
            self.release(leaf);
            self.root = None;
            return;
        };
        let [a, b] = self.nodes[parent].kids.unwrap_or([leaf, leaf]);
        let sibling = if a == leaf { b } else { a };
        // The sibling takes the parent's slot rather than the parent taking
        // the sibling's contents, so the grandparent needs no repointing
        let grand = self.nodes[parent].parent;
        self.nodes[parent] = self.nodes[sibling];
        self.nodes[parent].parent = grand;
        if let Some([x, y]) = self.nodes[parent].kids {
            self.nodes[x].parent = Some(parent);
            self.nodes[y].parent = Some(parent);
        }
        self.release(sibling);
        self.release(leaf);
    }

    /// Divide `area` between the leaves, and hand each one its box
    fn walk(&mut self, node: usize, area: Rect, gap: i32, out: &mut Vec<((u32, u32), Rect)>) {
        self.nodes[node].rect = area;
        let Some([a, b]) = self.nodes[node].kids else {
            if let Some(w) = self.nodes[node].win {
                out.push((w, area));
            }
            return;
        };
        let ratio = self.nodes[node].ratio.clamp(0.1, 1.9);
        let (first, second) = if self.nodes[node].vertical {
            let cut = ((area.h as f32 / 2.0) * ratio) as i32;
            let cut = cut.clamp(1, (area.h - 1).max(1));
            // The gap comes out of the middle, half from each side, so two
            // windows either side of a split are as far apart as two either
            // side of the master's edge
            (Rect::new(area.x, area.y, area.w, cut - gap / 2),
             Rect::new(area.x, area.y + cut + (gap - gap / 2), area.w,
                       area.h - cut - (gap - gap / 2)))
        } else {
            let cut = ((area.w as f32 / 2.0) * ratio) as i32;
            let cut = cut.clamp(1, (area.w - 1).max(1));
            (Rect::new(area.x, area.y, cut - gap / 2, area.h),
             Rect::new(area.x + cut + (gap - gap / 2), area.y,
                       area.w - cut - (gap - gap / 2), area.h))
        };
        self.walk(a, first, gap, out);
        self.walk(b, second, gap, out);
    }
}

/// One tree per workspace, grown on demand
///
/// Indexed by workspace, so index 0 is never used
static TREES: Mutex<Vec<Tree>> = Mutex::new(Vec::new());

/// Forgets every split
///
/// For the selftest, which cannot leave the desktop rearranged, and for a shell
/// that wants the layout back as it started
pub fn reset_trees() {
    TREES.lock().clear();
}

fn tile_dwindle(s: &mut Server) {
    let tiled = tiled_windows(s);
    let area = work_area(s, false);
    if area.is_empty() {
        return;
    }
    let p = params();
    let ws = server::workspace() as usize;

    let mut trees = TREES.lock();
    while trees.len() <= ws {
        trees.push(Tree::new());
    }
    let tree = &mut trees[ws];

    // The tree against the windows that exist. Removals first: a window that
    // closed while another opened must give its space back before the new one
    // decides how much there is to take
    let want: Vec<(u32, u32)> = tiled.iter()
        .map(|&si| (s.surfaces[si].client, s.surfaces[si].id))
        .collect();
    let have: Vec<(u32, u32)> = tree.leaves().iter()
        .filter_map(|&i| tree.nodes[i].win)
        .collect();
    for w in have.iter() {
        if !want.contains(w) {
            tree.remove(*w);
        }
    }
    let focused = server::focused_window(s).and_then(|f| tree.leaf_of(f));
    for w in want.iter() {
        if tree.leaf_of(*w).is_none() {
            tree.insert(*w, focused, p.split_ratio);
        }
    }

    let Some(root) = tree.root else { return };
    let mut boxes = Vec::new();
    tree.walk(root, area, p.gaps_in, &mut boxes);
    drop(trees);

    for (win, r) in boxes {
        if let Some(si) = s.surface_idx(win) {
            set_goal(s, si, r, true);
        }
    }
}

/// Moves the split above a window, which is what `splitratio` acts on
///
/// A window's own number: Hyprland's dispatcher resizes the active window's
/// parent split and leaves every other split alone, and a ratio set on the
/// desktop is only the value new splits start at
pub(in crate::mayland) fn set_window_split(s: &mut Server, win: (u32, u32), ratio: f32) -> bool {
    let ws = server::workspace() as usize;
    let mut trees = TREES.lock();
    if ws >= trees.len() {
        return false;
    }
    let tree = &mut trees[ws];
    let Some(leaf) = tree.leaf_of(win) else { return false };
    let Some(parent) = tree.nodes[leaf].parent else { return false };
    tree.nodes[parent].ratio = ratio.clamp(0.1, 1.9);
    drop(trees);
    arrange(s);
    true
}
