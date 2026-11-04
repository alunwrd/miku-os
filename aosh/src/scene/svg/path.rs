// SVG outlines in user units
//
// Path data per the SVG grammar: all commands, absolute and relative, repeated
// commands, and run-together numbers (`1.5.5`, `-1-2`, arc flags without
// spaces). Basic shapes are lines and cubics. Quadratics convert to cubics.
// Elliptical arcs use the SVG spec's center conversion, growing radii that
// cannot reach, and split into cubics of at most a quarter turn.
//
// Rounded rectangles and ellipses follow QPainterPath's arcs
// (`Shape::ellipse_arc`).

use crate::scene::canvas::shape::Shape;
use crate::scene::xf::Xf;

pub type P = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Move(P),
    Line(P),
    Cubic(P, P, P),
    Close,
    /// Qt-style ellipse arc with center and radii, between angles (radians,
    /// clockwise on screen), built by `Shape`
    Arc { c: P, r: P, a0: f64, a1: f64 },
}

/// Path data up to the first error, as the spec says
pub fn parse(d: &str) -> Vec<Cmd> {
    let mut out = Vec::new();
    let mut s = Scan { b: d.as_bytes(), at: 0 };
    let (mut cur, mut start) = ((0.0, 0.0), (0.0, 0.0));
    // The last curve's second control point, for S and T
    let mut last_c: Option<P> = None;
    let mut last_q: Option<P> = None;
    let mut cmd = 0u8;
    loop {
        s.sep();
        let Some(c) = s.peek() else { break };
        if c.is_ascii_alphabetic() {
            cmd = c;
            s.at += 1;
        } else if cmd == 0 {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        let pt = |s: &mut Scan<'_>| -> Option<P> { Some((s.num()? + base.0, s.num()? + base.1)) };
        let (c_was, q_was) = (last_c.take(), last_q.take());
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let Some(p) = pt(&mut s) else { break };
                out.push(Cmd::Move(p));
                cur = p;
                start = p;
                // Numbers after a moveto are linetos
                cmd = if rel { b'l' } else { b'L' };
            }
            b'L' => {
                let Some(p) = pt(&mut s) else { break };
                out.push(Cmd::Line(p));
                cur = p;
            }
            b'H' => {
                let Some(x) = s.num() else { break };
                cur = (x + if rel { cur.0 } else { 0.0 }, cur.1);
                out.push(Cmd::Line(cur));
            }
            b'V' => {
                let Some(y) = s.num() else { break };
                cur = (cur.0, y + if rel { cur.1 } else { 0.0 });
                out.push(Cmd::Line(cur));
            }
            b'C' => {
                let (Some(c1), Some(c2), Some(p)) = (pt(&mut s), pt(&mut s), pt(&mut s)) else { break };
                out.push(Cmd::Cubic(c1, c2, p));
                last_c = Some(c2);
                cur = p;
            }
            b'S' => {
                let (Some(c2), Some(p)) = (pt(&mut s), pt(&mut s)) else { break };
                let c1 = c_was.map(|c| (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1)).unwrap_or(cur);
                out.push(Cmd::Cubic(c1, c2, p));
                last_c = Some(c2);
                cur = p;
            }
            b'Q' => {
                let (Some(q), Some(p)) = (pt(&mut s), pt(&mut s)) else { break };
                out.push(quad(cur, q, p));
                last_q = Some(q);
                cur = p;
            }
            b'T' => {
                let Some(p) = pt(&mut s) else { break };
                let q = q_was.map(|q| (2.0 * cur.0 - q.0, 2.0 * cur.1 - q.1)).unwrap_or(cur);
                out.push(quad(cur, q, p));
                last_q = Some(q);
                cur = p;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (s.num(), s.num(), s.num()) else { break };
                let (Some(large), Some(sweep)) = (s.flag(), s.flag()) else { break };
                let Some(p) = pt(&mut s) else { break };
                arc(&mut out, cur, (rx, ry), rot, large, sweep, p);
                cur = p;
            }
            b'Z' => {
                out.push(Cmd::Close);
                cur = start;
                // A command letter has to follow
                cmd = 0;
            }
            _ => break,
        }
    }
    out
}

fn quad(from: P, q: P, to: P) -> Cmd {
    let c1 = (from.0 + 2.0 / 3.0 * (q.0 - from.0), from.1 + 2.0 / 3.0 * (q.1 - from.1));
    let c2 = (to.0 + 2.0 / 3.0 * (q.0 - to.0), to.1 + 2.0 / 3.0 * (q.1 - to.1));
    Cmd::Cubic(c1, c2, to)
}

