// GPU rules for triangles and quads
//
// A Rectangle is triangles with per-corner colors (`rectangle.rs`); an Image,
// glyph, or layer is a textured quad.
// - Coverage: a pixel center inside the triangle, with top-left fill on edges.
//   Corners are kept to 1/256 pixel.
// - Colors: interpolated to the center, premultiplied, times opacity.
// - Blending: premultiplied source-over in floating point, rounded to 8 bits.
// - Textures: sampled at the center, between the four nearest texels in 256ths
//   or the nearest, clamped.
//
// From Qt Quick: antialiased edge corners move on the screen along the item's
// axes by at most half a pixel (`Vertex::dx`, `dy`), keeping the ramp one pixel
// wide at any transform.

use crate::scene::paint::{Image, Rect};
use crate::scene::xf::Xf;

/// A colour, premultiplied, 0..=255 a channel
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const TRANSPARENT: Rgba = Rgba { r: 0, g: 0, b: 0, a: 0 };

    /// Premultiplies a straight `0xAARRGGBB`, rounded
    pub fn from_argb(c: u32) -> Rgba {
        let a = ((c >> 24) & 0xff) as f32 / 255.0;
        let ch = |shift: u32| (((c >> shift) & 0xff) as f32 / 255.0 * a * 255.0).round() as u8;
        Rgba { r: ch(16), g: ch(8), b: ch(0), a: (a * 255.0).round() as u8 }
    }

    /// Each channel times `t`, cut to a whole value
    pub fn times(self, t: f32) -> Rgba {
        let m = |v: u8| (v as f32 * t) as u8;
        Rgba { r: m(self.r), g: m(self.g), b: m(self.b), a: m(self.a) }
    }

    pub fn plus(self, o: Rgba) -> Rgba {
        Rgba { r: self.r.wrapping_add(o.r), g: self.g.wrapping_add(o.g), b: self.b.wrapping_add(o.b), a: self.a.wrapping_add(o.a) }
    }
}

/// Triangle corner in item coordinates and, on an antialiased edge, its offset
/// along each item axis before the half-pixel limit
#[derive(Clone, Copy, Debug, Default)]
pub struct Vertex {
    pub x: f32,
    pub y: f32,
    pub color: Rgba,
    pub dx: f32,
    pub dy: f32,
}

/// Corners and the triangles over them
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub verts: Vec<Vertex>,
    pub tris: Vec<[u32; 3]>,
}

/// Screen position of a corner in 1/256 pixel: through the matrix, then moved
/// along each item axis by its offset, at most half a pixel
pub(crate) fn place(v: &Vertex, xf: &Xf) -> (i64, i64) {
    let (px, py) = xf.map(v.x as f64, v.y as f64);
    let (mut x, mut y) = (px, py);
    for (off, ax, ay) in [(v.dx as f64, xf.a, xf.b), (v.dy as f64, xf.c, xf.d)] {
        let (mx, my) = (ax * off, ay * off);
        let len = (mx * mx + my * my).sqrt();
        if len > 0.0 {
            let k = (0.5 / len).min(1.0);
            x += mx * k;
            y += my * k;
        }
    }
    ((x * 256.0).round() as i64, (y * 256.0).round() as i64)
}

/// Premultiplied source-over onto `dst`, rounded to 8 bits (no `f32::round`: a
/// libm call on baseline x86_64)
#[inline]
fn blend(dst: &mut u32, r: f32, g: f32, b: f32, a: f32) {
    if a <= 0.0 && r <= 0.0 && g <= 0.0 && b <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    let d = *dst;
    let ch = |shift: u32, s: f32| -> u32 {
        let v = s * 255.0 + ((d >> shift) & 0xff) as f32 * inv;
        ((v + 0.5) as u32).min(255)
    };
    *dst = (ch(24, a) << 24) | (ch(16, r) << 16) | (ch(8, g) << 8) | ch(0, b);
}

