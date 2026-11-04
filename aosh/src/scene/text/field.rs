// Distance-field text, Qt Quick's default (`Text.QtRendering`), matched against
// qml6 (`dftext.qml`) and QDistanceField
//
// - Outline: the glyph loaded with FreeType's default hinting at units per em,
//   points scaled to the 54 px base size by 16.16 multiply, conics converted to
//   cubics (`outline`).
// - Field: the outline's box, curve extremes included, plus 5 px, one texel per
//   base pixel. Each texel is the signed distance from its center (inside
//   positive), 127 + 25.5 per pixel, as a byte. The outline flattens until
//   handles are within half a pixel of the chord (`flatten_by`), keeping only
//   true edges since variable font contours overlap (`boundary`).
// - Drawing: the field covers the glyph's box at the font size, from the
//   unrounded pen, filtered. Each pixel is the color times smoothstep(0.5 -
//   0.06 / s, 0.5 + 0.06 / s), s being size over 54 times scale (`threshold`,
//   `spread`). Grey in a layer.
//
// Not implemented: narrow-outline fonts (Qt uses 108 px), colored edges on
// opaque windows. Outlined and raised text are drawn natively.

use crate::scene::gpu;

/// Base size of glyph fields, and reach around the outline, in base pixels
pub const BASE: f64 = 54.0;
pub const REACH: f64 = 5.0;

/// Maximum summed handle distance from the chord when flattening, in base
/// pixels (fitted to Qt's fields)
const FLAT: f64 = 0.5;

/// Whether a cubic is flat enough to be its chord
fn flat([p0, c1, c2, p3]: [(f64, f64); 4]) -> bool {
    let (dx, dy) = (p3.0 - p0.0, p3.1 - p0.1);
    let len = (dx * dx + dy * dy).sqrt().max(1e-9);
    let off = |c: (f64, f64)| ((c.0 - p0.0) * dy - (c.1 - p0.1) * dx).abs() / len;
    off(c1) + off(c2) < FLAT
}

/// Glyph field: `w` by `h` texels with distance in the top byte, and its top
/// left corner from the pen in base pixels, y down
pub struct Field {
    pub px: Vec<u32>,
    /// What its copy in video memory is kept under (`gpu::serial`)
    pub serial: u64,
    pub w: usize,
    pub h: usize,
    pub x: f64,
    pub y: f64,
}

/// FreeType's 16.16 multiply and divide, rounding as it does
fn mul_fix(a: i64, b: i64) -> i64 {
    let ab = a * b;
    (ab + 0x8000 - (ab < 0) as i64) >> 16
}

fn div_fix(a: i64, b: i64) -> i64 {
    let (s, a, b) = ((a < 0) != (b < 0), a.abs(), b.abs());
    let q = if b == 0 { 0x7FFF_FFFF } else { ((a << 16) + (b >> 1)) / b };
    if s { -q } else { q }
}

/// A piece of an outline, in pixels, y down
#[derive(Clone, Copy, Debug)]
enum Piece {
    Line((f64, f64)),
    Cubic((f64, f64), (f64, f64), (f64, f64)),
}

