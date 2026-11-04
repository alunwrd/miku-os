// The shape of a window, as Hyprland draws it (`dev/mayland-window-shape.md`)
//
// Transcribed from Hyprland v0.56.2:
//   - `surface_coverage` is `rounding.glsl`: how much of a window's own pixel
//     its rounded corner leaves;
//   - `border_alpha` is `getBorder` in `border.glsl`: how much of the border's
//     color a pixel of the border's box takes;
//   - `Shape::base` is `CWindow::rounding()`, which scales the configured
//     radius by `power / 2` (22 at 2.5 is 27.5 pixels), and the radii built
//     on it are `CSurfacePassElement` (one less, for the surface) and
//     `CHyprBorderDecoration::draw` (the border's two).
//
// `kernel/drivers/gpu/nvidia/shaders/composite.cu` has the same functions, line
// for line.
//
// Coordinates are a pixel's center relative to the box's top-left, `x + 0.5`
// for the pixel at `x`, as `gl_FragCoord` is.

/// `SMOOTHING_CONSTANT`: how far either side of the curve the edge fades
pub const SMOOTHING: f32 = core::f32::consts::PI / 5.346_657_9;

/// A window's outline: the desktop's three numbers, or a window rule's
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Shape {
    /// `decoration { rounding }`, as written, not yet scaled by the power
    pub rounding: f32,
    /// `rounding_power`: 2 is a circle, higher is squarer
    pub power: f32,
    /// `border_size`
    pub border: f32,
}

impl Shape {
    /// The radius everything is built on: `rounding · power / 2`, Hyprland's
    /// "perceived roundness"
    pub fn base(&self) -> f32 {
        self.rounding * (self.power / 2.0)
    }

    /// The window's own pixels are cut one pixel inside `base`, so the border's
    /// inner curve lies outside the surface's and the backdrop never shows
    /// between them
    pub fn surface_radius(&self) -> f32 {
        (self.base() - 1.0).max(0.0)
    }

    /// The border's inner and outer radius, in its own box. The outer is pulled
    /// in by `border·(√2 − 1)·(2 − power)` below power 2, and is the inner from
    /// 2 up
    pub fn border_radii(&self) -> (f32, f32) {
        let base = self.base();
        let inner = if base == 0.0 { 0.0 } else { base + self.border };
        let correction = self.border * (core::f32::consts::SQRT_2 - 1.0) * (2.0 - self.power).max(0.0);
        (inner, base + self.border - correction)
    }

    /// How far in from a box's corner the curve can reach, in whole pixels: a
    /// square of this side at each corner holds every pixel not plainly inside
    pub fn corner_reach(&self) -> i32 {
        let (inner, outer) = self.border_radii();
        let r = self.surface_radius().max(inner).max(outer) + SMOOTHING;
        r as i32 + 2
    }
}

/// `smoothstep(0, 1, t)`
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `(x^p + y^p)^(1/p)`: `distanceWithRounding`. Neither may be negative, which
/// GLSL leaves undefined
#[inline]
pub fn distance(x: f32, y: f32, p: f32) -> f32 {
    pow(pow(x, p) + pow(y, p), 1.0 / p)
}

/// How much of a window's own pixel survives its corner, 0 to 1. `rounding()`
/// in `rounding.glsl`, for a pixel center `(px, py)` in a box of `w` by `h`.
/// The fold puts every corner on one, the `1 / size` nudge is Hyprland's, and a
/// zero radius is a square
pub fn surface_coverage(px: f32, py: f32, w: f32, h: f32, s: &Shape) -> f32 {
    let r = s.surface_radius();
    if r <= 0.0 {
        return 1.0;
    }
    let qx = (px - w * 0.5).abs() - (w * 0.5 - r) + 1.0 / w;
    let qy = (py - h * 0.5).abs() - (h * 0.5 - r) + 1.0 / h;
    if qx + qy <= r {
        return 1.0;
    }
    let d = distance(qx.max(0.0), qy.max(0.0), s.power);
    if d > r + SMOOTHING {
        return 0.0;
    }
    1.0 - smoothstep((d - r + SMOOTHING) / (SMOOTHING * 2.0))
}

