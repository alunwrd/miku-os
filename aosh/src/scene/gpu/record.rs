// Records the processor painter's output for the GPU
//
// `Painter` takes the same calls either way. Per-corner, per-column, and
// per-row work uses the same code as the processor (`place`, `quad_corners`,
// `upright_span`, `upright_taps`, `shaded_grid`); per-pixel work is
// `paint.cu`'s (kernel/drivers/gpu/nvidia/shaders/).
//
// A damage rectangle records whole or not at all: anything with no GPU form
// refuses it, and the processor paints it (`Scene::record`).
//
// Commands use `paint.cu`'s layout in 32-bit words: header, targets, textures,
// tiles, operations, tile lists, data. Textures stay in video memory between
// frames, keyed by the serial of their source (`Store`).

use std::collections::HashMap;

use super::raster::{place, quad_corners, shaded_grid, upright_span, upright_taps, is_upright, Geometry, Sampling, Shader, Tap, Texture};
use crate::scene::paint::{premultiply, Rect, TextStyle};
use crate::scene::xf::Xf;

pub const MAGIC: u32 = 0x544E_5041;
pub const VERSION: u32 = 1;

// Operations and shaders, numbered as in `paint.cu`
pub const OP_FILL: u32 = 0x0110;
pub const OP_TRIS: u32 = 0x0230;
pub const OP_QUAD: u32 = 0x0470;
pub const OP_QUAD_TURNED: u32 = 0x08f0;
pub const OP_SHADED: u32 = 0x1170;
pub const OP_MASK: u32 = 0x2330;
pub const OP_STYLED: u32 = 0x4410;
pub const OP_SHADED_TURNED: u32 = 0x8230;

pub const SH_BOX: u32 = 0x0150;
pub const SH_MASK: u32 = 0x0290;
pub const SH_INK: u32 = 0x04d0;
pub const SH_MIX: u32 = 0x0830;
pub const SH_GAUSS: u32 = 0x1050;
pub const SH_RADIAL: u32 = 0x2090;
pub const SH_MULTI: u32 = 0x40f0;

/// No texture for this shader
pub const NO_TEXTURE: u32 = 0xFFFF_FFFF;

const HEADER_WORDS: usize = 16;
const TARGET_WORDS: usize = 5;
const TEXTURE_WORDS: usize = 5;
const TILE_WORDS: usize = 8;
const OP_WORDS: usize = 16;
/// A tile's side, and the threads of a block are its pixels
pub const TILE: i32 = 16;
pub const BLOCK_THREADS: u32 = (TILE * TILE) as u32;
/// Corner coordinate limit in 1/256 pixel: the program's edge functions
/// multiply two in 64 bits
const MOST: i64 = 1 << 28;
/// A tap names its texels in 16 bits each
const MOST_SIDE: usize = 0xFFFF;
/// Limits for a run of a geometry's triangles (`tris`)
const RUN_TRIANGLES: usize = 16;
const RUN_SIDE: i64 = 48;
/// Limits on a Mix's levels and a Gauss's reads, as `paint.cu` reads them
const MOST_LEVELS: usize = 8;
const MOST_READS: usize = 64;

/// Memory the GPU reads textures from, which the processor writes
pub trait Memory {
    fn size(&self) -> usize;
    fn write(&mut self, at: usize, bytes: &[u8]);
}

/// Textures in video memory, kept by key until their space is needed
pub struct Store {
    mem: Box<dyn Memory>,
    kept: HashMap<u64, Kept>,
    /// Free ranges, `(at, len)`, by `at`
    free: Vec<(usize, usize)>,
    /// Current frame; its textures are not evicted
    frame: u64,
}

struct Kept {
    at: usize,
    len: usize,
    entry: [u32; 5],
    used: u64,
}

/// A texture's room is a multiple of this, and starts on one
const ALIGN: usize = 256;

