// Text layout and drawing, matching Qt (`tests/qtref/fixtures/text.qml`,
// `tests.rs`)
//
// A file sets family, size, weight, style name, and variable axes over the
// application font (`platform.rs`).
// - Face: chosen as in Qt's font database (`resolve`): the family's style of
//   that name, else the nearest weight. Named instances are styles with their
//   own coordinates, with `font.variableAxes` set over them. Weight 700 or more
//   on a non-bold face is emboldened.
// - Glyphs: from rustybuzz (`shape.rs`), at the fontconfig-listed named
//   instance, as Qt's HarfBuzz.
// - Advance: FreeType's unhinted advance at the face's axes in 64ths, plus
//   kerning.
// - Line: `ascent + descent` from `hhea` then OS/2 (`metrics`), truncated to
//   64ths. A Text is that high rounded up, and as wide as its advances plus the
//   last glyph's overhang (`layout.rs`, `raster.rs`).
// - Pixels: FreeType's (`raster.rs`), placed as in Qt Quick.
//
// Missing characters fall back to the first face with the glyph, starting with
// the default

mod field;
pub mod index;
mod kern;
pub mod layout;
pub mod platform;
mod raster;
mod shape;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::paint::{Painter, TextStyle};
use super::xf::Xf;
use index::{Entry, Style};
pub use platform::AppFont;
pub use raster::Hint;

/// A face at a size with its axes
#[derive(Clone, Debug, PartialEq)]
pub struct Font {
    pub face: usize,
    pub px: f32,
    /// Every face axis in face order, at its drawn value
    pub axes: Vec<([u8; 4], f32)>,
    /// Named instance the face opens at (`fvar` index) and whether axes are set
    /// over it. Qt sets them only for a Text naming some, and FreeType rounds
    /// differently with explicit coordinates
    pub named: Option<usize>,
    pub set: bool,
    /// FreeType's synthetic bold
    pub bold: bool,
    pub hint: Hint,
    /// How its glyphs are drawn
    pub render: Render,
    /// HarfBuzz's instance coordinates, or none for the default (`shape.rs`)
    pub instance: Vec<([u8; 4], f32)>,
    /// The request, for fallback faces
    pub ask: Rc<Request>,
}

/// What a QML item asks for, its application's font under it
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub family: String,
    /// 100 to 900, Qt's `Font.Normal` 400 and `Font.Bold` 700
    pub weight: f32,
    pub italic: bool,
    pub px: f32,
    /// `font.styleName`
    pub style: String,
    /// `font.variableAxes`, by tag
    pub axes: Vec<([u8; 4], f32)>,
    /// `font.features` by tag (`tnum` 1 for tabular figures). Passed to
    /// HarfBuzz for the whole text, as in Qt
    pub features: Vec<([u8; 4], u32)>,
    pub hint: Hint,
    pub render: Render,
}

/// Glyph drawing: FreeType (Text.NativeRendering) or distance fields
/// (Text.QtRendering, `field.rs`). Layout is the same
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Render {
    #[default]
    Native,
    Field,
}

/// A face's line, in pixels at a size
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
}

impl Metrics {
    pub fn height(&self) -> f32 {
        self.ascent + self.descent
    }
}

/// A placed glyph: its face and pen position in 64ths from the line's start, y
/// down
#[derive(Clone, Debug)]
struct Placed {
    face: usize,
    id: u16,
    x: i64,
    y: i64,
}

/// A line of text, shaped and placed
struct Run {
    glyphs: Vec<Placed>,
    /// The sum of the advances, 26.6
    advance: i64,
    /// Qt's natural width: advances plus the last glyph's overhang
    natural: f32,
    /// Cluster start along the line by byte index, for the caret
    starts: Vec<(usize, f32)>,
}

type MaskKey = (String, u16, i64);

#[derive(Default)]
pub struct Fonts {
    entries: Vec<Entry>,
    data: Vec<Option<Rc<[u8]>>>,
    /// Face for an unknown family: the system's interface face
    default: Option<usize>,
    app: AppFont,
    resolved: HashMap<String, Option<Font>>,
    runs: HashMap<(String, String), Rc<Run>>,
    /// Glyphs' distance fields, by face and glyph (`field.rs`)
    fields: HashMap<(String, u16), Option<Rc<field::Field>>>,
    /// FreeType faces by face and coordinates, with each one's current size.
    /// One serves every size, as in Qt, so the autohinter computes glyph styles
    /// once
    faces: HashMap<String, (freetype::Face, i64)>,
    infos: HashMap<(String, u16), Option<raster::Info>>,
    masks: HashMap<MaskKey, Option<Rc<raster::Mask>>>,
    /// Whether a face has a glyph for a character
    covered: HashMap<(usize, char), bool>,
}

