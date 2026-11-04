// MultiEffect color and mask, following QtQuick.Effects
// (`multieffect_c*.frag.qsb`, `multieffects_are_qts`)
//
// One quad on the effect's box with no prior texture. The source is filtered; a
// layer mask is nearest unless `layer.smooth`. Each pixel, premultiplied, in
// order:
// - contrast: away from half its alpha, `1 + contrast` times;
// - brightness: adds `brightness` times its alpha;
// - grey: 0.299 red, 0.587 green, 0.114 blue;
// - colorization: grey in `colorizationColor`, mixed by `colorization` times
//   the color's alpha;
// - saturation: from grey, `1 + saturation` times as far;
// - mask, by the mask's alpha between two thresholds softened by their spread:
//   smoothstep from `min·(1 + spread) - spread` to `min·(1 + spread)`, and
//   likewise at the top. With no spread, an alpha on the lower threshold is
//   out, on the upper one in.
//
// Channels clamp to 0..1, then blend over the target. Blur and shadow are not
// implemented and are logged once

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::gpu::{self, Tap};
use crate::scene::paint::Painter;
use crate::scene::xf::Xf;
use crate::scene::{hash, Scene};

/// What the shader is given
struct Multi<'a> {
    src: gpu::Texture<'a>,
    mask: Option<gpu::Texture<'a>>,
    /// Whether the mask is filtered (`Scene::smooth`); the source always is
    smooth_mask: bool,
    contrast: f32,
    brightness: f32,
    saturation: f32,
    /// The colour, straight, and how much of it
    tint: [f32; 4],
    /// Mask edges: in from `[0]` to `[1]`, out from `[2]` to `[3]`
    edges: [f32; 4],
    inverted: bool,
}

/// Print of the effect's non-source inputs
pub(super) fn print(core: &Core, oid: Oid) -> u64 {
    let nums: Vec<u64> = ["brightness", "contrast", "saturation", "colorization", "maskThresholdMin", "maskSpreadAtMin",
                          "maskThresholdMax", "maskSpreadAtMax"]
        .iter().map(|n| core.num(oid, n).to_bits()).collect();
    hash(&(nums, core.text(oid, "colorizationColor"), core.flag(oid, "maskEnabled"), core.flag(oid, "maskInverted")))
}

impl Scene {
    pub(super) fn multi_effect(&mut self, core: &Core, oid: Oid, xf: &Xf, (w, h): (f64, f64), opacity: f32,
                               p: &mut Painter<'_>) {
        if (core.flag(oid, "blurEnabled") && core.num(oid, "blur") > 0.0) || core.flag(oid, "shadowEnabled") {
            core.log_once(oid, "MultiEffect's blur and shadow are not drawn yet - its colour and mask are");
        }
        let (s, m) = (super::item_of(core, oid, "source"), super::item_of(core, oid, "maskSource"));
        let Some(src) = self.source(core, s, None) else { return };
        let masked = core.flag(oid, "maskEnabled");
        let mask = if masked { self.source(core, m, None) } else { None };
        // A missing requested mask masks everything out, like an empty texture
        if masked && mask.is_none() {
            return;
        }
        let num = |n: &str| core.num(oid, n) as f32;
        let colour = crate::scene::colour(&core.text(oid, "colorizationColor"));
        let ch = |s: u32| ((colour >> s) & 0xff) as f32 / 255.0;
        let tint = [ch(16), ch(8), ch(0), num("colorization") * ch(24)];
        let edge = |thr: f32, spread: f32| (thr * (1.0 + spread) - spread, thr * (1.0 + spread));
        let (x, y) = edge(num("maskThresholdMin"), num("maskSpreadAtMin"));
        let (z, ww) = edge(1.0 - num("maskThresholdMax"), num("maskSpreadAtMax"));
        let shader = Multi {
            src: src.gpu(),
            mask: mask.as_ref().map(|m| m.gpu()),
            smooth_mask: Self::smooth(core, m),
            contrast: num("contrast"),
            brightness: num("brightness"),
            saturation: num("saturation"),
            tint,
            edges: [x, y, z, ww],
            inverted: core.flag(oid, "maskInverted"),
        };
        p.shaded(xf, (0.0, 0.0, w, h), opacity, &shader);
    }
}

/// Smoothstep from `e0` to `e1`; at a step, `on` decides `x` on the edge
fn edge(e0: f32, e1: f32, x: f32, on: bool) -> f32 {
    if e1 <= e0 {
        return if x > e0 || (on && x == e0) { 1.0 } else { 0.0 };
    }
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl gpu::record::Gpu for Multi<'_> {
    const KIND: u32 = gpu::record::SH_MULTI;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        let mut p = vec![rec.texture(&self.src)?, match &self.mask {
            Some(m) => rec.texture(m)?,
            None => gpu::record::NO_TEXTURE,
        }];
        p.extend([self.contrast, self.brightness, self.saturation].map(f32::to_bits));
        p.extend(self.tint.map(f32::to_bits));
        p.extend(self.edges.map(f32::to_bits));
        p.push(self.inverted as u32);
        p.push(self.smooth_mask as u32);
        Some(p)
    }
    fn put_col(&self, c: &Self::Col, out: &mut Vec<u32>) {
        gpu::record::tap_words(&c.0, out);
        if let Some(t) = &c.1 {
            gpu::record::tap_words(t, out);
        }
    }
    fn put_row(&self, r: &Self::Row, out: &mut Vec<u32>) {
        gpu::record::tap_words(&r.0, out);
        if let Some(t) = &r.1 {
            gpu::record::tap_words(t, out);
        }
    }
}

impl gpu::Shader for Multi<'_> {
    type Col = (Tap, Option<Tap>);
    type Row = (Tap, Option<Tap>);
    fn col(&self, u: f32) -> Self::Col {
        (self.src.across(u), self.mask.as_ref().map(|m| m.across_by(u, self.smooth_mask)))
    }
    fn row(&self, v: f32) -> Self::Row {
        (self.src.down(v), self.mask.as_ref().map(|m| m.down_by(v, self.smooth_mask)))
    }
    fn at(&self, c: &Self::Col, r: &Self::Row) -> [f32; 4] {
        let [r0, g0, b0, a] = self.src.read(&c.0, &r.0);
        let mut rgb = [r0, g0, b0];
        for v in &mut rgb {
            *v = (*v - 0.5 * a) * (1.0 + self.contrast) + 0.5 * a + self.brightness * a;
        }
        let grey = 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2];
        let k = self.tint[3];
        for (i, v) in rgb.iter_mut().enumerate() {
            let tinted = self.tint[i] * grey * k + *v * (1.0 - k);
            *v = grey + (tinted - grey) * (1.0 + self.saturation);
        }
        let m = match (&self.mask, &c.1, &r.1) {
            (Some(mask), Some(mc), Some(mr)) => {
                let alpha = mask.read_alpha(mc, mr);
                let [x, y, z, w] = self.edges;
                let m = edge(x, y, alpha, false) * edge(z, w, 1.0 - alpha, true);
                if self.inverted { 1.0 - m } else { m }
            }
            _ => 1.0,
        };
        let keep = |v: f32| (v * m).clamp(0.0, 1.0);
        [keep(rgb[0]), keep(rgb[1]), keep(rgb[2]), keep(a)]
    }
}
