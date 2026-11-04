// The shell below the windows, as one surface
//
// The backdrop and every layer in BACKGROUND and BOTTOM are drawn before any
// window and never move with them. Hyprland draws them first, then blurs what
// it has so far once into `m_blurFB`, and every window with `xray` reads that
// (`renderWorkspace`, `preBlurForCurrentMonitor`). This is the same for a
// compositor whose engines copy and compose: the run of such surfaces at the
// bottom of the frame's list, the members, is kept as one screen-sized image in
// video memory, brought up to date on the compute engine only where a member
// changed, and the members are replaced in the frame's list by one opaque
// surface naming it.
//
// Without this, one see-through full-screen layer (negi's background) puts the
// whole frame on the processor and makes every window step out of the effects
// pass (`dev/mayland-below-windows.md`).
//
// The folded surface is client 0, surface 0, which no real surface is: the
// server numbers clients from 1 and ghosts from 1 << 31. It is in video memory,
// so the processor's few readers of it go through the mirrors `texture` keeps
// for any such window (`dev/gpu-window-buffers.md`).
//
// Nothing is folded unless the compute pass is alive. QEMU, an engine given up
// on, or a card with no memory for the image draws the frame as before.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

use crate::drivers::gpu::nvidia::{blur, scanout};
use crate::mayland::protocol;
use crate::mayland::server;

use super::damage::{Rect, force_full_frame, push_rect};

/// The folded surface's key, as `(client, surface)`
pub const KEY: (u32, u32) = (0, 0);

/// What a member was last frame: enough to tell what it owes the image
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Member {
    pub key: (u32, u32),
    pub rect: Rect,
    pub alpha: f32,
    pub format: u32,
}

impl Member {
    pub fn of(p: &server::Presentable) -> Member {
        Member {
            key: (p.client, p.surface),
            rect: Rect::new(p.offset.0, p.offset.1, p.width, p.height),
            alpha: p.fx.alpha,
            format: p.format,
        }
    }
}

struct State {
    /// The members the image holds, bottom first. Empty while nothing is
    /// folded; the first fold then starts from the whole screen
    members: Vec<Member>,
    /// The screen it was built for
    size: (i32, i32),
    /// The radius `blurred` holds the image at, if it holds it at all
    blurred: Option<f32>,
}

static STATE: Mutex<State> = Mutex::new(State { members: Vec::new(), size: (0, 0), blurred: None });

/// For `mayland status`
static FOLDED: AtomicU64 = AtomicU64::new(0);
static LAST_PX: AtomicU64 = AtomicU64::new(0);
static UPDATES: AtomicU64 = AtomicU64::new(0);
static BLURS: AtomicU64 = AtomicU64::new(0);
static REFUSED: AtomicU64 = AtomicU64::new(0);
static LAST_REFUSAL: Mutex<&'static str> = Mutex::new("");

#[derive(Clone, Copy, Default)]
pub struct Counts {
    /// Surfaces folded into the image on the last frame, 0 when none were
    pub folded: u64,
    /// Pixels the image was redrawn over, the last time anything changed
    pub last_px: u64,
    /// Times it was brought up to date, and times it was blurred
    pub updates: u64,
    pub blurs: u64,
    /// Times an update could not be made, and the last reason
    pub refused: u64,
    pub why: &'static str,
}

pub fn counts() -> Counts {
    Counts {
        folded: FOLDED.load(Ordering::Relaxed),
        last_px: LAST_PX.load(Ordering::Relaxed),
        updates: UPDATES.load(Ordering::Relaxed),
        blurs: BLURS.load(Ordering::Relaxed),
        refused: REFUSED.load(Ordering::Relaxed),
        why: *LAST_REFUSAL.lock(),
    }
}

/// How many surfaces at the bottom of `list` are the shell below the windows
///
/// A member drawn at less than its own size (a `popin` on a layer) is not one:
/// the image lays a surface one pixel to one, and the frame draws that curve.
/// Nor is anything past `MAX_BASE_LAYERS`, what one update can take; what is
/// left above the run is drawn normally
pub fn members(list: &[server::Presentable]) -> usize {
    list.iter()
        .take(blur::MAX_BASE_LAYERS)
        .take_while(|p| p.below && p.scale >= 0.999 && p.width > 0 && p.height > 0)
        .count()
}

