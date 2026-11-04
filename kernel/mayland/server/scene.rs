// What the compositor reads
//
// The frame loop must not need the server's lock to find out how a window wants
// to be drawn, so each question is answered here once and handed over
// flattened: where each surface's pixels are in physical memory, how big they
// are, what it declared damaged, and how it is decorated.
//
// It is a snapshot. The compositor gets a `Vec` it owns, the lock is dropped,
// and the frame is composed against a picture of the desktop that cannot change
// underneath it, so a compositor thread and a client thread can run at the same
// time.

use alloc::vec::Vec;

use super::{shm};
use super::state::*;

/// Where a surface is on the screen
///
/// A function because everything that draws or hit-tests has to agree on it
#[inline]
pub(in crate::mayland) fn placed(surf: &Surface) -> (i32, i32) {
    surf.pos
}

/// How big a surface is on the screen, from the buffer it is showing
///
/// A surface with no buffer has no size rather than a size of zero: "do not
/// place it", not "place it at nothing"
pub(in crate::mayland) fn size_of(s: &Server, si: usize) -> Option<(i32, i32)> {
    let surf = &s.surfaces[si];
    let bid = surf.current.buffer.or(surf.pending.buffer)?;
    s.buffers.iter()
        .find(|b| b.client == surf.client && b.id == bid)
        .map(|b| (b.width, b.height))
}

/// Where a surface's pixels are and how it wants to be drawn
///
/// One caller: `presentable` builds the frame from this, and the patch under
/// the cursor is composed from that same list by
/// `render::compose::compose_into`
fn pixels_of(s: &Server, surf: &Surface, base_fx: crate::mayland::render::Effects)
    -> Option<(u64, Option<u64>, i32, i32, usize, u32, crate::mayland::render::Effects)>
{
    // A ghost has no client and no buffer object: it is a copy the compositor
    // owns, drawn from that until its curve runs out
    let (phys, vram, width, height, stride, format) = match surf.ghost {
        Some(g) => (g.phys, None, g.width, g.height, g.stride, g.format),
        None => {
            let bid = surf.current.buffer?;
            let b = s.buffers.iter()
                .find(|b| b.client == surf.client && b.id == bid)?;
            // Which memory the buffer is in decides who may read it: system
            // memory is read by everyone, video memory by the engines alone
            // (`dev/gpu-window-buffers.md`)
            let (phys, vram) = match shm::location(b.pool).ok()? {
                (addr, shm::Memory::System) => (addr + b.offset as u64, None),
                (addr, shm::Memory::Video) => (0, Some(addr + b.offset as u64)),
            };
            (phys, vram, b.width, b.height, b.stride, b.format)
        }
    };
    let mut fx = surf.fx.over(base_fx);
    // Which of the two opacities, and whether it is dimmed: `alpha` is the
    // active window's and `inactive` every other window's, and only those
    // others are dimmed (Hyprland's `updateDynamicRules`,
    // `dev/mayland-window-shape.md`). Windows only: a panel or a popup keeps
    // the active numbers. So does the backdrop, and a fullscreen window, which
    // Hyprland draws at its own `fullscreen_opacity` whatever has the focus
    let other = surf.role == Role::Toplevel && !surf.backdrop && !surf.fullscreen
        && s.active != Some((surf.client, surf.id));
    if other {
        fx.alpha = fx.inactive;
    } else {
        fx.dim = 0.0;
    }
    // The animation multiplies: a window a rule put at 80% opacity fades in to
    // 80%, not to 100%
    fx.alpha *= surf.anim.alpha;
    Some((phys, vram, width, height, stride, format, fx))
}

/// One committed surface, flattened for the compositor
pub struct Presentable {
    pub client: u32,
    pub surface: u32,
    /// Physical address of the first pixel in system memory. Contiguous, so the
    /// copy engine can take it as is, and read by every path the processor
    /// draws. 0 for a window whose buffer is video memory (`vram`) until the
    /// processor needs it: then it is that window's mirror, which
    /// `render::texture::mirror` fills with what the processor reads
    pub phys: u64,
    /// The client's own buffer, when it is video memory: a `gpu_alloc` pool
    /// (`dev/gpu-window-buffers.md`). The engines read it in place; the
    /// processor never does
    pub vram: Option<u64>,
    /// Where the engines read the pixels in the card's memory, with the same
    /// stride: the client's buffer when it is there (`vram`), else the
    /// compositor's copy, filled in by `render::texture::sync` for the frame
    /// that is about to use it (`dev/mayland-architecture.md`, "Window pixels
    /// the GPU reads live in video memory")
    pub gpu: Option<u64>,
    /// The window a ghost is the afterimage of, so it can take that window's
    /// copy rather than upload its own
    pub from: Option<(u32, u32)>,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub format: u32,
    pub offset: (i32, i32),
    /// What the client says changed since the last frame, in surface
    /// coordinates. Empty means the pixels are the ones last copied, which is
    /// not "do not draw it": something above may have moved away and revealed
    /// more of it
    pub damage: Vec<(i32, i32, i32, i32)>,
    /// How this window wants to be drawn: the desktop's numbers with whatever
    /// the window manager said about this one laid over them
    pub fx: crate::mayland::render::Effects,
    /// Whether it may have decoration at all
    ///
    /// The backdrop and a fullscreen window may not: a window with nothing
    /// behind it has nothing to blur, and the blur reads the surface it writes,
    /// so a full-screen one is fed its own output and the desktop turns to fog
    /// within seconds
    pub decorated: bool,
    /// Whether it may go through the shader pass at all
    ///
    /// Separate from `decorated` so that a panel can fade. A layer surface is
    /// not decorated (blur and a rounded corner on the bar would land on the
    /// top of the screen), but alpha is not decoration, and a surface that
    /// cannot be composed cannot be faded
    pub composed: bool,
    /// What fraction of itself its pixels are read from, which a `popin` moves.
    /// 1 unless a curve is running
    pub scale: f32,
    /// Whether a curve is running on this surface
    ///
    /// Differs from `scale < 1` or `alpha < 1` by one frame: the last step of a
    /// curve sets both to their final values and stops the curve, so the frame
    /// drawing the window at full size looks like one never animated. This says
    /// it was moving, so that frame is repainted. See `damage::ANIMATED`
    pub animating: bool,
    /// Part of the shell below the windows: the backdrop, or a layer in
    /// BACKGROUND or BOTTOM. The bottom run of these is folded into one surface
    /// on the GPU path (`render::base`)
    pub below: bool,
}

