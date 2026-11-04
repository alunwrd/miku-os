// A damage rectangle composed by the processor, because something in it blends
//
// The rest of this directory rests on one sentence: a surface is opaque, so the
// frame is a list of copies and the last one written wins. So a frame is one
// submission to a copy engine, which cannot blend, the stacking order cannot go
// wrong, and a frame costs the pixels it changed.
//
// A bar the wallpaper shows through breaks that for the pixels it covers. A
// damage region is cut into the part a translucent surface touches and the part
// it does not; the second goes down the copy path, and the first is composed
// here (every surface under it, then it, in one buffer) and put on the screen
// as a single opaque copy.
//
// One copy, not two: the copies go into the surface the raster is scanning,
// with an engine round trip between the wallpaper's copy and the bar's. A
// raster crossing that band in the gap shows the wallpaper, and the bar
// flickers. Composing into RAM first keeps the rule in `present` of never
// writing a pixel twice.
//
// The processor: the card's compute pass (`render::effects`) is not always
// there. `blur::ready()` is false in a virtual machine, before the card is up,
// and on anything but NVIDIA. The strip is some ninety thousand pixels and only
// the changed part is composed.
//
// Opaque destination: straight-alpha source-over generally needs a division per
// pixel. Here the bottom of every stack is the opaque wallpaper, so the
// destination alpha is always 255 and the division is by one. See
// `paint::over`, the arithmetic of the cursor too.

use alloc::vec::Vec;
use spin::Mutex;

use crate::mayland::server::{self, shm};
use crate::mayland::protocol;

use super::damage::Rect;

/// The pool composed rectangles are built in, kept between frames
///
/// One pool for the frame, and kept: the blits go to the engine at the end of
/// `present` and are read after it returns, so two rectangles in one frame must
/// not share memory and last frame's must stay readable. A pool only ever grown
/// gives both.
struct Scratch {
    handle: u32,
    phys: u64,
    bytes: usize,
}

static SCRATCH: Mutex<Option<Scratch>> = Mutex::new(None);

/// How far into the pool this frame has got
static USED: Mutex<usize> = Mutex::new(0);

/// The surfaces that blend with what is under them: an alpha channel, an alpha
/// below one, or a curve drawing them smaller than themselves
///
/// A client committing `XRGB8888` has a padding byte, not a channel; one
/// committing `ARGB8888` asked to be blended. A window on its way in is drawn
/// at a fraction of its alpha and size, so it is composed while the curve runs,
/// on the compute pass with a working shader (`effect_windows`), or here. The
/// bottom of the list is the backdrop: nothing is under it.
///
/// What the effects pass decides on (`effects::pick`, `Under`). Not corners and
/// shadows: those are decoration, and what draws them depends on what the pass
/// takes
pub(in crate::mayland) fn blending(list: &[server::Presentable]) -> Vec<usize> {
    list.iter()
        .enumerate()
        .skip(1)
        .filter(|(_, p)| p.format == protocol::shm::FORMAT_ARGB8888
                         || p.fx.alpha < 0.999
                         || p.scale < 0.999)
        .map(|(i, _)| i)
        .collect()
}

/// Every surface the processor blends for this frame: what really blends, and
/// the corners and shadows of what the pass did not take. Asked after the pass
/// has picked its windows (`effects::remember_taken`)
pub(super) fn translucent(list: &[server::Presentable], screen: &Rect) -> Vec<usize> {
    // A rounded corner is a blend as an alpha is: at the corner the window is
    // partly itself and partly what is behind it, and a copy engine cannot do
    // that. On the card the compute pass lays the corner
    // (`shaders/composite.cu`); with no compute pass (a virtual machine, a card
    // that gave up, anything not NVIDIA) it is done here. `corner_of` is where
    // the two agree on which surfaces
    list.iter()
        .enumerate()
        .skip(1)
        .filter(|(_, p)| {
            p.format == protocol::shm::FORMAT_ARGB8888
                || p.fx.alpha < 0.999
                || p.scale < 0.999
                || shape_of(p, screen, Dress::Frame).is_some_and(|s| outlined(&s, p))
                || shadow_of(p, screen, Dress::Frame).0 > 0.0
        })
        .map(|(i, _)| i)
        .collect()
}

