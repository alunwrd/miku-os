// Stroke outlines, matching Qt's raster engine (`canvases_are_qts`)
//
// - Each side is the path offset by half the width. Lines offset whole; curves
//   offset their handle polygon, each leg moved out and re-intersected.
// - An open subpath is one outline: one side, end cap, the other side, start
//   cap. A closed one is two, one per side.
// - At a corner, the outer side has the join (miter, round, or bevel); the
//   inner side runs through the corner.
// - A round cap is cubics of at most a quarter turn from its start. A round
//   join is cut at the circle's quarter points, going the long way where the
//   outgoing angle passes 0 (`join`).
// - Filled with the nonzero rule, curves flattened as in `shape.rs`.

use super::shape::{Seg, Shape, Sub};

type P = (f64, f64);

/// How a line's open ends are drawn
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

/// How a line turns a corner
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    /// Pointed, cut straight past the miter limit
    Miter,
    Round,
    Bevel,
}

/// Width in screen pixels, caps, and joins
#[derive(Clone, Copy, Debug)]
pub struct Pen {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
}

/// One piece of a subpath: a line or a cubic
#[derive(Clone, Copy, Debug)]
enum Piece {
    Line(P, P),
    Cubic([P; 4]),
}

impl Piece {
    fn from(&self) -> P {
        match *self {
            Piece::Line(a, _) => a,
            Piece::Cubic(b) => b[0],
        }
    }

    fn to(&self) -> P {
        match *self {
            Piece::Line(_, b) => b,
            Piece::Cubic(b) => b[3],
        }
    }

    fn reversed(&self) -> Piece {
        match *self {
            Piece::Line(a, b) => Piece::Line(b, a),
            Piece::Cubic([p0, c1, c2, p3]) => Piece::Cubic([p3, c2, c1, p0]),
        }
    }

    /// Unit directions leaving the first point and arriving at the last; none
    /// for a too-short piece. A handle on its end point does not count
    fn ends(&self) -> Option<(P, P)> {
        let (first, last) = match *self {
            Piece::Line(a, b) => (sub(b, a), sub(b, a)),
            Piece::Cubic([p0, c1, c2, p3]) => {
                let first = [c1, c2, p3].into_iter().map(|c| sub(c, p0)).find(|d| !is_zero(*d));
                let last = [c2, c1, p0].into_iter().map(|c| sub(p3, c)).find(|d| !is_zero(*d));
                (first.unwrap_or((0.0, 0.0)), last.unwrap_or((0.0, 0.0)))
            }
        };
        (!is_zero(first)).then(|| (unit(first), unit(last)))
    }

    /// `ends`, of a piece kept for being long enough to have them
    fn way(&self) -> (P, P) {
        self.ends().unwrap_or(((1.0, 0.0), (1.0, 0.0)))
    }

    /// Moved `d` to its left
    fn moved(&self, d: f64) -> Piece {
        match *self {
            Piece::Line(a, b) => {
                let n = left(unit(sub(b, a)));
                Piece::Line(add(a, mul(n, d)), add(b, mul(n, d)))
            }
            Piece::Cubic([p0, c1, c2, p3]) => {
                // Offsets each handle leg; a zero-length leg uses the nearest
                // non-zero direction
                let chord = sub(p3, p0);
                let pick = |ls: [P; 3]| ls.into_iter().find(|l| !is_zero(*l)).unwrap_or((1.0, 0.0));
                let l0 = pick([sub(c1, p0), sub(c2, p0), chord]);
                let l2 = pick([sub(p3, c2), sub(p3, c1), chord]);
                let l1 = pick([sub(c2, c1), chord, l0]);
                let n = [l0, l1, l2].map(|l| mul(left(unit(l)), d));
                let q0 = add(p0, n[0]);
                let q3 = add(p3, n[2]);
                let q1 = meet(q0, l0, add(c1, n[1]), l1).unwrap_or(add(c1, n[1]));
                let q2 = meet(add(c2, n[1]), l1, q3, l2).unwrap_or(add(c2, n[1]));
                Piece::Cubic([q0, q1, q2, q3])
            }
        }
    }
}

fn add(a: P, b: P) -> P {
    (a.0 + b.0, a.1 + b.1)
}

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn mul(a: P, k: f64) -> P {
    (a.0 * k, a.1 * k)
}

fn cross(a: P, b: P) -> f64 {
    a.0 * b.1 - a.1 * b.0
}

fn dot(a: P, b: P) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// Too short to rasterize
fn is_zero(a: P) -> bool {
    a.0 * a.0 + a.1 * a.1 < 1e-12
}