/// Premultiplied color as a pixel when opaque
#[inline]
fn opaque(r: f32, g: f32, b: f32, a: f32) -> Option<u32> {
    (a >= 1.0).then(|| {
        let q = |v: f32| ((v * 255.0 + 0.5) as u32).min(255);
        0xFF00_0000 | q(r) << 16 | q(g) << 8 | q(b)
    })
}

/// Precomputed `blend` of one translucent color for all 256 channel values
/// below it
struct Over {
    lut: [[u8; 256]; 4],
}

impl Over {
    fn new(r: f32, g: f32, b: f32, a: f32) -> Over {
        let mut lut = [[0u8; 256]; 4];
        for (k, s) in [a, r, g, b].into_iter().enumerate() {
            for d in 0..256u32 {
                let mut px = d << (24 - 8 * k as u32);
                blend(&mut px, if k == 1 { s } else { 0.0 }, if k == 2 { s } else { 0.0 }, if k == 3 { s } else { 0.0 },
                      if k == 0 { s } else { a });
                lut[k][d as usize] = (px >> (24 - 8 * k as u32)) as u8;
            }
        }
        Over { lut }
    }

    #[inline]
    fn on(&self, d: u32) -> u32 {
        let l = &self.lut;
        (l[0][(d >> 24) as usize] as u32) << 24 | (l[1][(d >> 16 & 0xff) as usize] as u32) << 16
            | (l[2][(d >> 8 & 0xff) as usize] as u32) << 8 | l[3][(d & 0xff) as usize] as u32
    }
}

/// Whether an edge `a → b` of a positively wound triangle owns centers on it:
/// top (flat, going right) or left (going up)
fn top_left(a: (i64, i64), b: (i64, i64)) -> bool {
    let (ex, ey) = (b.0 - a.0, b.1 - a.1);
    (ey == 0 && ex > 0) || ey < 0
}

pub(crate) fn floor_div(a: i64, b: i64) -> i64 {
    let d = a / b;
    if a % b != 0 && ((a < 0) != (b < 0)) { d - 1 } else { d }
}

pub(crate) fn ceil_div(a: i64, b: i64) -> i64 {
    -floor_div(-a, b)
}

/// Rasterizes a triangle into `img` inside `clip`, passing each covered pixel
/// to `shade` with its three corner weights. Row spans are computed exactly in
/// 1/256 pixels
#[inline]
fn triangle(img: &mut Image, clip: Rect, p: [(i64, i64); 3], mut shade: impl FnMut(&mut u32, f32, f32, f32)) {
    let edge = |a: (i64, i64), b: (i64, i64), q: (i64, i64)| (b.0 - a.0) * (q.1 - a.1) - (b.1 - a.1) * (q.0 - a.0);
    let mut p = p;
    let mut area = edge(p[0], p[1], p[2]);
    if area == 0 {
        return;
    }
    let flipped = area < 0;
    if flipped {
        p.swap(1, 2);
        area = -area;
    }
    let fx0 = p.iter().map(|v| v.0).min().unwrap();
    let fx1 = p.iter().map(|v| v.0).max().unwrap();
    let fy0 = p.iter().map(|v| v.1).min().unwrap();
    let fy1 = p.iter().map(|v| v.1).max().unwrap();
    // Pixels whose centre (i + 1/2) is within the box
    let bx0 = ceil_div(fx0 - 128, 256).max(clip.x as i64);
    let bx1 = floor_div(fx1 - 128, 256).min((clip.right() - 1) as i64);
    let y0 = ceil_div(fy0 - 128, 256).max(clip.y as i64);
    let y1 = floor_div(fy1 - 128, 256).min((clip.bottom() - 1) as i64);
    if bx0 > bx1 || y0 > y1 {
        return;
    }
    // Edge k faces corner k. At center q its value is c + dx q.x + dy q.y; q is
    // inside when every value is over 0, or 0 on an owning edge
    let ends = [(p[1], p[2]), (p[2], p[0]), (p[0], p[1])];
    let lin = ends.map(|(a, b)| {
        let (dx, dy) = (-(b.1 - a.1), b.0 - a.0);
        (-(dy * a.1) - dx * a.0, dx, dy, if top_left(a, b) { 0i64 } else { 1 })
    });
    let inv_area = 1.0 / area as f32;
    let iw = img.w as i64;
    for y in y0..=y1 {
        let cy = y * 256 + 128;
        let (mut lo, mut hi) = (bx0, bx1);
        for &(c, dx, dy, t) in &lin {
            // c + dy cy + dx (256 x + 128) >= t
            let r = c + dy * cy;
            let (k, need) = (dx * 256, t - r - dx * 128);
            if k > 0 {
                lo = lo.max(ceil_div(need, k));
            } else if k < 0 {
                hi = hi.min(floor_div(need, k));
            } else if need > 0 {
                hi = lo - 1;
            }
        }
        if lo > hi {
            continue;
        }
        let q0 = lo * 256 + 128;
        let mut w = lin.map(|(c, dx, dy, _)| c + dy * cy + dx * q0);
        let step = lin.map(|(_, dx, _, _)| dx * 256);
        let row = (y * iw) as usize;
        for x in lo..=hi {
            let (l0, l1, l2) = (w[0] as f32 * inv_area, w[1] as f32 * inv_area, w[2] as f32 * inv_area);
            let (l1, l2) = if flipped { (l2, l1) } else { (l1, l2) };
            shade(&mut img.px[row + x as usize], l0, l1, l2);
            for k in 0..3 {
                w[k] += step[k];
            }
        }
    }
}