/// How much of the border's color a pixel takes, 0 to 1. `getBorder` in
/// `border.glsl`, for a pixel center `(px, py)` in the border's box of `bw` by
/// `bh`, the window's grown by the border on each side. The color's alpha and
/// the window's opacity multiply this; it is drawn over the surface
pub fn border_alpha(px: f32, py: f32, bw: f32, bh: f32, s: &Shape) -> f32 {
    let thick = s.border;
    if thick < 1.0 {
        return 0.0;
    }
    let (radius, outer) = s.border_radii();
    let (fx, fy) = ((px - bw * 0.5).abs(), (py - bh * 0.5).abs());
    let (cx, cy) = (fx - (bw * 0.5 - radius) + 1.0 / bw, fy - (bh * 0.5 - radius) + 1.0 / bh);
    let (ox, oy) = (fx - (bw * 0.5 - outer) + 1.0 / bw, fy - (bh * 0.5 - outer) + 1.0 / bh);
    let half = thick / 2.0;

    if cx.min(cy) > 0.0 && radius > 0.0 {
        let d = distance(cx, cy, s.power);
        if d < radius - half {
            // Below the band's middle: rising across the surface's edge, so the
            // two meet with no seam
            return smoothstep((d - radius + thick + SMOOTHING) / (SMOOTHING * 2.0));
        }
        if ox.min(oy) > 0.0 {
            // Above it: falling across the outer edge
            return 1.0 - smoothstep((distance(ox, oy, s.power) - outer + SMOOTHING) / (SMOOTHING * 2.0));
        }
        // GLSL's third branch reads `distOuter` of a negative coordinate, which
        // is NaN and never true, so the pixel falls through to the straight
        // edges
    }
    // Straight edges: solid within `thick` of the box, nothing inside
    let smallest = px.min(bw - px).min(py).min(bh - py);
    if smallest > thick { 0.0 } else { 1.0 }
}

// pow without libm
//
// `libmay` is `no_std` and the kernel soft-float.
// `pow(x, p) = exp2(p · log2 x)` for `x > 0`, as the card computes GLSL's
// `pow`. Each half is good to about 1e-7.

/// `log2(x)` for `x > 0`
fn log2(x: f32) -> f32 {
    if !(x > 0.0) {
        return f32::NEG_INFINITY;
    }
    let bits = x.to_bits();
    // A subnormal is near enough nothing
    if bits < 0x0080_0000 {
        return -127.0;
    }
    let mut e = ((bits >> 23) & 0xff) as i32 - 127;
    let mut m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    if m > core::f32::consts::SQRT_2 {
        m *= 0.5;
        e += 1;
    }
    // ln m = 2·atanh(t), t = (m − 1)/(m + 1), |t| < 0.172
    let t = (m - 1.0) / (m + 1.0);
    let t2 = t * t;
    let ln = 2.0 * t * (1.0 + t2 * (1.0 / 3.0 + t2 * (1.0 / 5.0 + t2 * (1.0 / 7.0 + t2 * (1.0 / 9.0)))));
    e as f32 + ln * core::f32::consts::LOG2_E
}

/// `2^y`
fn exp2(y: f32) -> f32 {
    if y < -126.0 {
        return 0.0;
    }
    if y > 127.0 {
        return f32::INFINITY;
    }
    let mut n = y as i32;
    if (n as f32) > y {
        n -= 1;
    }
    let z = (y - n as f32) * core::f32::consts::LN_2;
    let e = 1.0 + z * (1.0 + z * (1.0 / 2.0 + z * (1.0 / 6.0 + z * (1.0 / 24.0
        + z * (1.0 / 120.0 + z * (1.0 / 720.0 + z * (1.0 / 5040.0 + z * (1.0 / 40320.0))))))));
    e * f32::from_bits(((n + 127) as u32) << 23)
}

