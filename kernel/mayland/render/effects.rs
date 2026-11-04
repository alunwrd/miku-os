// How a window is drawn, as opposed to where
//
// Two owners and one vocabulary. The desktop has numbers (a border width, a
// corner radius, an opacity, a blur) and a window may have its own, which a
// Hyprland window rule becomes on the wire. `Effects` is the first, `Fx` the
// second, and `Fx::over` is where they meet.
//
// `paint_phys`: the copy engine reads memory, so a solid colour has to exist as
// pixels before it can be a border. The cache is keyed by colour and evicted
// least recently used: two windows in one frame ask for two border colours and
// neither may discard the other's.

use alloc::vec::Vec;
use spin::Mutex;

use crate::mayland::server;
use crate::mayland::server::shm;

use super::damage::{Rect, force_full_frame};
use super::{software};

/// What the effects pass is set to, and whether it runs at all
///
/// The desktop's numbers: what a window gets when it asked for nothing of its
/// own. A window that did (`may_wm.set_window_param`, a Hyprland window rule on
/// the wire) overrides fields through `Fx`, and `Fx::over` is where they meet
#[derive(Clone, Copy)]
pub struct Effects {
    pub on: bool,
    /// How much of the window shows through, 0 to 1
    pub alpha: f32,
    /// Corner radius in pixels
    pub corner: f32,
    /// How dark the shadow is at the window's edge, 0 to 1
    pub shadow: f32,
    /// How far the shadow reaches
    pub margin: u32,
    /// Tap offset for the blur, in pixels at each level
    pub radius: f32,
    /// `rounding_power`. The corner is `corner · power / 2` pixels and a
    /// superellipse of this power (`libmay::shape`)
    pub power: f32,
    /// What a window that is not the active one is drawn at, where `alpha` is
    /// what the active one is (`server::scene` picks)
    pub inactive: f32,
    /// How much a window that is not the active one is darkened. 0 is off
    pub dim: f32,
}

impl Effects {
    /// What the desktop looks like before any config has spoken
    ///
    /// A named constant, as `may_wm.reset` has to say "back to this".
    ///
    /// The alpha is 1.0, as `Border::DEFAULT.size` is zero: a machine told
    /// nothing is drawn plainly. Weston does the same: `weston_view_create`
    /// sets `alpha = 1.0`, and the pixman renderer blits an opaque view with
    /// `PIXMAN_OP_SRC`.
    ///
    /// A desktop wanting see-through windows says so: the shipped config has
    /// `decoration { active_opacity }`, and a window rule has `opacity`
    pub const DEFAULT: Effects = Effects {
        on: false, alpha: 1.0, corner: 12.0, shadow: 0.45, margin: 24, radius: 2.0,
        power: 2.0, inactive: 1.0, dim: 0.0,
    };

    /// The outline `libmay::shape` draws, with the desktop's border
    pub fn shape(&self, border: i32) -> libmay::shape::Shape {
        libmay::shape::Shape { rounding: self.corner, power: self.power, border: border.max(0) as f32 }
    }
}

pub static EFFECTS: Mutex<Effects> = Mutex::new(Effects::DEFAULT);

/// The compute engine is not to be used again, until the backend rebuilds it.
///
/// Not a config knob: set when the engine has failed to finish a frame, or the
/// pass could not map what it was about to read. The next submission would
/// fault the same way, and a faulted compute channel takes the copy channel
/// with it: the cursor batch acquires a fence that is never released. A desktop
/// without blur is preferable to a screen that never updates.
///
/// Holds the engine's generation at giving up (`Scanout::compute_generation`),
/// or `NOT_GIVEN_UP`. A driver rebuilding a dead channel hands out a new
/// generation, and a usable engine with a different number is worth trying
/// (`dev/gpu-memory-compute.md`, 3.6)
static GAVE_UP_AT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(NOT_GIVEN_UP);
const NOT_GIVEN_UP: u64 = u64::MAX;
/// Given up on an engine that had no generation to name
const NO_GENERATION: u64 = u64::MAX - 1;
/// Whether effects were on when the engine was given up on: giving up turns
/// them off, and an engine coming back turns them on only if they were
static ON_BEFORE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