/// A geometry's triangles, its vertices through `xf`, at `opacity`
pub fn fill(img: &mut Image, clip: Rect, g: &Geometry, xf: &Xf, opacity: f32) {
    let Some(clip) = clip.intersect(&img.bounds()) else { return };
    let placed: Vec<(i64, i64)> = g.verts.iter().map(|v| place(v, xf)).collect();
    let colour = |v: &Vertex| [v.color.r as f32 / 255.0, v.color.g as f32 / 255.0, v.color.b as f32 / 255.0, v.color.a as f32 / 255.0];
    for &[i0, i1, i2] in &g.tris {
        let (i0, i1, i2) = (i0 as usize, i1 as usize, i2 as usize);
        if i2 >= g.verts.len() || i1 >= g.verts.len() || i0 >= g.verts.len() {
            continue;
        }
        let pts = [placed[i0], placed[i1], placed[i2]];
        let (c0, c1, c2) = (colour(&g.verts[i0]), colour(&g.verts[i1]), colour(&g.verts[i2]));
        if c0 == c1 && c1 == c2 {
            // One color: constant per pixel, a plain write when opaque
            let [r, gg, b, a] = c0.map(|v| v * opacity);
            if a <= 0.0 && r <= 0.0 && gg <= 0.0 && b <= 0.0 {
                continue;
            }
            match opaque(r, gg, b, a) {
                Some(px) => triangle(img, clip, pts, |d, _, _, _| *d = px),
                None => {
                    let over = Over::new(r, gg, b, a);
                    triangle(img, clip, pts, |d, _, _, _| *d = over.on(*d));
                }
            }
            continue;
        }
        triangle(img, clip, pts, |d, wa, wb, wc| {
            let mix = |k: usize| (c0[k] * wa + c1[k] * wb + c2[k] * wc) * opacity;
            blend(d, mix(0), mix(1), mix(2), mix(3));
        });
    }
}

/// Texture texels, premultiplied `0xAARRGGBB`, `w` by `h`. `key` keys its video
/// memory copy (`record.rs`): the picture's serial (`super::serial`), or 0 to
/// upload on every draw
pub struct Texture<'a> {
    pub px: &'a [u32],
    pub w: usize,
    pub h: usize,
    pub key: u64,
}

impl<'a> Texture<'a> {
    pub fn new(px: &'a [u32], w: usize, h: usize, key: u64) -> Texture<'a> {
        Texture { px, w, h, key }
    }
}

/// How a quad takes its texels
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// Between the four nearest: a `smooth` Image or a layer
    Linear,
    /// The nearest one
    Nearest,
    /// Qt's text material (`textmask.vert`, `qsgdefaultglyphnode_p.cpp`):
    /// corners snapped to whole pixels after the matrix, nearest texel, and the
    /// glyph node's diagonal
    Glyph,
}