/// The parts of `p` the processor has to blend, pushed onto `out`
///
/// One rectangle, the whole surface, when it is see-through: every pixel mixes
/// with what is under it. A surface opaque and only rounded blends at four
/// small squares and is a plain copy elsewhere, so the corners are named as
/// corners, not a megapixel a frame through the per-pixel path
pub(super) fn covers_of(p: &server::Presentable, screen: &Rect, out: &mut Vec<Rect>) {
    let r = Rect::new(p.offset.0, p.offset.1, p.width, p.height);
    let shape = shape_of(p, screen, Dress::Frame);
    // A dimmed window is every pixel of it multiplied, which no copy can do
    let dimmed = shape.is_some() && p.fx.dim > 0.0;
    let really_blends = p.format == protocol::shm::FORMAT_ARGB8888
        || p.fx.alpha < 0.999
        || p.scale < 0.999
        || dimmed;
    if really_blends {
        out.push(r);
    } else if let Some(s) = shape.filter(|s| outlined(s, p)) {
        // Where the corner cuts the window's own pixels, and where the border's
        // curve reaches in over them: a square at each corner, clamped so two
        // never overlap and blend twice
        let c = s.corner_reach().min(p.width / 2).min(p.height / 2);
        if c > 0 {
            out.push(Rect::new(r.x0,     r.y0,     c, c));
            out.push(Rect::new(r.x1 - c, r.y0,     c, c));
            out.push(Rect::new(r.x0,     r.y1 - c, c, c));
            out.push(Rect::new(r.x1 - c, r.y1 - c, c, c));
        }
    }
    // The border, outside the window: four bands its width deep, blended
    // because its colour has an alpha and its corners a curve
    if let Some(b) = shape.map(|s| s.border as i32).filter(|&b| b > 0) {
        out.push(Rect::new(r.x0 - b, r.y0 - b, r.x1 - r.x0 + 2 * b, b));
        out.push(Rect::new(r.x0 - b, r.y1,     r.x1 - r.x0 + 2 * b, b));
        out.push(Rect::new(r.x0 - b, r.y0,     b, r.y1 - r.y0));
        out.push(Rect::new(r.x1,     r.y0,     b, r.y1 - r.y0));
    }
    // The shadow, outside the window: four bands around it, the margin wide. It
    // darkens the desktop, so those pixels blend, and what the window covers is
    // a plain copy
    let (dark, margin) = shadow_of(p, screen, Dress::Frame);
    if dark > 0.0 && margin > 0 {
        let m = margin;
        out.push(Rect::new(r.x0 - m, r.y0 - m, r.x1 - r.x0 + 2 * m, m));
        out.push(Rect::new(r.x0 - m, r.y1,     r.x1 - r.x0 + 2 * m, m));
        out.push(Rect::new(r.x0 - m, r.y0,     m, r.y1 - r.y0));
        out.push(Rect::new(r.x1,     r.y0,     m, r.y1 - r.y0));
    }
}

/// The drop shadow this surface casts on this path: (darkness, reach).
///
/// Gated as `corner_of` is: the compute pass lays the shadow when alive, and
/// laying it here too would darken the same pixels twice. `(0.0, 0)` means no
/// shadow, for a panel, the backdrop and every surface with effects off
pub(super) fn shadow_of(p: &server::Presentable, screen: &Rect, who: Dress)
    -> (f32, i32)
{
    if !dressed(p, screen, who) { return (0.0, 0); }
    let s = p.fx.shadow.clamp(0.0, 1.0);
    let m = p.fx.margin as i32;
    // A shadow too faint to move a byte is a band composed for nothing, and the
    // margin is the widest thing a frame reaches past a window
    if !(s > 0.004) || m <= 0 { return (0.0, 0); }
    (s, m)
}

/// Who is asking whether a surface is decorated
///
/// `Frame` asks whether the processor dresses this surface: no when the compute
/// pass took it this frame and is dressing it, since a shadow here too would
/// darken those pixels twice. A window the pass refused is the processor's to
/// dress.
///
/// `Patch` asks whether this surface is dressed on the screen, by anyone. The
/// cursor patch is an opaque copy overwriting the pixels under the arrow, so it
/// reproduces what the frame put there or punches a rectangle of undecorated
/// desktop that follows the pointer. The two paths agree numerically: `shade`
/// below is the shader's own formula, `k = 1 - dark * t²`
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::mayland) enum Dress {
    /// The frame, which only draws what the compute pass is not drawing
    Frame,
    /// The cursor patch, which reproduces the screen whoever drew it
    Patch,
}

