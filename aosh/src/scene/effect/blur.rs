// FastBlur, GaussianBlur, and DropShadow, following Qt5Compat's passes
//
// FastBlur (FastBlur.qml, fastblur_internal.*, fastblur.frag):
// - Level 1 is the source over the shader's box (the item's, plus 64 each side
//   with a transparent border).
// - Levels 2 to 6: the box rounded up to 32s and halved five times, each
//   averaging four reads around a texel center, one step across and a third
//   along, in four rotations.
// - Result: up to five levels, mixed by weights following √(radius / 64) that
//   sum to 1.
//
// GaussianBlur and DropShadow (GaussianBlur.qml, DropShadowBase.qml,
// ShaderBuilder's shaders):
// - `samples / 2` taps each side, deviation (radius + 1) / 3.3333 unless set;
//   across into a texture, then down. With taps equal to the radius, pairs
//   share one read between them; otherwise taps are `radius / taps` texels
//   apart.
// - A transparent border widens the source by `radius`, and the first pass's
//   texture is that much taller.
// - DropShadow uses alpha only: first pass white, second the shadow color, each
//   divided by a spread-reduced thickness. The source goes back on top, and the
//   shadow moves by offsets rounded to whole pixels.

use super::{item_of, shader_colour, texels};
use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::gpu::{self, Tap, Texture};
use crate::scene::paint::Painter;
use crate::scene::xf::Xf;
use crate::scene::{hash, Scene};

/// Where on FastBlur's scale its six levels are
const LEVELS: [f64; 6] = [0.1, 0.3, 0.5, 0.7, 0.9, 1.1];

/// Up to five levels and weights for a FastBlur of `radius`
fn fast_weights(radius: f64) -> Vec<(usize, f32)> {
    let lod = (radius / 64.0).sqrt() * 1.2 - 0.2;
    let w = LEVELS.map(|c| {
        let d = (lod - c).abs();
        if d >= 0.5 { 0.0 } else { 1.0 - d * 2.0 }
    });
    let sum: f64 = w.iter().sum();
    w.iter().enumerate().filter(|(_, &v)| v > 0.0).take(5).map(|(i, &v)| (i, (v / sum) as f32)).collect()
}

/// Gaussian reads along one axis: tap offset and weight. `total` divides the
/// sum for one-by-one taps
struct Kernel {
    reads: Vec<(f32, f32)>,
    total: f32,
}

fn kernel(taps: i64, deviation: f64, paired: bool) -> Kernel {
    let g = |i: i64| (-((i * i) as f64) / (2.0 * deviation * deviation)).exp();
    let total: f64 = (-taps..=taps).map(g).sum();
    if !paired {
        let reads = (-taps..=taps).map(|i| (i as f32, g(i) as f32)).collect();
        return Kernel { reads, total: total as f32 };
    }
    // Center tap, then pairs of side taps read at their weighted balance point
    let mut reads = vec![(0.0f32, (g(0) / total) as f32)];
    let mut i = 1;
    while i <= taps {
        let (a, b) = (g(i), if i < taps { g(i + 1) } else { 0.0 });
        let at = (i as f64 * a + (i + 1) as f64 * b) / (a + b);
        let wt = ((a + b) / total) as f32;
        reads.push((at as f32, wt));
        reads.push((-at as f32, wt));
        i += 2;
    }
    Kernel { reads, total: 1.0 }
}

/// Copies a texture as a level
struct Copy<'a>(Texture<'a>);

impl gpu::Shader for Copy<'_> {
    type Col = Tap;
    type Row = Tap;
    fn col(&self, u: f32) -> Tap {
        self.0.across(u)
    }
    fn row(&self, v: f32) -> Tap {
        self.0.down(v)
    }
    fn at(&self, c: &Tap, r: &Tap) -> [f32; 4] {
        self.0.read(c, r)
    }
}

/// Level from the previous: four reads around a texel center, one step (`xs`,
/// `ys`) across and a third along, in four rotations
struct Down<'a> {
    t: Texture<'a>,
    xs: f32,
    ys: f32,
}