impl Texture<'_> {
    #[inline]
    fn at(&self, x: usize, y: usize) -> [f32; 4] {
        let c = self.px[y * self.w + x];
        [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0, (c >> 24) as f32 / 255.0]
    }

    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        self.at(x.clamp(0, self.w as i64 - 1) as usize, y.clamp(0, self.h as i64 - 1) as usize)
    }

    /// Reads at `(u, v)`, 0..1, like a shader's `texture()`: premultiplied,
    /// 0..1, red first
    pub fn sample(&self, u: f32, v: f32, linear: bool) -> [f32; 4] {
        let (fx, fy) = (u * self.w as f32 - 0.5, v * self.h as f32 - 0.5);
        if !linear {
            return self.texel((fx + 0.5).floor() as i64, (fy + 0.5).floor() as i64);
        }
        self.read(&self.across(u), &self.down(v))
    }

    /// Where `u` falls: the neighboring texels and the fraction between
    pub fn across(&self, u: f32) -> Tap {
        self.across_by(u, true)
    }

    /// Where `v`, 0..1 down the texture, falls
    pub fn down(&self, v: f32) -> Tap {
        self.down_by(v, true)
    }

    /// Where `u` falls, filtered, or the nearest texel unless `linear`
    pub fn across_by(&self, u: f32, linear: bool) -> Tap {
        Tap::new((u * self.w as f32 - 0.5) as f64, self.w, linear)
    }

    pub fn down_by(&self, v: f32, linear: bool) -> Tap {
        Tap::new((v * self.h as f32 - 0.5) as f64, self.h, linear)
    }

    /// Filtered read at the texels `c` and `r` give. A position on a texel
    /// center reads only that texel along the axis
    #[inline]
    pub fn read(&self, c: &Tap, r: &Tap) -> [f32; 4] {
        let lerp = |a: [f32; 4], b: [f32; 4], t: f32| [0, 1, 2, 3].map(|k| a[k] + (b[k] - a[k]) * t);
        match (c.t == 0.0, r.t == 0.0) {
            (true, true) => self.at(c.i0, r.i0),
            (false, true) => lerp(self.at(c.i0, r.i0), self.at(c.i1, r.i0), c.t),
            (true, false) => lerp(self.at(c.i0, r.i0), self.at(c.i0, r.i1), r.t),
            (false, false) => {
                let top = lerp(self.at(c.i0, r.i0), self.at(c.i1, r.i0), c.t);
                let bottom = lerp(self.at(c.i0, r.i1), self.at(c.i1, r.i1), c.t);
                lerp(top, bottom, r.t)
            }
        }
    }

    /// `read`'s alpha alone
    #[inline]
    pub fn read_alpha(&self, c: &Tap, r: &Tap) -> f32 {
        let a = |x: usize, y: usize| (self.px[y * self.w + x] >> 24) as f32 * (1.0 / 255.0);
        match (c.t == 0.0, r.t == 0.0) {
            (true, true) => a(c.i0, r.i0),
            (false, true) => a(c.i0, r.i0) + (a(c.i1, r.i0) - a(c.i0, r.i0)) * c.t,
            (true, false) => a(c.i0, r.i0) + (a(c.i0, r.i1) - a(c.i0, r.i0)) * r.t,
            (false, false) => {
                let top = a(c.i0, r.i0) + (a(c.i1, r.i0) - a(c.i0, r.i0)) * c.t;
                let bottom = a(c.i0, r.i1) + (a(c.i1, r.i1) - a(c.i0, r.i1)) * c.t;
                top + (bottom - top) * r.t
            }
        }
    }
}

/// A ShaderEffect fragment shader in two steps: per-column (`u`) and per-row
/// (`v`) values once each, then a pixel from both. For an unrotated quad these
/// are constant down and across
pub trait Shader {
    type Col;
    type Row;
    fn col(&self, u: f32) -> Self::Col;
    fn row(&self, v: f32) -> Self::Row;
    /// Premultiplied, 0..1 a channel, red first
    fn at(&self, col: &Self::Col, row: &Self::Row) -> [f32; 4];
}