/// Does this surface carry decoration, for the composer asking?
///
/// One place, as a corner and a shadow have to agree: a window rounded but
/// unshadowed, or shadowed but square, reads as broken. Three parts: the
/// compute pass's own gate, there as `let plain = !e.on || !p.decorated`,
/// meaning no blur, no corner and no shadow; whether that pass is alive to do
/// it, which only `Frame` cares about; and the surface covering the whole
/// screen, the backdrop whatever it was told.
///
/// `decorated` is false for a window that declared itself fullscreen
/// (`scene.rs`); the screen-covering test is for one that fills it
fn dressed(p: &server::Presentable, screen: &Rect, who: Dress) -> bool {
    dressed_by(p, screen, who, super::effects::taken_reach(p).is_some())
}

/// `dressed`, told whether the pass took the surface this frame
pub(in crate::mayland) fn dressed_by(p: &server::Presentable, screen: &Rect, who: Dress,
                                     taken: bool) -> bool {
    if !p.fx.on || !p.decorated { return false; }
    if who == Dress::Frame && taken { return false; }
    !(p.offset.0 <= screen.x0 && p.offset.1 <= screen.y0
      && p.offset.0 + p.width  >= screen.x1
      && p.offset.1 + p.height >= screen.y1)
}

/// The outline this surface is drawn with on this path: its corner and its
/// border, as Hyprland draws them (`libmay::shape`,
/// `dev/mayland-window-shape.md`). `None` unless the processor dresses it.
///
/// When the compute pass is alive it outlines the window itself, and cutting
/// the corner here too (against the backdrop in the pool and against the
/// blurred copy) shows as a dark rim a pixel or two wide.
///
/// Undecorated surfaces have none: the panel is a layer surface, and the rule
/// keeping a border off it keeps a rounded corner off too
pub(super) fn shape_of(p: &server::Presentable, screen: &Rect, who: Dress) -> Option<libmay::shape::Shape> {
    if !dressed(p, screen, who) { return None; }
    Some(p.fx.shape(super::effects::border().size))
}

/// Whether an outline does anything a copy cannot: a curve, a border or a dim.
/// A square window with no border and its own colours is a plain copy
fn outlined(s: &libmay::shape::Shape, p: &server::Presentable) -> bool {
    s.surface_radius() > 0.0 || s.border >= 1.0 || p.fx.dim > 0.0
}

/// Where a surface's pixels land
///
/// The rectangle it occupies, until a `popin` is running: then that rectangle
/// shrunk towards its own centre, so the window grows outwards, not out of a
/// corner.
///
/// The shader draws the same box (`shaders/composite.cu`), as does the ring
/// round a window in `present`
pub(in crate::mayland) fn placed(p: &server::Presentable) -> Rect {
    let r = Rect::new(p.offset.0, p.offset.1, p.width, p.height);
    if p.scale >= 0.999 {
        return r;
    }
    let s = p.scale.clamp(0.05, 1.0);
    let (w, h) = ((p.width as f32 * s) as i32, (p.height as f32 * s) as i32);
    Rect::new(
        p.offset.0 + (p.width - w) / 2,
        p.offset.1 + (p.height - h) / 2,
        w.max(1),
        h.max(1),
    )
}

/// Cut one damage region into the parts that blend and the parts that copy
///
/// `mixed` never overlaps itself: two translucent surfaces crossing yield one
/// piece each and a third where they meet. `plain` is what is left. Between
/// them they cover the region exactly once, the invariant the whole frame rests
/// on.
///
/// Takes rectangles, not the scene, so a test can put a case to it
pub fn split(
    region: Rect,
    covers: &[Rect],
    plain: &mut Vec<Rect>,
    mixed: &mut Vec<Rect>,
) {
    plain.clear();
    mixed.clear();
    let mut scratch: Vec<Rect> = Vec::new();

    for r in covers {
        let Some(hit) = r.intersect(&region) else { continue };
        // What of this surface's share is not claimed by a translucent surface
        // below it
        let mut want = alloc::vec![hit];
        for m in mixed.iter() {
            scratch.clear();
            for w in want.iter() {
                w.subtract(m, &mut scratch);
            }
            core::mem::swap(&mut want, &mut scratch);
            if want.is_empty() {
                break;
            }
        }
        mixed.extend(want);
    }

    plain.push(region);
    for m in mixed.iter() {
        scratch.clear();
        for p in plain.iter() {
            p.subtract(m, &mut scratch);
        }
        core::mem::swap(plain, &mut scratch);
        if plain.is_empty() {
            break;
        }
    }
}

