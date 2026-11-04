// RectangularShadow, following QtQuick.Effects (rectangularshadow.frag,
// QQuickRectangularShadow's sizes)
//
// One quad: the item grown by `blur` and `spread` and moved by `offset`. Each
// pixel's distance from a rounded rectangle (the quad shrunk by 1.8 blurs, plus
// a pixel when antialiased) maps to a ramp 2.1 blurs long, squared, times the
// color. The corner radius grows with spread, shrinks with blur, and is limited
// by the rectangle

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::gpu;
use crate::scene::paint::Painter;
use crate::scene::xf::Xf;

/// Corner radius: `radius` plus spread (less for a radius under it), minus 3/4
/// of blur, at most half the shorter side and twice the spread
fn corner(radius: f64, spread: f64, blur: f64, (w, h): (f64, f64)) -> f64 {
    let most = w.min(h) * 0.5 + spread * 2.0;
    let grown = if radius < spread && spread != 0.0 {
        let r = radius / spread - 1.0;
        radius + spread * (1.0 + r * r * r)
    } else {
        radius + spread
    };
    (grown - blur * 0.75).max(0.0).min(most)
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn rectangular(core: &Core, oid: Oid, xf: &Xf, (w, h): (f64, f64), opacity: f32, p: &mut Painter<'_>) {
    let blur = core.num(oid, "blur").max(0.0);
    let spread = core.num(oid, "spread");
    let radius = core.num(oid, "radius").max(0.0);
    // Antialiased by default; a square one's edge is a pixel longer
    let aa = !core.is_explicit(oid, "antialiasing") || core.flag(oid, "antialiasing");
    let aa = if aa { 1.0 } else { 0.0 };
    let pad = blur * 2.0 + spread * 2.0;
    let (ew, eh) = (w + pad, h + pad);
    let (ex, ey) = ((w - ew) * 0.5 + core.num(oid, "offset.x"), (h - eh) * 0.5 + core.num(oid, "offset.y"));
    let shrink = blur * 1.8 + aa;
    let half = ((ew * 0.5 - shrink) as f32, (eh * 0.5 - shrink) as f32);
    let r = corner(radius, spread, blur, (w, h)) as f32;
    let ramp = (blur * 2.1 + aa) as f32;
    let colour = super::shader_colour(crate::scene::colour(&core.text(oid, "color")));
    p.shaded(xf, (ex, ey, ew, eh), opacity, &Box_ { size: (ew as f32, eh as f32), half, r, ramp, colour });
}

/// How far a pixel is in the rounded rectangle, made a ramp, as a colour
struct Box_ {
    size: (f32, f32),
    half: (f32, f32),
    r: f32,
    ramp: f32,
    colour: [f32; 4],
}

impl gpu::record::Gpu for Box_ {
    const KIND: u32 = gpu::record::SH_BOX;
    fn params(&self, _: &mut gpu::record::Record) -> Option<Vec<u32>> {
        let c = self.colour;
        // Then what a column and a row are worked out from, turned
        Some(vec![self.r.to_bits(), self.ramp.to_bits(), c[0].to_bits(), c[1].to_bits(), c[2].to_bits(), c[3].to_bits(),
                  self.size.0.to_bits(), self.size.1.to_bits(), self.half.0.to_bits(), self.half.1.to_bits()])
    }
    fn put_col(&self, c: &f32, out: &mut Vec<u32>) {
        out.push(c.to_bits());
    }
    fn put_row(&self, r: &f32, out: &mut Vec<u32>) {
        out.push(r.to_bits());
    }
}

impl gpu::Shader for Box_ {
    /// How far past the rectangle's straight edge, along each axis
    type Col = f32;
    type Row = f32;
    fn col(&self, u: f32) -> f32 {
        ((u * self.size.0 - self.size.0 * 0.5).abs() - self.half.0 + self.r).max(0.0)
    }
    fn row(&self, v: f32) -> f32 {
        ((v * self.size.1 - self.size.1 * 0.5).abs() - self.half.1 + self.r).max(0.0)
    }
    fn at(&self, qx: &f32, qy: &f32) -> [f32; 4] {
        let d = (qx * qx + qy * qy).sqrt() - self.r;
        let a = 1.0 - smoothstep(0.0, self.ramp, d);
        self.colour.map(|c| c * a * a)
    }
}