/// Sub-texel fraction in 256ths (8 bits, as Direct3D requires and Mesa and
/// NVIDIA use). A sample just off a center reads that texel
#[inline]
fn subtexel(t: f32) -> f32 {
    ((t * 256.0 + 0.5) as i32) as f32 / 256.0
}

/// Texture axis sample: texel at or before it, the next one (both clamped), and
/// the fraction
#[derive(Clone, Copy, Debug)]
pub struct Tap {
    pub(crate) i0: usize,
    pub(crate) i1: usize,
    pub(crate) t: f32,
}

impl Tap {
    fn new(f: f64, n: usize, linear: bool) -> Tap {
        let last = n as i64 - 1;
        if !linear {
            let i = ((f + 0.5).floor() as i64).clamp(0, last) as usize;
            return Tap { i0: i, i1: i, t: 0.0 };
        }
        let f0 = f.floor();
        let (i, t) = (f0 as i64, subtexel((f - f0) as f32));
        // A whole step on is the next texel's centre
        let (i, t) = if t >= 1.0 { (i + 1, 0.0) } else { (i, t) };
        Tap { i0: i.clamp(0, last) as usize, i1: (i + 1).clamp(0, last) as usize, t }
    }
}

/// Item rectangle `(x, y, w, h)` through `xf`, showing `(u0, v0)`-`(u1, v1)` of
/// `tex` at `opacity`: Qt's textured quad
#[allow(clippy::too_many_arguments)]
pub fn quad(img: &mut Image, clip: Rect, xf: &Xf, (x, y, w, h): (f64, f64, f64, f64), tex: &Texture<'_>,
            (u0, v0, u1, v1): (f32, f32, f32, f32), opacity: f32, sampling: Sampling) {
    let Some(clip) = clip.intersect(&img.bounds()) else { return };
    if tex.w == 0 || tex.h == 0 {
        return;
    }
    let p = quad_corners(xf, (x, y, w, h), sampling);
    let linear = sampling == Sampling::Linear;
    if is_upright(xf) {
        upright(img, clip, p[0], p[3], tex, (u0, v0, u1, v1), opacity, linear);
        return;
    }
    let uv = [(u0, v0), (u1, v0), (u0, v1), (u1, v1)];
    // An image node cuts top right to bottom left
    // (`updateTexturedRectGeometry`), a glyph top left to bottom right
    let tris = if sampling == Sampling::Glyph { [[0usize, 2, 3], [3, 1, 0]] } else { [[0usize, 2, 1], [2, 1, 3]] };
    for tri in tris {
        let pts = [p[tri[0]], p[tri[1]], p[tri[2]]];
        let (t0, t1, t2) = (uv[tri[0]], uv[tri[1]], uv[tri[2]]);
        triangle(img, clip, pts, |d, wa, wb, wc| {
            let u = t0.0 * wa + t1.0 * wb + t2.0 * wc;
            let v = t0.1 * wa + t1.1 * wb + t2.1 * wc;
            let s = tex.sample(u, v, linear);
            blend(d, s[0] * opacity, s[1] * opacity, s[2] * opacity, s[3] * opacity);
        });
    }
}

/// Quad corners in 1/256 pixel: top left, top right, bottom left, bottom right.
/// Glyphs snap to whole pixels
pub(crate) fn quad_corners(xf: &Xf, (x, y, w, h): (f64, f64, f64, f64), sampling: Sampling) -> [(i64, i64); 4] {
    let corner = |px: f64, py: f64| {
        let (sx, sy) = xf.map(px, py);
        if sampling == Sampling::Glyph {
            ((sx + 0.5).floor() as i64 * 256, (sy + 0.5).floor() as i64 * 256)
        } else {
            ((sx * 256.0).round() as i64, (sy * 256.0).round() as i64)
        }
    };
    [corner(x, y), corner(x + w, y), corner(x, y + h), corner(x + w, y + h)]
}