/// Hand the whole pool back. Called once at the top of a frame
pub(super) fn reset() {
    *USED.lock() = 0;
}

/// Make sure the pool can hold `bytes`, growing it if it cannot
///
/// Called once a frame with the total the split asked for, so the pool never
/// grows while blits point into it. Growing is destroy and create:
/// `shm::destroy` retires the memory, so last frame's copies still in flight
/// read pages nobody has handed back.
pub(super) fn reserve(bytes: usize) -> bool {
    if bytes == 0 {
        return true;
    }
    let mut s = SCRATCH.lock();
    if let Some(cur) = s.as_ref() {
        if cur.bytes >= bytes {
            return true;
        }
    }
    // Rounded up, so a bar growing by a pixel does not rebuild the pool. Owner
    // 0: the compositor's own memory
    let want = (bytes + 0xFFFF) & !0xFFFF;

    // The old pool goes back before the new one is asked for.
    //
    // Holding both needs the sum of the two free at once, which on a
    // long-running heap fails well before either size alone. A failed reserve
    // falls back to copying, so a see-through panel comes out solid for that
    // frame, and alternating that flickers. Dropping it first loses nothing:
    // this is reached only when the existing pool is too small
    let old = s.take();
    if let Some(old) = old {
        let _ = shm::destroy(old.handle);
    }
    let Ok(handle) = shm::create(0, want) else { return false };
    let Ok(phys) = shm::phys(handle) else {
        let _ = shm::destroy(handle);
        return false;
    };
    *s = Some(Scratch { handle, phys, bytes: want });
    true
}

/// Darken the pool where `win`'s shadow falls
///
/// The arithmetic of `shaders/composite.cu`, so a machine with a card and one
/// without look alike. Distance to the rounded rectangle, a falloff squared so
/// the shadow is dense at the edge and gone quickly with no visible ring, and a
/// plain multiply: the shadow is the backdrop, darker.
///
/// Only the band inside `area` is touched, and pixels the window covers are
/// left alone: it is drawn over them, and darkening them first would show
/// through anything translucent.
///
/// `margin` is the falloff, how far the shadow would reach if it could, and
/// `reach` how far it may, on the compute path what the screen left room for.
/// They are equal when the processor draws, and differ for a window close to an
/// edge
fn shade(dst: &mut [u8], w: usize, area: Rect, win: Rect,
         corner: i32, dark: f32, margin: i32, reach: i32) {
    let band = Rect::new(win.x0 - reach, win.y0 - reach,
                         win.x1 - win.x0 + 2 * reach,
                         win.y1 - win.y0 + 2 * reach);
    let Some(hit) = band.intersect(&area) else { return };
    let prect = libmay::paint::Rect::new(win.x0, win.y0, win.x1 - win.x0, win.y1 - win.y0);
    // How much each whole-pixel distance keeps, in 65536ths: the distance is a
    // whole number, so the falloff is a table of `reach` entries made once, not
    // three soft-float multiplies a pixel
    let keep: alloc::vec::Vec<u32> = (0..reach.min(margin).max(0))
        .map(|d| {
            let t = 1.0 - (d as f32) / (margin as f32);
            ((1.0 - dark * t * t).clamp(0.0, 1.0) * 65536.0) as u32
        })
        .collect();
    for y in hit.y0..hit.y1 {
        let row = (y - area.y0) as usize * w;
        for x in hit.x0..hit.x1 {
            let d = libmay::paint::round_rect_outside_dist(x, y, prect, corner) as i32;
            // Inside the window, past where the shadow fades out, or past what
            // the pass had room to write
            if d == 0 || d >= margin || d >= reach { continue; }
            let k = keep[d as usize];
            let o = (row + (x - area.x0) as usize) * 4;
            let Some(px) = dst.get_mut(o..o + 4) else { continue };
            for c in px.iter_mut().take(3) {
                *c = ((*c as u32 * k) >> 16) as u8;
            }
        }
    }
}