/// Elliptical arc per the SVG spec's implementation notes: degenerate ones are
/// lines, small radii grow, flags pick the center and angles
fn arc(out: &mut Vec<Cmd>, from: P, r: P, rot_deg: f64, large: bool, sweep: bool, to: P) {
    if from == to {
        return;
    }
    let (mut rx, mut ry) = (r.0.abs(), r.1.abs());
    if rx == 0.0 || ry == 0.0 {
        out.push(Cmd::Line(to));
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let (dx, dy) = ((from.0 - to.0) / 2.0, (from.1 - to.1) / 2.0);
    let (x1, y1) = (cos * dx + sin * dy, -sin * dx + cos * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = (rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1).max(0.0);
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = if den == 0.0 { 0.0 } else { (num / den).sqrt() };
    if large == sweep {
        k = -k;
    }
    let (cx1, cy1) = (k * rx * y1 / ry, -k * ry * x1 / rx);
    let c = (cos * cx1 - sin * cy1 + (from.0 + to.0) / 2.0, sin * cx1 + cos * cy1 + (from.1 + to.1) / 2.0);
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let a = (ux * vy - uy * vx).atan2(ux * vx + uy * vy);
        a
    };
    let a0 = angle(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut da = angle((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let tau = std::f64::consts::TAU;
    if !sweep && da > 0.0 {
        da -= tau;
    } else if sweep && da < 0.0 {
        da += tau;
    }
    let n = (da.abs() / (std::f64::consts::FRAC_PI_2 + 1e-3)).ceil().max(1.0) as usize;
    let step = da / n as f64;
    let t = 4.0 / 3.0 * (step / 4.0).tan();
    let on = |a: f64| {
        let (s, c0) = a.sin_cos();
        (rx * c0, ry * s)
    };
    let place = |p: P| (c.0 + cos * p.0 - sin * p.1, c.1 + sin * p.0 + cos * p.1);
    for i in 0..n {
        let (b0, b1) = (a0 + step * i as f64, a0 + step * (i + 1) as f64);
        let (p0, p1) = (on(b0), on(b1));
        let d0 = (-rx * b0.sin(), ry * b0.cos());
        let d1 = (-rx * b1.sin(), ry * b1.cos());
        let c1 = place((p0.0 + t * d0.0, p0.1 + t * d0.1));
        let c2 = place((p1.0 - t * d1.0, p1.1 - t * d1.1));
        let end = if i + 1 == n { to } else { place(p1) };
        out.push(Cmd::Cubic(c1, c2, end));
    }
}

/// `rect`, optionally with corners of radii `rx` by `ry`, clamped to half its
/// sides
pub fn rect(x: f64, y: f64, w: f64, h: f64, rx: f64, ry: f64) -> Vec<Cmd> {
    if rx <= 0.0 || ry <= 0.0 {
        return vec![Cmd::Move((x, y)), Cmd::Line((x + w, y)), Cmd::Line((x + w, y + h)), Cmd::Line((x, y + h)), Cmd::Close];
    }
    let pi = std::f64::consts::PI;
    vec![
        Cmd::Move((x + rx, y)),
        Cmd::Arc { c: (x + w - rx, y + ry), r: (rx, ry), a0: -pi / 2.0, a1: 0.0 },
        Cmd::Arc { c: (x + w - rx, y + h - ry), r: (rx, ry), a0: 0.0, a1: pi / 2.0 },
        Cmd::Arc { c: (x + rx, y + h - ry), r: (rx, ry), a0: pi / 2.0, a1: pi },
        Cmd::Arc { c: (x + rx, y + ry), r: (rx, ry), a0: pi, a1: pi * 1.5 },
        Cmd::Close,
    ]
}

/// `circle` and `ellipse`
pub fn ellipse(cx: f64, cy: f64, rx: f64, ry: f64) -> Vec<Cmd> {
    vec![
        Cmd::Move((cx + rx, cy)),
        Cmd::Arc { c: (cx, cy), r: (rx, ry), a0: 0.0, a1: std::f64::consts::TAU },
        Cmd::Close,
    ]
}

/// `points`, of a `polyline` or (closed) a `polygon`
pub fn poly(points: &str, closed: bool) -> Vec<Cmd> {
    let mut s = Scan { b: points.as_bytes(), at: 0 };
    let mut out = Vec::new();
    while let (Some(x), Some(y)) = (s.num(), s.num()) {
        out.push(if out.is_empty() { Cmd::Move((x, y)) } else { Cmd::Line((x, y)) });
    }
    if closed && !out.is_empty() {
        out.push(Cmd::Close);
    }
    out
}

/// Number list in a `transform` or `viewBox`: separated by spaces, commas, or
/// the next one's sign or point (`translate(-384.57-515.8)`)
pub fn numbers(v: &str) -> Vec<f64> {
    let mut s = Scan { b: v.as_bytes(), at: 0 };
    std::iter::from_fn(|| s.num()).collect()
}

/// The outline through `xf`, on the screen
pub fn shape(cmds: &[Cmd], xf: &Xf) -> Shape {
    let mut sh = Shape::default();
    for c in cmds {
        match *c {
            Cmd::Move(p) => sh.move_to(xf.map(p.0, p.1)),
            Cmd::Line(p) => sh.line_to(xf.map(p.0, p.1)),
            Cmd::Cubic(a, b, p) => sh.cubic_to(xf.map(a.0, a.1), xf.map(b.0, b.1), xf.map(p.0, p.1)),
            Cmd::Close => sh.close(),
            Cmd::Arc { c, r, a0, a1 } => sh.ellipse_arc(xf, c.0, c.1, r.0, r.1, a0, a1, false),
        }
    }
    sh
}

/// The box an outline fills, in its own units, curves to their furthest
pub fn bounds(cmds: &[Cmd]) -> Option<(f64, f64, f64, f64)> {
    let sh = shape(cmds, &Xf::IDENTITY);
    let mut b: Option<(f64, f64, f64, f64)> = None;
    for sub in &sh.subs {
        for p in sub.polyline() {
            b = Some(match b {
                None => (p.0, p.1, p.0, p.1),
                Some(b) => (b.0.min(p.0), b.1.min(p.1), b.2.max(p.0), b.3.max(p.1)),
            });
        }
    }
    b
}

/// Numbers as SVG writes them
struct Scan<'a> {
    b: &'a [u8],
    at: usize,
}

impl Scan<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.at).copied()
    }

    /// Spaces and at most one comma
    fn sep(&mut self) {
        while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
            self.at += 1;
        }
        if self.peek() == Some(b',') {
            self.at += 1;
            while self.peek().is_some_and(|c| c.is_ascii_whitespace()) {
                self.at += 1;
            }
        }
    }

    fn num(&mut self) -> Option<f64> {
        self.sep();
        let start = self.at;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.at += 1;
        }
        let mut digits = false;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.at += 1;
            digits = true;
        }
        if self.peek() == Some(b'.') {
            self.at += 1;
            while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                self.at += 1;
                digits = true;
            }
        }
        if !digits {
            self.at = start;
            return None;
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            let save = self.at;
            self.at += 1;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.at += 1;
            }
            if self.peek().is_some_and(|c| c.is_ascii_digit()) {
                while self.peek().is_some_and(|c| c.is_ascii_digit()) {
                    self.at += 1;
                }
            } else {
                self.at = save;
            }
        }
        std::str::from_utf8(&self.b[start..self.at]).ok()?.parse().ok()
    }

    /// Arc flag: one character, `0` or `1`, with no separator required
    fn flag(&mut self) -> Option<bool> {
        self.sep();
        let f = match self.peek()? {
            b'0' => false,
            b'1' => true,
            _ => return None,
        };
        self.at += 1;
        Some(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_data_reads_every_way_it_is_written() {
        assert_eq!(parse("M4,40-.5,47 12,47z"), vec![
            Cmd::Move((4.0, 40.0)), Cmd::Line((-0.5, 47.0)), Cmd::Line((12.0, 47.0)), Cmd::Close]);
        assert_eq!(parse("m1 1 h2v3H0z"), vec![
            Cmd::Move((1.0, 1.0)), Cmd::Line((3.0, 1.0)), Cmd::Line((3.0, 4.0)), Cmd::Line((0.0, 4.0)), Cmd::Close]);
        assert_eq!(parse("M1.5.5L2e1-1"), vec![Cmd::Move((1.5, 0.5)), Cmd::Line((20.0, -1.0))]);
        // An arc's flags run together, and a bad number ends the path
        let a = parse("M0 0a5 5 0 1010 0 L x");
        assert!(matches!(a.last(), Some(Cmd::Cubic(_, _, p)) if (p.0 - 10.0).abs() < 1e-9 && p.1.abs() < 1e-9));
        // A half turn is two quarter turns, through the bottom with sweep 0
        let half = parse("M0 0 A5 5 0 0 0 10 0");
        assert_eq!(half.len(), 3);
        let Cmd::Cubic(_, _, mid) = half[1] else { panic!() };
        assert!((mid.0 - 5.0).abs() < 1e-9 && (mid.1 - 5.0).abs() < 1e-9, "{mid:?}");
    }

    #[test]
    fn number_lists_run_together_as_breeze_writes_them() {
        assert_eq!(numbers("-384.57-515.8"), [-384.57, -515.8]);
        assert_eq!(numbers("1-1"), [1.0, -1.0]);
        assert_eq!(numbers(" 0, 0 32\t32 "), [0.0, 0.0, 32.0, 32.0]);
        assert_eq!(numbers(".5.5e1"), [0.5, 5.0]);
    }
}