/// Outline loaded at `upm` pixels, at base size: contours with a start and
/// pieces
fn outline(contours: &[Vec<freetype::Point>], upm: i64) -> Vec<((f64, f64), Vec<Piece>)> {
    let k = div_fix(BASE as i64, upm.max(1));
    let at = |p: &freetype::Point| (mul_fix(p.x, k) as f64 / 64.0, -(mul_fix(p.y, k) as f64) / 64.0);
    let mid = |a: (f64, f64), b: (f64, f64)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
    let mut out = Vec::new();
    for c in contours.iter().filter(|c| !c.is_empty()) {
        let pts: Vec<((f64, f64), &freetype::Point)> = c.iter().map(|p| (at(p), p)).collect();
        let n = pts.len();
        // Start on the curve: the first on-curve point, or between the first
        // two conic controls
        let first_on = pts.iter().position(|(_, p)| p.on);
        let (start, from) = match first_on {
            Some(i) => (pts[i].0, i),
            None => (mid(pts[n - 1].0, pts[0].0), 0),
        };
        let mut pieces = Vec::new();
        let mut cur = start;
        let mut conic: Option<(f64, f64)> = None;
        let mut cubic: Vec<(f64, f64)> = Vec::new();
        let quad = |p0: (f64, f64), q: (f64, f64), p2: (f64, f64)| {
            let c1 = (p0.0 + 2.0 / 3.0 * (q.0 - p0.0), p0.1 + 2.0 / 3.0 * (q.1 - p0.1));
            let c2 = (p2.0 + 2.0 / 3.0 * (q.0 - p2.0), p2.1 + 2.0 / 3.0 * (q.1 - p2.1));
            Piece::Cubic(c1, c2, p2)
        };
        // Once round from just after the start, and back to it
        let steps = if first_on.is_some() { n } else { n + 1 };
        for s in 1..=steps {
            let (pt, p) = if first_on.is_none() && s == steps { (start, None) } else {
                let (q, p) = &pts[(from + s) % n];
                (*q, Some(*p))
            };
            let on = p.is_none_or(|p| p.on);
            let is_cubic = p.is_some_and(|p| p.cubic);
            if on {
                if let Some(q) = conic.take() {
                    pieces.push(quad(cur, q, pt));
                } else if cubic.len() == 2 {
                    pieces.push(Piece::Cubic(cubic[0], cubic[1], pt));
                } else {
                    pieces.push(Piece::Line(pt));
                }
                cubic.clear();
                cur = pt;
            } else if is_cubic {
                cubic.push(pt);
            } else {
                if let Some(q) = conic {
                    let m = mid(q, pt);
                    pieces.push(quad(cur, q, m));
                    cur = m;
                }
                conic = Some(pt);
            }
        }
        out.push((start, pieces));
    }
    out
}

/// Bounding box of a cubic: its ends and turning points
fn extremes(p0: (f64, f64), c1: (f64, f64), c2: (f64, f64), p3: (f64, f64), b: &mut [f64; 4]) {
    let mut take = |p: (f64, f64)| {
        b[0] = b[0].min(p.0);
        b[1] = b[1].min(p.1);
        b[2] = b[2].max(p.0);
        b[3] = b[3].max(p.1);
    };
    take(p0);
    take(p3);
    let at = |t: f64| {
        let u = 1.0 - t;
        let f = |a: f64, b: f64, c: f64, d: f64| u * u * u * a + 3.0 * u * u * t * b + 3.0 * u * t * t * c + t * t * t * d;
        (f(p0.0, c1.0, c2.0, p3.0), f(p0.1, c1.1, c2.1, p3.1))
    };
    for axis in 0..2 {
        let g = |p: (f64, f64)| if axis == 0 { p.0 } else { p.1 };
        // Derivative roots: a t^2 + b t + c
        let (a0, a1, a2, a3) = (g(p0), g(c1), g(c2), g(p3));
        let a = -a0 + 3.0 * a1 - 3.0 * a2 + a3;
        let bq = 2.0 * (a0 - 2.0 * a1 + a2);
        let c = a1 - a0;
        let mut roots = Vec::new();
        if a.abs() < 1e-12 {
            if bq.abs() > 1e-12 {
                roots.push(-c / bq);
            }
        } else {
            let d = bq * bq - 4.0 * a * c;
            if d >= 0.0 {
                roots.push((-bq + d.sqrt()) / (2.0 * a));
                roots.push((-bq - d.sqrt()) / (2.0 * a));
            }
        }
        for t in roots.into_iter().filter(|t| *t > 0.0 && *t < 1.0) {
            take(at(t));
        }
    }
}