/// What the image owes for the members on the list, against the ones it holds:
/// where a member came, went, moved, changed its alpha or format, or moved in
/// the stack; what a still one damaged; all of one with a curve running. In
/// screen coordinates, merged as the frame merges damage
pub fn owed(was: &[Member], now: &[server::Presentable], screen: &Rect) -> Vec<Rect> {
    let mut out: Vec<Rect> = Vec::new();
    let mut full = false;
    for (i, m) in was.iter().enumerate() {
        if !now.iter().any(|p| (p.client, p.surface) == m.key) {
            push_rect(&mut out, m.rect, screen, &mut full);
        } else if now.get(i).map(|p| (p.client, p.surface)) != Some(m.key) {
            // Still here, at another place in the run
            push_rect(&mut out, m.rect, screen, &mut full);
        }
    }
    for (i, p) in now.iter().enumerate() {
        let m = Member::of(p);
        match was.iter().position(|w| w.key == m.key) {
            None => push_rect(&mut out, m.rect, screen, &mut full),
            Some(j) if j != i || was[j] != m || p.animating => {
                push_rect(&mut out, was[j].rect, screen, &mut full);
                push_rect(&mut out, m.rect, screen, &mut full);
            }
            Some(_) => {
                for &(dx, dy, dw, dh) in &p.damage {
                    let d = Rect::new(m.rect.x0 + dx, m.rect.y0 + dy, dw, dh);
                    if let Some(d) = d.intersect(&m.rect) {
                        push_rect(&mut out, d, screen, &mut full);
                    }
                }
            }
        }
    }
    if full {
        out.clear();
        out.push(*screen);
    }
    out
}

/// The surface the members are replaced with: opaque, the size of the screen,
/// read where it lies. `damage` is in screen coordinates, which are its own
pub fn folded(vram: u64, pitch: u32, screen: &Rect, damage: &[Rect],
              fx: super::Effects) -> server::Presentable {
    server::Presentable {
        client: KEY.0,
        surface: KEY.1,
        phys: 0,
        vram: Some(vram),
        gpu: Some(vram),
        from: None,
        width: screen.x1 - screen.x0,
        height: screen.y1 - screen.y0,
        stride: pitch as usize,
        format: protocol::shm::FORMAT_XRGB8888,
        offset: (screen.x0, screen.y0),
        damage: damage.iter()
            .map(|r| (r.x0 - screen.x0, r.y0 - screen.y0, r.x1 - r.x0, r.y1 - r.y0))
            .collect(),
        // Drawn as it is: the backdrop takes no alpha, no corner and no blur
        fx: super::Effects { on: false, alpha: 1.0, ..fx },
        decorated: false,
        composed: false,
        scale: 1.0,
        animating: false,
        below: true,
    }
}

/// Nothing is folded any more: the image is not what the screen shows, and the
/// next fold starts from the whole screen
fn unfold() {
    let mut st = STATE.lock();
    if st.members.is_empty() && st.blurred.is_none() {
        return;
    }
    st.members.clear();
    st.blurred = None;
    drop(st);
    blur::forget_blurred();
    super::texture::forget_mirror(KEY);
    FOLDED.store(0, Ordering::Relaxed);
}

/// A frame that dropped the damage without composing: `present` returning
/// before the fold. What the members changed is gone, so the image is no longer
/// theirs, as `texture::missed_frame` makes every copy whole again
pub(super) fn missed_frame() {
    unfold();
}

fn refuse(why: &'static str) {
    REFUSED.fetch_add(1, Ordering::Relaxed);
    let mut last = LAST_REFUSAL.lock();
    if *last != why {
        crate::kwarn!("[mayland] the shell below the windows is composed as separate surfaces this frame: {}", why);
        *last = why;
    }
}

/// Fold the shell below the windows into one surface for this frame, bringing
/// the image up to date first. After `texture::sync`, which keeps the members'
/// own copies current, and before anything decides what blends
pub(super) fn fold_for_frame(list: &mut Vec<server::Presentable>, screen: &Rect) {
    if super::software() || !super::effects::compute_composes() {
        unfold();
        return;
    }
    let k = members(list);
    let Some((vram, pitch)) = scanout::base_surface().filter(|_| k > 0) else {
        unfold();
        return;
    };
    let size = (screen.x1 - screen.x0, screen.y1 - screen.y0);
    let fx = super::effects();
    let want_blur = (fx.on && fx.radius > 0.0).then_some(fx.radius);

    let (damage, blur_now) = {
        let st = STATE.lock();
        let damage = if st.members.is_empty() || st.size != size {
            alloc::vec![*screen]
        } else {
            owed(&st.members, &list[..k], screen)
        };
        // Blurred again when the image changed under it, or when the radius it
        // was blurred at is not the one asked for
        let blur_now = want_blur.filter(|r| !damage.is_empty() || st.blurred != Some(*r));
        (damage, blur_now)
    };

    if !damage.is_empty() || blur_now.is_some() {
        if let Err(why) = update(&list[..k], &damage, screen, blur_now) {
            refuse(why);
            unfold();
            force_full_frame();
            return;
        }
        let mut st = STATE.lock();
        st.members = list[..k].iter().map(Member::of).collect();
        st.size = size;
        // An update that did not blur leaves `blurred` stale when it changed
        // the image (`blur::submit_base`), and an unchanged image keeps its
        // blur
        st.blurred = match blur_now {
            Some(r) => Some(r),
            None if damage.is_empty() => st.blurred,
            None => None,
        };
    }

    // What the windows have to be composed again over. The blur spreads a
    // change past where it was made, as far as the chain's taps reach, and a
    // window over that band would keep the old blur until it next redrew; a new
    // radius changes all of it
    let damage = match blur_now {
        Some(_) if damage.is_empty() => alloc::vec![*screen],
        Some(r) => {
            let reach = (r * (1u32 << blur::PASSES) as f32) as i32 + 1;
            let mut out: Vec<Rect> = Vec::with_capacity(damage.len());
            let mut full = false;
            for d in &damage {
                push_rect(&mut out, d.outset(reach), screen, &mut full);
            }
            if full { alloc::vec![*screen] } else { out }
        }
        None => damage,
    };

    let surface = folded(vram, pitch, screen, &damage, fx);
    list.splice(0..k, core::iter::once(surface));
    FOLDED.store(k as u64, Ordering::Relaxed);
}