/// Key of a font's FreeType face: face, instance, coordinates
fn face_key(f: &Font) -> String {
    let coords: Vec<String> = f.axes.iter().map(|(_, v)| v.to_string()).collect();
    format!("{}|{:?}|{}|{}", f.face, f.named, f.set, coords.join(","))
}

/// Key of a font's glyphs: face, size, load flags, shaping
fn font_key(f: &Font) -> String {
    format!("{}|{}|{}|{:?}|{:?}|{:?}", face_key(f), raster::size26(f.px), f.bold, f.hint, f.instance, f.ask.features)
}

impl Fonts {
    /// Loads every font under `dirs`. The default is the regular face of the
    /// first directory with fonts
    pub fn load(dirs: &[&Path]) -> Fonts {
        let mut f = Fonts::default();
        for d in dirs {
            let mut paths = Vec::new();
            index::files(d, &mut paths);
            let before = f.entries.len();
            for p in paths {
                f.add_file(&p, false);
            }
            if f.default.is_none() && f.entries.len() > before {
                f.default = (before..f.entries.len())
                    .filter(|&i| !f.entries[i].monospace && !f.entries[i].italic)
                    .min_by_key(|&i| (f.entries[i].weight - 400.0).abs() as i32);
            }
        }
        f
    }

    /// Shell fonts: `/etc/aosh/fonts` (or the tree's `assets`), then the
    /// user's, plus the application font (`platform.rs`)
    pub fn system() -> Fonts {
        let mut dirs: Vec<PathBuf> = ["/etc/aosh/fonts", "assets", "../assets"].iter().map(PathBuf::from).collect();
        dirs.extend(Fonts::user_dir());
        let refs: Vec<&Path> = dirs.iter().map(|d| d.as_path()).collect();
        let mut f = Fonts::load(&refs);
        f.app = AppFont::read();
        f
    }

    /// `$XDG_DATA_HOME/fonts`, else `~/.local/share/fonts`
    pub fn user_dir() -> Option<PathBuf> {
        let xdg = std::env::var("XDG_DATA_HOME").ok().filter(|v| v.starts_with('/'));
        match xdg {
            Some(d) => Some(Path::new(&d).join("fonts")),
            None => std::env::var_os("HOME").map(|h| Path::new(&h).join(".local/share/fonts")),
        }
    }

    /// Application font: the defaults a Text leaves unset
    pub fn app(&self) -> &AppFont {
        &self.app
    }

    /// Overrides the application font, for tests
    #[cfg(test)]
    pub fn set_app(&mut self, app: AppFont) {
        self.app = app;
        self.resolved.clear();
    }