/// Field of a glyph from a FreeType outline at `upm` pixels; `None` with no
/// outline (a space)
pub fn make(contours: &[Vec<freetype::Point>], upm: i64) -> Option<Field> {
    let shape = outline(contours, upm);
    let mut b = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
    let mut lines: Vec<Vec<(f64, f64)>> = Vec::new();
    for (start, pieces) in &shape {
        let mut pts = vec![*start];
        let mut cur = *start;
        for piece in pieces {
            match *piece {
                Piece::Line(p) => {
                    extremes(cur, cur, p, p, &mut b);
                    pts.push(p);
                    cur = p;
                }
                Piece::Cubic(c1, c2, p) => {
                    extremes(cur, c1, c2, p, &mut b);
                    crate::scene::canvas::shape::flatten_by(cur, c1, c2, p, &flat, &mut pts);
                    cur = p;
                }
            }
        }
        lines.push(pts);
    }
    if b[0] > b[2] {
        return None;
    }
    let w = (b[2] - b[0] + 2.0 * REACH).ceil() as usize;
    let h = (b[3] - b[1] + 2.0 * REACH).ceil() as usize;
    let (x0, y0) = (b[0] - REACH, b[1] - REACH);
    let edges: Vec<((f64, f64), (f64, f64))> = lines.iter()
        .flat_map(|pts| (0..pts.len()).map(move |k| (pts[k], pts[(k + 1) % pts.len()])))
        .filter(|(a, b)| a != b)
        .collect();
    let rim = boundary(&edges);
    let mut px = vec![0u32; w * h];
    for j in 0..h {
        let y = y0 + j as f64 + 0.5;
        for i in 0..w {
            let x = x0 + i as f64 + 0.5;
            let d = nearest(&rim, x, y);
            let d = if winding(&edges, x, y) != 0 { d } else { -d };
            let v = (127.0 + 25.5 * d + 0.5).floor().clamp(0.0, 255.0) as u32;
            px[j * w + i] = v << 24;
        }
    }
    Some(Field { px, serial: crate::scene::gpu::serial(), w, h, x: x0, y: y0 })
}

/// Keeps only true glyph edges, since variable font contours overlap: each line
/// in quarter-pixel pieces, kept where one side is filled and the other not, by
/// the winding rule
fn boundary(edges: &[((f64, f64), (f64, f64))]) -> Vec<((f64, f64), (f64, f64))> {
    const PIECE: f64 = 0.25;
    const SIDE: f64 = 1e-3;
    let mut out = Vec::with_capacity(edges.len());
    for &(a, b) in edges {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt();
        let n = (len / PIECE).ceil().max(1.0) as usize;
        let (nx, ny) = (-dy / len * SIDE, dx / len * SIDE);
        for k in 0..n {
            let (t0, t1) = (k as f64 / n as f64, (k + 1) as f64 / n as f64);
            let p = (a.0 + dx * t0, a.1 + dy * t0);
            let q = (a.0 + dx * t1, a.1 + dy * t1);
            let m = ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
            let left = winding(edges, m.0 + nx, m.1 + ny) != 0;
            let right = winding(edges, m.0 - nx, m.1 - ny) != 0;
            if left != right {
                out.push((p, q));
            }
        }
    }
    out
}

/// How far `(x, y)` is from the nearest of `segs`
fn nearest(segs: &[((f64, f64), (f64, f64))], x: f64, y: f64) -> f64 {
    let mut best = f64::MAX;
    for &(a, b) in segs {
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = dx * dx + dy * dy;
        let t = if len > 0.0 { (((x - a.0) * dx + (y - a.1) * dy) / len).clamp(0.0, 1.0) } else { 0.0 };
        let (qx, qy) = (a.0 + t * dx - x, a.1 + t * dy - y);
        best = best.min(qx * qx + qy * qy);
    }
    best.sqrt()
}

/// How many times the outline winds round `(x, y)`
fn winding(edges: &[((f64, f64), (f64, f64))], x: f64, y: f64) -> i32 {
    let mut w = 0;
    for &(a, b) in edges {
        if (a.1 <= y) != (b.1 <= y) {
            let cross = a.0 + (y - a.1) * (b.0 - a.0) / (b.1 - a.1);
            if x < cross {
                w += if b.1 > a.1 { 1 } else { -1 };
            }
        }
    }
    w
}