/// The members as the effects pass describes a surface it reads: where each
/// one's top-left is on the screen, and where its pixels are
fn layer_of(p: &server::Presentable) -> blur::Window {
    blur::Window {
        x: p.offset.0,
        y: p.offset.1,
        w: p.width as u32,
        h: p.height as u32,
        // The compositor's copy when there is one, the client's buffer when
        // not, as on every engine path
        src_phys: p.gpu.unwrap_or(p.phys),
        src_stride: p.stride as u32,
        src_vram: p.gpu.is_some(),
        src_has_alpha: p.format != protocol::shm::FORMAT_XRGB8888,
        alpha: p.fx.alpha.clamp(0.0, 1.0),
        corner: 0.0,
        shadow: 0.0,
        margin: 0,
        blur: false,
        clip: (0, 0, 0, 0),
        scale: 1.0,
        power: 2.0,
        dim: 0.0,
        border_colour: 0,
        border: 0.0,
        stacked: false,
    }
}

/// The work of one update, as `blur::submit_base` takes it: the rectangle
/// around everything owed, cleared, then every member laid over it bottom first
/// where it covers it
pub fn ops_for(members: &[server::Presentable], damage: &[Rect], screen: &Rect) -> Vec<blur::BaseOp> {
    let mut ops = Vec::with_capacity(members.len() + 1);
    let Some(first) = damage.first() else { return ops };
    let around = damage.iter().skip(1).fold(*first, |a, r| a.union(r));
    let Some(around) = around.intersect(screen) else { return ops };
    let op = |r: &Rect, layer: Option<usize>, alpha: f32| blur::BaseOp {
        x: (r.x0 - screen.x0) as u32,
        y: (r.y0 - screen.y0) as u32,
        w: (r.x1 - r.x0) as u32,
        h: (r.y1 - r.y0) as u32,
        layer,
        alpha,
    };
    ops.push(op(&around, None, 1.0));
    for (i, p) in members.iter().enumerate() {
        let r = Rect::new(p.offset.0, p.offset.1, p.width, p.height);
        if let Some(hit) = r.intersect(&around) {
            ops.push(op(&hit, Some(i), p.fx.alpha.clamp(0.0, 1.0)));
        }
    }
    ops
}

/// Bring the image up to date for `damage`, and blur it if asked
fn update(members: &[server::Presentable], damage: &[Rect], screen: &Rect,
          blur: Option<f32>) -> Result<(), &'static str> {
    let layers: Vec<blur::Window> = members.iter().map(layer_of).collect();
    // Mapped before anything is queued on an engine, while a TLB invalidate
    // costs microseconds (`scanout::base_map`)
    if !scanout::base_map(&layers) {
        return Err("a surface below the windows could not be mapped for the compute engine");
    }
    // The members' copies are what the chain reads, uploaded on the copy
    // engine; the chain waits for that engine before it starts. This frame's
    // uploads go now, so the frame's copies find nothing left to send.
    //
    // A refused upload leaves a member's copy older than its buffer. The copy
    // is made whole next frame, but this image would keep what it laid from the
    // old one, so the image starts again from the whole screen too
    if !super::texture::submit() {
        return Err("a surface below the windows could not be uploaded to its copy");
    }
    let ops = ops_for(members, damage, screen);
    if ops.is_empty() && blur.is_none() {
        return Ok(());
    }
    scanout::base_compose(&layers, &ops, blur)?;
    UPDATES.fetch_add(1, Ordering::Relaxed);
    if blur.is_some() {
        BLURS.fetch_add(1, Ordering::Relaxed);
    }
    if let Some(o) = ops.first() {
        LAST_PX.store(o.w as u64 * o.h as u64, Ordering::Relaxed);
    }
    Ok(())
}

/// The same fold for a look at the stack outside the frame (`mayland status`),
/// without touching the image: what the last frame folded, if still what the
/// list has at the bottom
pub(super) fn fold_for_status(list: &mut Vec<server::Presentable>, screen: &Rect) {
    let st = STATE.lock();
    let k = members(list);
    if st.members.is_empty() || k != st.members.len()
        || list[..k].iter().zip(st.members.iter()).any(|(p, m)| (p.client, p.surface) != m.key)
    {
        return;
    }
    drop(st);
    let Some((vram, pitch)) = scanout::base_surface() else { return };
    let surface = folded(vram, pitch, screen, &[], super::effects());
    list.splice(0..k, core::iter::once(surface));
}
