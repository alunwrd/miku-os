// Rectangle triangles, matching Qt Quick's output (`rectangles_are_qts` against
// qml6's render of `tests/qtref/fixtures/rects.qml`)
//
// Shape: an outline around the box. A rounded corner is a quarter circle in
// radius·π/6 segments, rounded up, from 3 to 18. Radius is at most 0.4999 of
// the shorter side, and at most half a pixel means square. A border is the band
// between the outline and the box inset by its width, whose corners keep the
// remaining radius (at least 0.01 px).
//
// Antialiasing: a one-pixel ramp across the edge, from two copies of its points
// moved half a pixel along each item axis (`gpu::Vertex::dx`, `dy`):
// - the fill's side toward the middle, stopping at half the shorter side from
//   its own edge;
// - the outside, away from the middle;
// - a border's sides into the border, by 0.49 of its width outward, at most
//   half a pixel.
//
// Gradient: linear between stops. The fill is cut into rows across the axis at
// every outline point and stop. Colors interpolate in premultiplied 8-bit
// channels, truncated

use super::gpu::{Geometry, Rgba, Vertex};

/// What a Rectangle asks for (`rect_spec` in `scene/mod.rs`)
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub w: f32,
    pub h: f32,
    /// Straight `0xAARRGGBB`
    pub color: u32,
    pub border_color: u32,
    /// The border's width, already rounded to a pixel where it is aligned
    pub pen_width: f32,
    pub radius: f32,
    /// `topLeftRadius`, `topRightRadius`, `bottomLeftRadius`,
    /// `bottomRightRadius`, where set
    pub corners: [Option<f32>; 4],
    pub antialiasing: bool,
    /// Position and straight colour, in order
    pub stops: Vec<(f32, u32)>,
    pub vertical: bool,
}

impl Spec {
    /// A fingerprint of everything the geometry is made from
    pub fn print(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        (self.w.to_bits(), self.h.to_bits(), self.color, self.border_color, self.pen_width.to_bits(), self.radius.to_bits()).hash(&mut h);
        self.corners.map(|c| c.map(f32::to_bits)).hash(&mut h);
        (self.antialiasing, self.vertical).hash(&mut h);
        self.stops.iter().map(|(p, c)| (p.to_bits(), *c)).collect::<Vec<_>>().hash(&mut h);
        h.finish()
    }
}

/// Outline point and outward direction: from the center on a curve, (±1, 0) or
/// (0, ±1) on a side, (±1, ±1) at a square corner
#[derive(Clone, Copy, Debug)]
struct Pt {
    x: f32,
    y: f32,
    nx: f32,
    ny: f32,
}

/// How many straight pieces a corner of radius `r` is cut into
fn pieces(r: f32) -> usize {
    if r <= 0.0 { 0 } else { ((r as f64 * std::f64::consts::PI / 6.0).ceil() as usize).clamp(3, 18) }
}

/// Outline of `(x0, y0)`-`(x1, y1)` with radius `r` and `n` segments per
/// corner, clockwise from the top left corner's start on the left side
fn outline(x0: f32, y0: f32, x1: f32, y1: f32, r: [f32; 4], n: [usize; 4]) -> Vec<Pt> {
    let mut out = Vec::new();
    let corners = [(x0 + r[0], y0 + r[0], -1.0, -1.0), (x1 - r[1], y0 + r[1], 1.0, -1.0),
                   (x1 - r[2], y1 - r[2], 1.0, 1.0), (x0 + r[3], y1 - r[3], -1.0, 1.0)];
    for (c, &(cx, cy, sx, sy)) in corners.iter().enumerate() {
        if r[c] <= 0.0 || n[c] == 0 {
            out.push(Pt { x: cx, y: cy, nx: sx, ny: sy });
            continue;
        }
        for k in 0..=n[c] {
            let (s, co) = ((k as f64 / n[c] as f64) * std::f64::consts::FRAC_PI_2).sin_cos();
            let (s, co) = (s as f32, co as f32);
            // From the side before the corner to the side after it
            let (ux, uy) = match c {
                0 => (-co, -s),
                1 => (s, -co),
                2 => (co, s),
                _ => (-s, co),
            };
            out.push(Pt { x: cx + r[c] * ux, y: cy + r[c] * uy, nx: ux, ny: uy });
        }
    }
    out
}