    /// Adds a font file from a FontLoader (`loaded`) or a scan, skipping
    /// duplicates
    pub fn add_file(&mut self, p: &Path, loaded: bool) {
        let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if let Some(e) = self.entries.iter_mut().find(|e| e.path == canon) {
            // A FontLoader of a file a scan found makes it the application's
            e.loaded |= loaded;
            return;
        }
        let Ok(bytes) = std::fs::read(&canon) else { return };
        let found = index::entries(&canon, &bytes);
        if found.is_empty() {
            crate::say(&format!("{} is not a font aosh can read", canon.display()));
            return;
        }
        for mut e in found {
            e.loaded = loaded;
            self.entries.push(e);
            self.data.push(None);
        }
        // Family resolution may change
        self.resolved.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The family a face is of, as FontLoader's `name` says it
    #[cfg(test)]
    pub fn family(&self, face: usize) -> &str {
        &self.entries[face].family
    }

    /// A face's file, read the first time it is drawn
    fn data(&mut self, face: usize) -> Option<Rc<[u8]>> {
        if let Some(d) = &self.data[face] {
            return Some(d.clone());
        }
        let bytes: Rc<[u8]> = std::fs::read(&self.entries[face].path).ok()?.into();
        // Every face of a collection shares its file
        let path = self.entries[face].path.clone();
        for i in 0..self.entries.len() {
            if self.entries[i].path == path {
                self.data[i] = Some(bytes.clone());
            }
        }
        Some(bytes)
    }

    /// Face, coordinates, and emboldening for a request: Qt's `bestStyle` among
    /// the family's styles. An unknown family uses the default face
    pub fn resolve(&mut self, r: &Request) -> Option<Font> {
        let key = format!("{}|{}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}", r.family.to_lowercase(), r.weight, r.italic, r.px, r.style, r.axes, r.hint, r.render, r.features);
        if let Some(f) = self.resolved.get(&key) {
            return f.clone();
        }
        let want = r.family.trim().to_lowercase();
        let family_of = |fonts: &Fonts, fam: &str| -> Vec<usize> {
            (0..fonts.entries.len()).filter(|&i| fonts.entries[i].family.to_lowercase() == fam).collect()
        };
        let mut pool = family_of(self, &want);
        if pool.is_empty() {
            pool = match self.default {
                Some(d) => family_of(self, &self.entries[d].family.to_lowercase()),
                None => (0..self.entries.len()).collect(),
            };
        }
        let ask = Rc::new(r.clone());
        let font = self.pick(&pool, &ask);
        self.resolved.insert(key, font.clone());
        font
    }

    /// Qt's `bestStyle` among `pool`: the requested style name, else the
    /// nearest weight, with slant outweighing any weight
    fn pick(&self, pool: &[usize], r: &Rc<Request>) -> Option<Font> {
        let styles: Vec<(usize, Style)> = pool.iter().flat_map(|&i| self.entries[i].styles().into_iter().map(move |s| (i, s))).collect();
        let named = (!r.style.is_empty()).then(|| styles.iter().find(|(_, s)| s.name == r.style)).flatten();
        let (face, style) = match named {
            Some(s) => s.clone(),
            None => {
                let distance = |s: &Style| ((r.weight as i32 - s.weight as i32) / 10).abs() + if s.italic != r.italic { 0x1000 } else { 0 };
                let mut best: Option<&(usize, Style)> = None;
                for s in &styles {
                    if best.is_none_or(|b| distance(&s.1) < distance(&b.1)) {
                        best = Some(s);
                    }
                }
                best?.clone()
            }
        };
        let e = &self.entries[face];
        // Item axes over the style's coordinates, clamped to each axis's range
        // as FreeType does
        let axes = e.axes.iter().zip(&style.coords).map(|(a, &v)| {
            let v = r.axes.iter().find(|(t, _)| *t == a.tag).map(|(_, v)| *v).unwrap_or(v);
            (a.tag, v.clamp(a.min, a.max))
        }).collect();
        // A FontLoader's face weighs what its instance does; others what OS/2
        // says (`calculateActualWeight`)
        let actual = if e.loaded && style.instance.is_some() { style.weight } else { e.weight };
        let bold = r.weight >= 700.0 && !e.bold && actual < 700.0 && r.px < 64.0;
        // Fontconfig's instance index, passed to HarfBuzz as in Qt; a
        // FontLoader's face has none
        let instance = match style.instance {
            Some(_) if !e.loaded => e.axes.iter().zip(&style.coords).map(|(a, &v)| (a.tag, v)).collect(),
            _ => Vec::new(),
        };
        let set = r.axes.iter().any(|(t, _)| e.axis(t).is_some());
        Some(Font { face, px: r.px, axes, named: style.instance, set, bold, hint: r.hint, render: r.render, instance, ask: r.clone() })
    }

    /// The same font in another face, for fallback characters
    fn in_face(&self, f: &Font, face: usize) -> Font {
        self.pick(&[face], &f.ask).unwrap_or_else(|| Font { face, ..f.clone() })
    }

    /// Ascent and descent at the font's size, as in Qt: `hhea`, then OS/2
    /// typographic with USE_TYPO_METRICS or Windows otherwise, truncated to
    /// 64ths (`processHheaTable`, `processOS2Table`)
    pub fn metrics(&mut self, f: &Font) -> Metrics {
        let l = self.entries[f.face].lines;
        let (upm, px) = (l.upm.max(1) as f64, f.px as f64);
        let fixed = |v: f64| ((v * 64.0) as i64) as f32 / 64.0;
        let mut m = Metrics { ascent: f.px * 0.8, descent: f.px * 0.2 };
        if l.hhea[0] != 0 || l.hhea[1] != 0 {
            m = Metrics { ascent: fixed(l.hhea[0] as f64 * px / upm), descent: -fixed(l.hhea[1] as f64 * px / upm) };
        }
        if let Some(o) = l.os2 {
            if o.fs_selection & 0x80 != 0 {
                if o.typo[0] != 0 || o.typo[1] != 0 {
                    m = Metrics { ascent: fixed(o.typo[0] as f64 * px / upm), descent: -fixed(o.typo[1] as f64 * px / upm) };
                }
            } else if o.win[0] != 0 || o.win[1] != 0 {
                m = Metrics { ascent: fixed(o.win[0] as f64 * px / upm), descent: fixed(o.win[1] as f64 * px / upm) };
            }
        }
        m
    }

    /// `text` as one line, shaped and placed, cached
    fn run(&mut self, f: &Font, text: &str) -> Rc<Run> {
        let key = (font_key(f), text.to_string());
        if let Some(r) = self.runs.get(&key) {
            return r.clone();
        }
        let run = Rc::new(self.make_run(f, text));
        if self.runs.len() > 4096 {
            self.runs.clear();
        }
        self.runs.insert(key, run.clone());
        run
    }

    fn make_run(&mut self, f: &Font, text: &str) -> Run {
        // Splits text into pieces by first face covering them: the font's,
        // default, then any
        let mut pieces: Vec<(usize, usize, usize)> = Vec::new();
        for (i, c) in text.char_indices() {
            let face = self.face_for(f.face, c);
            match pieces.last_mut() {
                Some(p) if p.0 == face => p.2 = i + c.len_utf8(),
                _ => pieces.push((face, i, i + c.len_utf8())),
            }
        }
        let mut run = Run { glyphs: Vec::new(), advance: 0, natural: 0.0, starts: Vec::new() };
        let mut last: Option<(Font, u16)> = None;
        for (face, start, end) in pieces {
            let pf = if face == f.face { f.clone() } else { self.in_face(f, face) };
            let Some(data) = self.data(face) else { continue };
            let index = self.entries[face].index;
            let Some(glyphs) = shape::shape(&data, index, &pf.instance, &f.ask.features, &text[start..end]) else { continue };
            let upm = self.entries[face].lines.upm.max(1) as i64;
            // HarfBuzz's scale is the pixel size in 64ths, truncated; units
            // scale by a rounded 16.16 multiplier; kerning as in `kern.rs`
            let scale = (pf.px as f64 * 64.0) as i64;
            let mult = (scale << 16) / upm;
            let em = |v: i32| (v as i64 * mult + 32768) >> 16;
            let kerned = kern::exact(&data, index, &pf.instance, &text[start..end], &glyphs, scale, upm);
            for (n, g) in glyphs.iter().enumerate() {
                let at = start + g.cluster as usize;
                if !run.starts.iter().any(|s| s.0 == at) {
                    run.starts.push((at, run.advance as f32 / 64.0));
                }
                run.glyphs.push(Placed { face, id: g.id, x: run.advance + em(g.x_offset), y: -em(g.y_offset) });
                // A mark shaping took the advance from has none
                let advance = if g.advance == 0 {
                    0
                } else {
                    let linear = self.info(&pf, g.id).map(|i| i.linear).unwrap_or_else(|| em(g.base));
                    linear + kerned[n].unwrap_or_else(|| em(g.advance - g.base))
                };
                run.advance += advance;
                last = Some((pf.clone(), g.id));
            }
        }
        run.starts.sort_by_key(|s| s.0);
        // A line ending in a space has no right bearing (`whiteSpaceOrObject`)
        let ends_in_space = text.chars().last().is_none_or(char::is_whitespace);
        let hang = match last {
            Some((pf, id)) if !ends_in_space => self.info(&pf, id).map(|i| (-i.right_bearing()).max(0)).unwrap_or(0),
            _ => 0,
        };
        run.natural = (run.advance + hang * 64) as f32 / 64.0;
        run
    }

    /// Face for `c`: `face`, else the default, else the first covering it
    fn face_for(&mut self, face: usize, c: char) -> usize {
        let has = |fonts: &mut Fonts, i: usize| -> bool {
            if let Some(&h) = fonts.covered.get(&(i, c)) {
                return h;
            }
            let h = fonts.data(i).is_some_and(|d| {
                rustybuzz::ttf_parser::Face::parse(&d, fonts.entries[i].index).is_ok_and(|f| shape::covers(&f, c))
            });
            if fonts.covered.len() > 65536 {
                fonts.covered.clear();
            }
            fonts.covered.insert((i, c), h);
            h
        };
        if has(self, face) {
            return face;
        }
        if let Some(d) = self.default.filter(|&d| d != face) {
            if has(self, d) {
                return d;
            }
        }
        (0..self.entries.len()).find(|&i| i != face && has(self, i)).unwrap_or(face)
    }

    /// FreeType's face for a font, made the first time, at the font's size
    fn ft_face(&mut self, f: &Font) -> Option<&mut freetype::Face> {
        let key = face_key(f);
        let size = raster::size26(f.px);
        if !self.faces.contains_key(&key) {
            let data = self.data(f.face)?;
            // A named instance is the face index with the instance above it, as
            // fontconfig and FreeType use it
            let index = self.entries[f.face].index | f.named.map_or(0, |n| (n as u32 + 1) << 16);
            let mut face = freetype::Face::new(data, index)?;
            let coords: Vec<f32> = if f.set { f.axes.iter().map(|(_, v)| *v).collect() } else { Vec::new() };
            if !face.setup(&coords, size) {
                return None;
            }
            // Face cache limit, for files animating axes
            if self.faces.len() > 64 {
                self.faces.clear();
            }
            self.faces.insert(key.clone(), (face, size));
        }
        let (face, at) = self.faces.get_mut(&key)?;
        if *at != size {
            if !face.set_size(size) {
                return None;
            }
            *at = size;
        }
        Some(face)
    }

    /// Glyph `id` metrics in `f`: advance and box
    fn info(&mut self, f: &Font, id: u16) -> Option<raster::Info> {
        let key = (font_key(f), id);
        if let Some(i) = self.infos.get(&key) {
            return *i;
        }
        let load = raster::load(f, false, 0);
        let i = self.ft_face(f).and_then(|face| face.glyph(id, load))
            .map(|g| raster::Info { linear: g.linear, advance: g.advance, bearing_x: g.bearing_x, width: g.width });
        if self.infos.len() > 65536 {
            self.infos.clear();
        }
        self.infos.insert(key, i);
        i
    }

    /// Line width as Qt measures it: the natural width
    pub fn width(&mut self, f: &Font, text: &str) -> f32 {
        if text.is_empty() {
            return 0.0;
        }
        self.run(f, text).natural
    }

    /// Caret position before character `n`
    pub fn caret(&mut self, f: &Font, text: &str, n: usize) -> f32 {
        let at = text.char_indices().nth(n).map(|(i, _)| i);
        let run = self.run(f, text);
        match at {
            None => run.advance as f32 / 64.0,
            Some(i) => run.starts.iter().rev().find(|s| s.0 <= i).map(|s| s.1).unwrap_or(0.0),
        }
    }

    /// Draws `text` in `f` from pen `x` at `baseline`, in item coordinates that
    /// `at` maps to the window. As in Qt Quick's glyph node: pen floored to a
    /// pixel with the rest rendered in quarters, baseline rounded
    /// (`qsgdefaultglyphnode_p.cpp`), and quad corners snapped to window pixels
    /// after the matrix (`textmask.vert`)
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, p: &mut Painter<'_>, f: &Font, at: &Xf, x: f32, baseline: f32, text: &str, colour: u32, opacity: f32) {
        self.draw_styled(p, f, at, x, baseline, text, colour, TextStyle::Normal, 0, opacity);
    }

