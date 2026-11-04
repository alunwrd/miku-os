// RadialGradient, following Qt5Compat (RadialGradient.qml and its shaders)
//
// The gradient is a 16 by 256 texture of a Rectangle filled top to bottom. The
// item is one quad reading it, filtered, at `2 × distance` down: distance from
// the center in per-axis radii, halved, after rotating by `angle`. The center
// is the item's middle plus the offsets; a radius of 0 or less is 1/16384 of
// the item. With a `source`, strength follows its alpha

use rquickjs::Ctx;

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::paint::{Image, Painter};
use crate::scene::rectangle;
use crate::scene::xf::Xf;
use crate::scene::{gpu, hash, Scene};

/// Unset radii default to the size, as Qt5Compat binds them
pub fn radii(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    for (radius, side) in [("horizontalRadius", "width"), ("verticalRadius", "height")] {
        if !core.is_explicit(oid, radius) {
            core.store_layout(ctx, oid, radius, core.num(oid, side));
        }
    }
}

/// Gradient texture: 16 by 256, top to bottom
fn ramp(stops: &[(f32, u32)]) -> Image {
    let mut img = Image::new(16, 256);
    let spec = rectangle::Spec { w: 16.0, h: 256.0, color: 0xffff_ffff, stops: stops.to_vec(), vertical: true, ..Default::default() };
    if let Some(g) = rectangle::geometry(&spec) {
        let all = img.bounds();
        Painter::new(&mut img, all).geometry(&g, &Xf::IDENTITY, 1.0);
    }
    img
}

impl Scene {
    pub(super) fn radial_gradient(&mut self, core: &Core, oid: Oid, xf: &Xf, (w, h): (f64, f64), opacity: f32,
                                  p: &mut Painter<'_>) {
        if w <= 0.0 || h <= 0.0 {
            return;
        }
        let stops = crate::scene::gradient_of(core, oid).map(|(s, _)| s)
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| vec![(0.0, 0xffff_ffff), (1.0, 0xff00_0000)]);
        let print = hash(&stops.iter().map(|(p, c)| (p.to_bits(), *c)).collect::<Vec<_>>());
        let made = self.passes(oid, print, |_| vec![ramp(&stops)]);
        let ramp = &made[0];
        let mask = self.source(core, super::item_of(core, oid, "source"), None);
        let ratio = |r: f64, side: f64| if r > 0.0 { side / (2.0 * r) } else { side * 16384.0 };
        let angle = -core.num(oid, "angle").to_radians();
        let ramp = ramp.texture();
        let shader = Radial {
            column: ramp.across(0.0),
            ramp,
            mask: mask.as_ref().map(|m| m.gpu()),
            centre: ((0.5 + core.num(oid, "horizontalOffset") / w) as f32, (0.5 + core.num(oid, "verticalOffset") / h) as f32),
            ratio: (ratio(core.num(oid, "horizontalRadius"), w) as f32, ratio(core.num(oid, "verticalRadius"), h) as f32),
            turn: (angle.sin() as f32, angle.cos() as f32),
        };
        p.shaded(xf, (0.0, 0.0, w, h), opacity, &shader);
    }
}

/// The gradient at a pixel's distance from the centre
struct Radial<'a> {
    ramp: gpu::Texture<'a>,
    /// Column read in the ramp: its left edge
    column: gpu::Tap,
    mask: Option<gpu::Texture<'a>>,
    centre: (f32, f32),
    ratio: (f32, f32),
    /// The turn's sine and cosine
    turn: (f32, f32),
}

impl gpu::record::Gpu for Radial<'_> {
    const KIND: u32 = gpu::record::SH_RADIAL;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        let mut p = vec![rec.texture(&self.ramp)?];
        gpu::record::tap_words(&self.column, &mut p);
        p.push(match &self.mask {
            Some(m) => rec.texture(m)?,
            None => gpu::record::NO_TEXTURE,
        });
        p.extend([self.ratio.0, self.ratio.1, self.turn.0, self.turn.1, self.centre.0, self.centre.1].map(f32::to_bits));
        Some(p)
    }
    fn put_col(&self, c: &Self::Col, out: &mut Vec<u32>) {
        out.push(c.0.to_bits());
        if let Some(t) = &c.1 {
            gpu::record::tap_words(t, out);
        }
    }
    fn put_row(&self, r: &Self::Row, out: &mut Vec<u32>) {
        out.push(r.0.to_bits());
        if let Some(t) = &r.1 {
            gpu::record::tap_words(t, out);
        }
    }
}

impl gpu::Shader for Radial<'_> {
    type Col = (f32, Option<gpu::Tap>);
    type Row = (f32, Option<gpu::Tap>);
    fn col(&self, u: f32) -> Self::Col {
        (u - self.centre.0, self.mask.as_ref().map(|m| m.across(u)))
    }
    fn row(&self, v: f32) -> Self::Row {
        (v - self.centre.1, self.mask.as_ref().map(|m| m.down(v)))
    }
    fn at(&self, c: &Self::Col, r: &Self::Row) -> [f32; 4] {
        let (dx, dy) = (c.0, r.0);
        let (s, co) = self.turn;
        let x = (dx * co - dy * s) * self.ratio.0;
        let y = (dx * s + dy * co) * self.ratio.1;
        let colour = self.ramp.read(&self.column, &self.ramp.down(2.0 * (x * x + y * y).sqrt()));
        match (&self.mask, &c.1, &r.1) {
            (Some(m), Some(mc), Some(mr)) => {
                let a = m.read_alpha(mc, mr);
                colour.map(|k| k * a)
            }
            _ => colour,
        }
    }
}