fn generation() -> u64 {
    super::backend().and_then(|b| b.compute_generation()).map_or(NO_GENERATION, |g| g as u64)
}

/// Stop routing anything through the compute engine. Reports once. Called
/// before anything turns effects off, so whether they were on is remembered
pub(super) fn give_up_on_compute(why: &str) {
    use core::sync::atomic::Ordering;
    let on = EFFECTS.lock().on;
    if GAVE_UP_AT.compare_exchange(NOT_GIVEN_UP, generation(), Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        ON_BEFORE.store(on, Ordering::Relaxed);
        crate::kwarn!(
            "[mayland] the compute engine is out of the frame: {} - windows are \
             copied from here, blur and fades included", why);
    }
}

/// Whether the compute engine has been given up on. One atomic load while it
/// has not; while it has, a look at whether the backend has rebuilt it
pub(super) fn given_up() -> bool {
    use core::sync::atomic::Ordering;
    let at = GAVE_UP_AT.load(Ordering::Relaxed);
    if at == NOT_GIVEN_UP {
        return false;
    }
    let now = generation();
    if now == NO_GENERATION || now == at {
        return true;
    }
    // A usable engine, and not the one that failed
    if GAVE_UP_AT.compare_exchange(at, NOT_GIVEN_UP, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        if ON_BEFORE.load(Ordering::Relaxed) {
            EFFECTS.lock().on = true;
        }
        BACKDROP_DIRTY.store(true, Ordering::Relaxed);
        super::damage::force_full_frame();
        crate::knotice!("[mayland] the compute engine is back (generation {}) - effects return", now);
    }
    false
}

/// How far past its own edge the effects pass may reach for a window: its
/// shadow. The blur is the shell below the windows, computed once for the whole
/// screen (`dev/mayland-below-windows.md`)
///
/// `margin` is what the shadow wants; the rest says how much room there is. The
/// pass places the window inside its region at the same margin on all four
/// sides, so the want is cut to the distance to each edge of the screen (a
/// window four pixels from the left gets four pixels of shadow) and to the gap
/// to every effect window `below` it. The pass paints the backdrop over the
/// whole of a window's margin, and the backdrop does not hold the effect
/// windows: a margin over one would paint the wallpaper across it. A margin `m`
/// reaches a rectangle `gap` pixels away on both axes, so it is cut to the
/// wider of the two gaps
pub fn margin_for(window: Rect, margin: u32, screen: &Rect, below: &[Rect]) -> i32 {
    let r = window;
    let mut m = (margin as i32)
        .min(r.x0 - screen.x0).min(r.y0 - screen.y0)
        .min(screen.x1 - r.x1).min(screen.y1 - r.y1)
        .max(0);
    for o in below {
        let gx = (o.x0 - r.x1).max(r.x0 - o.x1).max(0);
        let gy = (o.y0 - r.y1).max(r.y0 - o.y1).max(0);
        m = m.min(gx.max(gy));
    }
    m
}

/// What the pass took last frame, with the margin it drew each one with
///
/// Two readers, both on the processor's side, asking what the pass did:
/// - `compose::dressed`: a window the pass took is rounded and shadowed by the
///   pass, and one it refused is the processor's to dress;
/// - the cursor patch, which reproduces the margin the pass drew: the full
///   twenty-four pixels over a screen with room for four would be a dark
///   rectangle dragged along the window's edge
static TAKEN: Mutex<Vec<((u32, u32), i32)>> = Mutex::new(Vec::new());

/// Remember what this frame's pass takes. Called by the frame once it has
/// picked, not by `effect_windows`, which `mayland status` asks as well
pub(super) fn remember_taken(list: &[server::Presentable], picked: &[EffectWin]) {
    set_taken(picked.iter()
        .map(|w| { let p = &list[w.index]; ((p.client, p.surface), w.window.margin as i32) })
        .collect());
}