fn unit(a: P) -> P {
    let l = (a.0 * a.0 + a.1 * a.1).sqrt();
    if l == 0.0 { (0.0, 0.0) } else { (a.0 / l, a.1 / l) }
}

/// Left of a direction in screen coordinates: `(1, 0)` has `(0, -1)` on its
/// left
fn left(t: P) -> P {
    (t.1, -t.0)
}

/// Intersection of the line through `a` along `da` and the one through `b`
/// along `db`
fn meet(a: P, da: P, b: P, db: P) -> Option<P> {
    let den = cross(da, db);
    if den.abs() < 1e-12 * (dot(da, da) * dot(db, db)).sqrt().max(1e-300) {
        return None;
    }
    let t = cross(sub(b, a), db) / den;
    Some(add(a, mul(da, t)))
}

/// Arcs around `c` from `from`, turning `sweep` radians (positive is clockwise
/// on screen), in quarter turns from `from`. Used for caps, so the tip is where
/// the line points
fn arc(o: &mut Sub, c: P, from: P, sweep: f64) {
    let n = (sweep.abs() / std::f64::consts::FRAC_PI_2 - 1e-9).ceil().max(1.0) as usize;
    let step = sweep / n as f64;
    let mut at = from;
    for _ in 0..n {
        at = arc_piece(o, c, at, step);
    }
}

/// Arcs around `c` from `from`, cut at the circle's quarter points. Used for
/// round joins
fn arc_by_quarters(o: &mut Sub, c: P, from: P, sweep: f64) {
    use std::f64::consts::FRAC_PI_2;
    let start = (from.1 - c.1).atan2(from.0 - c.0);
    let (mut done, mut at) = (0.0, from);
    while sweep - done > 1e-9 {
        // The next quarter point on the way, or the end
        let now = start + done;
        let quarter = ((now / FRAC_PI_2 + 1e-9).floor() + 1.0) * FRAC_PI_2;
        let step = (quarter - now).min(sweep - done);
        at = arc_piece(o, c, at, step);
        done += step;
    }
}

/// One cubic of at most a quarter turn around `c`, from `at`; returns where it
/// ends
fn arc_piece(o: &mut Sub, c: P, at: P, step: f64) -> P {
    // Each end's handle along the way round, `k` radii long
    let k = 4.0 / 3.0 * (step / 4.0).tan();
    let round = |p: P| (-(p.1 - c.1), p.0 - c.0);
    let next = rot(at, c, step);
    o.segs.push(Seg::Cubic(add(at, mul(round(at), k)), sub(next, mul(round(next), k)), next));
    next
}

/// `p` turned `angle` radians round `c`
fn rot(p: P, c: P, angle: f64) -> P {
    let (s, co) = angle.sin_cos();
    let d = sub(p, c);
    (c.0 + d.0 * co - d.1 * s, c.1 + d.0 * s + d.1 * co)
}

/// The outline of the line `pen` draws along `path`, its subpaths closed
pub fn outline(path: &Shape, pen: &Pen) -> Shape {
    let mut out = Shape::default();
    let d = pen.width / 2.0;
    if d <= 0.0 {
        return out;
    }
    for s in &path.subs {
        let mut pieces = Vec::new();
        let mut at = s.start;
        for seg in &s.segs {
            let p = match *seg {
                Seg::Line(x, y) => Piece::Line(at, (x, y)),
                Seg::Cubic(c1, c2, to) => Piece::Cubic([at, c1, c2, to]),
            };
            at = p.to();
            if p.ends().is_some() {
                pieces.push(p);
            }
        }
        if s.closed && !is_zero(sub(s.start, at)) {
            pieces.push(Piece::Line(at, s.start));
        }
        if pieces.is_empty() {
            continue;
        }
        let back: Vec<Piece> = pieces.iter().rev().map(Piece::reversed).collect();
        if s.closed {
            for side in [&pieces, &back] {
                let mut o = Sub { start: (0.0, 0.0), segs: Vec::new(), closed: true };
                along(&mut o, side, d, pen, true);
                out.subs.push(o);
            }
        } else {
            let mut o = Sub { start: (0.0, 0.0), segs: Vec::new(), closed: true };
            let (end, start) = (pieces[pieces.len() - 1], back[back.len() - 1]);
            along(&mut o, &pieces, d, pen, false);
            cap(&mut o, end.to(), end.way().1, d, pen.cap);
            along(&mut o, &back, d, pen, false);
            cap(&mut o, start.to(), start.way().1, d, pen.cap);
            out.subs.push(o);
        }
    }
    out
}

