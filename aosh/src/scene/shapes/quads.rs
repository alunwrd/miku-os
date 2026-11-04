// Shape paths as the curve renderer's lines and quadratics
//
// A cubic becomes two quadratics: halved, each with its control point where the
// half's end tangents cross. This puts each half's middle slightly outside a
// circle's arc (0.93 px for a quarter of radius 300, as in qml6). Coverage is
// measured from these, not the cubics (`shapes_are_qts`)

use crate::scene::canvas::shape::{split, Seg, Sub};

pub type P = (f64, f64);

#[derive(Clone, Copy, Debug)]
pub enum Piece {
    Line(P, P),
    Quad(P, P, P),
}

/// A subpath as pieces, and whether it is closed (no caps)
pub struct Run {
    pub pieces: Vec<Piece>,
    pub closed: bool,
}

pub fn runs(subs: &[Sub]) -> Vec<Run> {
    subs.iter().map(|s| {
        let mut pieces = Vec::new();
        let mut at = s.start;
        for seg in &s.segs {
            at = match *seg {
                Seg::Line(x, y) => {
                    pieces.push(Piece::Line(at, (x, y)));
                    (x, y)
                }
                Seg::Cubic(c1, c2, to) => {
                    let (a, b) = split([at, c1, c2, to], 0.5);
                    pieces.push(quad(a));
                    pieces.push(quad(b));
                    to
                }
            };
        }
        let closed = s.closed || (!s.segs.is_empty() && near(at, s.start));
        if s.closed && !near(at, s.start) {
            pieces.push(Piece::Line(at, s.start));
        }
        Run { pieces, closed }
    }).filter(|r| !r.pieces.is_empty()).collect()
}

/// A subpath as flattened lines, for the geometry renderer
pub fn lines(subs: &[Sub]) -> Vec<Run> {
    subs.iter().map(|s| {
        let pts = s.polyline();
        let pieces: Vec<Piece> = pts.windows(2).map(|w| Piece::Line(w[0], w[1])).collect();
        let closed = s.closed || (pts.len() > 2 && near(pts[0], pts[pts.len() - 1]));
        Run { pieces, closed }
    }).filter(|r| !r.pieces.is_empty()).collect()
}