/// How many windows somebody opened
///
/// The backdrop and the wallpaper are the screen, so they do not count. Answers
/// whether anything is up in front of the desktop, which the software present
/// path uses to decide the console has the screen back
pub fn open_windows() -> usize {
    let s = SERVER.lock();
    s.surfaces.iter()
        .filter(|x| x.role == Role::Toplevel && !x.backdrop && !x.no_focus)
        .count()
}

/// Every surface with a committed buffer, bottom of the stack first
///
/// The compositor copies them in this order, so the last one written is seen
/// where they overlap. It costs a redundant copy where two windows overlap and
/// the order cannot be wrong
///
/// A surface with no role is skipped: Wayland does not show one
///
/// Read-only: the status command and the tests ask at any moment. The frame
/// asks `presentable_for_frame`, the same list, which also notes how much
/// damage it has taken
pub fn presentable() -> Vec<Presentable> {
    snapshot(&SERVER.lock())
}

/// `presentable`, for the frame about to compose it
///
/// Each surface remembers how much of its damage went in, so `frame_composed`
/// retires that much and not what was committed while the frame was composing
/// (see `Surface::damage_read`)
pub fn presentable_for_frame() -> Vec<Presentable> {
    let mut s = SERVER.lock();
    let out = snapshot(&s);
    for surf in s.surfaces.iter_mut() {
        surf.damage_read = None;
    }
    for p in &out {
        if let Some(si) = s.surface_idx((p.client, p.surface)) {
            s.surfaces[si].damage_read = Some(p.damage.len());
        }
    }
    out
}

fn snapshot(s: &Server) -> Vec<Presentable> {
    let base_fx = crate::mayland::render::effects();
    let mut out = Vec::new();
    for si in s.stack() {
        let surf = &s.surfaces[si];
        let Some((phys, vram, width, height, stride, format, fx)) =
            pixels_of(s, surf, base_fx) else { continue };
        out.push(Presentable {
            client: surf.client,
            surface: surf.id,
            phys,
            vram,
            // Read in place when the buffer is already the card's
            gpu: vram,
            from: surf.ghost.map(|g| g.from),
            width,
            height,
            stride,
            format,
            offset: placed(surf),
            damage: surf.damage.clone(),
            // Resolved in `pixels_of`, where both halves are in reach, so the
            // frame loop never takes the server lock for it and the cursor
            // patch resolves it the same way
            fx,
            // A panel is not decorated like a window: blur and a rounded corner
            // on the bar would land on the top of the screen. Nor is a menu: a
            // border around a popup is the window's border around something
            // else
            decorated: !surf.backdrop && !surf.fullscreen && surf.layer.is_none()
                       && surf.role != Role::Popup,
            // A backdrop still may not: everything else is composed over it,
            // and a pass reading it would read its own output
            composed: !surf.backdrop && !surf.fullscreen,
            scale: surf.anim.scale,
            animating: surf.anim.running,
            below: surf.below_windows(),
        });
    }
    out
}

/// The cursor image the client under the pointer asked for
///
/// `None` means no client has said anything, so the compositor draws its own
/// arrow; `Some(None)` means a client asked for no cursor, and nothing is drawn
pub fn cursor_image() -> Option<Option<CursorImage>> {
    let s = SERVER.lock();
    let c = s.cursor.as_ref()?;
    if c.surface == 0 {
        return Some(None);
    }
    // A cursor surface with no committed buffer yet draws nothing: the client
    // asked for its own image and is about to provide it
    let si = s.surface_idx((c.client, c.surface))?;
    let bid = s.surfaces[si].current.buffer?;
    let b = s.buffers.iter().find(|b| b.client == c.client && b.id == bid)?;
    let base = shm::phys(b.pool).ok()?;
    Some(Some(CursorImage {
        phys: base + b.offset as u64,
        width: b.width,
        height: b.height,
        stride: b.stride,
        hot_x: c.hot_x,
        hot_y: c.hot_y,
        format: b.format,
        pool: b.pool,
        offset: b.offset as usize,
    }))
}
