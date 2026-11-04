// Blending
//
// A see-through bar is two pieces of arithmetic, and both are silent when
// wrong. `split` decides which pixels are composed and which are copied; a
// mistake writes a pixel twice or leaves a hole. `over` and `blend` decide what
// a blended pixel comes out as; a mistake is a fringe or a wash.
//
// Both are checked by arithmetic, and `split` by counting how many pieces claim
// each pixel: the answer must be one everywhere.

use super::*;

use crate::mayland::render::compose::split;
use crate::mayland::render::damage::Rect;

/// How many pieces of `plain` and `mixed` together claim each pixel of
/// `region`, and whether every mixed pixel is inside a cover and every plain
/// one outside all of them
fn tally(region: Rect, covers: &[Rect], plain: &[Rect], mixed: &[Rect]) -> (u32, u32, bool) {
    let (mut least, mut most, mut placed_right) = (u32::MAX, 0u32, true);
    for y in region.y0..region.y1 {
        for x in region.x0..region.x1 {
            let inside = |r: &Rect| x >= r.x0 && x < r.x1 && y >= r.y0 && y < r.y1;
            let p = plain.iter().filter(|r| inside(r)).count() as u32;
            let m = mixed.iter().filter(|r| inside(r)).count() as u32;
            let covered = covers.iter().any(|r| inside(r));
            // A mixed pixel must be under something see-through, and a plain
            // one must not
            if (m > 0 && !covered) || (p > 0 && covered) {
                placed_right = false;
            }
            least = least.min(p + m);
            most = most.max(p + m);
        }
    }
    (least, most, placed_right)
}

/// Every pixel of a damage region belongs to exactly one piece
///
/// Three arrangements. In the third, two see-through surfaces cross, and the
/// band they share would be claimed by both unless something takes it away from
/// the second
pub(super) fn test_compose_split(t: &mut Runner) {
    let region = Rect::new(0, 0, 40, 20);
    let mut plain = Vec::new();
    let mut mixed = Vec::new();

    // A bar across the top, as a panel is
    let bar = [Rect::new(0, 0, 40, 6)];
    split(region, &bar, &mut plain, &mut mixed);
    let (a_least, a_most, a_right) = tally(region, &bar, &plain, &mixed);

    // Something see-through in the middle, so the plain part is a ring
    let island = [Rect::new(10, 5, 12, 8)];
    split(region, &island, &mut plain, &mut mixed);
    let (b_least, b_most, b_right) = tally(region, &island, &plain, &mixed);
    let ring = plain.len();

    // Two crossing. The band where they meet is one piece, not two
    let cross = [Rect::new(4, 2, 30, 6), Rect::new(12, 0, 8, 18)];
    split(region, &cross, &mut plain, &mut mixed);
    let (c_least, c_most, c_right) = tally(region, &cross, &plain, &mixed);

    // Nothing see-through at all: the region comes back whole
    split(region, &[], &mut plain, &mut mixed);
    let untouched = mixed.is_empty() && plain.len() == 1 && plain[0] == region;

    let exact = a_least == 1 && a_most == 1 && a_right
             && b_least == 1 && b_most == 1 && b_right
             && c_least == 1 && c_most == 1 && c_right;

    t.check("compose: a damage region is cut into pieces that cover it exactly once",
            exact && untouched,
            format!("bar {}..{} placed {}, island {}..{} placed {} in {} plain pieces, \
                     crossing {}..{} placed {}, empty cover {}",
                    a_least, a_most, a_right,
                    b_least, b_most, b_right, ring,
                    c_least, c_most, c_right,
                    if untouched { "left the region whole" } else { "did not" }));
}