/// Compose everything the screen holds over `area` into the pool
///
/// Returns where the engine can read the result. `None` means the pool could
/// not take it, and the caller draws nothing: last frame's pixels in a
/// rectangle are worse than a rectangle waiting for the next frame
pub(super) fn compose(area: Rect, list: &[server::Presentable], screen: &Rect)
    -> Option<(u64, u32)>
{
    let (w, h) = ((area.x1 - area.x0) as usize, (area.y1 - area.y0) as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let bytes = w * h * 4;

    let s = SCRATCH.lock();
    let pool = s.as_ref()?;
    let mut used = USED.lock();
    let offset = *used;
    if offset + bytes > pool.bytes {
        return None;
    }
    *used = offset + bytes;
    let (handle, phys) = (pool.handle, pool.phys + offset as u64);
    drop(used);
    drop(s);

    // Asked before the pool's lock, not under it. The server takes its own lock
    // and then the pools' (`shm::create` for a client's `may_pool`), so asking
    // from inside `with_bytes_mut` would take them in the other order
    let focus = server::active_key();
    shm::with_bytes_mut(handle, offset, bytes, |dst| {
        compose_focused(dst, w, area, list, screen, Dress::Frame, focus);
    })
    .ok()?;

    Some((phys, (w * 4) as u32))
}

/// Compose `area` of the screen into `dst`, `w` pixels wide with `XRGB8888`
/// rows and no padding
///
/// Separate from `compose` so the cursor patch has the same pixels without the
/// scratch pool: a few thousand pixels on the way to a DMA page, and a slice of
/// the frame's pool could run it out with a moving pointer. The two callers
/// differ only in `who`; see `Dress`
pub(in crate::mayland) fn compose_into(
    dst: &mut [u8],
    w: usize,
    area: Rect,
    list: &[server::Presentable],
    screen: &Rect,
    who: Dress,
) {
    compose_focused(dst, w, area, list, screen, who, server::active_key());
}

/// `compose_into`, told which surface has the keyboard: for a caller holding a
/// pool's lock (`compose`), under which the server's must not be taken
fn compose_focused(
    dst: &mut [u8],
    w: usize,
    area: Rect,
    list: &[server::Presentable],
    screen: &Rect,
    who: Dress,
    focus: Option<(u32, u32)>,
) {
    // Black under everything. The backdrop covers the screen, but a pixel no
    // surface claims must not be last frame's pixel from elsewhere in the pool
    dst.fill(0);
    let bord = super::effects::border();
    for p in list.iter() {
        let r = placed(p);
        // The shadow first, then the surface: it darkens what is in the pool
        // (every surface below this one), and the window is laid over the
        // result
        let (dark, margin) = shadow_of(p, screen, who);
        if dark > 0.0 && margin > 0 {
            // The frame's own path is the processor drawing the whole falloff.
            // The patch matches what is on the screen, and the compute pass
            // drew the margin it recorded for that window
            // (`effects::taken_reach`)
            let reach = match who {
                Dress::Patch => super::effects::taken_reach(p).unwrap_or(margin),
                Dress::Frame => margin,
            };
            if reach > 0 {
                let corner = shape_of(p, screen, who).map_or(0, |s| s.base() as i32);
                shade(dst, w, area, r, corner, dark, margin, reach);
            }
        }
        // A window whose buffer is video memory with no mirror this frame: the
        // processor does not read the card's memory (`texture::mirror`,
        // `dev/gpu-window-buffers.md`), so it is left out
        if p.phys == 0 {
            super::texture::left_out();
            continue;
        }
        // Its outline on this path (corner, border, dim), or none
        let shape = shape_of(p, screen, who);
        let (rw, rh) = (r.x1 - r.x0, r.y1 - r.y0);
        if let Some(hit) = r.intersect(&area) {
            match shape {
                Some(sh) => super::outline::with(sh, rw, rh, |o| {
                    draw_surface(dst, w, area, hit, r, p, Some(o));
                }),
                None => draw_surface(dst, w, area, hit, r, p, None),
            }
        }
        // The border straight after its own window, as Hyprland draws it: over
        // the window and under everything above it
        if let Some(sh) = shape.filter(|s| s.border >= 1.0) {
            let colour = if focus == Some((p.client, p.surface)) { bord.active } else { bord.inactive };
            super::outline::with(sh, rw, rh, |o| {
                draw_border(dst, w, area, r, sh.border as i32, colour, p.fx.alpha, o);
            });
        }
    }

    // The rings of the windows nobody dresses, after every window, as `present`
    // draws them: a window composed later writes a shadow past its own edge,
    // and would paint over a ring built in with the windows. A dressed window's
    // border was drawn with it, above.
    //
    // Only for the patch. On the frame's path these are fill blits the engine
    // lays over the finished frame. The patch is laid over those blits, so it
    // carries the ring or it cuts a cursor-sized notch out of it
    if who == Dress::Patch && bord.size > 0 {
        for (i, p) in list.iter().enumerate() {
            // The backdrop, a fullscreen window and every layer surface are not
            // decorated, and a ring is decoration, the rule in `present`. A
            // dressed one has its border already
            if !p.decorated || dressed(p, screen, Dress::Patch) {
                continue;
            }
            let colour = if focus == Some((p.client, p.surface)) {
                bord.active
            } else {
                bord.inactive
            };
            let r = placed(p);
            for band in r.ring(bord.size).iter() {
                let Some(hit) = band.intersect(&area) else { continue };
                for y in hit.y0..hit.y1 {
                    for x in hit.x0..hit.x1 {
                        // A window in front of this one hides its ring, as
                        // `uncovered` does for the frame's blits. Per pixel:
                        // the patch is at most 64 by 64 and the windows above
                        // it a handful
                        let behind = list[i + 1..].iter().any(|q| {
                            let b = placed(q);
                            b.x0 <= x && x < b.x1 && b.y0 <= y && y < b.y1
                        });
                        if behind {
                            continue;
                        }
                        let off = ((y - area.y0) as usize * w
                                 + (x - area.x0) as usize) * 4;
                        let Some(px) = dst.get_mut(off..off + 4) else { continue };
                        px.copy_from_slice(&(colour | 0xFF00_0000).to_le_bytes());
                    }
                }
            }
        }
    }
}

/// Lay one surface's pixels in `hit` over the pool, with its outline if any:
/// the corner from `o`, the dim, the alpha it is drawn at
fn draw_surface(dst: &mut [u8], w: usize, area: Rect, hit: Rect, r: Rect,
                p: &server::Presentable, o: Option<&super::outline::Outline>) {
    let blends = p.format == protocol::shm::FORMAT_ARGB8888;
    // What a curve does to this surface. `fade` multiplies every pixel's alpha;
    // `shrink` says the rectangle above is smaller than the buffer and the read
    // is scaled to match
    let fade = p.fx.alpha.clamp(0.0, 1.0);
    // Dimmed, as Hyprland's tint: the window's own colour, before its corner
    // and its alpha, in 1/256ths
    let tint = if o.is_some() && p.fx.dim > 0.0 {
        ((1.0 - p.fx.dim.clamp(0.0, 1.0)) * 256.0) as u32
    } else {
        256
    };
    // A corner makes the surface non-opaque at its four squares even when every
    // committed pixel is. `r` is in the coordinates the loop below indexes
    // with, so the test is against the window's place on screen, not in its
    // buffer, which matters while a `popin` shrinks it
    let cut = o.is_some_and(|_| p.fx.shape(0).surface_radius() > 0.0);
    let opaque = !blends && fade > 0.999 && !cut && tint == 256;
    // The fade as a fraction of 65536, worked out once: the kernel is
    // soft-float, and `a as f32 * fade` per pixel is two library calls a pixel.
    // `None` is a surface drawn at its own alpha, nearly all of them
    let fade16 = (fade < 0.999).then(|| (fade * 65536.0) as u32);
    let shrink = r.x1 - r.x0 != p.width || r.y1 - r.y0 != p.height;
    let (dw, dh) = (r.x1 - r.x0, r.y1 - r.y0);
    let run = (hit.x1 - hit.x0) as usize;
    for y in hit.y0..hit.y1 {
        // Nearest neighbour, the row chosen once, not per pixel: a curve
        // running for a fifth of a second with no shader
        let sy = if shrink { (y - r.y0) * p.height / dh } else { y - r.y0 };
        let src_off = sy as usize * p.stride;
        // Every client buffer is ordinary system memory, so the physical
        // address the engine would get is reachable through the direct map, the
        // route of `blit_batch_cpu`
        let src = crate::arch::x86_64::grub::phys_to_virt(
            p.phys + src_off as u64) as *const u8;
        let row = ((y - area.y0) as usize * w
                 + (hit.x0 - area.x0) as usize) * 4;
        let Some(out) = dst.get_mut(row..row + run * 4) else { continue };
        for (i, px) in out.chunks_exact_mut(4).enumerate() {
            let dx = hit.x0 - r.x0 + i as i32;
            let sx = if shrink { dx * p.width / dw } else { dx };
            // SAFETY: `sx` and `sy` are inside the surface by construction
            // (`hit` is inside `r`, and the scaling maps `0..dw` onto
            // `0..width`), so these bytes are inside the buffer the client
            // committed, and its pool is pinned while presentable
            let mut sp = unsafe {
                core::ptr::read_unaligned(
                    src.add(sx as usize * 4) as *const u32)
            };
            if opaque {
                px.copy_from_slice(&(sp | 0xFF00_0000).to_le_bytes());
                continue;
            }
            let a = if blends { sp >> 24 } else { 255 };
            let a = match fade16 {
                Some(f) => ((a * f) >> 16).min(255),
                None => a,
            };
            // The corner, after the alpha: it multiplies the pixel's alpha, so
            // a translucent window with rounded corners is both, and a fully
            // transparent one stays so
            let a = match o {
                Some(o) if cut => {
                    let cov = o.surface(dx, y - r.y0, dw, dh);
                    if cov == 0 { continue; }
                    (a * cov + 127) / 255
                }
                _ => a,
            };
            if tint != 256 {
                let mut v = sp & 0xFF00_0000;
                for shift in [0, 8, 16] {
                    v |= ((((sp >> shift) & 0xff) * tint) >> 8) << shift;
                }
                sp = v;
            }
            let d = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
            px.copy_from_slice(&libmay::paint::over(d, sp, a).to_le_bytes());
        }
    }
}

/// Lay a window's border over the pool where `area` meets it: `size` wide
/// around `r`, its curve from `o`, its colour's alpha and the window's opacity
/// both weighing it (`dev/mayland-window-shape.md`)
///
/// Only the parts where a border can be: the four corner squares of its box and
/// the bands its width deep along the straight edges. The box is cut into a
/// three by three grid, so no pixel is visited twice however small the window
fn draw_border(dst: &mut [u8], w: usize, area: Rect, r: Rect, size: i32,
               colour: u32, alpha: f32, o: &super::outline::Outline) {
    let bx = Rect::new(r.x0 - size, r.y0 - size,
                       r.x1 - r.x0 + 2 * size, r.y1 - r.y0 + 2 * size);
    let (bw, bh) = (bx.x1 - bx.x0, bx.y1 - bx.y0);
    let cx = o.reach().min(bw / 2);
    let cy = o.reach().min(bh / 2);
    let weight = (((colour >> 24) as f32) * alpha.clamp(0.0, 1.0)) as u32;
    if weight == 0 {
        return;
    }
    let cols = [(bx.x0, bx.x0 + cx), (bx.x0 + cx, bx.x1 - cx), (bx.x1 - cx, bx.x1)];
    let rows = [(bx.y0, bx.y0 + cy), (bx.y0 + cy, bx.y1 - cy), (bx.y1 - cy, bx.y1)];
    for (ri, &(y0, y1)) in rows.iter().enumerate() {
        for (ci, &(x0, x1)) in cols.iter().enumerate() {
            // The middle of the box holds no border, and the middle of each
            // edge holds it only `size` deep against the outside
            let cell = match (ri, ci) {
                (1, 1) => continue,
                (0, 1) => Rect::new(x0, y0, x1 - x0, size),
                (2, 1) => Rect::new(x0, y1 - size, x1 - x0, size),
                (1, 0) => Rect::new(x0, y0, size, y1 - y0),
                (1, 2) => Rect::new(x1 - size, y0, size, y1 - y0),
                _ => Rect::new(x0, y0, x1 - x0, y1 - y0),
            };
            let Some(hit) = cell.intersect(&area) else { continue };
            for y in hit.y0..hit.y1 {
                for x in hit.x0..hit.x1 {
                    let ba = o.border(x - bx.x0, y - bx.y0, bw, bh);
                    if ba == 0 { continue; }
                    let k = (ba * weight + 127) / 255;
                    let off = ((y - area.y0) as usize * w + (x - area.x0) as usize) * 4;
                    let Some(px) = dst.get_mut(off..off + 4) else { continue };
                    let d = u32::from_le_bytes([px[0], px[1], px[2], px[3]]);
                    px.copy_from_slice(&libmay::paint::over(d, colour, k).to_le_bytes());
                }
            }
        }
    }
}