/// What the pass took, as keys and the margins it drew them with
pub(in crate::mayland) fn set_taken(taken: Vec<((u32, u32), i32)>) {
    *TAKEN.lock() = taken;
}

/// The first of `below` that `r` overlaps: an effect window under it, which a
/// window may not be composed over (`Refusal::Over`)
pub fn first_overlap(below: &[Rect], r: &Rect) -> Option<usize> {
    below.iter().position(|o| o.intersect(r).is_some())
}

/// Whether a piece of the surface at `i` in the list lies over a window the
/// pass composes below it (`picked` as (index, region)), and so is copied after
/// the pass: the pass writes every pixel of its region
pub fn lies_over(i: usize, piece: &Rect, picked: &[(usize, Rect)]) -> bool {
    picked.iter().any(|(j, r)| *j < i && r.intersect(piece).is_some())
}

/// The margin the pass drew this surface with, if it took it
pub(super) fn taken_reach(p: &server::Presentable) -> Option<i32> {
    TAKEN.lock().iter().find(|(k, _)| *k == (p.client, p.surface)).map(|(_, m)| *m)
}

/// The backdrop buffer no longer matches what is under the effect windows
///
/// There is one buffer, not one per scanout surface, so `OWED` has nothing to
/// say about it. Ordinary damage keeps it current: a surface below a window
/// commits, and the pieces under that window go into the backdrop as well as
/// the screen. Ordinary damage cannot fill it the first time, or repair it
/// after something the compositor cannot see as damage. Set this flag, and the
/// next frame lays the whole region down once and clears it.
///
/// Set generously: a needless refill costs one frame of copies, and a stale
/// backdrop shows the wrong picture blurred under a window with nothing
/// repairing it
pub(super) static BACKDROP_DIRTY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(true);

pub(super) fn compute_composes() -> bool {
    use crate::drivers::gpu::nvidia::blur;
    !given_up() && blur::ready()
}

pub fn effects() -> Effects {
    *EFFECTS.lock()
}

/// The widest ring anybody may ask for
///
/// It bounds the copy: the software path reads `(h - 1) * pitch + w * 4` bytes
/// from the source with no check of its own, so the buffer is allocated for
/// this and `size` is clamped to it
pub(super) const MAX_BORDER: i32 = 8;

#[derive(Clone, Copy, PartialEq)]
pub struct Border {
    /// Zero means no borders, as on a machine with no config
    pub size: i32,
    /// The window with the keyboard, and every other window
    pub active: u32,
    pub inactive: u32,
}

impl Border {
    /// No ring at all, as on a machine with no config
    pub const DEFAULT: Border = Border {
        size: 0, active: 0xCFBDFE, inactive: 0x4C3E75,
    };
}

pub static BORDER: Mutex<Border> = Mutex::new(Border::DEFAULT);

pub fn border() -> Border {
    *BORDER.lock()
}

/// Everything this file holds, back to the compositor's own defaults
///
/// The paint buffers are left alone: cached pixels of a colour once asked for,
/// not a setting
pub fn reset() {
    *EFFECTS.lock() = Effects::DEFAULT;
    *BORDER.lock() = Border::DEFAULT;
    force_full_frame();
}

/// What the window manager asked for. The size is clamped, not refused
pub fn set_border(size: i32, active: u32, inactive: u32) {
    let mut b = BORDER.lock();
    let want = Border { size: size.clamp(0, MAX_BORDER), active, inactive };
    if *b == want {
        return;
    }
    *b = want;
    drop(b);
    // Every window's ring changes at once, and there is no list of them here
    force_full_frame();
}

/// One colour, as pixels the copy engine can read
pub(super) struct Paint {
    pub(super) handle: u32,
    pub(super) phys: u64,
    pub(super) colour: u32,
    /// How many pixels of it there are, which is the largest blit it can serve
    pub(super) px: usize,
}

pub(super) static PAINT: Mutex<Vec<Paint>> = Mutex::new(Vec::new());