/// Source-over against an opaque destination
pub(super) fn test_over(t: &mut Runner) {
    use libmay::paint::over;

    // Half of white over black is the middle grey, in every channel, and the
    // answer is declared opaque whatever the source said
    let half = over(0xFF00_0000, 0x00FF_FFFF, 128);
    // The ends are exact: a bar at alpha 255 is the bar's colour and not one
    // off it
    let none = over(0xFF11_2233, 0x00AA_BBCC, 0);
    let all = over(0xFF11_2233, 0x00AA_BBCC, 255);

    let mid = (half >> 16) & 0xFF;
    let ok = (127..=129).contains(&mid)
        && half & 0xFF == mid && (half >> 8) & 0xFF == mid
        && half >> 24 == 0xFF
        && none == 0xFF11_2233
        && all == 0xFFAA_BBCC;

    t.check("compose: source-over is exact at both ends and even in the middle",
            ok,
            format!("half {:08x}, none {:08x}, all {:08x}", half, none, all));
}

/// An antialiased edge against nothing keeps its colour and loses only alpha
///
/// The reason for clearing to the card colour at zero alpha. Lerping straight
/// alpha between two colours that disagree about RGB pulls the colour towards
/// the transparent side, which on a rounded corner is a dark fringe where the
/// bar meets the wallpaper
pub(super) fn test_edge_against_nothing(t: &mut Runner) {
    use libmay::paint::blend;
    use libmay::ui;

    let card = 0xBF21_1F24u32;          // the shipped bar colour, three quarters
    let ground = ui::at(card, 0);       // what a see-through layer clears to

    let edge = blend(ground, card, 128);
    let alpha = edge >> 24;
    let colour_kept = edge & 0x00FF_FFFF == card & 0x00FF_FFFF;

    // Half coverage of an alpha of 0xBF, and nothing else moved
    let ramped = (95..=96).contains(&alpha);

    // The naive ground (transparent black) for comparison, to show what the
    // rule changes
    let naive = blend(0x0000_0000, card, 128);
    let naive_darker = naive & 0x00FF_FFFF != card & 0x00FF_FFFF;

    t.check("compose: an antialiased edge against nothing ramps alpha and keeps its colour",
            colour_kept && ramped && naive_darker,
            format!("edge {:08x} from card {:08x}, over transparent black {:08x}",
                    edge, card, naive));
}

/// What a config file says about an alpha is what it gets
pub(super) fn test_colour_alpha(t: &mut Runner) {
    use libmay::conf::colour;

    // Six digits: nothing said, so solid
    let solid = colour("rgb(211f24)");
    let hash = colour("#211f24");
    // Eight: hyprland puts the alpha last, this puts it first
    let three_quarters = colour("rgba(211f24bf)");
    let clear = colour("rgba(211f2400)");

    let ok = solid == Some(0xFF21_1F24)
        && hash == Some(0xFF21_1F24)
        && three_quarters == Some(0xBF21_1F24)
        && clear == Some(0x0021_1F24)
        && colour("rgba(211f2)").is_none();

    t.check("compose: a colour without an alpha is solid, and rgba() keeps its own",
            ok,
            format!("rgb {:?}, # {:?}, rgba {:?}, clear {:?}",
                    solid, hash, three_quarters, clear));
}


/// A desktop that has been told nothing draws its windows solid
///
/// `translucent` asks about the alpha and not about `on`, so
/// `Effects::DEFAULT.alpha` below 1 with `on` false would send every window
/// down the composed path, blended over the wallpaper, while `present`'s
/// fallback for rectangles too large for the blend pool copies the same window
/// opaque. That shows as hard-edged regions of a darker hue which never repair.
///
/// So: nothing said means solid, and something said means exactly what was said
pub(super) fn test_default_is_opaque(t: &mut Runner) {
    use crate::mayland::render::{Effects, Fx};

    let plain = Fx::default().over(Effects::DEFAULT);
    let rule = Fx { alpha: Some(0.5), ..Default::default() }.over(Effects::DEFAULT);
    // What `wm_set_param(ALPHA)` does to the desktop as a whole still reaches
    // a window that has asked for nothing of its own
    let desktop = Effects { alpha: 0.6, ..Effects::DEFAULT };
    let inherited = Fx::default().over(desktop);

    let ok = plain.alpha >= 0.999
        && (rule.alpha - 0.5).abs() < 0.001
        && (inherited.alpha - 0.6).abs() < 0.001;

    t.check("compose: a window nobody has configured is opaque, a configured one is not",
            ok,
            format!("default {:.2}, window rule {:.2}, desktop-wide {:.2}",
                    plain.alpha, rule.alpha, inherited.alpha));
}