impl Store {
    pub fn new(mem: Box<dyn Memory>) -> Store {
        let size = mem.size();
        Store { mem, kept: HashMap::new(), free: vec![(0, size)], frame: 0 }
    }

    fn take(&mut self, len: usize) -> Option<usize> {
        let len = len.div_ceil(ALIGN) * ALIGN;
        let i = self.free.iter().position(|&(_, l)| l >= len)?;
        let (at, l) = self.free[i];
        if l == len {
            self.free.remove(i);
        } else {
            self.free[i] = (at + len, l - len);
        }
        Some(at)
    }

    fn give(&mut self, at: usize, len: usize) {
        let len = len.div_ceil(ALIGN) * ALIGN;
        let i = self.free.partition_point(|&(a, _)| a < at);
        self.free.insert(i, (at, len));
        // With the one after, then the one before
        if i + 1 < self.free.len() && self.free[i].0 + self.free[i].1 == self.free[i + 1].0 {
            self.free[i].1 += self.free[i + 1].1;
            self.free.remove(i + 1);
        }
        if i > 0 && self.free[i - 1].0 + self.free[i - 1].1 == self.free[i].0 {
            self.free[i - 1].1 += self.free[i].1;
            self.free.remove(i);
        }
    }

    /// Evicts this frame's least recently used unused texture
    fn evict(&mut self) -> bool {
        let Some((&key, _)) = self.kept.iter().filter(|(_, k)| k.used < self.frame).min_by_key(|(_, k)| k.used) else {
            return false;
        };
        let k = self.kept.remove(&key).unwrap();
        self.give(k.at, k.len);
        true
    }

    /// `key`'s texture, written from `bytes` if absent. `None` if evicting
    /// everything unused still leaves no room
    fn get(&mut self, key: u64, (stride, w, h, format): (u32, u32, u32, u32), bytes: &[u8]) -> Option<[u32; 5]> {
        if let Some(k) = self.kept.get_mut(&key) {
            k.used = self.frame;
            return Some(k.entry);
        }
        let at = loop {
            if let Some(at) = self.take(bytes.len().max(1)) {
                break at;
            }
            if !self.evict() {
                return None;
            }
        };
        self.mem.write(at, bytes);
        let entry = [at as u32, stride, w, h, format];
        self.kept.insert(key, Kept { at, len: bytes.len().max(1), entry, used: self.frame });
        Some(entry)
    }

}

/// The rectangle being recorded
struct Open {
    target: u32,
    rect: Rect,
    /// Target bounds, clipping every operation
    bounds: Rect,
    first: usize,
    data: usize,
    /// Each operation's pixels, for the tiles' lists
    reach: Vec<Rect>,
    refused: bool,
}

/// A frame's commands for `paint.cu`
pub struct Record {
    pub store: Store,
    targets: Vec<[u32; TARGET_WORDS]>,
    textures: Vec<[u32; TEXTURE_WORDS]>,
    /// This frame's textures' places in `textures`, by key
    named: HashMap<u64, u32>,
    ops: Vec<[u32; OP_WORDS]>,
    data: Vec<u32>,
    tiles: Vec<[u32; TILE_WORDS]>,
    lists: Vec<u32>,
    open: Option<Open>,
    refused: Option<&'static str>,
}

/// Four bytes as a word, first in the low bits: a `premultiply` color, alpha
/// first
fn pack(c: [u32; 4]) -> u32 {
    c[0].min(255) | c[1].min(255) << 8 | c[2].min(255) << 16 | c[3].min(255) << 24
}

pub fn tap_words(t: &Tap, out: &mut Vec<u32>) {
    out.push(t.i0 as u32 | (t.i1 as u32) << 16);
    out.push(t.t.to_bits());
}

/// A shader `paint.cu` also has: its id, parameters and textures by recording
/// position, and column and row values as words. `Painter::shaded` takes only
/// these, so a shader with no GPU form does not compile
pub trait Gpu: Shader {
    const KIND: u32;
    fn params(&self, rec: &mut Record) -> Option<Vec<u32>>;
    fn put_col(&self, c: &Self::Col, out: &mut Vec<u32>);
    fn put_row(&self, r: &Self::Row, out: &mut Vec<u32>);
}