/// Where a rectangle of this colour can be copied from, allocating or
/// repainting the buffer if this is a colour nobody has asked for yet
///
/// `want` is the most pixels any one band will need - the longer side of the
/// screen times the border - and the buffer is only ever grown
pub(super) fn paint_phys(colour: u32, want: usize) -> Option<u64> {
    let mut pots = PAINT.lock();
    if let Some(i) = pots.iter().position(|p| p.colour == colour) {
        if pots[i].px >= want {
            // To the back, as eviction below takes from the front. Called once
            // per decorated window in the frame, and a plain queue could
            // discard the colour the previous window just asked for
            let p = pots.remove(i);
            let phys = p.phys;
            pots.push(p);
            return Some(phys);
        }
        // Outgrown: release it and make a bigger one
        let _ = shm::destroy(pots[i].handle);
        pots.remove(i);
    }
    // A colour nobody wants, so the desktop does not accumulate a pool per
    // colour. Safe mid-frame though a blit in this frame's batch may name it:
    // `shm::destroy` retires the memory, and the compositor hands it back to
    // the allocator once the engine's fence has been waited on
    while pots.len() >= 4 {
        let old = pots.remove(0);
        let _ = shm::destroy(old.handle);
    }
    let bytes = want * 4;
    let handle = shm::create(0, bytes).ok()?;
    let filled = shm::with_bytes_mut(handle, 0, bytes, |b| {
        let px = (0xFF00_0000u32 | colour).to_le_bytes();
        for p in b.chunks_exact_mut(4) {
            p.copy_from_slice(&px);
        }
    }).is_ok();
    let phys = shm::phys(handle).ok()?;
    if !filled {
        let _ = shm::destroy(handle);
        return None;
    }
    pots.push(Paint { handle, phys, colour, px: want });
    Some(phys)
}

/// What one window asked for, where it asked for anything
///
/// Every field inherits unless told otherwise, as a window rule means:
/// `opacity 0.8` says one thing about one window. Unsetting is the same request
/// with `set` false (`may_wm.set_window_param`)
#[derive(Clone, Copy, Default)]
pub struct Fx {
    pub on: Option<bool>,
    pub alpha: Option<f32>,
    pub corner: Option<f32>,
    pub shadow: Option<f32>,
    pub margin: Option<u32>,
    pub radius: Option<f32>,
    pub power: Option<f32>,
    pub inactive: Option<f32>,
    pub dim: Option<f32>,
}

impl Fx {
    /// This window's answer, laid over the desktop's
    pub fn over(&self, base: Effects) -> Effects {
        Effects {
            on: self.on.unwrap_or(base.on),
            alpha: self.alpha.unwrap_or(base.alpha),
            corner: self.corner.unwrap_or(base.corner),
            shadow: self.shadow.unwrap_or(base.shadow),
            margin: self.margin.unwrap_or(base.margin),
            radius: self.radius.unwrap_or(base.radius),
            power: self.power.unwrap_or(base.power),
            inactive: self.inactive.unwrap_or(base.inactive),
            dim: self.dim.unwrap_or(base.dim),
        }
    }

    /// Whether this window has been told anything at all
    pub fn any(&self) -> bool {
        self.on.is_some() || self.alpha.is_some() || self.corner.is_some()
            || self.shadow.is_some() || self.margin.is_some() || self.radius.is_some()
            || self.power.is_some() || self.inactive.is_some() || self.dim.is_some()
    }

    /// Set or clear one of them, by the protocol's own numbering
    ///
    /// The only place param ids and effect fields are written together: a new
    /// decoration knob is a constant in `protocol.rs`, a field above, and one
    /// arm here
    pub fn set(&mut self, param: u32, value: f32, set: bool) -> bool {
        use crate::mayland::protocol::wm::param;
        match param {
            param::EFFECTS     => self.on     = set.then(|| value != 0.0),
            param::ALPHA       => self.alpha  = set.then_some(value.clamp(0.0, 1.0)),
            param::ROUNDING    => self.corner = set.then_some(value.max(0.0)),
            param::SHADOW      => self.shadow = set.then_some(value.clamp(0.0, 1.0)),
            param::SHADOW_SIZE => self.margin = set.then_some(value.max(0.0) as u32),
            param::BLUR_SIZE   => self.radius = set.then_some(value.max(0.0)),
            // Not clamped, as a Hyprland rule's is not
            param::ROUNDING_POWER => self.power = set.then_some(value.max(1.0)),
            param::INACTIVE_ALPHA => self.inactive = set.then_some(value.clamp(0.0, 1.0)),
            param::DIM            => self.dim = set.then_some(value.clamp(0.0, 1.0)),
            _ => return false,
        }
        true
    }
}