/// Neither rotated nor mirrored: drawn by column and row
pub(crate) fn is_upright(xf: &Xf) -> bool {
    xf.b == 0.0 && xf.c == 0.0 && xf.a > 0.0 && xf.d > 0.0
}

/// Pixels an upright quad covers inside `clip`: columns `x0..x1`, rows
/// `y0..y1`, including the left and top edges only
pub(crate) fn upright_span(clip: Rect, a: (i64, i64), b: (i64, i64)) -> Option<(i64, i64, i64, i64)> {
    let x0 = ceil_div(a.0 - 128, 256).max(clip.x as i64);
    let x1 = ceil_div(b.0 - 128, 256).min(clip.right() as i64);
    let y0 = ceil_div(a.1 - 128, 256).max(clip.y as i64);
    let y1 = ceil_div(b.1 - 128, 256).min(clip.bottom() as i64);
    (x0 < x1 && y0 < y1).then_some((x0, x1, y0, y1))
}

/// Shader values for the columns and rows of an upright quad, computed once
/// each. Shared with `record.rs`
pub(crate) fn shaded_grid<S: Shader>(shader: &S, a: (i64, i64), b: (i64, i64), (x0, x1, y0, y1): (i64, i64, i64, i64))
    -> (Vec<S::Col>, Vec<S::Row>) {
    let (wq, hq) = ((b.0 - a.0) as f32, (b.1 - a.1) as f32);
    let cols = (x0..x1).map(|i| shader.col((i * 256 + 128 - a.0) as f32 / wq)).collect();
    let rows = (y0..y1).map(|j| shader.row((j * 256 + 128 - a.1) as f32 / hq)).collect();
    (cols, rows)
}

/// Upright textured quad taps per column and row, and whether all are on texel
/// centers
pub(crate) fn upright_taps(a: (i64, i64), b: (i64, i64), tex: (usize, usize), (u0, v0, u1, v1): (f32, f32, f32, f32), linear: bool,
                           (x0, x1, y0, y1): (i64, i64, i64, i64)) -> (Vec<Tap>, Vec<Tap>, bool) {
    let (wq, hq) = ((b.0 - a.0) as f64, (b.1 - a.1) as f64);
    let (u0, v0, u1, v1) = (u0 as f64, v0 as f64, u1 as f64, v1 as f64);
    let cols: Vec<Tap> = (x0..x1).map(|x| {
        let u = u0 + ((x * 256 + 128 - a.0) as f64 / wq) * (u1 - u0);
        Tap::new(u * tex.0 as f64 - 0.5, tex.0, linear)
    }).collect();
    let rows: Vec<Tap> = (y0..y1).map(|y| {
        let v = v0 + ((y * 256 + 128 - a.1) as f64 / hq) * (v1 - v0);
        Tap::new(v * tex.1 as f64 - 0.5, tex.1, linear)
    }).collect();
    let whole = cols.iter().chain(&rows).all(|t| t.t == 0.0);
    (cols, rows, whole)
}

/// Item rectangle `(x, y, w, h)` through `xf`, each covered pixel from `shader`
/// at its 0..1 coordinates, blended at `opacity`: a ShaderEffect in the scene
pub fn shaded<S: Shader>(img: &mut Image, clip: Rect, xf: &Xf, (x, y, w, h): (f64, f64, f64, f64), opacity: f32, shader: &S) {
    let Some(clip) = clip.intersect(&img.bounds()) else { return };
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let p = quad_corners(xf, (x, y, w, h), Sampling::Linear);
    let put = |d: &mut u32, c: [f32; 4]| blend(d, c[0] * opacity, c[1] * opacity, c[2] * opacity, c[3] * opacity);
    if is_upright(xf) {
        let (a, b) = (p[0], p[3]);
        let Some(span) = upright_span(clip, a, b) else { return };
        let (x0, _, y0, _) = span;
        let (cols, rows) = shaded_grid(shader, a, b, span);
        let iw = img.w as usize;
        for (k, row) in rows.iter().enumerate() {
            let at = (y0 as usize + k) * iw + x0 as usize;
            for (d, c) in img.px[at..at + cols.len()].iter_mut().zip(&cols) {
                put(d, shader.at(c, row));
            }
        }
        return;
    }
    let uv = [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)];
    for tri in [[0usize, 2, 1], [2, 1, 3]] {
        let (t0, t1, t2) = (uv[tri[0]], uv[tri[1]], uv[tri[2]]);
        triangle(img, clip, [p[tri[0]], p[tri[1]], p[tri[2]]], |d, wa, wb, wc| {
            let (u, v) = (t0.0 * wa + t1.0 * wb + t2.0 * wc, t0.1 * wa + t1.1 * wb + t2.1 * wc);
            put(d, shader.at(&shader.col(u), &shader.row(v)));
        });
    }
}