/// The composed path and the copied path agree, pixel for pixel, at alpha 1
///
/// `present` puts a rectangle on the screen as a copy of the client's buffer,
/// or, where something see-through covers it, composed in RAM by `compose` and
/// copied once. Both must produce the same bytes wherever the surface is
/// opaque, because which one a rectangle takes depends on the damage, on what
/// else is on screen and on whether the blend pool could be grown. Any
/// difference is a seam along the edge of a damage rectangle that nothing
/// repairs.
///
/// Source-over at full alpha is the identity. Also checked at a fade of 0.85,
/// where the two differ
pub(super) fn test_paths_agree_when_opaque(t: &mut Runner) {
    use libmay::paint::over;

    let cases = [0x0021_1F24u32, 0x00FF_FFFF, 0x0000_0000, 0x0012_3456, 0x00CF_BDFE];
    let wallpaper = 0xFF0A_0A0Cu32;

    // The copy path: the engine moves the client's pixels and the padding byte
    // reads back as opaque
    let copied = |sp: u32| sp | 0xFF00_0000;
    // The composed path at the alpha of an unconfigured desktop
    let composed = |sp: u32| over(wallpaper, sp, 255);

    let agree = cases.iter().all(|&sp| composed(sp) == copied(sp));

    // At 0.85 they do not agree
    let faded = |sp: u32| over(wallpaper, sp, (255.0 * 0.85) as u32);
    let differed = cases.iter().any(|&sp| faded(sp) != copied(sp));

    t.check("compose: an opaque surface composes to the same bytes it would copy to",
            agree && differed,
            format!("at alpha 1 {}, at the old 0.85 default {}",
                    if agree { "identical" } else { "DIFFERENT" },
                    if differed { "differs, as it did" } else { "identical" }));
}

/// The processor's outline is `libmay::shape`'s, read from a table
///
/// The table is one corner square, folded onto all four. A mistake in the fold
/// or the indexing would round one corner and not another, so the near and the
/// far corner are both checked against the function, and a few pixels past the
/// square, where the table is not read
pub(super) fn test_outline(t: &mut Runner) {
    use crate::mayland::render::outline;
    use libmay::shape::{self, Shape};

    let s = Shape { rounding: 22.0, power: 2.5, border: 1.0 };
    let (w, h) = (200i32, 120i32);
    let (bw, bh) = (w + 2, h + 2);
    let unit = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as i32;
    let (worst, reach) = outline::with(s, w, h, |o| {
        let mut worst = 0;
        for j in 0..o.reach() + 4 {
            for i in 0..o.reach() + 4 {
                let (px, py) = (i as f32 + 0.5, j as f32 + 0.5);
                let near = unit(shape::surface_coverage(px, py, w as f32, h as f32, &s));
                let far = unit(shape::surface_coverage(w as f32 - px, h as f32 - py,
                                                       w as f32, h as f32, &s));
                let ring = unit(shape::border_alpha(px, py, bw as f32, bh as f32, &s));
                let ring_far = unit(shape::border_alpha(bw as f32 - px, bh as f32 - py,
                                                        bw as f32, bh as f32, &s));
                worst = worst
                    .max((o.surface(i, j, w, h) as i32 - near).abs())
                    .max((o.surface(w - 1 - i, h - 1 - j, w, h) as i32 - far).abs())
                    .max((o.border(i, j, bw, bh) as i32 - ring).abs())
                    .max((o.border(bw - 1 - i, bh - 1 - j, bw, bh) as i32 - ring_far).abs());
            }
        }
        (worst, o.reach())
    });
    // At most one step of rounding apart: the table is the function, rounded
    t.check("compose: the processor's corner and border are libmay::shape's, at all four corners",
            worst <= 1 && reach == s.corner_reach(),
            format!("worst difference {} of 255 over a {}-pixel corner square", worst, reach));
}