impl gpu::Shader for Down<'_> {
    type Col = [Tap; 4];
    type Row = [Tap; 4];
    fn col(&self, u: f32) -> [Tap; 4] {
        [u + self.xs, u + self.xs * 0.36, u - self.xs * 0.36, u - self.xs].map(|u| self.t.across(u))
    }
    fn row(&self, v: f32) -> [Tap; 4] {
        [v + self.ys * 0.36, v - self.ys, v + self.ys, v - self.ys * 0.36].map(|v| self.t.down(v))
    }
    fn at(&self, c: &[Tap; 4], r: &[Tap; 4]) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for k in 0..4 {
            let s = self.t.read(&c[k], &r[k]);
            for i in 0..4 {
                out[i] += s[i];
            }
        }
        out.map(|x| x * 0.25)
    }
}

/// The levels mixed, each by its weight
struct Mix<'a>(Vec<(Texture<'a>, f32)>);

impl gpu::record::Gpu for Mix<'_> {
    const KIND: u32 = gpu::record::SH_MIX;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        if !gpu::record::levels_ok(self.0.len()) {
            return None;
        }
        let mut p = vec![self.0.len() as u32];
        for (t, wt) in &self.0 {
            p.push(rec.texture(t)?);
            p.push(wt.to_bits());
        }
        Some(p)
    }
    fn put_col(&self, c: &Vec<Tap>, out: &mut Vec<u32>) {
        c.iter().for_each(|t| gpu::record::tap_words(t, out));
    }
    fn put_row(&self, r: &Vec<Tap>, out: &mut Vec<u32>) {
        r.iter().for_each(|t| gpu::record::tap_words(t, out));
    }
}

impl gpu::Shader for Mix<'_> {
    type Col = Vec<Tap>;
    type Row = Vec<Tap>;
    fn col(&self, u: f32) -> Vec<Tap> {
        self.0.iter().map(|(t, _)| t.across(u)).collect()
    }
    fn row(&self, v: f32) -> Vec<Tap> {
        self.0.iter().map(|(t, _)| t.down(v)).collect()
    }
    fn at(&self, c: &Vec<Tap>, r: &Vec<Tap>) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for (k, (t, wt)) in self.0.iter().enumerate() {
            let s = t.read(&c[k], &r[k]);
            for i in 0..4 {
                out[i] += s[i] * wt;
            }
        }
        out
    }
}

/// One Gaussian pass: `kernel` reads `step` apart along x (`across`) or y. A
/// shadow reads alpha at `colour` strength, over `thickness`
struct Gauss<'a> {
    t: Texture<'a>,
    k: &'a Kernel,
    step: f32,
    across: bool,
    shadow: Option<([f32; 4], f32)>,
}

impl gpu::record::Gpu for Gauss<'_> {
    const KIND: u32 = gpu::record::SH_GAUSS;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        let n = self.k.reads.len();
        if !gpu::record::reads_ok(n) {
            return None;
        }
        // Tap count per column and row: every read along the pass, one across
        let (lc, lr) = if self.across { (n, 1) } else { (1, n) };
        let (colour, thickness) = self.shadow.unwrap_or(([0.0; 4], 0.0));
        let mut p = vec![rec.texture(&self.t)?, n as u32, lc as u32, lr as u32, self.k.total.to_bits(),
                         self.shadow.is_some() as u32];
        p.extend(colour.iter().map(|c| c.to_bits()));
        p.push(thickness.to_bits());
        p.extend(self.k.reads.iter().map(|&(_, wt)| wt.to_bits()));
        // Then what a turned quad's taps are worked out from
        p.extend_from_slice(&[self.step.to_bits(), self.across as u32]);
        p.extend(self.k.reads.iter().map(|&(at, _)| at.to_bits()));
        Some(p)
    }
    fn put_col(&self, c: &Vec<Tap>, out: &mut Vec<u32>) {
        c.iter().for_each(|t| gpu::record::tap_words(t, out));
    }
    fn put_row(&self, r: &Vec<Tap>, out: &mut Vec<u32>) {
        r.iter().for_each(|t| gpu::record::tap_words(t, out));
    }
}