impl Record {
    pub fn new(mem: Box<dyn Memory>) -> Record {
        Record {
            store: Store::new(mem), targets: Vec::new(), textures: Vec::new(), named: HashMap::new(), ops: Vec::new(),
            data: Vec::new(), tiles: Vec::new(), lists: Vec::new(), open: None, refused: None,
        }
    }

    /// Starts a frame: nothing recorded, all textures evictable
    pub fn frame(&mut self) {
        self.targets.clear();
        self.textures.clear();
        self.named.clear();
        self.ops.clear();
        self.data.clear();
        self.tiles.clear();
        self.lists.clear();
        self.open = None;
        self.store.frame += 1;
    }

    /// Adds a target: `stride` pixels a row at `off` bytes in pool `pool` (of
    /// `paint.cu`'s four), `w` by `h`
    pub fn target(&mut self, pool: u32, off: u32, stride: u32, w: u32, h: u32) -> u32 {
        self.targets.push([off, stride, w, h, pool]);
        (self.targets.len() - 1) as u32
    }

    /// Start a rectangle of `target`
    pub fn begin(&mut self, target: u32, rect: Rect) {
        let [_, _, w, h, _] = self.targets[target as usize];
        self.open = Some(Open {
            target, rect, bounds: Rect::new(0, 0, w as i32, h as i32), first: self.ops.len(), data: self.data.len(),
            reach: Vec::new(), refused: false,
        });
    }

    /// The box of what is being recorded into
    pub fn bounds(&self) -> Rect {
        self.open.as_ref().map(|o| o.bounds).unwrap_or_default()
    }

    /// The rectangle cannot be recorded; the processor paints it
    pub fn refuse(&mut self, why: &'static str) {
        if let Some(o) = self.open.as_mut() {
            if !o.refused {
                self.refused = Some(why);
            }
            o.refused = true;
        }
    }