/// One effect pass into a `tw` by `th` texture: each pixel the quad
/// `(x, y, w, h)` covers gets `shader`'s value, written unblended and rounded
/// to 8 bits. Uncovered pixels are clear
pub fn pass<S: Shader>(tw: i32, th: i32, (x, y, w, h): (f64, f64, f64, f64), shader: &S) -> Image {
    let mut img = Image::new(tw.max(0), th.max(0));
    if w <= 0.0 || h <= 0.0 {
        return img;
    }
    // Centers from the left and top edges, excluding the far ones
    let x0 = ((x - 0.5).ceil() as i64).max(0);
    let x1 = ((x + w - 0.5).ceil() as i64).min(tw as i64);
    let y0 = ((y - 0.5).ceil() as i64).max(0);
    let y1 = ((y + h - 0.5).ceil() as i64).min(th as i64);
    if x0 >= x1 {
        return img;
    }
    let q = |v: f32| ((v.clamp(0.0, 1.0) * 255.0 + 0.5) as u32).min(255);
    let cols: Vec<S::Col> = (x0..x1).map(|i| shader.col(((i as f64 + 0.5 - x) / w) as f32)).collect();
    for j in y0..y1 {
        let row = shader.row(((j as f64 + 0.5 - y) / h) as f32);
        let at = j as usize * tw as usize + x0 as usize;
        for (d, c) in img.px[at..at + cols.len()].iter_mut().zip(&cols) {
            let c = shader.at(c, &row);
            *d = q(c[3]) << 24 | q(c[0]) << 16 | q(c[1]) << 8 | q(c[2]);
        }
    }
    img
}

