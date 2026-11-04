// Layers, masks, blurs, shadows, gradients, and MultiEffect color
//
// As in Qt, these run as shader passes over textures, here on the CPU pass for
// pass (`effects_are_qts`):
// - A texture of an item (`render`) draws it and its subtree with the scene's
//   painter, visible or not, without its own position, rotation, opacity, or
//   layer.
// - An effect's source (`source`) is that texture, a ShaderEffectSource's, or
//   an Image's decoded picture.
// - A pass into a texture is `gpu::pass`: the shader at every texel center,
//   reads filtered in 256ths, written in 8 bits. The last pass is
//   `gpu::shaded`, through the effect's matrix.
// - `layer.effect` is created when the layer is on (`layer_effects`): an item
//   under the layered item's parent, in no children list, sized like it, with
//   it as `source`. It draws instead of the item. A layer with no effect draws
//   its texture.
//
// Textures and all but the last pass are cached by a print of their inputs.
//
// Pass arithmetic comes from Qt's shaders (`qsb -d` on qt6-5compat and
// QtQuick.Effects) and QML, with no shader code carried over.

mod blur;
mod gradient;
mod multi;
mod shadow;

pub use gradient::radii;

use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::Ctx;

use super::paint::{Image, Painter, Rect};
use super::picture::Picture;
use super::xf::Xf;
use super::{gpu, hash, Draw, Scene, Shown};
use crate::object::build::{Builder, Scope};
use crate::object::value::{Oid, Value};
use crate::object::Core;

#[derive(Default)]
pub struct Effects {
    /// Each layered item's `layer.effect`
    pub layered: HashMap<Oid, Oid>,
    /// Item textures by item, part, and size
    textures: HashMap<(Oid, [u64; 6]), (u64, Rc<Image>)>,
    /// An effect's passes before the last, by effect
    passes: HashMap<Oid, (u64, Rc<Vec<Image>>)>,
    /// Items being rendered into textures. A cycle draws nothing, as in
    /// ShaderEffectSource without `recursive`
    making: Vec<Oid>,
}

impl Effects {
    /// Drops cached data of destroyed objects
    pub fn prune(&mut self, core: &Core) {
        let live = |o: &Oid| core.0.borrow().objs.is_live(*o);
        self.textures.retain(|(o, _), _| live(o));
        self.passes.retain(|o, _| live(o));
        self.layered.retain(|o, fx| live(o) && live(fx));
    }
}

/// Effect input: a texture made here, or a picture
#[derive(Clone)]
enum Tex {
    Made(Rc<Image>),
    Picture(Rc<Picture>),
}

impl Tex {
    fn gpu(&self) -> gpu::Texture<'_> {
        match self {
            Tex::Made(i) => i.texture(),
            Tex::Picture(p) => p.texture(),
        }
    }
}

/// How far an effect draws past its box, in its own units, for damage
pub fn reach(core: &Core, oid: Oid) -> f64 {
    let kind = Scene::kind_name(core, oid);
    match kind {
        "FastBlur" if core.flag(oid, "transparentBorder") => 64.0,
        "GaussianBlur" | "DropShadow" if core.flag(oid, "transparentBorder") => {
            let offset = if kind == "DropShadow" {
                core.num(oid, "horizontalOffset").abs().max(core.num(oid, "verticalOffset").abs()).round()
            } else {
                0.0
            };
            core.num(oid, "radius").max(0.0).ceil() + offset + 1.0
        }
        "RectangularShadow" => {
            let (blur, spread) = (core.num(oid, "blur").max(0.0), core.num(oid, "spread"));
            blur + spread.abs() + core.num(oid, "offset.x").abs().max(core.num(oid, "offset.y").abs()) + 1.0
        }
        _ => 0.0,
    }
}

/// The item a property of an effect names, if it is one that is there
fn item_of(core: &Core, oid: Oid, prop: &str) -> Option<Oid> {
    core.peek_named(oid, prop).and_then(|v| v.as_obj())
        .filter(|&o| core.0.borrow().objs.get(o).is_some_and(|x| crate::types::is_a(x.def, "Item")))
}

/// Texture size of a part of an item; ShaderEffectSource rounds up
fn texels(w: f64, h: f64) -> (i32, i32) {
    (w.abs().ceil().max(1.0) as i32, h.abs().ceil().max(1.0) as i32)
}

fn bits(r: (f64, f64, f64, f64), size: (i32, i32)) -> [u64; 6] {
    [r.0.to_bits(), r.1.to_bits(), r.2.to_bits(), r.3.to_bits(), size.0 as u64, size.1 as u64]
}