fn near(a: P, b: P) -> bool {
    (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9
}

/// A cubic as one quadratic: control point at the tangent crossing, or where
/// the tangents do not cross, the point preserving the cubic's middle
fn quad([p0, c1, c2, p3]: [P; 4]) -> Piece {
    let t0 = if near(c1, p0) { (c2.0 - p0.0, c2.1 - p0.1) } else { (c1.0 - p0.0, c1.1 - p0.1) };
    let t3 = if near(c2, p3) { (c1.0 - p3.0, c1.1 - p3.1) } else { (c2.0 - p3.0, c2.1 - p3.1) };
    let cross = t0.0 * t3.1 - t0.1 * t3.0;
    let mid = ((3.0 * (c1.0 + c2.0) - p0.0 - p3.0) / 4.0, (3.0 * (c1.1 + c2.1) - p0.1 - p3.1) / 4.0);
    if cross.abs() < 1e-12 {
        return if near(p0, p3) { Piece::Line(p0, p3) } else { Piece::Quad(p0, mid, p3) };
    }
    // p0 + s t0 = p3 + u t3
    let d = (p3.0 - p0.0, p3.1 - p0.1);
    let s = (d.0 * t3.1 - d.1 * t3.0) / cross;
    let c = (p0.0 + s * t0.0, p0.1 + s * t0.1);
    // A crossing behind an end is not a curve between them
    if s <= 0.0 || !c.0.is_finite() || !c.1.is_finite() {
        return Piece::Quad(p0, mid, p3);
    }
    Piece::Quad(p0, c, p3)
}

impl Piece {
    pub fn ends(&self) -> (P, P) {
        match *self {
            Piece::Line(a, b) | Piece::Quad(a, _, b) => (a, b),
        }
    }

    /// Its box, every point of it inside
    pub fn bounds(&self) -> (P, P) {
        match *self {
            Piece::Line(a, b) => ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1))),
            Piece::Quad(a, c, b) => (
                (a.0.min(b.0).min(c.0), a.1.min(b.1).min(c.1)),
                (a.0.max(b.0).max(c.0), a.1.max(b.1).max(c.1)),
            ),
        }
    }

    /// How far `p` is from the nearest point of it
    pub fn distance(&self, p: P) -> f64 {
        self.nearest(p).1
    }

    /// Where along it (0 to 1) its nearest point to `p` is, and how far
    pub fn nearest(&self, p: P) -> (f64, f64) {
        match *self {
            Piece::Line(a, b) => line_nearest(a, b, p),
            Piece::Quad(a, c, b) => {
                // B(t) = A t^2 + B t + C; the nearest point has (B(t) - p) .
                // B'(t) = 0, a cubic in t
                let ka = (a.0 - 2.0 * c.0 + b.0, a.1 - 2.0 * c.1 + b.1);
                let kb = (2.0 * (c.0 - a.0), 2.0 * (c.1 - a.1));
                let kc = (a.0 - p.0, a.1 - p.1);
                let dot = |u: P, v: P| u.0 * v.0 + u.1 * v.1;
                let aa = dot(ka, ka);
                if aa < 1e-12 {
                    return line_nearest(a, b, p);
                }
                let at = |t: f64| {
                    let q = (ka.0 * t * t + kb.0 * t + kc.0, ka.1 * t * t + kb.1 * t + kc.1);
                    q.0.hypot(q.1)
                };
                let mut best = if at(0.0) <= at(1.0) { (0.0, at(0.0)) } else { (1.0, at(1.0)) };
                for t in cubic_roots(2.0 * aa, 3.0 * dot(ka, kb), dot(kb, kb) + 2.0 * dot(ka, kc), dot(kb, kc)) {
                    if t > 0.0 && t < 1.0 && at(t) < best.1 {
                        best = (t, at(t));
                    }
                }
                best
            }
        }
    }

    /// Which way it leaves its start and arrives at its end, of length one
    pub fn tangents(&self) -> (P, P) {
        let unit = |v: P| {
            let l = v.0.hypot(v.1);
            if l > 0.0 { (v.0 / l, v.1 / l) } else { (0.0, 0.0) }
        };
        match *self {
            Piece::Line(a, b) => {
                let t = unit((b.0 - a.0, b.1 - a.1));
                (t, t)
            }
            Piece::Quad(a, c, b) => {
                let (s, e) = (unit((c.0 - a.0, c.1 - a.1)), unit((b.0 - c.0, b.1 - c.1)));
                let chord = unit((b.0 - a.0, b.1 - a.1));
                (if s == (0.0, 0.0) { chord } else { s }, if e == (0.0, 0.0) { chord } else { e })
            }
        }
    }

    /// Crossings with the horizontal line at `y`: +1 going down the screen, -1
    /// going up. The top end of a piece counts, the bottom does not, so pieces
    /// meeting on the line make one
    pub fn crossings(&self, y: f64, out: &mut Vec<(f64, i32)>) {
        match *self {
            Piece::Line(a, b) => line_crossing(a, b, y, out),
            Piece::Quad(a, c, b) => {
                // Splits at the y turning point, so each part is monotonic
                let den = a.1 - 2.0 * c.1 + b.1;
                let t = if den.abs() > 1e-12 { (a.1 - c.1) / den } else { -1.0 };
                let parts: &[(f64, f64)] = if t > 0.0 && t < 1.0 { &[(0.0, t), (t, 1.0)] } else { &[(0.0, 1.0)] };
                let at = |t: f64| {
                    let u = 1.0 - t;
                    (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1)
                };
                for &(t0, t1) in parts {
                    let (p0, p1) = (at(t0), at(t1));
                    if (p0.1 <= y) == (p1.1 <= y) {
                        continue;
                    }
                    // Bisects the part for the height
                    let (mut lo, mut hi) = (t0, t1);
                    let down = p1.1 > p0.1;
                    for _ in 0..50 {
                        let mid = (lo + hi) / 2.0;
                        if (at(mid).1 <= y) == down { lo = mid } else { hi = mid }
                    }
                    out.push((at((lo + hi) / 2.0).0, if down { 1 } else { -1 }));
                }
            }
        }
    }
}