/// Upright quad from `a` to `b` in 1/256 pixel: sample positions are constant
/// per column and row, so computed once each. At 1:1 it copies texels
#[allow(clippy::too_many_arguments)]
fn upright(img: &mut Image, clip: Rect, a: (i64, i64), b: (i64, i64), tex: &Texture<'_>, uv: (f32, f32, f32, f32),
           opacity: f32, linear: bool) {
    let Some(span) = upright_span(clip, a, b) else { return };
    let (x0, _, y0, _) = span;
    let (cols, rows, whole) = upright_taps(a, b, (tex.w, tex.h), uv, linear, span);
    let iw = img.w as usize;
    for (ry, row_tap) in rows.iter().enumerate() {
        let row = (y0 as usize + ry) * iw + x0 as usize;
        let dst = &mut img.px[row..row + cols.len()];
        if whole {
            let src = &tex.px[row_tap.i0 * tex.w..];
            for (d, c) in dst.iter_mut().zip(&cols) {
                let t = src[c.i0];
                if t >> 24 == 255 && opacity >= 1.0 {
                    *d = t;
                } else if t != 0 {
                    let f = |sh: u32| ((t >> sh) & 0xff) as f32 / 255.0 * opacity;
                    blend(d, f(16), f(8), f(0), f(24));
                }
            }
            continue;
        }
        for (d, c) in dst.iter_mut().zip(&cols) {
            let (p, q, r, s) = (tex.at(c.i0, row_tap.i0), tex.at(c.i1, row_tap.i0), tex.at(c.i0, row_tap.i1), tex.at(c.i1, row_tap.i1));
            let mut out = [0.0f32; 4];
            for k in 0..4 {
                let top = p[k] + (q[k] - p[k]) * c.t;
                let bottom = r[k] + (s[k] - r[k]) * c.t;
                out[k] = (top + (bottom - top) * row_tap.t) * opacity;
            }
            blend(d, out[0], out[1], out[2], out[3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_triangles_tile_without_seams_or_overlap() {
        // Two triangles sharing a diagonal cover every pixel once
        let mut img = Image::new(8, 8);
        let v = |x: f32, y: f32| Vertex { x, y, color: Rgba { r: 0, g: 0, b: 0, a: 128 }, ..Vertex::default() };
        let g = Geometry { verts: vec![v(1.0, 1.0), v(7.0, 1.0), v(1.0, 7.0), v(7.0, 7.0)], tris: vec![[0, 1, 2], [1, 2, 3]] };
        let all = img.bounds();
        fill(&mut img, all, &g, &Xf::IDENTITY, 1.0);
        let inside = (1..7).flat_map(|y| (1..7).map(move |x| (x, y)));
        assert!(inside.clone().all(|(x, y)| img.px[y * 8 + x] >> 24 == 128), "one coat, no gap, no double");
        assert_eq!(img.px.iter().filter(|p| **p >> 24 != 0).count(), 36);
    }

    #[test]
    fn a_picture_scales_into_its_box() {
        // Two premultiplied texels, red then transparent, stretched to four
        // pixels: no dark fringe
        let src = [0xFFFF0000u32, 0x00000000];
        let mut img = Image::new(4, 1);
        let all = img.bounds();
        quad(&mut img, all, &Xf::IDENTITY, (0.0, 0.0, 4.0, 1.0), &Texture::new(&src, 2, 1, 0), (0.0, 0.0, 1.0, 1.0), 1.0, Sampling::Linear);
        assert_eq!(img.px[0], 0xFFFF0000);
        assert_eq!(img.px[3] >> 24, 0);
        let mid = crate::scene::paint::to_straight(img.px[1]);
        assert_eq!((mid >> 16) & 0xff, 0xFF, "premultiplied: no grey halo");
    }
}

/// Painter cost at screen size:
/// `cargo test-host --release gpu_cost -- --ignored --nocapture`
#[cfg(test)]
mod cost {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore]
    fn gpu_cost() {
        let (w, h) = (1920i32, 1080i32);
        let mut img = Image::new(w, h);
        let all = img.bounds();
        let px: Vec<u32> = (0..(w * h) as u32).map(|i| 0xFF00_0000 | i.wrapping_mul(2654435761) >> 8).collect();
        let tex = Texture::new(&px, w as usize, h as usize, 0);
        let t = Instant::now();
        quad(&mut img, all, &Xf::IDENTITY, (0.0, 0.0, w as f64, h as f64), &tex, (0.0, 0.0, 1.0, 1.0), 1.0, Sampling::Linear);
        eprintln!("wallpaper quad 1:1      {:>6.1} ms", t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        quad(&mut img, all, &Xf::IDENTITY, (0.0, 0.0, w as f64, h as f64), &Texture::new(&px, 960, 540, 0), (0.0, 0.0, 1.0, 1.0), 1.0, Sampling::Linear);
        eprintln!("wallpaper quad scaled   {:>6.1} ms", t.elapsed().as_secs_f64() * 1e3);
        let v = |x: f32, y: f32| Vertex { x, y, color: Rgba { r: 20, g: 30, b: 40, a: 191 }, ..Vertex::default() };
        let g = Geometry { verts: vec![v(0.0, 0.0), v(w as f32, 0.0), v(0.0, h as f32), v(w as f32, h as f32)], tris: vec![[0, 1, 2], [1, 2, 3]] };
        let t = Instant::now();
        fill(&mut img, all, &g, &Xf::IDENTITY, 1.0);
        eprintln!("translucent rect        {:>6.1} ms", t.elapsed().as_secs_f64() * 1e3);
        let t = Instant::now();
        fill(&mut img, all, &g, &Xf::IDENTITY, 0.7);
        eprintln!("translucent rect at .7  {:>6.1} ms", t.elapsed().as_secs_f64() * 1e3);
    }
}