impl gpu::Shader for Gauss<'_> {
    type Col = Vec<Tap>;
    type Row = Vec<Tap>;
    fn col(&self, u: f32) -> Vec<Tap> {
        if self.across {
            self.k.reads.iter().map(|&(at, _)| self.t.across(u + self.step * at)).collect()
        } else {
            vec![self.t.across(u)]
        }
    }
    fn row(&self, v: f32) -> Vec<Tap> {
        if self.across {
            vec![self.t.down(v)]
        } else {
            self.k.reads.iter().map(|&(at, _)| self.t.down(v + self.step * at)).collect()
        }
    }
    fn at(&self, c: &Vec<Tap>, r: &Vec<Tap>) -> [f32; 4] {
        let reads = self.k.reads.iter().enumerate().map(|(i, &(_, wt))| (&c[i.min(c.len() - 1)], &r[i.min(r.len() - 1)], wt));
        match self.shadow {
            Some((colour, thickness)) => {
                let sum = reads.map(|(c, r, wt)| self.t.read_alpha(c, r) * wt).sum::<f32>() / self.k.total;
                let a = if thickness <= 0.0 { (sum > 0.0) as u8 as f32 } else { (sum / thickness).clamp(0.0, 1.0) };
                colour.map(|c| c * a)
            }
            None => {
                let mut out = [0.0f32; 4];
                for (c, r, wt) in reads {
                    let s = self.t.read(c, r);
                    for i in 0..4 {
                        out[i] += s[i] * wt;
                    }
                }
                out.map(|x| x / self.k.total)
            }
        }
    }
}

impl Scene {
    pub(super) fn fast_blur(&mut self, core: &Core, oid: Oid, xf: &Xf, (w, h): (f64, f64), opacity: f32, p: &mut Painter<'_>) {
        let radius = core.num(oid, "radius").max(0.0);
        let border = core.flag(oid, "transparentBorder");
        let Some(src) = self.source(core, item_of(core, oid, "source"), None) else { return };
        // The shader's box, in the item
        let (bx, by, bw, bh) = if border { (-64.0, -64.0, w + 128.0, h + 128.0) } else { (0.0, 0.0, w, h) };
        let print = hash(&(self.effect_print(core, oid, None), w.to_bits(), h.to_bits()));
        let levels = self.passes(oid, print, |_| {
            let src = src.gpu();
            // Level 1: the source over the item's box, in the shader's box
            let (tw, th) = texels(bw, bh);
            let (kx, ky) = (tw as f64 / bw, th as f64 / bh);
            let one = gpu::pass(tw, th, ((0.0 - bx) * kx, (0.0 - by) * ky, w * kx, h * ky), &Copy(src));
            let mut levels = vec![one];
            let (mut lw, mut lh) = (((bw / 32.0).ceil() * 32.0) as i32, ((bh / 32.0).ceil() * 32.0) as i32);
            for _ in 0..5 {
                lw /= 2;
                lh /= 2;
                let before = levels.last().unwrap();
                let t = before.texture();
                let down = Down { t, xs: 1.0 / lw as f32, ys: 1.0 / lh as f32 };
                let next = gpu::pass(lw, lh, (0.0, 0.0, lw as f64, lh as f64), &down);
                levels.push(next);
            }
            levels
        });
        let weights = fast_weights(radius);
        let mix = Mix(weights.iter()
            .map(|&(i, wt)| (levels[i].texture(), wt))
            .collect());
        p.shaded(xf, (bx, by, bw, bh), opacity, &mix);
    }