impl Scene {
    /// Creates each layer's effect on first use, sized like the layered item;
    /// removes it when the layer goes off
    pub(super) fn layer_effects(&mut self, ctx: &Ctx<'_>, core: &Core, win: Oid) {
        let mut items = Vec::new();
        let mut stack = vec![win];
        while let Some(o) = stack.pop() {
            let kids = core.0.borrow().objs.get(o).map(|x| x.children.clone()).unwrap_or_default();
            // Unchanged subtrees keep their effects
            stack.extend(kids.iter().copied().filter(|&c| Self::is_item(core, c) && self.due(core, c)));
            if core.flag(o, "layer.enabled") {
                items.push(o);
            }
        }
        for item in items {
            let comp = core.peek_named(item, "layer.effect").and_then(|v| v.as_obj())
                .filter(|c| core.0.borrow().comps.contains_key(c));
            let Some(comp) = comp else { continue };
            let fx = match self.effects.layered.get(&item) {
                Some(&fx) if core.0.borrow().objs.is_live(fx) => fx,
                _ => {
                    let parent = core.0.borrow().objs[item].parent;
                    match Builder::new(core, ctx).instantiate(comp, parent, item, Scope::default()) {
                        Ok(fx) => {
                            let sampler = core.text(item, "layer.samplerName");
                            let sampler = if sampler.is_empty() { "source".to_string() } else { sampler };
                            if let Some(i) = core.prop_idx(fx, &sampler) {
                                core.store(ctx, fx, i, Value::Obj(item));
                            }
                            self.effects.layered.insert(item, fx);
                            // Drawing through the effect is not a property
                            // change, so the item is marked
                            core.touch(item);
                            fx
                        }
                        Err(e) => {
                            core.log(e);
                            continue;
                        }
                    }
                }
            };
            core.store_layout(ctx, fx, "width", core.num(item, "width"));
            core.store_layout(ctx, fx, "height", core.num(item, "height"));
        }
        let off: Vec<(Oid, Oid)> = self.effects.layered.iter().map(|(&i, &f)| (i, f))
            .filter(|&(i, _)| !core.0.borrow().objs.is_live(i) || !core.flag(i, "layer.enabled")).collect();
        for (item, fx) in off {
            self.effects.layered.remove(&item);
            if core.0.borrow().objs.is_live(fx) {
                core.destroy(ctx, fx);
            }
            core.touch(item);
        }
    }

    /// Print of an effect's inputs: its properties and the prints of items it
    /// reads. `layer` is the layered item, already in the layer's print
    pub(super) fn effect_print(&mut self, core: &Core, oid: Oid, layer: Option<Oid>) -> u64 {
        let kind = Self::kind_name(core, oid);
        let nums: Vec<u64> = ["radius", "samples", "deviation", "spread", "horizontalOffset", "verticalOffset", "blur",
                              "offset.x", "offset.y", "sourceRect.x", "sourceRect.y", "sourceRect.width",
                              "sourceRect.height", "textureSize.width", "textureSize.height", "width", "height",
                              "horizontalRadius", "verticalRadius", "angle"]
            .iter().map(|n| core.num(oid, n).to_bits()).collect();
        let flags = (core.flag(oid, "invert"), core.flag(oid, "transparentBorder"), core.flag(oid, "antialiasing"));
        let mut inputs = Vec::new();
        for prop in ["source", "maskSource", "sourceItem"] {
            if let Some(src) = item_of(core, oid, prop).filter(|&s| Some(s) != layer) {
                inputs.push(self.source_print(core, src));
            }
        }
        let stops = super::gradient_of(core, oid).map(|(s, v)| (s.iter().map(|(p, c)| (p.to_bits(), *c)).collect::<Vec<_>>(), v));
        let multi = (kind == "MultiEffect").then(|| multi::print(core, oid));
        hash(&(kind, nums, flags, core.text(oid, "color"), stops, inputs, multi))
    }

    /// Whether a texture from `src` is filtered when an effect takes it as
    /// provided (MultiEffect's mask): a layer per `layer.smooth`, an Image or
    /// ShaderEffectSource per `smooth`, any other item yes. Qt5Compat effects
    /// and MultiEffect's source always filter
    fn smooth(core: &Core, src: Option<Oid>) -> bool {
        let Some(src) = src else { return true };
        if core.flag(src, "layer.enabled") {
            return core.flag(src, "layer.smooth");
        }
        match Self::kind_name(core, src) {
            "Image" | "ShaderEffectSource" => core.flag(src, "smooth"),
            _ => true,
        }
    }

