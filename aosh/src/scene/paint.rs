// A window's image and its painter
//
// Premultiplied ARGB, so layered translucency composes with one multiply-add
// per channel. Mayland buffers use straight alpha, so `to_straight` converts on
// copy out.
//
// Draws what Qt Quick sends its GPU: Rectangle triangles (`rectangle.rs`),
// textured quads for Images and layers, glyph quads. `gpu.rs` draws them as a
// GPU would. A translated glyph takes a direct path with the same pixels.

use super::gpu::{self, Geometry, Sampling, Texture};
use super::xf::Xf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect { x, y, w, h }
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }

    pub fn right(&self) -> i32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        let x1 = self.right().max(o.right());
        let y1 = self.bottom().max(o.bottom());
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn area(&self) -> i64 {
        if self.is_empty() { 0 } else { self.w as i64 * self.h as i64 }
    }

    /// Parts of this outside `o`, up to four rectangles: above, below, then
    /// left and right
    pub fn minus(&self, o: &Rect) -> Vec<Rect> {
        let Some(i) = self.intersect(o) else {
            return if self.is_empty() { Vec::new() } else { vec![*self] };
        };
        [
            Rect::new(self.x, self.y, self.w, i.y - self.y),
            Rect::new(self.x, i.bottom(), self.w, self.bottom() - i.bottom()),
            Rect::new(self.x, i.y, i.x - self.x, i.h),
            Rect::new(i.right(), i.y, self.right() - i.right(), i.h),
        ].into_iter().filter(|r| !r.is_empty()).collect()
    }

    /// The band `n` pixels wide just inside the edge, as four rectangles
    pub fn ring(&self, n: i32) -> Vec<Rect> {
        let inner = Rect::new(self.x + n, self.y + n, self.w - 2 * n, self.h - 2 * n);
        if inner.is_empty() {
            return if self.is_empty() { Vec::new() } else { vec![*self] };
        }
        self.minus(&inner)
    }

    /// `n` pixels bigger on every side
    pub fn grown(&self, n: i32) -> Rect {
        if self.is_empty() {
            return *self;
        }
        Rect::new(self.x - n, self.y - n, self.w + 2 * n, self.h + 2 * n)
    }

    /// Whole pixels a fractional rectangle touches
    pub fn covering(x: f64, y: f64, w: f64, h: f64) -> Rect {
        let x0 = x.floor() as i32;
        let y0 = y.floor() as i32;
        let x1 = (x + w).ceil() as i32;
        let y1 = (y + h).ceil() as i32;
        Rect::new(x0, y0, x1 - x0, y1 - y0)
    }
}

pub struct Image {
    pub w: i32,
    pub h: i32,
    /// Premultiplied `0xAARRGGBB`, row after row
    pub px: Vec<u32>,
    /// Key of its video memory copy (`gpu::serial`). `renew` after drawing into
    /// it
    pub serial: u64,
}

impl Image {
    pub fn new(w: i32, h: i32) -> Image {
        Image { w, h, px: vec![0; (w.max(0) * h.max(0)) as usize], serial: gpu::serial() }
    }

    /// Pixels changed; textures made from it are stale
    pub fn renew(&mut self) {
        self.serial = gpu::serial();
    }

    /// It, as a texture
    pub fn texture(&self) -> Texture<'_> {
        Texture::new(&self.px, self.w.max(0) as usize, self.h.max(0) as usize, self.serial)
    }

    pub fn bounds(&self) -> Rect {
        Rect::new(0, 0, self.w, self.h)
    }

    /// Back to nothing, inside `r`
    pub fn clear(&mut self, r: Rect) {
        let Some(r) = r.intersect(&self.bounds()) else { return };
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            self.px[row + r.x as usize..row + r.right() as usize].fill(0);
        }
    }

    /// Copies `r` to a straight-alpha `ARGB8888` buffer of the same size,
    /// `stride` pixels per row
    pub fn copy_out(&self, r: Rect, dst: &mut [u8], stride: usize) {
        let Some(r) = r.intersect(&self.bounds()) else { return };
        let table = unpremultiply();
        let (x0, x1) = (r.x as usize, r.right() as usize);
        for y in r.y..r.bottom() {
            let row = (y * self.w) as usize;
            let src = &self.px[row + x0..row + x1];
            // Row by row, clipped to the buffer; a partial pixel is not written
            let at = (y as usize * stride + x0) * 4;
            let end = (at + src.len() * 4).min(dst.len());
            let Some(out) = dst.get_mut(at..end) else { continue };
            for (d, &p) in out.chunks_exact_mut(4).zip(src) {
                d.copy_from_slice(&straight(table, p).to_le_bytes());
            }
        }
    }
}