/// `x^p` for `x >= 0`
pub fn pow(x: f32, p: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    exp2(p * log2(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    const S64: f64 = std::f64::consts::PI / 5.34665792551;

    fn ss(t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    }

    /// `rounding.glsl` in f64, written from the shader and not the code above
    fn surface_ref(px: f64, py: f64, w: f64, h: f64, rounding: f64, power: f64) -> f64 {
        let r = rounding * power / 2.0 - 1.0;
        if r <= 0.0 {
            return 1.0;
        }
        let qx = (px - w / 2.0).abs() - (w / 2.0 - r) + 1.0 / w;
        let qy = (py - h / 2.0).abs() - (h / 2.0 - r) + 1.0 / h;
        if qx + qy > r {
            let d = (qx.powf(power) + qy.powf(power)).powf(1.0 / power);
            if d > r + S64 {
                return 0.0;
            }
            return 1.0 - ss((d - r + S64) / (S64 * 2.0));
        }
        1.0
    }

    /// `getBorder` in f64, likewise
    fn border_ref(px: f64, py: f64, bw: f64, bh: f64, rounding: f64, power: f64, thick: f64) -> f64 {
        let base = rounding * power / 2.0;
        let radius = base + thick;
        let outer = base + thick - thick * (2f64.sqrt() - 1.0) * (2.0 - power).max(0.0);
        let fx = (px - bw / 2.0).abs();
        let fy = (py - bh / 2.0).abs();
        let p = (fx - (bw / 2.0 - radius) + 1.0 / bw, fy - (bh / 2.0 - radius) + 1.0 / bh);
        let o = (fx - (bw / 2.0 - outer) + 1.0 / bw, fy - (bh / 2.0 - outer) + 1.0 / bh);
        let dist = |a: (f64, f64)| (a.0.powf(power) + a.1.powf(power)).powf(1.0 / power);
        if p.0.min(p.1) > 0.0 {
            let d = dist(p);
            let dout = dist(o);
            let h = thick / 2.0;
            if d < radius - h {
                return ss((d - radius + thick + S64) / (S64 * 2.0));
            } else if o.0.min(o.1) > 0.0 {
                return 1.0 - ss((dout - outer + S64) / (S64 * 2.0));
            } else if dout < outer - h {
                return 1.0;
            }
        }
        let smallest = px.min(bw - px).min(py).min(bh - py);
        if smallest > thick { 0.0 } else { 1.0 }
    }

    #[test]
    fn pow_matches_std() {
        let mut worst = 0f64;
        for i in 1..4000 {
            let x = i as f32 * 0.0137;
            for &p in &[1.0f32, 1.5, 2.0, 2.5, 3.0, 4.0, 1.0 / 2.5, 0.5, 10.0] {
                let ours = pow(x, p) as f64;
                let want = (x as f64).powf(p as f64);
                let rel = ((ours - want) / want.max(1e-30)).abs();
                worst = worst.max(rel);
            }
        }
        // At p = 10 and x = 55 the exponent `p · log2 x` is 58, whose f32 step
        // is 4e-6, the floor. The card's `ex2(lg2)` has the same one
        assert!(worst < 1e-5, "pow off by {worst}");
        assert_eq!(pow(0.0, 2.5), 0.0);
    }

    #[test]
    fn hyprland_numbers() {
        // 22 at 2.5 is 27.5, the surface one less, the border's two radii level
        // at this power
        let s = Shape { rounding: 22.0, power: 2.5, border: 1.0 };
        assert!((s.base() - 27.5).abs() < 1e-6);
        assert!((s.surface_radius() - 26.5).abs() < 1e-6);
        let (i, o) = s.border_radii();
        assert!((i - 28.5).abs() < 1e-6 && (o - 28.5).abs() < 1e-6);
        // Below power 2 the outer radius is pulled in
        let c = Shape { rounding: 10.0, power: 1.5, border: 2.0 };
        let (ci, co) = c.border_radii();
        assert!(co < ci);
    }

    /// Every pixel of a corner square and a few beyond, against the f64
    /// transcription
    #[test]
    fn surface_matches_the_shader() {
        for &(rounding, power) in &[(22.0, 2.5), (18.0, 2.5), (4.0, 2.0), (12.0, 2.0), (10.0, 4.0), (8.0, 1.5)] {
            let s = Shape { rounding, power, border: 1.0 };
            let (w, h) = (1904.0f32, 1031.0f32);
            let reach = s.corner_reach();
            for y in 0..reach {
                for x in 0..reach {
                    for &(fx, fy) in &[(x as f32, y as f32), (w - 1.0 - x as f32, h - 1.0 - y as f32)] {
                        let (px, py) = (fx + 0.5, fy + 0.5);
                        let ours = surface_coverage(px, py, w, h, &s) as f64;
                        let want = surface_ref(px as f64, py as f64, w as f64, h as f64, rounding as f64, power as f64);
                        assert!((ours - want).abs() < 2e-3,
                            "surface r={rounding} p={power} at ({fx},{fy}): {ours} vs {want}");
                    }
                }
            }
        }
    }

    #[test]
    fn border_matches_the_shader() {
        for &(rounding, power, thick) in &[(22.0, 2.5, 1.0), (22.0, 2.5, 2.0), (4.0, 2.0, 2.0), (12.0, 1.5, 3.0), (10.0, 4.0, 1.0)] {
            let s = Shape { rounding, power, border: thick };
            let (bw, bh) = (1906.0f32, 1033.0f32);
            let reach = s.corner_reach() + thick as i32;
            for y in 0..reach {
                for x in 0..reach {
                    let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                    let ours = border_alpha(px, py, bw, bh, &s) as f64;
                    let want = border_ref(px as f64, py as f64, bw as f64, bh as f64, rounding as f64, power as f64, thick as f64);
                    assert!((ours - want).abs() < 2e-3,
                        "border r={rounding} p={power} b={thick} at ({x},{y}): {ours} vs {want}");
                }
            }
        }
    }

    /// The straight parts: the window's pixels whole, the border one pixel deep
    /// and nothing inside it
    #[test]
    fn straight_edges() {
        let s = Shape { rounding: 22.0, power: 2.5, border: 1.0 };
        let (w, h) = (800.0, 600.0);
        assert_eq!(surface_coverage(0.5, 300.5, w, h, &s), 1.0);
        assert_eq!(surface_coverage(400.5, 599.5, w, h, &s), 1.0);
        let (bw, bh) = (802.0, 602.0);
        assert_eq!(border_alpha(0.5, 300.5, bw, bh, &s), 1.0);
        assert_eq!(border_alpha(1.5, 300.5, bw, bh, &s), 0.0);
        assert_eq!(border_alpha(401.5, 601.5, bw, bh, &s), 1.0);
        // A corner pixel well outside the curve is nothing, for both
        assert_eq!(surface_coverage(0.5, 0.5, w, h, &s), 0.0);
        assert_eq!(border_alpha(0.5, 0.5, bw, bh, &s), 0.0);
    }
}