/// Set one of the desktop's own numbers, by the same numbering
///
/// The mirror of `Fx::set`: what the window manager can say about one window it
/// can say about the desktop. `false` means the param is not one of these, and
/// the caller offers it elsewhere: the layout owns the gaps
pub fn set_param(param: u32, value: f32) -> bool {
    use crate::mayland::protocol::wm::param;
    let mut e = EFFECTS.lock();
    match param {
        param::EFFECTS     => e.on     = value != 0.0,
        param::ALPHA       => e.alpha  = value.clamp(0.0, 1.0),
        param::ROUNDING    => e.corner = value.max(0.0),
        param::SHADOW      => e.shadow = value.clamp(0.0, 1.0),
        param::SHADOW_SIZE => e.margin = value.max(0.0) as u32,
        param::BLUR_SIZE   => e.radius = value.max(0.0),
        // Hyprland clamps the config's to 1..10, and a rule's not at all
        param::ROUNDING_POWER => e.power = value.clamp(1.0, 10.0),
        param::INACTIVE_ALPHA => e.inactive = value.clamp(0.0, 1.0),
        param::DIM            => e.dim = value.clamp(0.0, 1.0),
        _ => return false,
    }
    drop(e);
    // Each of these changes what is on the screen everywhere, and none damages
    // anything by being changed
    force_full_frame();
    true
}

/// A window this frame draws with the GPU instead of copying
pub(in crate::mayland) struct EffectWin {
    /// Where it sits in the presentable list, so the copy pass can skip it
    pub(in crate::mayland) index: usize,
    /// The window plus its shadow margin: what the pass writes, and what has to
    /// be in the backdrop first
    pub(in crate::mayland) rect: Rect,
    pub(in crate::mayland) window: crate::drivers::gpu::nvidia::blur::Window,
}

impl EffectWin {
    /// What this frame's launch covers: the clip, or the whole region
    pub(in crate::mayland) fn clip_rect(&self) -> Rect {
        let (x, y, w, h) = self.window.clip;
        if w == 0 || h == 0 { self.rect } else { Rect::new(x as i32, y as i32, w as i32, h as i32) }
    }

    /// Grow the clip to take in what of `other` lies on this window's region.
    /// Whether that changed it
    fn take_in(&mut self, other: &Rect) -> bool {
        let Some(meet) = other.intersect(&self.rect) else { return false };
        let was = self.clip_rect();
        let grown = was.union(&meet);
        if grown == was {
            return false;
        }
        self.window.clip = (grown.x0 as u32, grown.y0 as u32,
                            (grown.x1 - grown.x0) as u32, (grown.y1 - grown.y0) as u32);
        true
    }
}

/// Settle the clips of the windows the pass composes, until they agree
///
/// Two overlapping windows can spoil each other two ways, each making one redo
/// more (`dev/mayland-layers-above.md`, section 3):
/// - the upper one reads the frame under it (`blur::Window::stacked`), so the
///   lower one recomposes where the upper one's clip meets it, or the upper one
///   reads last frame's result, itself included, and darkens a little each
///   frame;
/// - the lower one writes every pixel of its clip, so the upper one recomposes
///   where the lower one's clip meets it, or it is painted over until it next
///   redraws.
///
/// A clip is one box, so taking a piece in can take in more than the piece and
/// reach a third window: they go round until nothing moves. A box only grows,
/// and only as far as its window's region, so this ends
pub(in crate::mayland) fn settle_stacked_clips(effects: &mut [EffectWin]) {
    let mut moved = true;
    while moved {
        moved = false;
        for i in (1..effects.len()).rev() {
            for j in 0..i {
                let up = effects[i].clip_rect();
                moved |= effects[j].take_in(&up);
                let low = effects[j].clip_rect();
                moved |= effects[i].take_in(&low);
            }
        }
    }
}

