// Shape path coverage per pixel (0 to 255), for Qt's two renderers
//
// Curve renderer (`qsb -d` on shapecurve.frag and shapestroke.frag), by
// distance from the path's edge at each center:
// - a fill ramps half a pixel each way of the edge;
// - a stroke has two edges half its width from the line, each ramping over a
//   pixel's diagonal (`edge`). Caps and joins are in `stroke`.
//
// Geometry renderer (Qt's default): a pixel is covered if its center is inside,
// with no smoothing.

use super::quads::{Piece, Run, P};
use crate::scene::canvas::stroke::{Cap, Join, Pen};

/// Fill coverage: nonzero `winding` or odd-even, ramped at the edge when
/// `smooth`
pub fn fill(runs: &[Run], winding: bool, smooth: bool, w: usize, h: usize) -> Vec<u8> {
    // A fill closes what is left open
    let mut pieces: Vec<Piece> = Vec::new();
    for r in runs {
        pieces.extend_from_slice(&r.pieces);
        let (first, last) = (r.pieces[0].ends().0, r.pieces[r.pieces.len() - 1].ends().1);
        if first != last {
            pieces.push(Piece::Line(last, first));
        }
    }
    let inside = inside(&pieces, winding, w, h);
    if !smooth {
        return inside.iter().map(|&i| if i { 255 } else { 0 }).collect();
    }
    let near = Near::new(&pieces, 0.5);
    let mut cov = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            let c = match near.distance(p) {
                Some(d) => (0.5 + if inside[i] { d } else { -d }).clamp(0.0, 1.0),
                None => if inside[i] { 1.0 } else { 0.0 },
            };
            cov[i] = (c * 255.0).round() as u8;
        }
    }
    cov
}