    /// What a texture of `src` would be made from
    fn source_print(&mut self, core: &Core, src: Oid) -> u64 {
        if self.effects.making.contains(&src) {
            return 0;
        }
        self.effects.making.push(src);
        let print = self.source_print_of(core, src);
        self.effects.making.retain(|&o| o != src);
        print
    }

    fn source_print_of(&mut self, core: &Core, src: Oid) -> u64 {
        match Self::kind_name(core, src) {
            "Image" => hash(&(core.text(src, "source"), core.num(src, "sourceSize.width").to_bits(),
                              core.num(src, "sourceSize.height").to_bits(), core.num(src, "fillMode") as i64)),
            _ => {
                let (w, h) = (core.num(src, "width"), core.num(src, "height"));
                let mut list = Vec::new();
                self.apart(|sc| sc.collect_at(core, src, Xf::IDENTITY, Rect::new(-1 << 20, -1 << 20, 1 << 21, 1 << 21), 1.0,
                                              true, true, &mut list));
                hash(&(w.to_bits(), h.to_bits(), list.iter().map(|s| s.print).collect::<Vec<u64>>()))
            }
        }
    }

    /// Texture for `src`: a ShaderEffectSource's, an Image's picture, or the
    /// item drawn at its size (or `rect` of it)
    fn source(&mut self, core: &Core, src: Option<Oid>, rect: Option<(f64, f64, f64, f64)>) -> Option<Tex> {
        let src = src?;
        let (w, h) = (core.num(src, "width"), core.num(src, "height"));
        if let Some(r) = rect {
            return Some(Tex::Made(self.render(core, src, r, texels(r.2, r.3))));
        }
        match Self::kind_name(core, src) {
            "ShaderEffectSource" => {
                let item = item_of(core, src, "sourceItem")?;
                let part = (core.num(src, "sourceRect.x"), core.num(src, "sourceRect.y"),
                            core.num(src, "sourceRect.width"), core.num(src, "sourceRect.height"));
                let part = if part.2 == 0.0 || part.3 == 0.0 {
                    (0.0, 0.0, core.num(item, "width"), core.num(item, "height"))
                } else {
                    part
                };
                let (tw, th) = (core.num(src, "textureSize.width"), core.num(src, "textureSize.height"));
                let size = if tw > 0.0 && th > 0.0 { (tw as i32, th as i32) } else { texels(part.2, part.3) };
                Some(Tex::Made(self.render(core, item, part, size)))
            }
            "Image" => self.picture(core, src).map(Tex::Picture),
            _ => Some(Tex::Made(self.render(core, src, (0.0, 0.0, w, h), texels(w, h)))),
        }
    }

    /// Draws `oid` and its subtree into a `size` texture, with the part
    /// `(x, y, w, h)` filling it
    fn render(&mut self, core: &Core, oid: Oid, (x, y, w, h): (f64, f64, f64, f64), size: (i32, i32)) -> Rc<Image> {
        let (tw, th) = size;
        if self.effects.making.contains(&oid) {
            return Rc::new(Image::new(tw, th));
        }
        self.effects.making.push(oid);
        let img = self.render_of(core, oid, (x, y, w, h), size);
        self.effects.making.retain(|&o| o != oid);
        img
    }

    fn render_of(&mut self, core: &Core, oid: Oid, (x, y, w, h): (f64, f64, f64, f64), size: (i32, i32)) -> Rc<Image> {
        let (tw, th) = size;
        let base = Xf::scale(tw as f64 / w.max(1e-9), th as f64 / h.max(1e-9)).times(&Xf::translate(-x, -y));
        let all = Rect::new(0, 0, tw, th);
        let mut list = Vec::new();
        self.apart(|sc| sc.collect_at(core, oid, base, all, 1.0, true, true, &mut list));
        let print = hash(&list.iter().map(|s| s.print).collect::<Vec<u64>>());
        let key = (oid, bits((x, y, w, h), size));
        if let Some((p, img)) = self.effects.textures.get(&key) {
            if *p == print {
                return img.clone();
            }
        }
        let mut img = Image::new(tw, th);
        {
            let mut p = Painter::new(&mut img, all);
            self.draw_list(core, &list, &mut p);
        }
        let img = Rc::new(img);
        self.effects.textures.insert(key, (print, img.clone()));
        img
    }