/// Pick the windows that get effects
///
/// Every window that asked for them and may have them. `Presentable.fx` is what
/// the window manager said about this window laid over the desktop's settings,
/// and `decorated` is the compositor's veto: the backdrop and a fullscreen
/// window are refused, as a window with nothing behind it has nothing to blur
/// and a full-screen one is fed its own output
pub(super) fn effect_windows(
    list: &[server::Presentable],
    screen: &Rect,
    trans: &[usize],
) -> Vec<EffectWin> {
    let mut refused: Vec<(usize, Refusal)> = Vec::new();
    let out = pick(list, screen, trans, &mut refused);
    note_refusals(list, trans, &refused);
    out
}

/// Why a surface did not get into the pass. Reported only for a see-through
/// one: an opaque window refused is a copy at no cost, and a see-through one is
/// blended pixel by pixel on the processor (a console fading in costs 1.9 Mpx
/// and 40 ms a frame)
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::mayland) enum Refusal {
    /// `give_up_on_compute` took the engine out of the frame
    GivenUp,
    /// The backdrop, or a full-screen surface (`composed` is false)
    NotComposed,
    /// No effects asked for and nothing fading: the bar
    Plain,
    /// It lies over a see-through surface: client, surface
    Under(u32, u32),
    /// It overlaps a window the pass composes below it, and between the two
    /// lies a surface the pass does not draw, which would land on top: that
    /// surface's client and surface
    Between(u32, u32),
    /// None of it is on the screen
    OffScreen,
    /// It covers the whole screen
    CoversScreen,
    /// The pass already has `blur::MAX_WINDOWS`
    Full,
}

impl Refusal {
    fn say(self) -> alloc::string::String {
        match self {
            Refusal::GivenUp => "the compute engine is out of the frame".into(),
            Refusal::NotComposed => "it is the backdrop or full-screen".into(),
            Refusal::Plain => "it asks the pass for nothing - no effects and no fade".into(),
            Refusal::Under(c, s) => alloc::format!(
                "it lies over client {} surface {}, which is see-through, and steps aside", c, s),
            Refusal::Between(c, s) => alloc::format!(
                "it is stacked over a window the pass composes, and client {} surface {} lies between \
                 them, drawn after the pass", c, s),
            Refusal::OffScreen => "none of it is on the screen".into(),
            Refusal::CoversScreen => "it covers the whole screen".into(),
            Refusal::Full => "the pass already has as many windows as it takes".into(),
        }
    }
}

/// What was reported last, per surface, so a reason is reported once when it
/// starts
static REFUSED: Mutex<Vec<((u32, u32), Refusal)>> = Mutex::new(Vec::new());

/// Report, once, each see-through surface the processor now blends and why
fn note_refusals(list: &[server::Presentable], trans: &[usize], refused: &[(usize, Refusal)]) {
    let mut was = REFUSED.lock();
    let mut now: Vec<((u32, u32), Refusal)> = Vec::new();
    for &(i, why) in refused {
        if !trans.contains(&i) {
            continue;
        }
        let p = &list[i];
        let key = (p.client, p.surface);
        now.push((key, why));
        if !was.contains(&(key, why)) {
            crate::kdebug!(
                "[mayland] client {} surface {} ({}x{} at {},{}) is blended by the processor, \
                 not the compute pass: {}",
                p.client, p.surface, p.width, p.height, p.offset.0, p.offset.1, why.say());
        }
    }
    *was = now;
}