/// QQuickText's `style`: a glyph's shadow or outline
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextStyle {
    #[default]
    Normal,
    Outline,
    Raised,
    Sunken,
}

impl TextStyle {
    /// Glyph quad growth for the style: left, top, right, bottom
    /// (`qsgdefaultglyphnode.cpp`)
    pub fn margins(self) -> (i32, i32, i32, i32) {
        match self {
            TextStyle::Normal => (0, 0, 0, 0),
            TextStyle::Outline => (1, 1, 1, 1),
            TextStyle::Raised => (0, 0, 0, 1),
            TextStyle::Sunken => (0, 1, 0, 0),
        }
    }

    /// `Text.Normal` 0, `Text.Outline` 1, `Text.Raised` 2, `Text.Sunken` 3, the
    /// same in `paint.cu`
    pub fn code(self) -> u32 {
        match self {
            TextStyle::Normal => 0,
            TextStyle::Outline => 1,
            TextStyle::Raised => 2,
            TextStyle::Sunken => 3,
        }
    }

    pub fn from_qml(v: i64) -> TextStyle {
        match v {
            1 => TextStyle::Outline,
            2 => TextStyle::Raised,
            3 => TextStyle::Sunken,
            _ => TextStyle::Normal,
        }
    }
}

/// A straight-alpha colour at `opacity`, premultiplied
pub fn premultiply(c: u32, opacity: f32) -> [u32; 4] {
    let a = (((c >> 24) & 0xff) as f32 * opacity.clamp(0.0, 1.0)).round() as u32;
    let ch = |shift: u32| (((c >> shift) & 0xff) * a + 127) / 255;
    [a, ch(16), ch(8), ch(0)]
}

/// Premultiplied to straight
pub fn to_straight(p: u32) -> u32 {
    straight(unpremultiply(), p)
}

/// `to_straight` with the table already in hand, for a loop over a buffer
#[inline]
fn straight(t: &[[u8; 256]; 256], p: u32) -> u32 {
    let a = p >> 24;
    if a == 0 {
        return 0;
    }
    if a == 255 {
        return p;
    }
    let row = &t[a as usize];
    let un = |shift: u32| row[((p >> shift) & 0xff) as usize] as u32;
    (a << 24) | (un(16) << 16) | (un(8) << 8) | un(0)
}

/// `(c · 255 + a/2) / a`, at most 255, for every alpha and channel. 64 KiB,
/// built on first use
fn unpremultiply() -> &'static [[u8; 256]; 256] {
    static TABLE: std::sync::OnceLock<Box<[[u8; 256]; 256]>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Box::new([[0u8; 256]; 256]);
        for a in 1..256usize {
            for c in 0..256usize {
                t[a][c] = ((c * 255 + a / 2) / a).min(255) as u8;
            }
        }
        t
    })
}

/// Source-over of premultiplied `[a, r, g, b]` scaled by `cov` onto `dst`
#[inline]
fn over(dst: u32, src: &[u32; 4], cov: u32) -> u32 {
    if cov == 0 {
        return dst;
    }
    let s = if cov >= 255 {
        *src
    } else {
        [(src[0] * cov + 127) / 255, (src[1] * cov + 127) / 255, (src[2] * cov + 127) / 255, (src[3] * cov + 127) / 255]
    };
    let inv = 255 - s[0];
    let ch = |shift: u32, v: u32| v + ((((dst >> shift) & 0xff) * inv + 127) / 255);
    let a = ch(24, s[0]).min(255);
    let r = ch(16, s[1]).min(255);
    let g = ch(8, s[2]).min(255);
    let b = ch(0, s[3]).min(255);
    (a << 24) | (r << 16) | (g << 8) | b
}

pub struct Painter<'a> {
    to: To<'a>,
    clip: Rect,
}