/// Left side of `pieces`, `d` out, with joins; back to the first when closed
fn along(o: &mut Sub, pieces: &[Piece], d: f64, pen: &Pen, closed: bool) {
    for (i, p) in pieces.iter().enumerate() {
        let m = p.moved(d);
        if i == 0 {
            if o.segs.is_empty() {
                o.start = m.from();
            } else if o.end() != m.from() {
                o.segs.push(Seg::Line(m.from().0, m.from().1));
            }
        }
        match m {
            Piece::Line(_, b) => o.segs.push(Seg::Line(b.0, b.1)),
            Piece::Cubic([_, c1, c2, p3]) => o.segs.push(Seg::Cubic(c1, c2, p3)),
        }
        let next = match pieces.get(i + 1) {
            Some(n) => n,
            None if closed => &pieces[0],
            None => break,
        };
        join(o, p.to(), p.way().1, next.way().0, d, pen);
    }
}

/// Joins the left side arriving at `v` along `t_in` to the left side leaving
/// along `t_out`
fn join(o: &mut Sub, v: P, t_in: P, t_out: P, d: f64, pen: &Pen) {
    let a = add(v, mul(left(t_in), d));
    let b = add(v, mul(left(t_out), d));
    let turn = cross(t_in, t_out);
    if is_zero(sub(a, b)) {
        return;
    }
    // The left side is outside the corner only on a right turn (clockwise on
    // screen)
    if turn <= 0.0 {
        o.segs.push(Seg::Line(v.0, v.1));
        o.segs.push(Seg::Line(b.0, b.1));
        return;
    }
    match pen.join {
        Join::Bevel => o.segs.push(Seg::Line(b.0, b.1)),
        Join::Miter => {
            if let Some(m) = meet(a, t_in, b, t_out) {
                let far = sub(m, v);
                if dot(far, far).sqrt() <= pen.miter_limit * d {
                    o.segs.push(Seg::Line(m.0, m.1));
                }
            }
            o.segs.push(Seg::Line(b.0, b.1));
        }
        Join::Round => {
            // Qt turns by the difference of the angles, counted anticlockwise
            // from 0 to 360 with y up. Where the outgoing angle passes 0 this
            // goes the long way (270 degrees at a rectangle's first corner),
            // and a line returns to the next side. The loop adds coverage,
            // darkening thin corners (`stretched.svg`)
            let angle = |t: P| (-t.1).atan2(t.0).rem_euclid(std::f64::consts::TAU);
            arc_by_quarters(o, v, a, (angle(t_out) - angle(t_in)).abs());
            if !is_zero(sub(o.end(), b)) {
                o.segs.push(Seg::Line(b.0, b.1));
            }
        }
    }
}

/// Cap at `v`, outward along `t`, from the left side to the right
fn cap(o: &mut Sub, v: P, t: P, d: f64, cap: Cap) {
    let a = add(v, mul(left(t), d));
    let b = sub(v, mul(left(t), d));
    match cap {
        Cap::Butt => o.segs.push(Seg::Line(b.0, b.1)),
        Cap::Square => {
            let (a2, b2) = (add(a, mul(t, d)), add(b, mul(t, d)));
            o.segs.push(Seg::Line(a2.0, a2.1));
            o.segs.push(Seg::Line(b2.0, b2.1));
            o.segs.push(Seg::Line(b.0, b.1));
        }
        Cap::Round => arc(o, v, a, std::f64::consts::PI),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_line_is_a_box_round_it() {
        let mut s = Shape::default();
        s.move_to((0.0, 0.0));
        s.line_to((10.0, 0.0));
        let o = outline(&s, &Pen { width: 2.0, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0 });
        assert_eq!(o.subs.len(), 1);
        assert_eq!(o.subs[0].start, (0.0, -1.0));
        let ends: Vec<P> = o.subs[0].segs.iter().map(|s| match *s { Seg::Line(x, y) => (x, y), Seg::Cubic(_, _, p) => p }).collect();
        assert_eq!(ends, vec![(10.0, -1.0), (10.0, 1.0), (0.0, 1.0), (0.0, -1.0)]);
    }

    #[test]
    fn a_miter_points_and_a_bevel_cuts() {
        let mut s = Shape::default();
        s.move_to((0.0, 0.0));
        s.line_to((10.0, 0.0));
        s.line_to((10.0, 10.0));
        let pen = Pen { width: 2.0, cap: Cap::Butt, join: Join::Miter, miter_limit: 10.0 };
        let has = |o: &Shape, p: P| o.subs[0].segs.iter().any(|s| matches!(*s, Seg::Line(x, y) if (x, y) == p));
        assert!(has(&outline(&s, &pen), (11.0, -1.0)));
        assert!(!has(&outline(&s, &Pen { join: Join::Bevel, ..pen }), (11.0, -1.0)));
    }
}