/// The gradient's colour at `t` along it
fn colour_at(stops: &[(f32, u32)], t: f32) -> Rgba {
    let t = t.clamp(0.0, 1.0);
    if t < stops[0].0 {
        return Rgba::from_argb(stops[0].1);
    }
    for pair in stops.windows(2) {
        let ((p0, c0), (p1, c1)) = (pair[0], pair[1]);
        if p0 <= t && t < p1 {
            let f = (t - p0) / (p1 - p0);
            return Rgba::from_argb(c0).times(1.0 - f).plus(Rgba::from_argb(c1).times(f));
        }
    }
    Rgba::from_argb(stops[stops.len() - 1].1)
}

/// `pts` as rows across the `vertical` axis, at each point and each of `cuts`,
/// clockwise, so row `k` is points `k` and `len - 1 - k`
fn rows(pts: &[Pt], vertical: bool, cuts: &[f32]) -> Vec<Pt> {
    let along = |p: &Pt| if vertical { p.y } else { p.x };
    let across = |p: &Pt| if vertical { p.x } else { p.y };
    let mut at: Vec<f32> = pts.iter().map(along).chain(cuts.iter().copied()).collect();
    at.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    at.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
    let mut low = Vec::with_capacity(at.len());
    let mut high = Vec::with_capacity(at.len());
    for &v in &at {
        // Where the line at `v` meets the outline, with the outward direction
        // interpolated
        let mut hits: Vec<(f32, f32, f32)> = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let (pa, pb) = (along(&a), along(&b));
            if (pa - v).abs() < 1e-4 {
                hits.push((across(&a), a.nx, a.ny));
            }
            if (pa - v) * (pb - v) < 0.0 {
                let t = (v - pa) / (pb - pa);
                hits.push((across(&a) + t * (across(&b) - across(&a)), a.nx + t * (b.nx - a.nx), a.ny + t * (b.ny - a.ny)));
            }
        }
        let lo = hits.iter().copied().reduce(|m, h| if h.0 < m.0 { h } else { m });
        let hi = hits.iter().copied().reduce(|m, h| if h.0 > m.0 { h } else { m });
        let (Some(lo), Some(hi)) = (lo, hi) else { continue };
        let pt = |(s, nx, ny): (f32, f32, f32)| if vertical { Pt { x: s, y: v, nx, ny } } else { Pt { x: v, y: s, nx, ny } };
        low.push(pt(lo));
        high.push(pt(hi));
    }
    // Clockwise on screen: top left to right, or down the right side
    let (mut first, second) = if vertical { (high, low) } else { (low, high) };
    first.extend(second.into_iter().rev());
    first
}

/// Geometry under construction, one outline copy at a time
struct Build {
    g: Geometry,
}

impl Build {
    fn copy(&mut self, pts: &[Pt], colour: impl Fn(&Pt) -> Rgba, offset: impl Fn(&Pt) -> (f32, f32)) -> Vec<u32> {
        pts.iter().map(|p| {
            let (dx, dy) = offset(p);
            self.g.verts.push(Vertex { x: p.x, y: p.y, color: colour(p), dx, dy });
            (self.g.verts.len() - 1) as u32
        }).collect()
    }

    /// The ring between two copies of the same outline
    fn ring(&mut self, inner: &[u32], outer: &[u32]) {
        let n = inner.len();
        for i in 0..n {
            let j = (i + 1) % n;
            self.g.tris.push([inner[i], outer[i], outer[j]]);
            self.g.tris.push([inner[i], outer[j], inner[j]]);
        }
    }