/// Threshold at `scale` times the base size: half, less a bit for small text
/// (Qt's `thresholdFunc` defaults)
pub fn threshold(scale: f64) -> f64 {
    let (base, dev, lo, hi) = (0.5, 0.065, 0.15, 0.3);
    base - ((scale.clamp(lo, hi) - lo) / (hi - lo) * -dev + dev)
}

/// How far either side of the edge the ramp runs (`spreadFunc`)
pub fn spread(scale: f64) -> f64 {
    0.06 / scale
}

/// Drawn glyph field: color at the ramp's strength
pub struct Ink<'a> {
    pub tex: gpu::Texture<'a>,
    pub colour: [f32; 4],
    pub lo: f32,
    pub hi: f32,
}

impl gpu::record::Gpu for Ink<'_> {
    const KIND: u32 = gpu::record::SH_INK;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        let c = self.colour;
        Some(vec![rec.texture(&self.tex)?, c[0].to_bits(), c[1].to_bits(), c[2].to_bits(), c[3].to_bits(),
                  self.lo.to_bits(), self.hi.to_bits()])
    }
    fn put_col(&self, c: &gpu::Tap, out: &mut Vec<u32>) {
        gpu::record::tap_words(c, out);
    }
    fn put_row(&self, r: &gpu::Tap, out: &mut Vec<u32>) {
        gpu::record::tap_words(r, out);
    }
}

impl gpu::Shader for Ink<'_> {
    type Col = gpu::Tap;
    type Row = gpu::Tap;
    fn col(&self, u: f32) -> gpu::Tap {
        self.tex.across(u)
    }
    fn row(&self, v: f32) -> gpu::Tap {
        self.tex.down(v)
    }
    fn at(&self, c: &gpu::Tap, r: &gpu::Tap) -> [f32; 4] {
        let d = self.tex.read_alpha(c, r);
        let t = ((d - self.lo) / (self.hi - self.lo)).clamp(0.0, 1.0);
        let a = t * t * (3.0 - 2.0 * t);
        self.colour.map(|k| k * a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freetypes_fixed_point() {
        // 1000 units per em to 54 px: 0.054 as 16.16, rounded up
        assert_eq!(div_fix(54, 1000), 3539);
        // A point Qt has at 367/64 and 311/64 at the base size
        assert_eq!(mul_fix(6787, 3539), 367);
        assert_eq!(mul_fix(5760, 3539), 311);
        assert_eq!(mul_fix(-6787, 3539), -367);
    }

    #[test]
    fn small_text_reads_the_edge_lower() {
        assert_eq!(threshold(64.0 / 54.0), 0.5);
        assert!((threshold(0.1) - 0.435).abs() < 1e-9);
        assert!(threshold(15.0 / 54.0) < 0.5);
    }

    /// A 10 px square: the field spans it plus 5 px, deepest in the middle,
    /// half at its edge
    #[test]
    fn a_square_is_its_distances() {
        let p = |x: i64, y: i64| freetype::Point { x: x * 64, y: y * 64, on: true, cubic: false };
        // At 54 units per em, a unit is a pixel at the base size
        let f = make(&[vec![p(0, 0), p(0, 10), p(10, 10), p(10, 0)]], 54).unwrap();
        assert_eq!((f.w, f.h), (20, 20));
        assert_eq!((f.x, f.y), (-5.0, -15.0));
        let at = |i: usize, j: usize| f.px[j * f.w + i] >> 24;
        // The texel whose centre is half a pixel in from the left edge
        assert_eq!(at(5, 10), (127.0f64 + 12.75 + 0.5).floor() as u32);
        assert_eq!(at(4, 10), (127.0f64 - 12.75 + 0.5).floor() as u32);
        assert_eq!(at(0, 0), 0);
    }
}
