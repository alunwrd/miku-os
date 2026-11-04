// Canvas path in screen coordinates
//
// Subpaths of lines and cubics, already through the matrix as in the HTML
// canvas. Arcs become cubics of at most a quarter turn, quadratics their
// equivalent cubic. Curves flatten to lines as Qt does for FreeType's
// rasterizer (`freetype::path`). Dashes are cut here; outlines are in
// `stroke.rs`.

use freetype::path::Path as FtPath;

use crate::scene::xf::Xf;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Line(f64, f64),
    Cubic((f64, f64), (f64, f64), (f64, f64)),
}

#[derive(Clone, Debug, Default)]
pub struct Sub {
    pub start: (f64, f64),
    pub segs: Vec<Seg>,
    pub closed: bool,
}

impl Sub {
    /// Points with curves flattened (`flatten`), returning to the start when
    /// closed
    pub fn polyline(&self) -> Vec<(f64, f64)> {
        let mut pts = vec![self.start];
        for seg in &self.segs {
            let at = pts[pts.len() - 1];
            match *seg {
                Seg::Line(x, y) => pts.push((x, y)),
                Seg::Cubic(c1, c2, to) => flatten(at, c1, c2, to, &mut pts),
            }
        }
        if self.closed {
            pts.push(self.start);
        }
        pts
    }