    /// Band between two outlines whose points need not pair: steps along
    /// whichever is behind going around the middle `c`
    fn band(&mut self, inner: &[u32], outer: &[u32], c: (f32, f32)) {
        if inner.len() == outer.len() {
            self.ring(inner, outer);
            return;
        }
        let turn = |list: &[u32]| -> Vec<(u32, f64)> {
            // Clockwise on the screen, from the middle of the left side
            let mut v: Vec<(u32, f64)> = list.iter().map(|&i| {
                let p = &self.g.verts[i as usize];
                let a = ((p.y - c.1) as f64).atan2((p.x - c.0) as f64) - std::f64::consts::PI;
                (i, a.rem_euclid(std::f64::consts::TAU))
            }).collect();
            let start = v.iter().enumerate().min_by(|a, b| a.1 .1.total_cmp(&b.1 .1)).map(|(k, _)| k).unwrap_or(0);
            v.rotate_left(start);
            // Reversed if going the other way, keeping the first point first
            if v.len() > 2 && v[v.len() - 1].1 < v[1].1 {
                v[1..].reverse();
            }
            v
        };
        let (a, b) = (turn(inner), turn(outer));
        let (n, m) = (a.len(), b.len());
        if n == 0 || m == 0 {
            return;
        }
        let key = |v: &[(u32, f64)], k: usize| if k < v.len() { v[k].1 } else { v[k - v.len()].1 + std::f64::consts::TAU };
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            let (ai, bj) = (a[i % n].0, b[j % m].0);
            if j >= m || (i < n && key(&a, i + 1) <= key(&b, j + 1)) {
                self.g.tris.push([ai, bj, a[(i + 1) % n].0]);
                i += 1;
            } else {
                self.g.tris.push([ai, bj, b[(j + 1) % m].0]);
                j += 1;
            }
        }
    }
}