/// A window composed by the processor, corner and border and dim, checked pixel
/// by pixel against the arithmetic
///
/// The order is Hyprland's: the window's corner cut out of it, then its border
/// laid over it in the colour of a window without the keyboard, at that
/// colour's own alpha; and a dimmed window's colour darkened before any of it
/// (`dev/mayland-window-shape.md`)
pub(super) fn test_outlined_window(t: &mut Runner) {
    use crate::mayland::render::{compose, effects, Effects};
    use libmay::paint::over;
    use libmay::shape::{self, Shape};
    const NAME: &str = "compose: a window is drawn with Hyprland's corner, its border over it, and dimmed when asked";

    let colour = 0x0020_4080u32;
    let (ww, wh) = (120i32, 80i32);
    let bytes = (ww * wh * 4) as usize;
    let Ok(handle) = shm::create(0, bytes) else {
        t.check(NAME, false, "no memory for the window".to_string());
        return;
    };
    let _ = shm::with_bytes_mut(handle, 0, bytes, |b| {
        for p in b.chunks_exact_mut(4) {
            p.copy_from_slice(&colour.to_le_bytes());
        }
    });
    let Ok(phys) = shm::phys(handle) else {
        let _ = shm::destroy(handle);
        t.check(NAME, false, "the window has no address".to_string());
        return;
    };
    let (x, y) = (40i32, 30i32);
    let fx = Effects { on: true, corner: 22.0, power: 2.5, shadow: 0.0, alpha: 1.0,
                       dim: 0.0, ..Effects::DEFAULT };
    let window = |fx: Effects| server::Presentable {
        // Nobody's: it never has the keyboard, so it wears the inactive colour
        client: 0xFFFF_FFF0,
        surface: 1,
        phys,
        vram: None,
        gpu: None,
        from: None,
        width: ww,
        height: wh,
        stride: ww as usize * 4,
        format: protocol::shm::FORMAT_XRGB8888,
        offset: (x, y),
        damage: Vec::new(),
        fx,
        decorated: true,
        composed: true,
        scale: 1.0,
        animating: false,
        below: false,
    };
    let screen = Rect::new(0, 0, 1920, 1080);
    // Two pixels round the window: the border's one, and one of nothing
    let area = Rect::new(x - 2, y - 2, ww + 4, wh + 4);
    let aw = (ww + 4) as usize;
    let compose_one = |p: &server::Presentable| {
        let mut dst = alloc::vec![0u8; aw * (wh + 4) as usize * 4];
        compose::compose_into(&mut dst, aw, area, core::slice::from_ref(p), &screen,
                              compose::Dress::Frame);
        dst
    };
    let at = |dst: &[u8], sx: i32, sy: i32| {
        let o = ((sy - area.y0) as usize * aw + (sx - area.x0) as usize) * 4;
        u32::from_le_bytes([dst[o], dst[o + 1], dst[o + 2], dst[o + 3]])
    };

    let was = effects::border();
    let (inactive, weight) = (0x3318_1C20u32, 0x33u32);
    effects::set_border(1, 0x7741_484D, inactive);
    let plain = compose_one(&window(fx));
    let dimmed = compose_one(&window(Effects { dim: 0.05, ..fx }));
    effects::set_border(was.size, was.active, was.inactive);
    let _ = shm::destroy(handle);

    // Every pixel of the top-left corner square of the border's box, and the
    // pixel of nothing outside it
    let s = Shape { rounding: 22.0, power: 2.5, border: 1.0 };
    let unit = |v: f32| (v * 255.0 + 0.5).clamp(0.0, 255.0) as u32;
    let mut wrong = 0u32;
    let mut first = None;
    for j in -1..s.corner_reach() {
        for i in -1..s.corner_reach() {
            // `i`, `j` in the border's box, which starts one pixel out
            let (sx, sy) = (x - 1 + i, y - 1 + j);
            let mut want = 0u32;
            if i >= 0 && j >= 0 {
                let (wx, wy) = (sx - x, sy - y);
                if wx >= 0 && wy >= 0 {
                    let cov = unit(shape::surface_coverage(wx as f32 + 0.5, wy as f32 + 0.5,
                                                           ww as f32, wh as f32, &s));
                    if cov > 0 {
                        want = over(want, colour, cov);
                    }
                }
                let ba = unit(shape::border_alpha(i as f32 + 0.5, j as f32 + 0.5,
                                                  (ww + 2) as f32, (wh + 2) as f32, &s));
                if ba > 0 {
                    want = over(want, inactive, (ba * weight + 127) / 255);
                }
            }
            let got = at(&plain, sx, sy);
            if got != want {
                wrong += 1;
                first.get_or_insert((sx - x, sy - y, got, want));
            }
        }
    }
    let centre = at(&plain, x + ww / 2, y + wh / 2);
    let edge = at(&plain, x + ww, y + wh / 2);
    let past = at(&plain, x + ww + 1, y + wh / 2);
    let tint = ((1.0f32 - 0.05) * 256.0) as u32;
    let mut darker = 0xFF00_0000u32;
    for shift in [0, 8, 16] {
        darker |= ((((colour >> shift) & 0xff) * tint) >> 8) << shift;
    }
    let dim_centre = at(&dimmed, x + ww / 2, y + wh / 2);
    let ok = wrong == 0
        && centre == colour | 0xFF00_0000
        && edge == over(0, inactive, weight)
        && past == 0
        && dim_centre == darker;
    t.check(NAME, ok,
            format!("corner pixels wrong {} (first {:?}); centre {:08x}, border at the edge {:08x} \
                     (want {:08x}), past it {:08x}, dimmed centre {:08x} (want {:08x})",
                    wrong, first, centre, edge, over(0, inactive, weight), past,
                    dim_centre, darker));
}