    /// `draw`, with a Text's `style` in `style_colour`
    #[allow(clippy::too_many_arguments)]
    pub fn draw_styled(&mut self, p: &mut Painter<'_>, f: &Font, at: &Xf, x: f32, baseline: f32, text: &str, colour: u32,
                       style: TextStyle, style_colour: u32, opacity: f32) {
        if text.is_empty() {
            return;
        }
        if f.render == Render::Field && style == TextStyle::Normal {
            self.draw_fields(p, f, at, x, baseline, text, colour, opacity);
            return;
        }
        // Translation only: the glyph's corners moved by whole pixels
        let moved = at.is_translation().then(|| ((at.tx + 0.5).floor() as i32, (at.ty + 0.5).floor() as i32));
        let run = self.run(f, text);
        for g in &run.glyphs {
            let pf = if g.face == f.face { f.clone() } else { self.in_face(f, g.face) };
            let pen = x as f64 + g.x as f64 / 64.0;
            let sub = raster::sub_for((pen * 64.0) as i64);
            let Some(m) = self.mask(&pf, g.id, sub) else { continue };
            let gx = pen.floor() as i32 + m.left;
            let gy = (baseline as f64 + g.y as f64 / 64.0).round() as i32 - m.top;
            let Some((dx, dy)) = moved else {
                p.glyph(at, gx, gy, &m.data, m.w, m.h, m.serial, colour, style, style_colour, opacity);
                continue;
            };
            let (gx, gy) = (gx + dx, gy + dy);
            if style == TextStyle::Normal {
                p.mask(gx, gy, &m.data, m.w, m.h, m.serial, colour, opacity);
            } else {
                p.styled_mask(gx, gy, &m.data, m.w, m.h, m.serial, colour, style, style_colour, opacity);
                // An outline redraws the glyph fill on top
                // (QSGInternalTextNode::addGlyphs' `fillNode`)
                if style == TextStyle::Outline && colour >> 24 != 0 && style_colour != colour {
                    p.mask(gx, gy, &m.data, m.w, m.h, m.serial, colour, opacity);
                }
            }
        }
    }