/// The geometry, or None for a Rectangle that draws nothing
pub fn geometry(s: &Spec) -> Option<Geometry> {
    let (w, h) = (s.w, s.h);
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let alpha = |c: u32| c >> 24;
    if s.stops.is_empty() && alpha(s.color) == 0 && (s.pen_width <= 0.0 || alpha(s.border_color) == 0) {
        return None;
    }
    let short = w.min(h);
    let pen = (short * 0.5).min(s.pen_width.max(0.0));
    let radius = |given: Option<f32>| {
        let r = given.unwrap_or(s.radius).min(short * 0.4999);
        if r <= 0.5 { 0.0 } else { r }
    };
    // Top left, top right, bottom right, bottom left
    let r = [radius(s.corners[0]), radius(s.corners[1]), radius(s.corners[3]), radius(s.corners[2])];
    let n = r.map(pieces);
    let outer = outline(0.0, 0.0, w, h, r, n);
    let inner = (pen > 0.0).then(|| {
        let ri = r.map(|r| if r > 0.0 { (r - pen).max(0.01) } else { 0.0 });
        outline(pen, pen, w - pen, h - pen, ri, n)
    });

    // The fill's outline, in rows when it is a gradient
    let length = if s.vertical { h } else { w };
    let gradient = !s.stops.is_empty();
    let edge = inner.as_ref().unwrap_or(&outer);
    let fill_pts = if gradient {
        let cuts: Vec<f32> = s.stops.iter().map(|&(p, _)| p)
            .filter(|&p| p > pen / length && p < 1.0 - pen / length)
            .map(|p| p * length)
            .collect();
        rows(edge, s.vertical, &cuts)
    } else {
        edge.clone()
    };
    let fill_colour = Rgba::from_argb(s.color);
    let fill_of = |p: &Pt| if gradient { colour_at(&s.stops, (if s.vertical { p.y } else { p.x }) / length) } else { fill_colour };
    let border = Rgba::from_argb(s.border_color);
    let has_fill = alpha(s.color) > 0 || gradient;

    // Moves: toward the middle (stopping at half the shorter side from the
    // edge), away from it, and outward
    let delta = short * 0.5;
    let side = |v: f32, len: f32| if v < len * 0.5 { -1.0f32 } else { 1.0 };
    let inward = |p: &Pt| {
        let (sx, sy) = (side(p.x, w), side(p.y, h));
        let (dx, dy) = (if sx < 0.0 { p.x } else { w - p.x }, if sy < 0.0 { p.y } else { h - p.y });
        (-sx * (delta - dx).max(0.0), -sy * (delta - dy).max(0.0))
    };
    let outward = |p: &Pt| (side(p.x, w) * delta, side(p.y, h) * delta);
    let along = |k: f32| move |p: &Pt| (k * 0.49 * pen * p.nx, k * 0.49 * pen * p.ny);
    let still = |_: &Pt| (0.0, 0.0);

    let mut b = Build { g: Geometry::default() };
    let middle = (w * 0.5, h * 0.5);
    let fill_idx = if s.antialiasing { b.copy(&fill_pts, fill_of, inward) } else { b.copy(&fill_pts, fill_of, still) };
    if has_fill {
        if gradient {
            // Row by row, each cut from one side's point to the other's next
            let len = fill_idx.len();
            for k in 0..(len / 2).saturating_sub(1) {
                let (lo0, lo1, hi0, hi1) = (fill_idx[k], fill_idx[k + 1], fill_idx[len - 1 - k], fill_idx[len - 2 - k]);
                b.g.tris.push([hi0, lo0, hi1]);
                b.g.tris.push([lo0, hi1, lo1]);
            }
        } else {
            for k in 1..fill_idx.len().saturating_sub(1) {
                b.g.tris.push([fill_idx[0], fill_idx[k], fill_idx[k + 1]]);
            }
        }
    }
    let transparent = |_: &Pt| Rgba::TRANSPARENT;
    let in_border = |_: &Pt| border;
    match (s.antialiasing, pen > 0.0) {
        (true, false) => {
            let out = b.copy(&fill_pts, transparent, outward);
            b.ring(&fill_idx, &out);
        }
        (true, true) => {
            let edge_in = b.copy(&fill_pts, in_border, along(1.0));
            b.ring(&fill_idx, &edge_in);
            let edge_out = b.copy(&outer, in_border, along(-1.0));
            b.band(&edge_in, &edge_out, middle);
            let out = b.copy(&outer, transparent, outward);
            b.ring(&edge_out, &out);
        }
        (false, true) => {
            let edge_in = b.copy(&fill_pts, in_border, still);
            let edge_out = b.copy(&outer, in_border, still);
            b.band(&edge_in, &edge_out, middle);
        }
        (false, false) => {}
    }
    Some(b.g)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scene::gpu::fill;
    use crate::scene::paint::Image;
    use crate::scene::xf::Xf;

    fn drawn(s: &Spec, w: i32, h: i32) -> Image {
        let mut img = Image::new(w, h);
        let all = img.bounds();
        fill(&mut img, all, &geometry(s).unwrap(), &Xf::IDENTITY, 1.0);
        img
    }

    #[test]
    fn a_rounded_corner_is_antialiased_and_the_middle_is_solid() {
        let img = drawn(&Spec { w: 20.0, h: 20.0, color: 0xFFFFFFFF, radius: 8.0, antialiasing: true, ..Spec::default() }, 20, 20);
        assert_eq!(img.px[0] >> 24, 0, "the corner pixel is outside the curve");
        assert_eq!(img.px[10 * 20 + 10] >> 24, 255);
        let edge = img.px[2 * 20 + 2] >> 24;
        assert!(edge > 0 && edge < 255, "{edge}");
    }

    #[test]
    fn a_border_is_inside_the_edge() {
        let img = drawn(&Spec { w: 10.0, h: 10.0, color: 0xFF000000, border_color: 0xFFFF0000, pen_width: 1.0, ..Spec::default() }, 10, 10);
        assert_eq!(img.px[0], 0xFFFF0000);
        assert_eq!(img.px[9 * 10 + 9], 0xFFFF0000);
        assert_eq!(img.px[5 * 10 + 5], 0xFF000000);
    }

    #[test]
    fn a_gradient_is_straight_between_its_stops() {
        let s = Spec { w: 10.0, h: 11.0, stops: vec![(0.0, 0xFF000000), (1.0, 0xFFFFFFFF)], vertical: true, ..Spec::default() };
        let img = drawn(&s, 10, 11);
        let grey = |y: usize| img.px[y * 10 + 3] & 0xff;
        assert!((1..11).all(|y| grey(y) > grey(y - 1)), "lighter all the way down");
        assert!((grey(5) as i32 - 127).abs() <= 1, "the middle is half way: {}", grey(5));
    }
}