fn pick(
    list: &[server::Presentable],
    screen: &Rect,
    trans: &[usize],
    refused: &mut Vec<(usize, Refusal)>,
) -> Vec<EffectWin> {
    use crate::drivers::gpu::nvidia::blur;

    // Given up on, not merely turned off. `EFFECTS.on` is a preference: a
    // fading or scaling window takes this path regardless, since a fade is the
    // pass. A session logging in has every window animating in at once, so the
    // frame after the engines failed would send them all straight back at it.
    //
    // No pass at all is the processor's job by design and reports nothing. One
    // given up on is a fault, and reports it
    if given_up() {
        refused.extend(trans.iter().map(|&t| (t, Refusal::GivenUp)));
    }
    if given_up() || !blur::ready() || list.len() < 2 {
        return Vec::new();
    }
    choose(list, screen, trans, refused)
}

/// `pick` once the pass is there to take anything: what it takes, what it
/// stacks, and why the rest were refused. A function of the list alone, so a
/// test can put a desktop to it with no card
pub(in crate::mayland) fn choose(
    list: &[server::Presentable],
    screen: &Rect,
    trans: &[usize],
    refused: &mut Vec<(usize, Refusal)>,
) -> Vec<EffectWin> {
    use crate::drivers::gpu::nvidia::blur;

    let mut out: Vec<EffectWin> = Vec::new();
    // The bottom of the list is what everything above is composed over, so it
    // is not composed itself: nothing is under it to read
    for (i, p) in list.iter().enumerate().skip(1) {
        if out.len() >= blur::MAX_WINDOWS {
            refused.push((i, Refusal::Full));
            continue;
        }
        let e = p.fx;
        // Composed is the question here, not decorated. A panel is not
        // decorated, but alpha is not decoration, and a surface not reaching
        // this pass could not fade
        if !p.composed {
            refused.push((i, Refusal::NotComposed));
            continue;
        }
        // Something has to be worth doing: the effects on, or an animation
        // asking for an alpha that is not 1
        let fading = e.alpha < 0.999 || p.scale < 0.999;
        if !e.on && !fading {
            refused.push((i, Refusal::Plain));
            continue;
        }
        let r = Rect::new(p.offset.0, p.offset.1, p.width, p.height);
        // Not over a see-through surface
        //
        // Two passes want the same pixels: this one is submitted after the copy
        // batch and writes its whole rectangle, and `render::compose` composes
        // a rectangle in RAM and copies it once. The later would paint over the
        // other.
        //
        // The order decides who gives way. A see-through surface above the
        // window is composed by the processor over everything under it, this
        // window included, and its rectangle goes on after the pass (`present`,
        // the batch with the rings and the cursor), so it lands on top. One
        // below the window is the other case: the pass blends over the
        // backdrop, and a processor-blended rectangle never reaches the
        // backdrop, so the window would be blended over what was there before
        // it. There the window steps aside.
        //
        // `t < i` makes this a test about other surfaces underneath: otherwise
        // every translucent surface (an ARGB8888 panel, any window
        // mid-animation) would intersect itself and be refused its own effects.
        //
        // One the pass draws is not in the way: this one is stacked over it,
        // and composed over what it wrote (below, and
        // `dev/mayland-layers-above.md`, section 3)
        if let Some(&t) = trans.iter().find(|&&t| {
            t < i && !out.iter().any(|w| w.index == t) && {
                let o = &list[t];
                Rect::new(o.offset.0, o.offset.1, o.width, o.height)
                    .intersect(&r).is_some()
            }
        }) {
            refused.push((i, Refusal::Under(list[t].client, list[t].surface)));
            continue;
        }
        // Some of it on the screen. A window part way past an edge (sliding in
        // with a workspace) is the pass's for the part on it: the region is cut
        // to the screen and the compose is told where the window's corner lies
        // outside it (`blur::submit`)
        if r.intersect(screen).is_none() {
            refused.push((i, Refusal::OffScreen));
            continue;
        }
        // Covering the screen means being the backdrop, whatever it was told
        if r.x0 <= screen.x0 && r.y0 <= screen.y0
            && r.x1 >= screen.x1 && r.y1 >= screen.y1 {
            refused.push((i, Refusal::CoversScreen));
            continue;
        }
        // The windows the pass composes below it, for its margin. Overlapping
        // one is not a refusal: it is stacked, further down
        let below: Vec<Rect> = out.iter()
            .map(|w| { let o = &list[w.index]; Rect::new(o.offset.0, o.offset.1, o.width, o.height) })
            .collect();
        // A surface here only to fade gets no blur, no corner and no shadow, as
        // a panel must not
        let plain = !e.on || !p.decorated;
        let (radius, corner, shadow, margin) = if plain {
            (0.0, 0.0, 0.0, 0)
        } else {
            (e.radius, e.corner, e.shadow, e.margin)
        };
        // A surface with nothing to do in this pass does not enter it.
        //
        // `plain` means no blur, no corner and no shadow, right for a panel. A
        // plain surface not fading belongs on the copy path.
        //
        // See-through is a reason of its own. A plain see-through surface
        // outside the pass is blended by the processor, pixel by pixel, every
        // frame anything under it changes: a megapixel a frame during the
        // overview's animation. The pass lays it over in order, as Hyprland's
        // GL does
        if plain && !fading && !trans.contains(&i) {
            refused.push((i, Refusal::Plain));
            continue;
        }
        // The border is the pass's to draw for a window it takes, over the
        // window, in the colour of whether it is the active one
        // (`dev/mayland-window-shape.md`)
        let bord = border();
        let ring = if plain { 0 } else { bord.size.max(0) };
        let border_colour = if server::active_key() == Some((p.client, p.surface)) {
            bord.active
        } else {
            bord.inactive
        };
        // The margin the pass draws it with: its shadow or its border,
        // whichever reaches further, cut to the screen and to the effect
        // windows below it. The cursor patch reads the number back
        // (`taken_reach`)
        let m = margin_for(r, margin.max(ring as u32), screen, &below) as u32;
        // The region on the screen: past an edge there is nothing to draw
        let Some(rect) = Rect::new(r.x0 - m as i32, r.y0 - m as i32,
                                   p.width + 2 * m as i32, p.height + 2 * m as i32)
            .intersect(screen) else { continue };
        // Over an effect window below it: stacked, composed over what that one
        // wrote, not over the backdrop. Unless something the pass does not draw
        // lies between the two and over this one (a see-through surface the
        // processor blends, an opaque one copied after the pass): laid on after
        // the pass, it would land on top of this window
        let lowest = out.iter().filter(|w| w.rect.intersect(&rect).is_some()).map(|w| w.index).min();
        if let Some(lowest) = lowest {
            let between = (lowest + 1..i).find(|&k| {
                !out.iter().any(|w| w.index == k) && {
                    let o = &list[k];
                    Rect::new(o.offset.0, o.offset.1, o.width, o.height).intersect(&rect).is_some()
                }
            });
            if let Some(k) = between {
                refused.push((i, Refusal::Between(list[k].client, list[k].surface)));
                continue;
            }
        }
        out.push(EffectWin {
            index: i,
            rect,
            window: blur::Window {
                x: r.x0,
                y: r.y0,
                w: p.width as u32,
                h: p.height as u32,
                // The compositor's copy in video memory when there is one
                // (`texture`), the client's buffer when not; same stride
                src_phys: p.gpu.unwrap_or(p.phys),
                src_stride: p.stride as u32,
                src_vram: p.gpu.is_some(),
                // As `input::pointer` asks of a cursor image: XRGB8888 has a
                // padding byte, not an alpha channel, and clients leave it at
                // zero
                src_has_alpha: p.format != crate::mayland::protocol::shm::FORMAT_XRGB8888,
                alpha: e.alpha,
                corner,
                shadow,
                margin: m,
                // Over the blurred shell below the windows, if it asked for a
                // blur; a window only fading is blended over the sharp backdrop
                blur: radius > 0.0,
                // The whole region until `present` knows what changed, after
                // the debt is drained, as the debt is part of what this frame
                // redoes
                clip: (0, 0, 0, 0),
                scale: p.scale,
                power: e.power,
                dim: if plain { 0.0 } else { e.dim },
                border_colour,
                border: ring as f32,
                stacked: lowest.is_some(),
            },
        });
    }
    out
}