    /// Last point, or the start when empty
    pub fn end(&self) -> (f64, f64) {
        match self.segs.last() {
            Some(Seg::Line(x, y)) => (*x, *y),
            Some(Seg::Cubic(_, _, p)) => *p,
            None => self.start,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Shape {
    pub subs: Vec<Sub>,
}

impl Shape {
    pub fn clear(&mut self) {
        self.subs.clear();
    }

    /// Where the pen is, on the screen, if a subpath is open
    pub fn current(&self) -> Option<(f64, f64)> {
        self.subs.last().map(|s| if s.closed { s.start } else { s.end() })
    }

    pub fn move_to(&mut self, p: (f64, f64)) {
        self.subs.push(Sub { start: p, segs: Vec::new(), closed: false });
    }

    /// A line on from the pen, or the start of a subpath when there is none
    pub fn line_to(&mut self, p: (f64, f64)) {
        match self.subs.last_mut() {
            Some(s) if !s.closed => s.segs.push(Seg::Line(p.0, p.1)),
            // After a close, a new subpath starts where the closed one did
            Some(s) => {
                let start = s.start;
                self.subs.push(Sub { start, segs: vec![Seg::Line(p.0, p.1)], closed: false });
            }
            None => self.move_to(p),
        }
    }

    pub fn cubic_to(&mut self, c1: (f64, f64), c2: (f64, f64), p: (f64, f64)) {
        if self.current().is_none() {
            self.move_to(c1);
        }
        if self.subs.last().is_some_and(|s| s.closed) {
            let start = self.subs.last().map(|s| s.start).unwrap_or(c1);
            self.subs.push(Sub { start, segs: Vec::new(), closed: false });
        }
        if let Some(s) = self.subs.last_mut() {
            s.segs.push(Seg::Cubic(c1, c2, p));
        }
    }

    pub fn close(&mut self) {
        if let Some(s) = self.subs.last_mut() {
            s.closed = true;
        }
    }

    /// Arc of the circle at `(cx, cy)` with radius `r` from `a0` to `a1`
    /// (radians, clockwise on screen), reversed with `anticlockwise`, in
    /// coordinates `xf` maps to the screen. A line joins the pen to its start
    #[allow(clippy::too_many_arguments)]
    pub fn arc(&mut self, xf: &Xf, cx: f64, cy: f64, r: f64, a0: f64, a1: f64, anticlockwise: bool) {
        self.ellipse_arc(xf, cx, cy, r, r, a0, a1, anticlockwise);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse_arc(&mut self, xf: &Xf, cx: f64, cy: f64, rx: f64, ry: f64, a0: f64, a1: f64, anticlockwise: bool) {
        let tau = std::f64::consts::TAU;
        // HTML canvas rule: a full turn or more is the whole circle
        let sweep = if !anticlockwise && a1 - a0 >= tau {
            tau
        } else if anticlockwise && a0 - a1 >= tau {
            -tau
        } else if anticlockwise {
            -(a0 - a1).rem_euclid(tau)
        } else {
            (a1 - a0).rem_euclid(tau)
        };
        let at = |a: f64| xf.map(cx + rx * a.cos(), cy + ry * a.sin());
        let first = at(a0);
        if self.current().is_some() && !self.subs.last().is_some_and(|s| s.closed) {
            self.line_to(first);
        } else {
            self.move_to(first);
        }
        if sweep == 0.0 || rx <= 0.0 || ry <= 0.0 {
            return;
        }
        if let Some(s) = self.subs.last_mut() {
            arc_segs(&mut s.segs, xf, (cx, cy), (rx, ry), a0, sweep);
        }
    }

    /// HTML canvas `arcTo`: a line from the pen toward `p1`, turning on a
    /// circle of radius `r` toward `p2`. Item coordinates; the pen maps back
    /// through `inv`
    #[allow(clippy::too_many_arguments)]
    pub fn arc_to(&mut self, xf: &Xf, inv: &Xf, p1: (f64, f64), p2: (f64, f64), r: f64) {
        let Some(cur) = self.current() else {
            self.move_to(xf.map(p1.0, p1.1));
            return;
        };
        let p0 = inv.map(cur.0, cur.1);
        let (d0, d2) = ((p0.0 - p1.0, p0.1 - p1.1), (p2.0 - p1.0, p2.1 - p1.1));
        let (l0, l2) = ((d0.0 * d0.0 + d0.1 * d0.1).sqrt(), (d2.0 * d2.0 + d2.1 * d2.1).sqrt());
        let cross = d0.0 * d2.1 - d0.1 * d2.0;
        if r <= 0.0 || l0 == 0.0 || l2 == 0.0 || cross.abs() < 1e-9 * l0 * l2 {
            self.line_to(xf.map(p1.0, p1.1));
            return;
        }
        let (u0, u2) = ((d0.0 / l0, d0.1 / l0), (d2.0 / l2, d2.1 / l2));
        // Half the angle between the lines, and the tangent distance from the
        // corner
        let cos = (u0.0 * u2.0 + u0.1 * u2.1).clamp(-1.0, 1.0);
        let half = cos.acos() / 2.0;
        let along = r / half.tan();
        let t0 = (p1.0 + u0.0 * along, p1.1 + u0.1 * along);
        let t2 = (p1.0 + u2.0 * along, p1.1 + u2.1 * along);
        let bis = (u0.0 + u2.0, u0.1 + u2.1);
        let bl = (bis.0 * bis.0 + bis.1 * bis.1).sqrt();
        let centre_d = r / half.sin();
        let c = (p1.0 + bis.0 / bl * centre_d, p1.1 + bis.1 / bl * centre_d);
        let a0 = (t0.1 - c.1).atan2(t0.0 - c.0);
        let a1 = (t2.1 - c.1).atan2(t2.0 - c.0);
        self.line_to(xf.map(t0.0, t0.1));
        // The short way round between the two touching points
        self.arc(xf, c.0, c.1, r, a0, a1, cross > 0.0);
    }

    /// The path for FreeType, its curves as lines (`flatten`)
    pub fn ft(&self) -> FtPath {
        let mut p = FtPath::new();
        for s in self.subs.iter().filter(|s| !s.segs.is_empty()) {
            let pts = s.polyline();
            p.move_to(pts[0].0, pts[0].1);
            for &(x, y) in &pts[1..] {
                p.line_to(x, y);
            }
            if s.closed {
                p.close();
            }
        }
        p
    }

    /// Cuts the path into open subpaths, `dash` on and off from `offset` along
    /// each subpath. Curves flatten first
    pub fn dashed(&self, dash: &[f64], offset: f64) -> Shape {
        let total: f64 = dash.iter().sum();
        let mut out = Shape::default();
        if total <= 0.0 {
            return self.clone();
        }
        for s in &self.subs {
            let pts = s.polyline();
            // Where in the pattern the subpath starts
            let mut k = 0;
            let mut left = dash[0];
            let mut skip = offset.rem_euclid(total);
            while skip > 0.0 {
                if skip >= left {
                    skip -= left;
                    k = (k + 1) % dash.len();
                    left = dash[k];
                } else {
                    left -= skip;
                    skip = 0.0;
                }
            }
            let mut on = k % 2 == 0;
            let mut cur: Option<Sub> = on.then(|| Sub { start: pts[0], segs: Vec::new(), closed: false });
            for w in pts.windows(2) {
                let (a, b) = (w[0], w[1]);
                let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
                let mut done = 0.0;
                while len - done > left {
                    done += left;
                    let t = done / len;
                    let p = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
                    if let Some(mut c) = cur.take() {
                        c.segs.push(Seg::Line(p.0, p.1));
                        out.subs.push(c);
                    } else {
                        cur = Some(Sub { start: p, segs: Vec::new(), closed: false });
                    }
                    on = !on;
                    k = (k + 1) % dash.len();
                    left = dash[k];
                    if on && cur.is_none() {
                        cur = Some(Sub { start: p, segs: Vec::new(), closed: false });
                    }
                }
                left -= len - done;
                if let Some(c) = cur.as_mut() {
                    c.segs.push(Seg::Line(b.0, b.1));
                }
            }
            if let Some(c) = cur {
                if !c.segs.is_empty() {
                    out.subs.push(c);
                }
            }
        }
        out
    }
}

/// Angles closer than this are equal
const ANGLE: f64 = 1e-9;

/// How far a quarter circle's cubic handles reach, over its radius
const KAPPA: f64 = 0.552_284_749_830_793_6;

/// A point of the unit quarter circle's cubic, from `(1, 0)` to `(0, 1)`
fn quarter_at(t: f64) -> (f64, f64) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (a + b + c * KAPPA, b * KAPPA + c + d)
}

/// Parameter where the unit quarter's cubic reaches `angle` (0 to a quarter
/// turn)
fn quarter_t(angle: f64) -> f64 {
    let q = std::f64::consts::FRAC_PI_2;
    if angle <= ANGLE {
        return 0.0;
    }
    if angle >= q - ANGLE {
        return 1.0;
    }
    // The angle rises monotonically: bisects
    let (mut lo, mut hi) = (0.0, 1.0);
    for _ in 0..60 {
        let mid = (lo + hi) / 2.0;
        let (x, y) = quarter_at(mid);
        if y.atan2(x) < angle {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (lo + hi) / 2.0
}

/// The part of the unit quarter's cubic from `t0` to `t1`, either way along
fn quarter_part(t0: f64, t1: f64) -> [(f64, f64); 4] {
    let (lo, hi) = (t0.min(t1), t0.max(t1));
    // The cubic up to `hi`, and of that the part from `lo`
    let upto = split([(1.0, 0.0), (1.0, KAPPA), (KAPPA, 1.0), (0.0, 1.0)], hi).0;
    let part = if hi > 0.0 { split(upto, lo / hi).1 } else { upto };
    if t0 <= t1 { part } else { [part[3], part[2], part[1], part[0]] }
}

/// Arcs of the ellipse around `c` with radii `r`, from `a0` turning `sweep`
/// (positive is clockwise on screen), through `xf`, after `segs`. Cut at the
/// circle's quarter points as in Qt
pub fn arc_segs(segs: &mut Vec<Seg>, xf: &Xf, c: (f64, f64), r: (f64, f64), a0: f64, sweep: f64) {
    let q = std::f64::consts::FRAC_PI_2;
    let (dir, end) = (sweep.signum(), a0 + sweep);
    let mut from = a0;
    while (end - from) * dir > ANGLE {
        // The quarter the arc goes on round, and where it leaves it
        let k = if dir > 0.0 { ((from + ANGLE) / q).floor() } else { ((from - ANGLE) / q).ceil() - 1.0 };
        let edge = if dir > 0.0 { (k + 1.0) * q } else { k * q };
        let to = if (edge - from) * dir >= (end - from) * dir - ANGLE { end } else { edge };
        let [_, c1, c2, p3] = quarter_part(quarter_t(from - k * q), quarter_t(to - k * q));
        // From the quarter's own frame to the ellipse, and the screen
        let place = |(u, v): (f64, f64)| {
            let (x, y) = match (k as i64).rem_euclid(4) {
                0 => (u, v),
                1 => (-v, u),
                2 => (-u, -v),
                _ => (v, -u),
            };
            xf.map(c.0 + r.0 * x, c.1 + r.1 * y)
        };
        segs.push(Seg::Cubic(place(c1), place(c2), place(p3)));
        from = to;
    }
}

/// A cubic cut in two at `t`
pub(crate) fn split(b: [(f64, f64); 4], t: f64) -> ([(f64, f64); 4], [(f64, f64); 4]) {
    let lerp = |p: (f64, f64), q: (f64, f64)| (p.0 + (q.0 - p.0) * t, p.1 + (q.1 - p.1) * t);
    let (a, h, c) = (lerp(b[0], b[1]), lerp(b[1], b[2]), lerp(b[2], b[3]));
    let (ah, hc) = (lerp(a, h), lerp(h, c));
    let m = lerp(ah, hc);
    ([b[0], a, ah, m], [m, hc, c, b[3]])
}

/// Flatness limit: summed handle distances from the chord against a quarter of
/// the chord's axis-measured length (`canvases_are_qts`, case 0)
const FLAT: f64 = 0.25;

/// How many times a curve may be halved
const DEEPEST: u32 = 9;

/// Flattens a cubic by halving until flat, pushing end points but not the start
fn flatten(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64), out: &mut Vec<(f64, f64)>) {
    flatten_by(p0, c1, c2, p3, &|[p0, c1, c2, p3]| {
        let (dx, dy) = (p3.0 - p0.0, p3.1 - p0.1);
        // A handle's distance from the chord, times the chord's length
        let off = |c: (f64, f64)| ((c.0 - p0.0) * dy - (c.1 - p0.1) * dx).abs();
        off(c1) + off(c2) < FLAT * (dx.abs() + dy.abs()).max(1.0)
    }, out);
}

impl Shape {
    /// The path through `xf` as Qt's raster engine draws one: flattened in the
    /// path's units at the matrix's larger axis scale, then mapped to the
    /// screen
    pub fn mapped(&self, xf: &Xf) -> Shape {
        let scale = (xf.a * xf.a + xf.b * xf.b).max(xf.c * xf.c + xf.d * xf.d).sqrt();
        let flat = if scale > 0.0 { FLAT / scale } else { FLAT };
        let mut out = Shape::default();
        for sub in &self.subs {
            let mut pts = vec![sub.start];
            for seg in &sub.segs {
                let at = pts[pts.len() - 1];
                match *seg {
                    Seg::Line(x, y) => pts.push((x, y)),
                    // Chords over one unit are measured by handle distance as
                    // for Canvas; shorter ones by handle distance from the
                    // start along x and y, matching QtSvg's small icon curves
                    // (`svgs_are_qts`)
                    Seg::Cubic(c1, c2, to) => flatten_by(at, c1, c2, to, &|[p0, c1, c2, p3]| {
                        let (dx, dy) = (p3.0 - p0.0, p3.1 - p0.1);
                        let l = dx.abs() + dy.abs();
                        if l > 1.0 {
                            let off = |c: (f64, f64)| ((c.0 - p0.0) * dy - (c.1 - p0.1) * dx).abs();
                            off(c1) + off(c2) < flat * l
                        } else {
                            let from = |c: (f64, f64)| (c.0 - p0.0).abs() + (c.1 - p0.1).abs();
                            from(c1) + from(c2) < flat
                        }
                    }, &mut pts),
                }
            }
            let start = xf.map(sub.start.0, sub.start.1);
            out.subs.push(Sub {
                start,
                segs: pts[1..].iter().map(|p| { let q = xf.map(p.0, p.1); Seg::Line(q.0, q.1) }).collect(),
                closed: sub.closed,
            });
        }
        out
    }
}

/// Flattens a cubic by halving until `flat` accepts a piece or `DEEPEST` is
/// reached
pub(crate) fn flatten_by(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64),
                         flat: &dyn Fn([(f64, f64); 4]) -> bool, out: &mut Vec<(f64, f64)>) {
    halve([p0, c1, c2, p3], DEEPEST, flat, out);
}

fn halve(b: [(f64, f64); 4], left: u32, flat: &dyn Fn([(f64, f64); 4]) -> bool, out: &mut Vec<(f64, f64)>) {
    if left == 0 || flat(b) {
        out.push(b[3]);
        return;
    }
    let (l, r) = split(b, 0.5);
    halve(l, left - 1, flat, out);
    halve(r, left - 1, flat, out);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_full_circle_is_four_quarters_back_where_it_began() {
        let mut s = Shape::default();
        s.arc(&Xf::IDENTITY, 10.0, 10.0, 5.0, 0.0, std::f64::consts::TAU, false);
        assert_eq!(s.subs.len(), 1);
        assert_eq!(s.subs[0].segs.len(), 4);
        let (x, y) = s.subs[0].end();
        assert!((x - 15.0).abs() < 1e-9 && (y - 10.0).abs() < 1e-9);
    }

    #[test]
    fn a_dash_pattern_cuts_a_line_into_its_dashes() {
        let mut s = Shape::default();
        s.move_to((0.0, 0.0));
        s.line_to((20.0, 0.0));
        let d = s.dashed(&[6.0, 4.0], 0.0);
        let spans: Vec<(f64, f64)> = d.subs.iter().map(|s| (s.start.0, s.end().0)).collect();
        assert_eq!(spans, vec![(0.0, 6.0), (10.0, 16.0)]);
    }
}