/// Paint target: an image, or a GPU recording (`gpu/record.rs`)
enum To<'a> {
    Image(&'a mut Image),
    Record(&'a mut gpu::record::Record),
}

impl<'a> Painter<'a> {
    pub fn new(img: &'a mut Image, clip: Rect) -> Self {
        let clip = clip.intersect(&img.bounds()).unwrap_or_default();
        Painter { to: To::Image(img), clip }
    }

    /// One that records, for the rectangle `rec` has open
    pub fn record(rec: &'a mut gpu::record::Record, clip: Rect) -> Self {
        let clip = clip.intersect(&rec.bounds()).unwrap_or_default();
        Painter { to: To::Record(rec), clip }
    }

    /// Where it paints
    pub fn clip(&self) -> Rect {
        self.clip
    }

    /// Adds `r` to the clip until the guard is dropped
    pub fn with_clip<R>(&mut self, r: Rect, f: impl FnOnce(&mut Painter<'_>) -> R) -> Option<R> {
        let inner = self.clip.intersect(&r)?;
        let saved = self.clip;
        self.clip = inner;
        let out = f(self);
        self.clip = saved;
        Some(out)
    }

    /// `colour` over everything inside the clip
    pub fn fill(&mut self, colour: u32) {
        let area = self.clip;
        let img = match &mut self.to {
            To::Image(img) => img,
            To::Record(rec) => return rec.fill(area, colour),
        };
        let src = premultiply(colour, 1.0);
        let iw = img.w;
        for py in area.y..area.bottom() {
            let row = (py * iw) as usize;
            for px in area.x..area.right() {
                let i = row + px as usize;
                img.px[i] = over(img.px[i], &src, 255);
            }
        }
    }

    /// Rectangle triangles through `xf` at `opacity`
    pub fn geometry(&mut self, g: &Geometry, xf: &Xf, opacity: f32) {
        match &mut self.to {
            To::Image(img) => gpu::fill(img, self.clip, g, xf, opacity),
            To::Record(rec) => rec.tris(self.clip, g, xf, opacity),
        }
    }

    /// A texture on a quad of the item's (`gpu::quad`)
    #[allow(clippy::too_many_arguments)]
    pub fn quad(&mut self, xf: &Xf, rect: (f64, f64, f64, f64), tex: &Texture<'_>, uv: (f32, f32, f32, f32), opacity: f32,
                sampling: Sampling) {
        match &mut self.to {
            To::Image(img) => gpu::quad(img, self.clip, xf, rect, tex, uv, opacity, sampling),
            To::Record(rec) => rec.quad(self.clip, xf, rect, tex, uv, opacity, sampling),
        }
    }

    /// Item rectangle colored per pixel by `shader` (`gpu::shaded`)
    pub fn shaded<S: gpu::record::Gpu>(&mut self, xf: &Xf, rect: (f64, f64, f64, f64), opacity: f32, shader: &S) {
        match &mut self.to {
            To::Image(img) => gpu::shaded(img, self.clip, xf, rect, opacity, shader),
            To::Record(rec) => rec.shaded(self.clip, xf, rect, opacity, shader),
        }
    }

    /// Glyph mask at `(x, y)` of an item rotated or scaled by `xf`, as a
    /// textured quad (`Sampling::Glyph`). The recorded texture is keyed by
    /// mask, colors, and style, so each glyph uploads once during scaling
    #[allow(clippy::too_many_arguments)]
    pub fn glyph(&mut self, xf: &Xf, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, key: u64, colour: u32,
                 style: TextStyle, style_colour: u32, opacity: f32) {
        let (l, t, r, b) = style.margins();
        let (tw, th) = (mw as i32 + l + r, mh as i32 + t + b);
        let mut tex = Image::new(tw, th);
        {
            let mut p = Painter::new(&mut tex, Rect::new(0, 0, tw, th));
            p.styled_mask(l, t, mask, mw, mh, 0, colour, style, style_colour, opacity);
            if style == TextStyle::Outline && colour >> 24 != 0 && style_colour != colour {
                p.mask(l, t, mask, mw, mh, 0, colour, opacity);
            }
        }
        let rect = ((x - l) as f64, (y - t) as f64, tw as f64, th as f64);
        // Above every serial: a print of the texel inputs, available only for
        // masks with a serial
        let print = (key != 0).then(|| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (key, colour, style.code(), style_colour, opacity.to_bits()).hash(&mut h);
            h.finish() | 1 << 63
        });
        let tex = Texture::new(&tex.px, tw as usize, th as usize, print.unwrap_or(0));
        let clip = self.clip;
        match &mut self.to {
            To::Image(img) => gpu::quad(img, clip, xf, rect, &tex, (0.0, 0.0, 1.0, 1.0), 1.0, Sampling::Glyph),
            To::Record(rec) => rec.quad(clip, xf, rect, &tex, (0.0, 0.0, 1.0, 1.0), 1.0, Sampling::Glyph),
        }
    }

    /// Coverage mask at `(x, y)` in `colour`. `key` keys it in video memory
    /// (`gpu::serial`), 0 for none
    #[allow(clippy::too_many_arguments)]
    pub fn mask(&mut self, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, key: u64, colour: u32, opacity: f32) {
        let clip = self.clip;
        let img = match &mut self.to {
            To::Image(img) => img,
            To::Record(rec) => return rec.mask(clip, x, y, mask, mw, mh, key, colour, opacity),
        };
        let src = premultiply(colour, opacity);
        let r = Rect::new(x, y, mw as i32, mh as i32);
        let Some(area) = r.intersect(&clip) else { return };
        let iw = img.w;
        for py in area.y..area.bottom() {
            let mrow = (py - y) as usize * mw;
            let row = (py * iw) as usize;
            for px in area.x..area.right() {
                let cov = mask[mrow + (px - x) as usize] as u32;
                if cov != 0 {
                    let i = row + px as usize;
                    img.px[i] = over(img.px[i], &src, cov);
                }
            }
        }
    }

    /// The target image. A recording refuses the rectangle, since what follows
    /// has no GPU form
    fn image(&mut self) -> Option<&mut Image> {
        match &mut self.to {
            To::Image(img) => Some(img),
            To::Record(rec) => {
                rec.refuse("a layer or a shaded mask drawn into the window");
                None
            }
        }
    }

    /// Premultiplied picture of the same size over this one at `opacity`, as
    /// QtSvg draws a group with opacity
    pub fn layer(&mut self, src: &Image, opacity: f32) {
        let clip = self.clip;
        let Some(img) = self.image() else { return };
        let ca = (opacity.clamp(0.0, 1.0) * 255.0).round() as u32;
        if ca == 0 || src.w != img.w || src.h != img.h {
            return;
        }
        let Some(area) = img.bounds().intersect(&clip) else { return };
        let iw = img.w;
        for py in area.y..area.bottom() {
            let row = (py * iw) as usize;
            for px in area.x..area.right() {
                let i = row + px as usize;
                let s = src.px[i];
                if s >> 24 != 0 {
                    img.px[i] = over(img.px[i], &[s >> 24, (s >> 16) & 0xff, (s >> 8) & 0xff, s & 0xff], ca);
                }
            }
        }
    }

    /// Coverage mask at `(x, y)` colored per pixel by `shade`, straight
    /// `0xAARRGGBB`
    #[allow(clippy::too_many_arguments)]
    pub fn mask_shaded(&mut self, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, opacity: f32, shade: impl Fn(i32, i32) -> u32) {
        let clip = self.clip;
        let Some(img) = self.image() else { return };
        let r = Rect::new(x, y, mw as i32, mh as i32);
        let Some(area) = r.intersect(&clip) else { return };
        let iw = img.w;
        for py in area.y..area.bottom() {
            let mrow = (py - y) as usize * mw;
            let row = (py * iw) as usize;
            for px in area.x..area.right() {
                let cov = mask[mrow + (px - x) as usize] as u32;
                if cov != 0 {
                    let i = row + px as usize;
                    img.px[i] = over(img.px[i], &premultiply(shade(px, py), opacity), cov);
                }
            }
        }
    }

    /// Glyph mask with a Text `style`, as in `styledtext.frag` and
    /// `outlinedtext.frag`: the mask offset a pixel (down for Raised, up for
    /// Sunken, both for Outline) minus the glyph in `style_colour`, under the
    /// glyph in `colour`
    #[allow(clippy::too_many_arguments)]
    pub fn styled_mask(&mut self, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, key: u64, colour: u32, style: TextStyle,
                       style_colour: u32, opacity: f32) {
        let clip = self.clip;
        let img = match &mut self.to {
            To::Image(img) => img,
            To::Record(rec) => return rec.styled(clip, x, y, mask, mw, mh, key, colour, style, style_colour, opacity),
        };
        let (c, sc) = (premultiply(colour, opacity), premultiply(style_colour, opacity));
        let at = |mx: i32, my: i32| -> i32 {
            if mx < 0 || my < 0 || mx >= mw as i32 || my >= mh as i32 { 0 } else { mask[my as usize * mw + mx as usize] as i32 }
        };
        let (l, t, r, b) = style.margins();
        let quad = Rect::new(x - l, y - t, mw as i32 + l + r, mh as i32 + t + b);
        let Some(area) = quad.intersect(&clip) else { return };
        let iw = img.w;
        for py in area.y..area.bottom() {
            let row = (py * iw) as usize;
            let my = py - y;
            for px in area.x..area.right() {
                let mx = px - x;
                let g = at(mx, my);
                let around = match style {
                    TextStyle::Normal => 0,
                    TextStyle::Raised => at(mx, my - 1),
                    TextStyle::Sunken => at(mx, my + 1),
                    TextStyle::Outline => (at(mx, my + 1) + at(mx, my - 1) + at(mx + 1, my) + at(mx - 1, my)).min(255),
                };
                let st = (around - g).clamp(0, 255) as u32;
                let g = g as u32;
                if st == 0 && g == 0 {
                    continue;
                }
                let mix = |k: usize| ((st * sc[k] + g * c[k] + 127) / 255).min(255);
                let i = row + px as usize;
                img.px[i] = over(img.px[i], &[mix(0), mix(1), mix(2), mix(3)], 255);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    /// The table is the division, for every alpha and every channel value
    #[test]
    fn straight_is_the_division() {
        for a in 0..256u32 {
            for c in 0..256u32 {
                let p = (a << 24) | (c << 16) | ((255 - c) << 8) | (c / 2);
                let want = if a == 0 {
                    0
                } else if a == 255 {
                    p
                } else {
                    let un = |v: u32| ((v * 255 + a / 2) / a).min(255);
                    (a << 24) | (un(c) << 16) | (un(255 - c) << 8) | un(c / 2)
                };
                assert_eq!(super::to_straight(p), want, "a {a} c {c}");
            }
        }
    }

    /// A copied rectangle has straight pixels and leaves its surroundings
    /// alone, at any stride
    #[test]
    fn copy_out_is_each_pixel() {
        let mut img = super::Image::new(7, 5);
        for (i, px) in img.px.iter_mut().enumerate() {
            let a = (i as u32 * 37) & 0xff;
            *px = (a << 24) | ((a / 2) << 16) | ((a / 3) << 8) | (a / 5);
        }
        let stride = 9;
        let mut out = vec![0xAAu8; stride * 5 * 4];
        img.copy_out(super::Rect::new(1, 1, 4, 3), &mut out, stride);
        for y in 0..5 {
            for x in 0..stride {
                let o = (y * stride + x) * 4;
                let got = u32::from_le_bytes([out[o], out[o + 1], out[o + 2], out[o + 3]]);
                let inside = (1..5).contains(&x) && (1..4).contains(&y);
                let want = if inside { super::to_straight(img.px[y * 7 + x]) } else { 0xAAAA_AAAA };
                assert_eq!(got, want, "({x}, {y})");
            }
        }
    }

    use super::*;

    #[test]
    fn translucent_over_nothing_and_back() {
        let mut img = Image::new(4, 4);
        Painter::new(&mut img, Rect::new(0, 0, 4, 4)).fill(0xBF211F24);
        // Straight alpha round-trips within rounding
        let s = to_straight(img.px[5]);
        assert_eq!(s >> 24, 0xBF);
        assert!(((s >> 16) & 0xff).abs_diff(0x21) <= 1);
    }

    #[test]
    fn the_clip_is_respected() {
        let mut img = Image::new(10, 10);
        let mut p = Painter::new(&mut img, Rect::new(0, 0, 10, 10));
        p.with_clip(Rect::new(0, 0, 5, 10), |p| p.fill(0xFFFFFFFF));
        assert_eq!(img.px[4], 0xFFFFFFFF);
        assert_eq!(img.px[5], 0);
    }
}