/// Curve renderer stroke, matched against qml6 with pens 0.5 to 6 wide: along a
/// piece by distance (`band`), past its end by the cap or join.
/// - Round cap: distance from the end, cut square half a width past it without
///   smoothing.
/// - Flat cap: cut square at the end, the same way.
/// - Square cap: half a width past the end, its far side an edge.
/// - Round join: distance from the corner. Bevel: cut between the outer
///   corners. Miter: outer sides extended until they meet, cut at the limit.
/// A pixel near two pieces takes the higher coverage
pub fn stroke(runs: &[Run], pen: &Pen, w: usize, h: usize) -> Vec<u8> {
    let all: Vec<Piece> = runs.iter().flat_map(|r| r.pieces.iter().copied()).collect();
    let half = pen.width / 2.0;
    let reach = half.max(half * pen.miter_limit) + std::f64::consts::FRAC_1_SQRT_2;
    let near = Near::new(&all, reach);
    let mut cov = vec![0u8; w * h];
    for y in 0..h {
        for x in 0..w {
            let p = (x as f64 + 0.5, y as f64 + 0.5);
            if near.distance(p).is_none() {
                continue;
            }
            let mut c: f64 = 0.0;
            for r in runs {
                for k in 0..r.pieces.len() {
                    c = c.max(piece(r, k, pen, p));
                }
            }
            cov[y * w + x] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    cov
}

/// How much the `k`th piece of `run` covers `p`, its cap or join included
fn piece(run: &Run, k: usize, pen: &Pen, p: P) -> f64 {
    let (pieces, n) = (&run.pieces, run.pieces.len());
    let (t, d) = pieces[k].nearest(p);
    if t > 0.0 && t < 1.0 {
        return band(d, pen.width);
    }
    let (ends, (ts, te)) = (pieces[k].ends(), pieces[k].tangents());
    if t <= 0.0 {
        let before = if k > 0 { Some(k - 1) } else if run.closed { Some(n - 1) } else { None };
        match before {
            Some(j) => join(p, ends.0, pieces[j].tangents().1, ts, d, pen),
            None => cap(p, ends.0, (-ts.0, -ts.1), d, pen),
        }
    } else {
        let after = if k + 1 < n { Some(k + 1) } else if run.closed { Some(0) } else { None };
        match after {
            Some(j) => join(p, ends.1, te, pieces[j].tangents().0, d, pen),
            None => cap(p, ends.1, te, d, pen),
        }
    }
}

/// Coverage between two edges half a width from the line, `d` from it. On the
/// line, a sub-pixel pen covers by its width
fn band(d: f64, width: f64) -> f64 {
    if d < width * 0.01 {
        return width.min(1.0);
    }
    edge(d - width / 2.0) - edge(d + width / 2.0)
}

/// Past the end `v` of a run going on `out`, `d` from it
fn cap(p: P, v: P, out: P, d: f64, pen: &Pen) -> f64 {
    let rel = (p.0 - v.0, p.1 - v.1);
    let along = rel.0 * out.0 + rel.1 * out.1;
    match pen.cap {
        Cap::Butt => 0.0,
        Cap::Round if along <= pen.width / 2.0 => band(d, pen.width),
        Cap::Round => 0.0,
        Cap::Square => {
            let across = (rel.0 * out.1 - rel.1 * out.0).abs();
            band(across, pen.width).min(edge(along - pen.width / 2.0))
        }
    }
}

/// Coverage past the corner `v` between pieces arriving along `into` and
/// leaving along `on`, `d` from it
fn join(p: P, v: P, into: P, on: P, d: f64, pen: &Pen) -> f64 {
    let turn = into.0 * on.1 - into.1 * on.0;
    if turn.abs() < 1e-9 || pen.join == Join::Round {
        return band(d, pen.width);
    }
    let half = pen.width / 2.0;
    // The sides away from the turn
    let side = |u: P, towards: P| {
        let n = (-u.1, u.0);
        if n.0 * towards.0 + n.1 * towards.1 < 0.0 { n } else { (-n.0, -n.1) }
    };
    let n_in = side(into, on);
    let n_on = side(on, (-into.0, -into.1));
    let mid = {
        let m = (n_in.0 + n_on.0, n_in.1 + n_on.1);
        let l = m.0.hypot(m.1);
        if l > 0.0 { (m.0 / l, m.1 / l) } else { n_in }
    };
    let rel = (p.0 - v.0, p.1 - v.1);
    let dot = |a: P, b: P| a.0 * b.0 + a.1 * b.1;
    // Inside the turn or before the corner, the pieces' own bands apply
    if dot(rel, into) < 0.0 || dot(rel, on) > 0.0 {
        return band(d, pen.width);
    }
    match pen.join {
        Join::Bevel => {
            // The line between the two outer corners, `mid` its way out
            let corner = (n_in.0 * half, n_in.1 * half);
            band(d, pen.width).min(edge(dot((rel.0 - corner.0, rel.1 - corner.1), mid)))
        }
        _ => {
            let sides = edge(dot(rel, n_in) - half).min(edge(dot(rel, n_on) - half));
            sides.min(edge(dot(rel, mid) - pen.miter_limit * half))
        }
    }
}

/// All-or-nothing nonzero fill of `pieces`: a stroke outline under the geometry
/// renderer
pub fn solid(pieces: &[Piece], w: usize, h: usize) -> Vec<u8> {
    inside(pieces, true, w, h).iter().map(|&i| if i { 255 } else { 0 }).collect()
}

/// Coverage of a pixel by an edge `t` from its center (inside for negative
/// `t`), using a pixel rotated 45°, with corners 0.707 from center
fn edge(t: f64) -> f64 {
    let r = std::f64::consts::FRAC_1_SQRT_2;
    if t <= -r {
        1.0
    } else if t < 0.0 {
        1.0 - (r + t) * (r + t)
    } else if t < r {
        (r - t) * (r - t)
    } else {
        0.0
    }
}

/// Whether each pixel's centre is inside, row by row from the crossings
fn inside(pieces: &[Piece], winding: bool, w: usize, h: usize) -> Vec<bool> {
    let mut out = vec![false; w * h];
    let mut xs = Vec::new();
    for y in 0..h {
        xs.clear();
        for p in pieces {
            p.crossings(y as f64 + 0.5, &mut xs);
        }
        xs.sort_by(|a, b| a.0.total_cmp(&b.0));
        let (mut k, mut wind) = (0, 0);
        for x in 0..w {
            let cx = x as f64 + 0.5;
            while k < xs.len() && xs[k].0 < cx {
                wind += xs[k].1;
                k += 1;
            }
            out[y * w + x] = if winding { wind != 0 } else { wind % 2 != 0 };
        }
    }
    out
}

/// The pieces near a point, by their boxes grown by `reach`
struct Near<'a> {
    pieces: &'a [Piece],
    boxes: Vec<(P, P)>,
}

impl<'a> Near<'a> {
    fn new(pieces: &'a [Piece], reach: f64) -> Self {
        let boxes = pieces.iter().map(|p| {
            let (lo, hi) = p.bounds();
            ((lo.0 - reach, lo.1 - reach), (hi.0 + reach, hi.1 + reach))
        }).collect();
        Near { pieces, boxes }
    }

    /// How far the nearest piece within reach is, if one is
    fn distance(&self, p: P) -> Option<f64> {
        self.pieces.iter().zip(&self.boxes)
            .filter(|(_, (lo, hi))| p.0 >= lo.0 && p.0 <= hi.0 && p.1 >= lo.1 && p.1 <= hi.1)
            .map(|(q, _)| q.distance(p))
            .min_by(f64::total_cmp)
    }
}