    /// A GaussianBlur, or a DropShadow's blur, its shadow and its source
    pub(super) fn gaussian(&mut self, core: &Core, oid: Oid, xf: &Xf, (w, h): (f64, f64), opacity: f32, p: &mut Painter<'_>) {
        let shadow = Self::kind_name(core, oid) == "DropShadow";
        let radius = core.num(oid, "radius").max(0.0);
        let samples = core.num(oid, "samples").max(0.0) as i64;
        let taps = samples / 2;
        let deviation = if !shadow && core.is_explicit(oid, "deviation") { core.num(oid, "deviation") } else { (radius + 1.0) / 3.3333 };
        let border = core.flag(oid, "transparentBorder");
        let src_item = item_of(core, oid, "source");
        // Blur texture size, truncated as Qt's `int` properties do
        let (pw, ph) = if border { ((w + 2.0 * radius) as i64 as f64, (h + 2.0 * radius) as i64 as f64) } else { (w as i64 as f64, h as i64 as f64) };
        let (bw, bh) = if border { (pw, ph) } else { (w, h) };
        let spread_taps = if taps > 0 { radius / taps as f64 } else { 0.0 };
        let k = kernel(taps, deviation.max(0.00001), radius == taps as f64);
        // Thinner as the shadow spreads, for the alpha it divides by
        let thickness = (1.0 - core.num(oid, "spread") * 0.98).clamp(0.0, 0.98) as f32;
        let colour = shader_colour(crate::scene::colour(&core.text(oid, "color")));
        let rect = border.then_some((-radius, 0.0, pw, h));
        let Some(src) = self.source(core, src_item, rect) else { return };
        let (offx, offy) = if shadow { (core.num(oid, "horizontalOffset").round(), core.num(oid, "verticalOffset").round()) } else { (0.0, 0.0) };
        let print = hash(&(self.effect_print(core, oid, None), w.to_bits(), h.to_bits()));
        let across = self.passes(oid, print, |_| {
            let t = src.gpu();
            // Across, into a texture as wide as the source and, with a
            // transparent border, `radius` taller each way
            let (tw, th) = texels(bw, bh);
            let quad = if border { (0.0, radius * th as f64 / bh, tw as f64, h * th as f64 / bh) } else { (0.0, 0.0, tw as f64, th as f64) };
            let pass = Gauss { t, k: &k, step: (spread_taps / pw) as f32, across: true, shadow: shadow.then_some(([1.0; 4], thickness)) };
            vec![gpu::pass(tw, th, quad, &pass)]
        });
        let first = across[0].texture();
        let (qx, qy) = if border { (offx - radius, offy - radius) } else { (offx, offy) };
        let pass = Gauss { t: first, k: &k, step: (spread_taps / ph) as f32, across: false, shadow: shadow.then_some((colour, thickness)) };
        p.shaded(xf, (qx, qy, bw, bh), opacity, &pass);
        // A shadow has its source on top, where the source is
        if shadow {
            let t = src.gpu();
            let (sx, sw) = if border { (-radius, pw) } else { (0.0, w) };
            p.quad(xf, (sx, 0.0, sw, h), &t, (0.0, 0.0, 1.0, 1.0), opacity, gpu::Sampling::Linear);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ShaderBuilder's values for radius 8, 17 samples
    #[test]
    fn paired_taps_are_qts() {
        let k = kernel(8, 9.0 / 3.3333, true);
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(k.reads[0].1, 0.147984));
        assert!(near(k.reads[1].0, 1.44874) && near(k.reads[1].1, 0.250652));
        assert!(near(k.reads[7].0, 7.26332) && near(k.reads[7].1, 0.00697269));
    }

    /// ShaderBuilder's values for radius 4, 24 samples, every tap read
    #[test]
    fn single_taps_are_qts() {
        let k = kernel(12, 5.0 / 3.3333, false);
        assert!((k.total - 3.75998).abs() < 1e-4);
        assert!((k.reads[13].1 - 0.800741).abs() < 1e-5);
    }

    #[test]
    fn a_fast_blur_of_32_is_four_levels_and_some_of_a_fifth() {
        let w = fast_weights(32.0);
        assert_eq!(w.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 2, 3, 4, 5]);
        assert!((w.iter().map(|x| x.1).sum::<f32>() - 1.0).abs() < 1e-3);
    }
}