    /// A layer, or an effect, from the scene's list
    pub(super) fn draw_effect(&mut self, core: &Core, s: &Shown, p: &mut Painter<'_>) {
        let (w, h) = s.size;
        if s.draw == Draw::Layer {
            if let Some(&fx) = self.effects.layered.get(&s.oid) {
                self.draw_kind(core, fx, &s.xf, s.size, s.opacity, p);
                return;
            }
            let tex = self.render(core, s.oid, (0.0, 0.0, w, h), texels(w, h));
            let sampling = if core.flag(s.oid, "layer.smooth") { gpu::Sampling::Linear } else { gpu::Sampling::Nearest };
            let t = tex.texture();
            p.quad(&s.xf, (0.0, 0.0, w, h), &t, (0.0, 0.0, 1.0, 1.0), s.opacity, sampling);
            return;
        }
        self.draw_kind(core, s.oid, &s.xf, s.size, s.opacity, p);
    }

    /// The effect `oid`, `size` big, through `xf`
    fn draw_kind(&mut self, core: &Core, oid: Oid, xf: &Xf, size: (f64, f64), opacity: f32, p: &mut Painter<'_>) {
        let (w, h) = size;
        match Self::kind_name(core, oid) {
            // What the source has where the mask has, as the mask has it
            "OpacityMask" => {
                let (Some(src), Some(mask)) = (self.source(core, item_of(core, oid, "source"), None),
                                               self.source(core, item_of(core, oid, "maskSource"), None)) else { return };
                p.shaded(xf, (0.0, 0.0, w, h), opacity, &Mask { src: src.gpu(), mask: mask.gpu(), invert: core.flag(oid, "invert") });
            }
            "FastBlur" => self.fast_blur(core, oid, xf, size, opacity, p),
            "GaussianBlur" | "DropShadow" => self.gaussian(core, oid, xf, size, opacity, p),
            "RectangularShadow" => shadow::rectangular(core, oid, xf, size, opacity, p),
            "RadialGradient" => self.radial_gradient(core, oid, xf, size, opacity, p),
            "MultiEffect" => self.multi_effect(core, oid, xf, size, opacity, p),
            // Its texture on its box
            "ShaderEffectSource" => {
                let Some(tex) = self.source(core, Some(oid), None) else { return };
                p.quad(xf, (0.0, 0.0, w, h), &tex.gpu(), (0.0, 0.0, 1.0, 1.0), opacity, gpu::Sampling::Linear);
            }
            _ => {}
        }
    }

    /// Passes before the last, rebuilt by `make` unless `print` matches
    fn passes(&mut self, oid: Oid, print: u64, make: impl FnOnce(&mut Scene) -> Vec<Image>) -> Rc<Vec<Image>> {
        if let Some((p, kept)) = self.effects.passes.get(&oid) {
            if *p == print {
                return kept.clone();
            }
        }
        let made = Rc::new(make(self));
        self.effects.passes.insert(oid, (print, made.clone()));
        made
    }
}

/// Source strength follows the mask, or its inverse
struct Mask<'a> {
    src: gpu::Texture<'a>,
    mask: gpu::Texture<'a>,
    invert: bool,
}

impl gpu::record::Gpu for Mask<'_> {
    const KIND: u32 = gpu::record::SH_MASK;
    fn params(&self, rec: &mut gpu::record::Record) -> Option<Vec<u32>> {
        Some(vec![rec.texture(&self.src)?, rec.texture(&self.mask)?, self.invert as u32])
    }
    fn put_col(&self, c: &Self::Col, out: &mut Vec<u32>) {
        gpu::record::tap_words(&c.0, out);
        gpu::record::tap_words(&c.1, out);
    }
    fn put_row(&self, r: &Self::Row, out: &mut Vec<u32>) {
        gpu::record::tap_words(&r.0, out);
        gpu::record::tap_words(&r.1, out);
    }
}

impl gpu::Shader for Mask<'_> {
    type Col = (gpu::Tap, gpu::Tap);
    type Row = (gpu::Tap, gpu::Tap);
    fn col(&self, u: f32) -> Self::Col {
        (self.src.across(u), self.mask.across(u))
    }
    fn row(&self, v: f32) -> Self::Row {
        (self.src.down(v), self.mask.down(v))
    }
    fn at(&self, c: &Self::Col, r: &Self::Row) -> [f32; 4] {
        let a = self.mask.read_alpha(&c.1, &r.1);
        let a = if self.invert { 1.0 - a } else { a };
        self.src.read(&c.0, &r.0).map(|x| x * a)
    }
}

/// Effect color as a shader gets it: premultiplied, 0..1, red first
fn shader_colour(c: u32) -> [f32; 4] {
    let a = (c >> 24) as f32 / 255.0;
    let ch = |s: u32| ((c >> s) & 0xff) as f32 / 255.0 * a;
    [ch(16), ch(8), ch(0), a]
}