    /// Draws `text` from distance fields: each over the glyph's box at the
    /// font's size from the unrounded pen
    #[allow(clippy::too_many_arguments)]
    fn draw_fields(&mut self, p: &mut Painter<'_>, f: &Font, at: &Xf, x: f32, baseline: f32, text: &str, colour: u32, opacity: f32) {
        let run = self.run(f, text);
        let size = f.px as f64 / field::BASE;
        let s = size * (at.a * at.d - at.b * at.c).abs().sqrt();
        let (edge, ramp) = (field::threshold(s), field::spread(s));
        let (lo, hi) = ((edge - ramp).max(0.0) as f32, (edge + ramp).min(1.0) as f32);
        let a = (colour >> 24) as f32 / 255.0;
        let ch = |k: u32| ((colour >> k) & 0xff) as f32 / 255.0 * a;
        let ink = [ch(16), ch(8), ch(0), a];
        for g in &run.glyphs {
            let pf = if g.face == f.face { f.clone() } else { self.in_face(f, g.face) };
            let Some(fd) = self.field(&pf, g.id) else { continue };
            let (px, py) = (x as f64 + g.x as f64 / 64.0, baseline as f64 + g.y as f64 / 64.0);
            let rect = (px + fd.x * size, py + fd.y * size, fd.w as f64 * size, fd.h as f64 * size);
            let tex = super::gpu::Texture::new(&fd.px, fd.w, fd.h, fd.serial);
            p.shaded(at, rect, opacity, &field::Ink { tex, colour: ink, lo, hi });
        }
    }