    /// Reason for the last refusal
    pub fn refused(&self) -> Option<&'static str> {
        self.refused
    }

    /// Ends a rectangle, building tiles with the operations reaching each, in
    /// order. False, keeping nothing, if refused
    pub fn end(&mut self) -> bool {
        let Some(o) = self.open.take() else { return false };
        if o.refused {
            self.ops.truncate(o.first);
            self.data.truncate(o.data);
            return false;
        }
        let r = o.rect;
        let (nx, ny) = ((r.w + TILE - 1) / TILE, (r.h + TILE - 1) / TILE);
        if nx <= 0 || ny <= 0 {
            return true;
        }
        let mut lists: Vec<Vec<u32>> = vec![Vec::new(); (nx * ny) as usize];
        for (k, reach) in o.reach.iter().enumerate() {
            let Some(q) = reach.intersect(&r) else { continue };
            let (tx0, ty0) = ((q.x - r.x) / TILE, (q.y - r.y) / TILE);
            let (tx1, ty1) = ((q.right() - 1 - r.x) / TILE, (q.bottom() - 1 - r.y) / TILE);
            for ty in ty0..=ty1 {
                for tx in tx0..=tx1 {
                    lists[(ty * nx + tx) as usize].push((o.first + k) as u32);
                }
            }
        }
        for ty in 0..ny {
            for tx in 0..nx {
                let (x, y) = (r.x + tx * TILE, r.y + ty * TILE);
                let (w, h) = (TILE.min(r.right() - x), TILE.min(r.bottom() - y));
                let list = &lists[(ty * nx + tx) as usize];
                self.tiles.push([o.target, x as u32, y as u32, w as u32, h as u32, self.lists.len() as u32, list.len() as u32, 0]);
                self.lists.extend_from_slice(list);
            }
        }
        true
    }

    /// Tile count, which is the launch's block count
    pub fn tiles(&self) -> u32 {
        self.tiles.len() as u32
    }

    /// The frame's commands, as `paint.cu` reads them
    pub fn words(&self) -> Vec<u32> {
        let targets_at = HEADER_WORDS;
        let textures_at = targets_at + self.targets.len() * TARGET_WORDS;
        let tiles_at = textures_at + self.textures.len() * TEXTURE_WORDS;
        let ops_at = tiles_at + self.tiles.len() * TILE_WORDS;
        let lists_at = ops_at + self.ops.len() * OP_WORDS;
        let data_at = lists_at + self.lists.len();
        let words = data_at + self.data.len();
        let mut w = Vec::with_capacity(words);
        w.extend_from_slice(&[MAGIC, VERSION, self.tiles.len() as u32, tiles_at as u32, self.targets.len() as u32,
                              targets_at as u32, self.textures.len() as u32, textures_at as u32, self.ops.len() as u32,
                              ops_at as u32, lists_at as u32, data_at as u32, words as u32, 0, 0, 0]);
        self.targets.iter().for_each(|t| w.extend_from_slice(t));
        self.textures.iter().for_each(|t| w.extend_from_slice(t));
        self.tiles.iter().for_each(|t| w.extend_from_slice(t));
        self.ops.iter().for_each(|o| w.extend_from_slice(o));
        w.extend_from_slice(&self.lists);
        w.extend_from_slice(&self.data);
        w
    }

    /// A texture's slot in this frame's table, written to video memory if
    /// absent. `None` refuses the rectangle
    pub fn texture(&mut self, tex: &Texture<'_>) -> Option<u32> {
        if tex.w == 0 || tex.h == 0 || tex.w > MOST_SIDE || tex.h > MOST_SIDE || tex.px.len() < tex.w * tex.h {
            return None;
        }
        let key = if tex.key == 0 { super::serial() } else { tex.key };
        if let Some(&i) = self.named.get(&key) {
            return Some(i);
        }
        let px = &tex.px[..tex.w * tex.h];
        // SAFETY: u32 to four little-endian bytes, the channel order `paint.cu`
        // reads
        let bytes = unsafe { std::slice::from_raw_parts(px.as_ptr() as *const u8, px.len() * 4) };
        let entry = self.store.get(key, (tex.w as u32, tex.w as u32, tex.h as u32, 0), bytes)?;
        self.textures.push(entry);
        let i = (self.textures.len() - 1) as u32;
        self.named.insert(key, i);
        Some(i)
    }

    /// A glyph's coverage, a byte a pixel, as a texture
    fn mask_texture(&mut self, key: u64, data: &[u8], w: usize, h: usize) -> Option<u32> {
        if w == 0 || h == 0 || w > MOST_SIDE || h > MOST_SIDE || data.len() < w * h {
            return None;
        }
        let key = if key == 0 { super::serial() } else { key };
        if let Some(&i) = self.named.get(&key) {
            return Some(i);
        }
        let entry = self.store.get(key, (w as u32, w as u32, h as u32, 1), &data[..w * h])?;
        self.textures.push(entry);
        let i = (self.textures.len() - 1) as u32;
        self.named.insert(key, i);
        Some(i)
    }

    /// One operation in force over `clip`, reaching `reach` of it
    fn push(&mut self, kind: u32, clip: Rect, reach: Rect, data: &[u32], opacity: f32, args: &[u32]) {
        let at = self.data.len() as u32;
        self.push_at(kind, clip, reach, at, data.len() as u32, data, opacity, args);
    }

    /// One operation whose data is `words` words from `at`, `new` written now
    /// (0 to share the previous operation's data)
    #[allow(clippy::too_many_arguments)]
    fn push_at(&mut self, kind: u32, clip: Rect, reach: Rect, at: u32, words: u32, new: &[u32], opacity: f32, args: &[u32]) {
        let Some(open) = self.open.as_mut() else { return };
        self.data.extend_from_slice(new);
        let Some(reach) = reach.intersect(&clip) else { return };
        let mut op = [0u32; OP_WORDS];
        op[0] = kind;
        op[1] = clip.x as u32;
        op[2] = clip.y as u32;
        op[3] = clip.right() as u32;
        op[4] = clip.bottom() as u32;
        op[5] = at;
        op[6] = words;
        op[7] = opacity.to_bits();
        for (k, a) in args.iter().take(8).enumerate() {
            op[8 + k] = *a;
        }
        open.reach.push(reach);
        self.ops.push(op);
    }

    /// Painter clip cut to the target; `None` when empty
    fn cut(&self, clip: Rect) -> Option<Rect> {
        clip.intersect(&self.open.as_ref()?.bounds)
    }

    /// `Painter::fill`
    pub fn fill(&mut self, clip: Rect, colour: u32) {
        let Some(clip) = self.cut(clip) else { return };
        let src = pack(premultiply(colour, 1.0));
        self.push(OP_FILL, clip, clip, &[], 1.0, &[src]);
    }

    /// `raster::fill`: a geometry's triangles
    pub fn tris(&mut self, clip: Rect, g: &Geometry, xf: &Xf, opacity: f32) {
        let Some(clip) = self.cut(clip) else { return };
        if g.tris.is_empty() || g.verts.is_empty() {
            return;
        }
        let placed: Vec<(i64, i64)> = g.verts.iter().map(|v| place(v, xf)).collect();
        if placed.iter().any(|&(x, y)| x.abs() >= MOST || y.abs() >= MOST) {
            self.refuse("a triangle's corner a million pixels away");
            return;
        }
        let mut data = Vec::with_capacity(g.verts.len() * 3 + g.tris.len() * 3);
        for (v, &(x, y)) in g.verts.iter().zip(&placed) {
            let c = v.color;
            data.extend_from_slice(&[x as i32 as u32, y as i32 as u32, pack([c.r as u32, c.g as u32, c.b as u32, c.a as u32])]);
        }
        for t in &g.tris {
            data.extend_from_slice(t);
        }
        // Runs of neighboring triangles, each an operation reaching only its
        // own area, so a pixel does not test hundreds of thin edge triangles.
        // Sequential, keeping triangle order per pixel
        let at = self.data.len() as u32;
        let bbox = |t: &[u32; 3]| -> Option<(i64, i64, i64, i64)> {
            let p: Vec<(i64, i64)> = t.iter().map(|&i| placed.get(i as usize).copied()).collect::<Option<_>>()?;
            Some((p.iter().map(|q| q.0).min()?, p.iter().map(|q| q.1).min()?, p.iter().map(|q| q.0).max()?, p.iter().map(|q| q.1).max()?))
        };
        let reach_of = |b: (i64, i64, i64, i64)| Rect::new((b.0 >> 8) as i32 - 1, (b.1 >> 8) as i32 - 1,
                                                            ((b.2 - b.0) >> 8) as i32 + 3, ((b.3 - b.1) >> 8) as i32 + 3);
        let mut first = true;
        let mut run: Option<(usize, usize, (i64, i64, i64, i64))> = None;
        let mut runs = Vec::new();
        for (k, t) in g.tris.iter().enumerate() {
            // Triangles naming a missing corner are skipped when drawn, so
            // reach nothing
            let Some(b) = bbox(t) else { continue };
            run = match run {
                Some((s, n, u)) => {
                    let joined = (u.0.min(b.0), u.1.min(b.1), u.2.max(b.2), u.3.max(b.3));
                    let small = (joined.2 - joined.0) <= RUN_SIDE << 8 && (joined.3 - joined.1) <= RUN_SIDE << 8;
                    if k == s + n && (small || n == 0) && n < RUN_TRIANGLES {
                        Some((s, n + 1, joined))
                    } else {
                        runs.push((s, n, u));
                        Some((k, 1, b))
                    }
                }
                None => Some((k, 1, b)),
            };
        }
        runs.extend(run);
        for (s, n, b) in runs {
            let words = if first { data.as_slice() } else { &[] };
            first = false;
            self.push_at(OP_TRIS, clip, reach_of(b), at, data.len() as u32, words, opacity,
                         &[g.verts.len() as u32, n as u32, s as u32]);
        }
    }

    /// `raster::quad`: a textured item quad
    #[allow(clippy::too_many_arguments)]
    pub fn quad(&mut self, clip: Rect, xf: &Xf, rect: (f64, f64, f64, f64), tex: &Texture<'_>, uv: (f32, f32, f32, f32),
                opacity: f32, sampling: Sampling) {
        let Some(clip) = self.cut(clip) else { return };
        if tex.w == 0 || tex.h == 0 {
            return;
        }
        let p = quad_corners(xf, rect, sampling);
        if p.iter().any(|&(x, y)| x.abs() >= MOST || y.abs() >= MOST) {
            self.refuse("a quad's corner a million pixels away");
            return;
        }
        let linear = sampling == Sampling::Linear;
        let Some(t) = self.texture(tex) else {
            self.refuse("a texture with no room, or too big");
            return;
        };
        if is_upright(xf) {
            let Some(span) = upright_span(clip, p[0], p[3]) else { return };
            let (x0, x1, y0, y1) = span;
            let (cols, rows, whole) = upright_taps(p[0], p[3], (tex.w, tex.h), uv, linear, span);
            let mut data = Vec::with_capacity((cols.len() + rows.len()) * 2);
            cols.iter().chain(&rows).for_each(|t| tap_words(t, &mut data));
            let reach = Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32);
            self.push(OP_QUAD, clip, reach, &data, opacity,
                      &[t, x0 as u32, y0 as u32, cols.len() as u32, rows.len() as u32, linear as u32 | (whole as u32) << 1]);
            return;
        }
        let mut data = Vec::with_capacity(12);
        for &(x, y) in &p {
            data.extend_from_slice(&[x as i32 as u32, y as i32 as u32]);
        }
        data.extend_from_slice(&[uv.0.to_bits(), uv.1.to_bits(), uv.2.to_bits(), uv.3.to_bits()]);
        let (x0, x1) = (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.0).max().unwrap());
        let (y0, y1) = (p.iter().map(|q| q.1).min().unwrap(), p.iter().map(|q| q.1).max().unwrap());
        let reach = Rect::new((x0 >> 8) as i32 - 1, (y0 >> 8) as i32 - 1, ((x1 - x0) >> 8) as i32 + 3, ((y1 - y0) >> 8) as i32 + 3);
        let glyph = sampling == Sampling::Glyph;
        self.push(OP_QUAD_TURNED, clip, reach, &data, opacity, &[t, linear as u32 | (glyph as u32) << 1]);
    }

    /// `raster::shaded`: a shader on an upright item quad
    pub fn shaded<S: Gpu>(&mut self, clip: Rect, xf: &Xf, rect: (f64, f64, f64, f64), opacity: f32, shader: &S) {
        let Some(clip) = self.cut(clip) else { return };
        if rect.2 <= 0.0 || rect.3 <= 0.0 {
            return;
        }
        let p = quad_corners(xf, rect, Sampling::Linear);
        if p.iter().any(|&(x, y)| x.abs() >= MOST || y.abs() >= MOST) {
            self.refuse("a shader's corner a million pixels away");
            return;
        }
        // Rotated: corners and parameters only; `paint.cu` computes per-pixel
        // columns and rows
        if !is_upright(xf) {
            let Some(params) = shader.params(self) else {
                self.refuse("a shader's texture with no room, or too many reads");
                return;
            };
            let mut data = Vec::with_capacity(8 + params.len());
            for &(x, y) in &p {
                data.extend_from_slice(&[x as i32 as u32, y as i32 as u32]);
            }
            let pw = params.len() as u32;
            data.extend(params);
            let (x0, x1) = (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.0).max().unwrap());
            let (y0, y1) = (p.iter().map(|q| q.1).min().unwrap(), p.iter().map(|q| q.1).max().unwrap());
            let reach = Rect::new((x0 >> 8) as i32 - 1, (y0 >> 8) as i32 - 1, ((x1 - x0) >> 8) as i32 + 3, ((y1 - y0) >> 8) as i32 + 3);
            self.push(OP_SHADED_TURNED, clip, reach, &data, opacity, &[S::KIND, 0, 0, 0, 0, 0, 0, pw]);
            return;
        }
        let (a, b) = (p[0], p[3]);
        let Some(span) = upright_span(clip, a, b) else { return };
        let (x0, x1, y0, y1) = span;
        let Some(params) = shader.params(self) else {
            self.refuse("a shader's texture with no room, or too many reads");
            return;
        };
        let (cols, rows) = shaded_grid(shader, a, b, span);
        let mut data = params;
        let pw = data.len();
        let mut one = Vec::new();
        let mut words = |put: &mut dyn FnMut(&mut Vec<u32>), data: &mut Vec<u32>| -> usize {
            one.clear();
            put(&mut one);
            data.extend_from_slice(&one);
            one.len()
        };
        let mut cw = None;
        for c in &cols {
            let n = words(&mut |o| shader.put_col(c, o), &mut data);
            if *cw.get_or_insert(n) != n {
                self.refuse("a shader's columns of different sizes");
                return;
            }
        }
        let mut rw = None;
        for r in &rows {
            let n = words(&mut |o| shader.put_row(r, o), &mut data);
            if *rw.get_or_insert(n) != n {
                self.refuse("a shader's rows of different sizes");
                return;
            }
        }
        let reach = Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32);
        self.push(OP_SHADED, clip, reach, &data, opacity,
                  &[S::KIND, x0 as u32, y0 as u32, cols.len() as u32, rows.len() as u32, cw.unwrap_or(0) as u32,
                    rw.unwrap_or(0) as u32, pw as u32]);
    }

    /// `Painter::mask`: glyph coverage in one color
    #[allow(clippy::too_many_arguments)]
    pub fn mask(&mut self, clip: Rect, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, key: u64, colour: u32, opacity: f32) {
        let Some(clip) = self.cut(clip) else { return };
        let reach = Rect::new(x, y, mw as i32, mh as i32);
        if reach.intersect(&clip).is_none() {
            return;
        }
        let Some(t) = self.mask_texture(key, mask, mw, mh) else {
            self.refuse("a glyph's mask with no room");
            return;
        };
        let src = pack(premultiply(colour, opacity));
        self.push(OP_MASK, clip, reach, &[], 1.0, &[t, x as u32, y as u32, src]);
    }

    /// `Painter::styled_mask`
    #[allow(clippy::too_many_arguments)]
    pub fn styled(&mut self, clip: Rect, x: i32, y: i32, mask: &[u8], mw: usize, mh: usize, key: u64, colour: u32,
                  style: TextStyle, style_colour: u32, opacity: f32) {
        let Some(clip) = self.cut(clip) else { return };
        let (l, t, r, b) = style.margins();
        let reach = Rect::new(x - l, y - t, mw as i32 + l + r, mh as i32 + t + b);
        if reach.intersect(&clip).is_none() {
            return;
        }
        let Some(tex) = self.mask_texture(key, mask, mw, mh) else {
            self.refuse("a glyph's mask with no room");
            return;
        };
        let (c, sc) = (pack(premultiply(colour, opacity)), pack(premultiply(style_colour, opacity)));
        self.push(OP_STYLED, clip, reach, &[], 1.0, &[tex, x as u32, y as u32, c, sc, style.code()]);
    }
}

/// A Mix's or a Gauss's count, kept to what `paint.cu` reads
pub fn levels_ok(n: usize) -> bool {
    n <= MOST_LEVELS
}

pub fn reads_ok(n: usize) -> bool {
    n <= MOST_READS
}