fn line_nearest(a: P, b: P, p: P) -> (f64, f64) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (t, (a.0 + t * dx - p.0).hypot(a.1 + t * dy - p.1))
}

fn line_crossing(a: P, b: P, y: f64, out: &mut Vec<(f64, i32)>) {
    if (a.1 <= y) == (b.1 <= y) {
        return;
    }
    let t = (y - a.1) / (b.1 - a.1);
    out.push((a.0 + t * (b.0 - a.0), if b.1 > a.1 { 1 } else { -1 }));
}

/// The real roots of a t^3 + b t^2 + c t + d
fn cubic_roots(a: f64, b: f64, c: f64, d: f64) -> Vec<f64> {
    if a.abs() < 1e-12 {
        // b t^2 + c t + d
        if b.abs() < 1e-12 {
            return if c.abs() < 1e-12 { vec![] } else { vec![-d / c] };
        }
        let disc = c * c - 4.0 * b * d;
        if disc < 0.0 {
            return vec![];
        }
        let s = disc.sqrt();
        return vec![(-c + s) / (2.0 * b), (-c - s) / (2.0 * b)];
    }
    // t = x - b/3a: x^3 + p x + q
    let (b, c, d) = (b / a, c / a, d / a);
    let p = c - b * b / 3.0;
    let q = 2.0 * b * b * b / 27.0 - b * c / 3.0 + d;
    let shift = -b / 3.0;
    let disc = q * q / 4.0 + p * p * p / 27.0;
    if disc > 0.0 {
        let s = disc.sqrt();
        vec![(-q / 2.0 + s).cbrt() + (-q / 2.0 - s).cbrt() + shift]
    } else if p.abs() < 1e-12 {
        vec![shift]
    } else {
        // Three real roots, round the circle
        let r = (-p / 3.0).sqrt();
        let phi = (-q / (2.0 * r * r * r)).clamp(-1.0, 1.0).acos();
        (0..3).map(|k| 2.0 * r * ((phi + std::f64::consts::TAU * k as f64) / 3.0).cos() + shift).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_quarter_circle_is_two_quadratics_just_outside_it() {
        let mut segs = Vec::new();
        crate::scene::canvas::shape::arc_segs(&mut segs, &crate::scene::xf::Xf::IDENTITY, (0.0, 0.0), (300.0, 300.0), 0.0, std::f64::consts::FRAC_PI_2);
        let sub = Sub { start: (300.0, 0.0), segs, closed: false };
        let r = &runs(&[sub])[0];
        assert_eq!(r.pieces.len(), 2);
        // Its middle, an eighth of a turn in, 0.94 out
        let Piece::Quad(a, c, b) = r.pieces[0] else { panic!() };
        let m = ((a.0 + 2.0 * c.0 + b.0) / 4.0, (a.1 + 2.0 * c.1 + b.1) / 4.0);
        assert!((m.0.hypot(m.1) - 300.94).abs() < 0.01, "{m:?}");
    }

    #[test]
    fn the_nearest_point_of_a_curve_is_found() {
        let q = Piece::Quad((0.0, 0.0), (10.0, 20.0), (20.0, 0.0));
        // Its top is at (10, 10)
        assert!((q.distance((10.0, 15.0)) - 5.0).abs() < 1e-6);
        assert!((q.distance((-3.0, -4.0)) - 5.0).abs() < 1e-6);
    }
}