/// The three window-shape numbers go down the param channel to the desktop and
/// to one window, clamped as Hyprland clamps them
pub(super) fn test_shape_params(t: &mut Runner) {
    use crate::mayland::protocol::wm::param;
    use crate::mayland::render::{Effects, Fx};

    let mut fx = Fx::default();
    let took = fx.set(param::ROUNDING_POWER, 2.5, true)
        && fx.set(param::INACTIVE_ALPHA, 0.9, true)
        && fx.set(param::DIM, 0.05, true);
    let e = fx.over(Effects::DEFAULT);
    let set = (e.power - 2.5).abs() < 1e-6 && (e.inactive - 0.9).abs() < 1e-6
        && (e.dim - 0.05).abs() < 1e-6;
    // A rule's power is not clamped above, and a window's dim is a fraction
    let mut wild = Fx::default();
    wild.set(param::ROUNDING_POWER, 25.0, true);
    wild.set(param::DIM, 3.0, true);
    let w = wild.over(Effects::DEFAULT);
    let clamped = w.power == 25.0 && w.dim == 1.0;
    // Unset is the desktop's again
    fx.set(param::DIM, 0.0, false);
    let back = fx.over(Effects::DEFAULT).dim == Effects::DEFAULT.dim;
    let defaults = Effects::DEFAULT.power == 2.0 && Effects::DEFAULT.inactive == 1.0
        && Effects::DEFAULT.dim == 0.0;
    t.check("compose: rounding_power, inactive_opacity and dim travel as params",
            took && set && clamped && back && defaults,
            format!("took {}, set {:?}, rule power/dim {}/{}, unset {}, defaults {}",
                    took, (e.power, e.inactive, e.dim), w.power, w.dim, back, defaults));
}