    /// Glyph `id`'s distance field, built on first use from its outline at
    /// units per em, as Qt loads it for a path
    fn field(&mut self, f: &Font, id: u16) -> Option<Rc<field::Field>> {
        let key = (face_key(f), id);
        if let Some(fd) = self.fields.get(&key) {
            return fd.clone();
        }
        let upm = self.entries[f.face].lines.upm.max(1) as i64;
        let at_upm = Font { px: upm as f32, ..f.clone() };
        let fd = self.ft_face(&at_upm).and_then(|face| face.outline(id, true))
            .and_then(|o| field::make(&o, upm)).map(Rc::new);
        if self.fields.len() > 16384 {
            self.fields.clear();
        }
        self.fields.insert(key, fd.clone());
        fd
    }

    fn mask(&mut self, f: &Font, id: u16, sub: i64) -> Option<Rc<raster::Mask>> {
        let key = (font_key(f), id, sub);
        if let Some(m) = self.masks.get(&key) {
            return m.clone();
        }
        let load = raster::load(f, true, sub);
        let m = self.ft_face(f).and_then(|face| face.glyph(id, load)).and_then(raster::mask).map(Rc::new);
        // Glyph cache limit
        if self.masks.len() > 16384 {
            self.masks.clear();
        }
        self.masks.insert(key, m.clone());
        m
    }
}

#[cfg(test)]
pub(crate) mod tests;
