//! A gpui scene drawn on the processor, into a Mayland buffer.
//!
//! MikuOS has no Vulkan and no GL, so wgpu has nothing to render with. A window
//! asks for eight kinds of primitive, each a formula over a pixel's position.
//! This is `gpui_wgpu`'s `shaders.wgsl` evaluated per pixel: the same signed
//! distances, the same blur integral, the same glyph gamma.
//!
//! A primitive covers the pixels whose centres fall inside the rectangle its
//! vertex stage would have emitted, and inside its content mask, as a
//! rasteriser decides it. Blending is the pipelines': straight alpha over an
//! opaque target, and premultiplied alpha for the path layer.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use gpui::{
    AtlasBackend, AtlasKey, AtlasState, AtlasTextureId, AtlasTextureKind, AtlasTile, Background,
    BackgroundTag, BorderStyle, Bounds, ColorSpace, Corners, DevicePixels, Hsla,
    MonochromeSprite, PaddedBool32, Path, PlatformAtlas, Point, PolychromeSprite, PrimitiveBatch,
    Quad, ScaledPixels, Scene, Shadow, Size, SubpixelSprite, TileId, Underline,
};
use parking_lot::Mutex;

type Rgba = [f32; 4];

// The atlas

/// A tile's pixels, as the text system and the image cache produced them:
/// one byte of coverage per pixel for glyphs, BGRA for everything in colour
struct Texture {
    width: usize,
    height: usize,
    bytes: Vec<u8>,
}

/// Every tile is a texture of its own. A GPU packs them to save binds; here a
/// tile owns its bytes and is freed by dropping them
#[derive(Default)]
pub(crate) struct CpuAtlasBackend {
    next_id: u32,
    textures: HashMap<u32, Texture>,
}

impl AtlasBackend for CpuAtlasBackend {
    fn insert(
        &mut self,
        kind: AtlasTextureKind,
        size: Size<DevicePixels>,
        bytes: &[u8],
    ) -> Result<AtlasTile> {
        self.next_id += 1;
        let id = self.next_id;
        self.textures.insert(
            id,
            Texture {
                width: size.width.0.max(0) as usize,
                height: size.height.0.max(0) as usize,
                bytes: bytes.to_vec(),
            },
        );
        Ok(AtlasTile {
            texture_id: AtlasTextureId { index: id, kind },
            tile_id: TileId(id),
            padding: 0,
            bounds: Bounds {
                origin: Point::default(),
                size,
            },
        })
    }

    fn remove(&mut self, tile: AtlasTile) {
        self.textures.remove(&tile.texture_id.index);
    }
}

#[derive(Default)]
pub(crate) struct CpuAtlas(Mutex<AtlasState<CpuAtlasBackend>>);

impl PlatformAtlas for CpuAtlas {
    fn get_or_insert_with<'a>(
        &self,
        key: AtlasKey,
        build: &mut dyn FnMut() -> Result<Option<(Size<DevicePixels>, Cow<'a, [u8]>)>>,
    ) -> Result<Option<AtlasTile>> {
        self.0.lock().get_or_insert_with(key, build)
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().remove(key);
    }
}

// The target

/// Pixels as Mayland's `FORMAT_XRGB8888` lays them out: little-endian words,
/// so blue, green, red and an ignored byte
pub(crate) struct Target<'a> {
    pub pixels: &'a mut [u8],
    pub width: usize,
    pub height: usize,
}

impl Target<'_> {
    fn clear(&mut self) {
        self.pixels.fill(0);
    }

    /// Straight alpha over what is there: the pipelines' `ALPHA_BLENDING`
    #[inline]
    fn blend(&mut self, x: usize, y: usize, color: Rgba) {
        let alpha = color[3].clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let i = (y * self.width + x) * 4;
        let Some(px) = self.pixels.get_mut(i..i + 4) else {
            return;
        };
        for (channel, source) in [(2usize, color[0]), (1, color[1]), (0, color[2])] {
            let below = px[channel] as f32 / 255.0;
            let out = source.clamp(0.0, 1.0) * alpha + below * (1.0 - alpha);
            px[channel] = (out * 255.0 + 0.5) as u8;
        }
    }

    /// Premultiplied colour over what is there: how the path layer lands
    #[inline]
    fn blend_premultiplied(&mut self, x: usize, y: usize, color: Rgba) {
        let alpha = color[3].clamp(0.0, 1.0);
        if alpha <= 0.0 {
            return;
        }
        let i = (y * self.width + x) * 4;
        let Some(px) = self.pixels.get_mut(i..i + 4) else {
            return;
        };
        for (channel, source) in [(2usize, color[0]), (1, color[1]), (0, color[2])] {
            let below = px[channel] as f32 / 255.0;
            let out = source + below * (1.0 - alpha);
            px[channel] = (out.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
        }
    }
}

/// The pixels whose centres are inside `lo..hi` and the clip, on one axis
fn span(lo: f32, hi: f32, clip_lo: f32, clip_hi: f32, limit: usize) -> (usize, usize) {
    let start = (lo.max(clip_lo) - 0.5).ceil().max(0.0);
    let end = (hi.min(clip_hi) - 0.5).ceil().min(limit as f32);
    if end <= start {
        (0, 0)
    } else {
        (start as usize, end as usize)
    }
}

#[derive(Clone, Copy)]
struct Rect {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

impl Rect {
    fn of(bounds: &Bounds<ScaledPixels>) -> Self {
        Rect {
            x: bounds.origin.x.0,
            y: bounds.origin.y.0,
            w: bounds.size.width.0,
            h: bounds.size.height.0,
        }
    }

    fn grow(&self, by: f32) -> Self {
        Rect {
            x: self.x - by,
            y: self.y - by,
            w: self.w + 2.0 * by,
            h: self.h + 2.0 * by,
        }
    }
}

/// Every pixel of `geometry` inside `clip`, as (x, y, centre x, centre y)
fn for_each_pixel(
    target: &mut Target,
    geometry: &Rect,
    clip: &Rect,
    mut f: impl FnMut(&mut Target, usize, usize, f32, f32),
) {
    let (x0, x1) = span(geometry.x, geometry.x + geometry.w, clip.x, clip.x + clip.w, target.width);
    let (y0, y1) = span(geometry.y, geometry.y + geometry.h, clip.y, clip.y + clip.h, target.height);
    for y in y0..y1 {
        for x in x0..x1 {
            f(target, x, y, x as f32 + 0.5, y as f32 + 0.5);
        }
    }
}

// Colour, as shaders.wgsl has it

fn saturate(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

fn hsla_to_rgba(c: Hsla) -> Rgba {
    let h = c.h * 6.0;
    let (s, l, a) = (c.s, c.l, c.a);
    let chroma = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = chroma * (1.0 - (h % 2.0 - 1.0).abs());
    let m = l - chroma / 2.0;
    let (r, g, b) = if (0.0..1.0).contains(&h) {
        (chroma, x, 0.0)
    } else if (1.0..2.0).contains(&h) {
        (x, chroma, 0.0)
    } else if (2.0..3.0).contains(&h) {
        (0.0, chroma, x)
    } else if (3.0..4.0).contains(&h) {
        (0.0, x, chroma)
    } else if (4.0..5.0).contains(&h) {
        (x, 0.0, chroma)
    } else {
        (chroma, 0.0, x)
    };
    [m + r, m + g, m + b, a]
}

fn srgb_to_linear(v: f32) -> f32 {
    if v < 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(v: f32) -> f32 {
    if v < 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

fn linear_srgb_to_oklab(c: Rgba) -> Rgba {
    let l = 0.4122214708 * c[0] + 0.5363325363 * c[1] + 0.0514459929 * c[2];
    let m = 0.2119034982 * c[0] + 0.6806995451 * c[1] + 0.1073969566 * c[2];
    let s = 0.0883024619 * c[0] + 0.2817188376 * c[1] + 0.6299787005 * c[2];
    let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
    [
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
        c[3],
    ]
}

fn oklab_to_linear_srgb(c: Rgba) -> Rgba {
    let l = c[0] + 0.3963377774 * c[1] + 0.2158037573 * c[2];
    let m = c[0] - 0.1055613458 * c[1] - 0.0638541728 * c[2];
    let s = c[0] - 0.0894841775 * c[1] - 1.2914855480 * c[2];
    let (l, m, s) = (l * l * l, m * m * m, s * s * s);
    [
        4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
        -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
        -0.0041960863 * l - 0.7034186147 * m + 1.7076147010 * s,
        c[3],
    ]
}

fn over(below: Rgba, above: Rgba) -> Rgba {
    let alpha = above[3] + below[3] * (1.0 - above[3]);
    if alpha <= 0.0 {
        return [0.0; 4];
    }
    let mut out = [0.0, 0.0, 0.0, alpha];
    for i in 0..3 {
        out[i] = (above[i] * above[3] + below[i] * below[3] * (1.0 - above[3])) / alpha;
    }
    out
}

/// A background's colours, worked out once per primitive, as the vertex stage
/// does
struct Fill {
    tag: BackgroundTag,
    color_space: ColorSpace,
    angle_or_pattern: f32,
    stops: [f32; 2],
    solid: Rgba,
    color0: Rgba,
    color1: Rgba,
}

impl Fill {
    fn of(background: &Background) -> Self {
        let (tag, color_space, solid, angle_or_pattern, stops) = background.parts();
        let mut fill = Fill {
            tag,
            color_space,
            angle_or_pattern,
            stops: [stops[0].percentage, stops[1].percentage],
            solid: [0.0; 4],
            color0: [0.0; 4],
            color1: [0.0; 4],
        };
        if tag == BackgroundTag::LinearGradient {
            let prepare = |c: Hsla| {
                let rgba = hsla_to_rgba(c);
                match color_space {
                    ColorSpace::Srgb => [
                        linear_to_srgb(rgba[0]),
                        linear_to_srgb(rgba[1]),
                        linear_to_srgb(rgba[2]),
                        rgba[3],
                    ],
                    ColorSpace::Oklab => linear_srgb_to_oklab(rgba),
                }
            };
            fill.color0 = prepare(stops[0].color);
            fill.color1 = prepare(stops[1].color);
        } else {
            fill.solid = hsla_to_rgba(solid);
        }
        fill
    }

    fn is_solid(&self) -> bool {
        self.tag == BackgroundTag::Solid
    }

    /// `gradient_color`
    fn at(&self, px: f32, py: f32, bounds: &Rect) -> Rgba {
        match self.tag {
            BackgroundTag::Solid => self.solid,
            BackgroundTag::LinearGradient => {
                let radians = (self.angle_or_pattern % 360.0 - 90.0).to_radians();
                let (mut dx, mut dy) = (radians.cos(), radians.sin());
                if bounds.w > bounds.h {
                    dy *= bounds.h / bounds.w;
                } else {
                    dx *= bounds.w / bounds.h;
                }
                let (hw, hh) = (bounds.w / 2.0, bounds.h / 2.0);
                let (cx, cy) = (px - (bounds.x + hw), py - (bounds.y + hh));
                let length = (dx * dx + dy * dy).sqrt();
                let mut t = (cx * dx + cy * dy) / length;
                t = if dx.abs() > dy.abs() {
                    (t + hw) / bounds.w
                } else {
                    (t + hh) / bounds.h
                };
                t = saturate((t - self.stops[0]) / (self.stops[1] - self.stops[0]));
                let mixed = mix(self.color0, self.color1, t);
                match self.color_space {
                    ColorSpace::Srgb => [
                        srgb_to_linear(mixed[0]),
                        srgb_to_linear(mixed[1]),
                        srgb_to_linear(mixed[2]),
                        mixed[3],
                    ],
                    ColorSpace::Oklab => oklab_to_linear_srgb(mixed),
                }
            }
            BackgroundTag::PatternSlash => {
                let packed = self.angle_or_pattern;
                let width = (packed / 65535.0) / 255.0;
                let interval = (packed % 65535.0) / 255.0;
                let height = width + interval;
                let angle = std::f32::consts::FRAC_PI_4;
                let period = height * angle.sin();
                let (rx, ry) = (px - bounds.x, py - bounds.y);
                // WGSL's `mat2x2(c, -s, s, c) * v` with its columns as given
                let rotated_x = angle.cos() * rx + angle.sin() * ry;
                let pattern = rotated_x % period;
                let distance = pattern.min(period - pattern) - period * (width / height) / 2.0;
                let mut color = self.solid;
                color[3] *= saturate(0.5 - distance);
                color
            }
            BackgroundTag::Checkerboard => {
                let size = self.angle_or_pattern;
                let xi = ((px - bounds.x) / size).floor();
                let yi = ((py - bounds.y) / size).floor();
                let colored = (xi + yi) % 2.0;
                let mut color = self.solid;
                color[3] *= saturate(colored);
                color
            }
        }
    }
}

// Signed distances

fn pick_corner_radius(cx: f32, cy: f32, radii: &Corners<ScaledPixels>) -> f32 {
    if cx < 0.0 {
        if cy < 0.0 {
            radii.top_left.0
        } else {
            radii.bottom_left.0
        }
    } else if cy < 0.0 {
        radii.top_right.0
    } else {
        radii.bottom_right.0
    }
}

fn quad_sdf_impl(ccx: f32, ccy: f32, corner_radius: f32) -> f32 {
    if corner_radius == 0.0 {
        ccx.max(ccy)
    } else {
        let outside = (ccx.max(0.0).powi(2) + ccy.max(0.0).powi(2)).sqrt();
        let inside = ccx.max(ccy).min(0.0);
        outside + inside - corner_radius
    }
}

fn quad_sdf(px: f32, py: f32, bounds: &Rect, radii: &Corners<ScaledPixels>) -> f32 {
    let (hw, hh) = (bounds.w / 2.0, bounds.h / 2.0);
    let (cx, cy) = (px - (bounds.x + hw), py - (bounds.y + hh));
    let r = pick_corner_radius(cx, cy, radii);
    quad_sdf_impl(cx.abs() - hw + r, cy.abs() - hh + r, r)
}

fn quarter_ellipse_sdf(px: f32, py: f32, rx: f32, ry: f32) -> f32 {
    let (ux, uy) = (px / rx, py / ry);
    let unit = (ux * ux + uy * uy).sqrt() - 1.0;
    unit * (rx + ry) * -0.5
}

fn fmod(a: f32, b: f32) -> f32 {
    a - b * (a / b).trunc()
}

fn corner_dash_velocity(dv1: f32, dv2: f32) -> f32 {
    if dv1 == 0.0 {
        dv2
    } else if dv2 == 0.0 {
        dv1
    } else {
        dv1.min(dv2)
    }
}

fn dash_alpha(t: f32, period: f32, length: f32, velocity: f32, threshold: f32) -> f32 {
    let half_period = period / 2.0;
    let half_length = length / 2.0;
    let centered = fmod(t + half_period - half_length, period) - half_period;
    let signed_distance = centered.abs() - half_length;
    saturate(threshold - signed_distance / velocity)
}

const THRESHOLD: f32 = 0.5;

/// `fs_quad`, for one pixel centre. Straight colour and its alpha
fn quad_fragment(quad: &Quad, fill: &Fill, border_color: Rgba, px: f32, py: f32) -> Rgba {
    let bounds = Rect::of(&quad.bounds);
    let background = fill.at(px, py, &bounds);
    let radii = &quad.corner_radii;
    let widths = &quad.border_widths;
    let unrounded = radii.top_left.0 == 0.0
        && radii.bottom_left.0 == 0.0
        && radii.top_right.0 == 0.0
        && radii.bottom_right.0 == 0.0;
    if widths.top.0 == 0.0
        && widths.left.0 == 0.0
        && widths.right.0 == 0.0
        && widths.bottom.0 == 0.0
        && unrounded
    {
        return background;
    }

    let (w, h) = (bounds.w, bounds.h);
    let (hw, hh) = (w / 2.0, h / 2.0);
    let (point_x, point_y) = (px - bounds.x, py - bounds.y);
    let (cx, cy) = (point_x - hw, point_y - hh);
    let corner_radius = pick_corner_radius(cx, cy, radii);
    let border_x = if cx < 0.0 { widths.left.0 } else { widths.right.0 };
    let border_y = if cy < 0.0 { widths.top.0 } else { widths.bottom.0 };
    let reduced_x = if border_x == 0.0 { -THRESHOLD } else { border_x };
    let reduced_y = if border_y == 0.0 { -THRESHOLD } else { border_y };
    let (ctx, cty) = (cx.abs() - hw, cy.abs() - hh);
    let (ccx, ccy) = (ctx + corner_radius, cty + corner_radius);
    let near_rounded = ccx >= 0.0 && ccy >= 0.0;
    let (six, siy) = (ctx + reduced_x, cty + reduced_y);
    let beyond_inner_straight = six > 0.0 || siy > 0.0;
    let within_inner_straight = six < -THRESHOLD && siy < -THRESHOLD;
    if within_inner_straight && !near_rounded {
        return background;
    }

    let outer_sdf = quad_sdf_impl(ccx, ccy, corner_radius);
    let inner_sdf = if ccx <= 0.0 || ccy <= 0.0 {
        -six.max(siy)
    } else if beyond_inner_straight {
        -1.0
    } else if reduced_x == reduced_y {
        -(outer_sdf + reduced_x)
    } else {
        quarter_ellipse_sdf(
            ccx,
            ccy,
            (corner_radius - reduced_x).max(0.0),
            (corner_radius - reduced_y).max(0.0),
        )
    };
    let border_sdf = inner_sdf.max(outer_sdf);

    let mut color = background;
    if border_sdf < THRESHOLD {
        let mut border = border_color;
        if quad.border_style == BorderStyle::Dashed {
            border[3] *= dash_factor(
                quad,
                unrounded,
                near_rounded,
                (point_x, point_y),
                (cx, cy),
                (ccx, ccy),
                corner_radius,
            );
        }
        let blended = over(background, border);
        color = mix(background, blended, saturate(THRESHOLD - inner_sdf));
    }
    color[3] *= saturate(THRESHOLD - outer_sdf);
    color
}

/// The dashed-border arm of `fs_quad`: how much of a dash this pixel is in
fn dash_factor(
    quad: &Quad,
    unrounded: bool,
    near_rounded: bool,
    (point_x, point_y): (f32, f32),
    (cx, cy): (f32, f32),
    (ccx, ccy): (f32, f32),
    corner_radius: f32,
) -> f32 {
    let (w, h) = (quad.bounds.size.width.0, quad.bounds.size.height.0);
    let widths = &quad.border_widths;
    let radii = &quad.corner_radii;
    let dash_length_per_width = 2.0;
    let dash_gap_per_width = 1.0;
    let dash_period_per_width = dash_length_per_width + dash_gap_per_width;
    let dv_numerator = 1.0 / dash_period_per_width;

    let (t, mut max_t, velocity);
    if unrounded {
        let horizontal = ccx < ccy;
        let dashed_x = widths.bottom.0.max(widths.top.0);
        let dashed_y = widths.right.0.max(widths.left.0);
        let border_width = if horizontal { dashed_x } else { dashed_y };
        velocity = dv_numerator / border_width;
        t = if horizontal { point_x } else { point_y } * velocity;
        max_t = if horizontal { w } else { h } * velocity;
    } else {
        let (r_tr, r_br, r_bl, r_tl) = (
            radii.top_right.0,
            radii.bottom_right.0,
            radii.bottom_left.0,
            radii.top_left.0,
        );
        let side = |width: f32| if width <= 0.0 { 0.0 } else { dv_numerator / width };
        let (dv_t, dv_r, dv_b, dv_l) = (
            side(widths.top.0),
            side(widths.right.0),
            side(widths.bottom.0),
            side(widths.left.0),
        );
        let s_t = (w - r_tl - r_tr) * dv_t;
        let s_r = (h - r_tr - r_br) * dv_r;
        let s_b = (w - r_br - r_bl) * dv_b;
        let s_l = (h - r_bl - r_tl) * dv_l;
        let cdv_tr = corner_dash_velocity(dv_t, dv_r);
        let cdv_br = corner_dash_velocity(dv_b, dv_r);
        let cdv_bl = corner_dash_velocity(dv_b, dv_l);
        let cdv_tl = corner_dash_velocity(dv_t, dv_l);
        let quarter = std::f32::consts::FRAC_PI_2;
        let c_tr = r_tr * quarter * cdv_tr;
        let c_br = r_br * quarter * cdv_br;
        let c_bl = r_bl * quarter * cdv_bl;
        let c_tl = r_tl * quarter * cdv_tl;
        let upto_tr = s_t;
        let upto_r = upto_tr + c_tr;
        let upto_br = upto_r + s_r;
        let upto_b = upto_br + c_br;
        let upto_bl = upto_b + s_b;
        let upto_l = upto_bl + c_bl;
        let upto_tl = upto_l + s_l;
        max_t = upto_tl + c_tl;
        if near_rounded {
            let corner_t = ccy.atan2(ccx) * corner_radius;
            (t, velocity) = if cx >= 0.0 {
                if cy < 0.0 {
                    (upto_r - corner_t * cdv_tr, cdv_tr)
                } else {
                    (upto_br + corner_t * cdv_br, cdv_br)
                }
            } else if cy >= 0.0 {
                (upto_l - corner_t * cdv_bl, cdv_bl)
            } else {
                (upto_tl + corner_t * cdv_tl, cdv_tl)
            };
        } else if ccx < ccy {
            (t, velocity) = if cy < 0.0 {
                ((point_x - r_tl) * dv_t, dv_t)
            } else {
                (upto_bl - (point_x - r_bl) * dv_b, dv_b)
            };
        } else {
            (t, velocity) = if cx < 0.0 {
                (upto_tl - (point_y - r_tl) * dv_l, dv_l)
            } else {
                (upto_r + (point_y - r_tr) * dv_r, dv_r)
            };
        }
    }

    let dash_length = dash_length_per_width / dash_period_per_width;
    if unrounded {
        max_t -= dash_length;
    }
    if max_t >= 1.0 {
        let period = max_t / max_t.floor();
        dash_alpha(t, period, dash_length, velocity, THRESHOLD)
    } else if unrounded {
        let gap = max_t - dash_length;
        if gap > 0.0 {
            dash_alpha(t, dash_length + gap, dash_length, velocity, THRESHOLD)
        } else {
            1.0
        }
    } else {
        1.0
    }
}

// The gaussian

fn gaussian(x: f32, sigma: f32) -> f32 {
    (-(x * x) / (2.0 * sigma * sigma)).exp() / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
}

fn erf(v: f32) -> f32 {
    let s = v.signum();
    let a = v.abs();
    let r1 = 1.0 + (0.278393 + (0.230389 + (0.000972 + 0.078108 * a) * a) * a) * a;
    let r2 = r1 * r1;
    s - s / (r2 * r2)
}

fn blur_along_x(x: f32, y: f32, sigma: f32, corner: f32, hw: f32, hh: f32) -> f32 {
    let delta = (hh - corner - y.abs()).min(0.0);
    let curved = hw - corner + (corner * corner - delta * delta).max(0.0).sqrt();
    let k = std::f32::consts::FRAC_1_SQRT_2 / sigma;
    let low = 0.5 + 0.5 * erf((x - curved) * k);
    let high = 0.5 + 0.5 * erf((x + curved) * k);
    high - low
}

/// `fs_shadow`, for one pixel centre: the shadow colour's alpha factor
fn shadow_alpha(shadow: &Shadow, px: f32, py: f32) -> f32 {
    let bounds = Rect::of(&shadow.bounds);
    let (hw, hh) = (bounds.w / 2.0, bounds.h / 2.0);
    let (cx, cy) = (px - (bounds.x + hw), py - (bounds.y + hh));
    let corner = pick_corner_radius(cx, cy, &shadow.corner_radii);
    let sigma = shadow.blur_radius.0;
    let mut alpha = if sigma == 0.0 {
        saturate(0.5 - quad_sdf(px, py, &bounds, &shadow.corner_radii))
    } else {
        let low = cy - hh;
        let high = cy + hh;
        let start = (-3.0 * sigma).clamp(low, high);
        let end = (3.0 * sigma).clamp(low, high);
        let step = (end - start) / 4.0;
        let mut y = start + step * 0.5;
        let mut sum = 0.0;
        for _ in 0..4 {
            sum += blur_along_x(cx, cy - y, sigma, corner, hw, hh) * gaussian(y, sigma) * step;
            y += step;
        }
        sum
    };
    if shadow.inset != 0 {
        alpha = 1.0 - alpha;
        let element = Rect::of(&shadow.element_bounds);
        alpha *= saturate(0.5 - quad_sdf(px, py, &element, &shadow.element_corner_radii));
    }
    alpha
}

// Glyph gamma, as the text pipeline has it

fn brightness(c: Rgba) -> f32 {
    c[0] * 0.30 + c[1] * 0.59 + c[2] * 0.11
}

fn corrected_coverage(sample: f32, color: Rgba, contrast: f32, g: &[f32; 4]) -> f32 {
    let b = brightness(color);
    let k = contrast * saturate(4.0 * (0.75 - b));
    let contrasted = sample * (k + 1.0) / (sample * k + 1.0);
    let adjustment = g[0] * b + g[1];
    let correction = adjustment * contrasted + (g[2] * b + g[3]);
    contrasted + contrasted * (1.0 - contrasted) * correction
}

/// A texel, filtered the way the sprite sampler filters: linearly, clamped
/// to the texture's edge. `u`, `v` are in texels with centres on the halves
fn sample(texture: &Texture, bytes_per_pixel: usize, u: f32, v: f32, channel: usize) -> f32 {
    if texture.width == 0 || texture.height == 0 {
        return 0.0;
    }
    let u = (u - 0.5).clamp(0.0, (texture.width - 1) as f32);
    let v = (v - 0.5).clamp(0.0, (texture.height - 1) as f32);
    let (x0, y0) = (u.floor() as usize, v.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(texture.width - 1), (y0 + 1).min(texture.height - 1));
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let at = |x: usize, y: usize| {
        texture
            .bytes
            .get((y * texture.width + x) * bytes_per_pixel + channel)
            .copied()
            .unwrap_or(0) as f32
            / 255.0
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x1, y0) * fx;
    let bottom = at(x0, y1) * (1.0 - fx) + at(x1, y1) * fx;
    top * (1.0 - fy) + bottom * fy
}

// The renderer

pub(crate) struct CpuRenderer {
    atlas: Arc<CpuAtlas>,
    gamma_ratios: [f32; 4],
    grayscale_enhanced_contrast: f32,
    /// The path batch's intermediate layer, premultiplied, reused across
    /// frames so a scene full of paths does not allocate per batch
    layer: Vec<Rgba>,
}

impl CpuRenderer {
    pub(crate) fn new() -> Self {
        // The same knobs, with the same defaults, that `gpui_wgpu` reads
        let gamma = std::env::var("ZED_FONTS_GAMMA")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.8_f32)
            .clamp(1.0, 2.2);
        let grayscale_enhanced_contrast = std::env::var("ZED_FONTS_GRAYSCALE_ENHANCED_CONTRAST")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0_f32)
            .max(0.0);
        CpuRenderer {
            atlas: Arc::new(CpuAtlas::default()),
            gamma_ratios: gpui::get_gamma_correction_ratios(gamma),
            grayscale_enhanced_contrast,
            layer: Vec::new(),
        }
    }

    pub(crate) fn sprite_atlas(&self) -> Arc<CpuAtlas> {
        self.atlas.clone()
    }

    pub(crate) fn draw(&mut self, scene: &Scene, target: &mut Target) {
        target.clear();
        for batch in scene.batches() {
            match batch {
                PrimitiveBatch::Quads(range) => {
                    for quad in &scene.quads[range] {
                        draw_quad(target, quad);
                    }
                }
                PrimitiveBatch::Shadows(range) => {
                    for shadow in &scene.shadows[range] {
                        draw_shadow(target, shadow);
                    }
                }
                PrimitiveBatch::Paths(range) => self.draw_paths(target, &scene.paths[range]),
                PrimitiveBatch::Underlines(range) => {
                    for underline in &scene.underlines[range] {
                        draw_underline(target, underline);
                    }
                }
                PrimitiveBatch::MonochromeSprites { range, .. } => {
                    let atlas = self.atlas.0.lock();
                    for sprite in &scene.monochrome_sprites[range] {
                        if let Some(texture) = atlas.backend.textures.get(&sprite.tile.texture_id.index) {
                            self.draw_mono(target, sprite, texture);
                        }
                    }
                }
                PrimitiveBatch::SubpixelSprites { range, .. } => {
                    let atlas = self.atlas.0.lock();
                    for sprite in &scene.subpixel_sprites[range] {
                        if let Some(texture) = atlas.backend.textures.get(&sprite.tile.texture_id.index) {
                            self.draw_subpixel(target, sprite, texture);
                        }
                    }
                }
                PrimitiveBatch::PolychromeSprites { range, .. } => {
                    let atlas = self.atlas.0.lock();
                    for sprite in &scene.polychrome_sprites[range] {
                        if let Some(texture) = atlas.backend.textures.get(&sprite.tile.texture_id.index) {
                            draw_poly(target, sprite, texture);
                        }
                    }
                }
                // Video frames from the system's decoder: macOS only
                #[allow(unreachable_patterns)]
                _ => {}
            }
        }
    }

    fn draw_mono(&self, target: &mut Target, sprite: &MonochromeSprite, texture: &Texture) {
        let color = hsla_to_rgba(sprite.color);
        let clip = Rect::of(&sprite.content_mask.bounds);
        let bounds = Rect::of(&sprite.bounds);
        let tile = &sprite.tile.bounds;
        let (tile_x, tile_y) = (tile.origin.x.0 as f32, tile.origin.y.0 as f32);
        let (tile_w, tile_h) = (tile.size.width.0 as f32, tile.size.height.0 as f32);
        let m = sprite.transformation.rotation_scale;
        let t = sprite.transformation.translation;
        let identity = m == [[1.0, 0.0], [0.0, 1.0]] && t == [0.0, 0.0];

        // Where each pixel centre is on the unit square of the sprite: the
        // inverse of `to_device_position_transformed`
        let determinant = m[0][0] * m[1][1] - m[0][1] * m[1][0];
        if determinant == 0.0 {
            return;
        }
        let geometry = if identity {
            bounds
        } else {
            let corners = [
                (bounds.x, bounds.y),
                (bounds.x + bounds.w, bounds.y),
                (bounds.x, bounds.y + bounds.h),
                (bounds.x + bounds.w, bounds.y + bounds.h),
            ]
            .map(|(x, y)| (m[0][0] * x + m[0][1] * y + t[0], m[1][0] * x + m[1][1] * y + t[1]));
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for (x, y) in corners {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            Rect { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
        };
        let contrast = self.grayscale_enhanced_contrast;
        let gamma = self.gamma_ratios;
        for_each_pixel(target, &geometry, &clip, |target, x, y, px, py| {
            let (sx, sy) = if identity {
                (px, py)
            } else {
                let (dx, dy) = (px - t[0], py - t[1]);
                (
                    (m[1][1] * dx - m[0][1] * dy) / determinant,
                    (-m[1][0] * dx + m[0][0] * dy) / determinant,
                )
            };
            let (ux, uy) = ((sx - bounds.x) / bounds.w, (sy - bounds.y) / bounds.h);
            if !(0.0..1.0).contains(&ux) || !(0.0..1.0).contains(&uy) {
                return;
            }
            let coverage = sample(texture, 1, tile_x + ux * tile_w, tile_y + uy * tile_h, 0);
            if coverage <= 0.0 {
                return;
            }
            let alpha = corrected_coverage(coverage, color, contrast, &gamma);
            let mut out = color;
            out[3] *= alpha;
            target.blend(x, y, out);
        });
    }

    /// Not asked for (`is_subpixel_rendering_supported` says no), but a tile
    /// made that way still draws, as grey coverage
    fn draw_subpixel(&self, target: &mut Target, sprite: &SubpixelSprite, texture: &Texture) {
        let color = hsla_to_rgba(sprite.color);
        let clip = Rect::of(&sprite.content_mask.bounds);
        let bounds = Rect::of(&sprite.bounds);
        let tile = &sprite.tile.bounds;
        let (tile_x, tile_y) = (tile.origin.x.0 as f32, tile.origin.y.0 as f32);
        let (tile_w, tile_h) = (tile.size.width.0 as f32, tile.size.height.0 as f32);
        for_each_pixel(target, &bounds, &clip, |target, x, y, px, py| {
            let (ux, uy) = ((px - bounds.x) / bounds.w, (py - bounds.y) / bounds.h);
            let (u, v) = (tile_x + ux * tile_w, tile_y + uy * tile_h);
            let coverage = (sample(texture, 4, u, v, 0)
                + sample(texture, 4, u, v, 1)
                + sample(texture, 4, u, v, 2))
                / 3.0;
            let mut out = color;
            out[3] *= coverage;
            target.blend(x, y, out);
        });
    }

    /// `fs_path_rasterization` into a layer, then `fs_path` onto the frame
    fn draw_paths(&mut self, target: &mut Target, paths: &[Path<ScaledPixels>]) {
        let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0usize, 0usize);
        for path in paths {
            let bounds = Rect::of(&path.clipped_bounds());
            let (a, b) = span(bounds.x, bounds.x + bounds.w, f32::MIN, f32::MAX, target.width);
            let (c, d) = span(bounds.y, bounds.y + bounds.h, f32::MIN, f32::MAX, target.height);
            if b > a && d > c {
                x0 = x0.min(a);
                x1 = x1.max(b);
                y0 = y0.min(c);
                y1 = y1.max(d);
            }
        }
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let (lw, lh) = (x1 - x0, y1 - y0);
        self.layer.clear();
        self.layer.resize(lw * lh, [0.0; 4]);

        // The four sample positions of 4x MSAA, which is what the path
        // pipeline asks for: the fragment is shaded once at the centre and
        // lands on the samples its triangle covers
        const SAMPLES: [(f32, f32); 4] = [(0.375, 0.125), (0.875, 0.375), (0.125, 0.625), (0.625, 0.875)];

        for path in paths {
            let clip = Rect::of(&path.clipped_bounds());
            let fill = Fill::of(&path.color);
            for triangle in path.vertices.chunks_exact(3) {
                let p = [0, 1, 2].map(|i| (triangle[i].xy_position.x.0, triangle[i].xy_position.y.0));
                let st = [0, 1, 2].map(|i| (triangle[i].st_position.x, triangle[i].st_position.y));
                let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
                if area == 0.0 {
                    continue;
                }
                // st is affine over the triangle, so its derivatives, which
                // `dpdx`/`dpdy` give the shader, are constants
                let (e1x, e1y) = (p[1].0 - p[0].0, p[1].1 - p[0].1);
                let (e2x, e2y) = (p[2].0 - p[0].0, p[2].1 - p[0].1);
                let l1_dx = e2y / area;
                let l1_dy = -e2x / area;
                let l2_dx = -e1y / area;
                let l2_dy = e1x / area;
                let ds_dx = (st[1].0 - st[0].0) * l1_dx + (st[2].0 - st[0].0) * l2_dx;
                let ds_dy = (st[1].0 - st[0].0) * l1_dy + (st[2].0 - st[0].0) * l2_dy;
                let dt_dx = (st[1].1 - st[0].1) * l1_dx + (st[2].1 - st[0].1) * l2_dx;
                let dt_dy = (st[1].1 - st[0].1) * l1_dy + (st[2].1 - st[0].1) * l2_dy;
                let flat = (ds_dx * ds_dx + ds_dy * ds_dy).sqrt() < 0.001;

                let inside = |x: f32, y: f32| {
                    let edge = |a: (f32, f32), b: (f32, f32)| {
                        ((b.0 - a.0) * (y - a.1) - (b.1 - a.1) * (x - a.0)) * area.signum()
                    };
                    edge(p[0], p[1]) >= 0.0 && edge(p[1], p[2]) >= 0.0 && edge(p[2], p[0]) >= 0.0
                };
                let (mut tx0, mut ty0, mut tx1, mut ty1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
                for &(x, y) in &p {
                    tx0 = tx0.min(x);
                    ty0 = ty0.min(y);
                    tx1 = tx1.max(x);
                    ty1 = ty1.max(y);
                }
                let (ax, bx) = (tx0.floor().max(x0 as f32) as usize, (tx1.ceil().min(x1 as f32)) as usize);
                let (ay, by) = (ty0.floor().max(y0 as f32) as usize, (ty1.ceil().min(y1 as f32)) as usize);
                for y in ay..by {
                    for x in ax..bx {
                        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
                        // The clip is tested at the centre, as the shader
                        // tests its clip distances there
                        if px < clip.x || px > clip.x + clip.w || py < clip.y || py > clip.y + clip.h {
                            continue;
                        }
                        let covered = SAMPLES
                            .iter()
                            .filter(|(sx, sy)| inside(x as f32 + sx, y as f32 + sy))
                            .count();
                        if covered == 0 {
                            continue;
                        }
                        let alpha = if flat {
                            1.0
                        } else {
                            let (dx, dy) = (px - p[0].0, py - p[0].1);
                            let l1 = l1_dx * dx + l1_dy * dy;
                            let l2 = l2_dx * dx + l2_dy * dy;
                            let s = st[0].0 + (st[1].0 - st[0].0) * l1 + (st[2].0 - st[0].0) * l2;
                            let t = st[0].1 + (st[1].1 - st[0].1) * l1 + (st[2].1 - st[0].1) * l2;
                            let gx = 2.0 * s * ds_dx - dt_dx;
                            let gy = 2.0 * s * ds_dy - dt_dy;
                            let f = s * s - t;
                            saturate(0.5 - f / (gx * gx + gy * gy).sqrt())
                        };
                        let color = fill.at(px, py, &clip);
                        let a = color[3] * alpha * covered as f32 / 4.0;
                        let source = [color[0] * a, color[1] * a, color[2] * a, a];
                        let slot = &mut self.layer[(y - y0) * lw + (x - x0)];
                        for i in 0..4 {
                            slot[i] = source[i] + slot[i] * (1.0 - a);
                        }
                    }
                }
            }
        }

        for y in 0..lh {
            for x in 0..lw {
                let color = self.layer[y * lw + x];
                if color[3] > 0.0 {
                    target.blend_premultiplied(x0 + x, y0 + y, color);
                }
            }
        }
    }
}

fn draw_quad(target: &mut Target, quad: &Quad) {
    let bounds = Rect::of(&quad.bounds);
    let clip = Rect::of(&quad.content_mask.bounds);
    let fill = Fill::of(&quad.background);
    let border = hsla_to_rgba(quad.border_color);
    let plain = fill.is_solid()
        && quad.corner_radii == Corners::default()
        && quad.border_widths.top.0 == 0.0
        && quad.border_widths.right.0 == 0.0
        && quad.border_widths.bottom.0 == 0.0
        && quad.border_widths.left.0 == 0.0;
    if plain {
        // Most of a frame: a flat rectangle, one colour for every pixel
        let color = fill.solid;
        for_each_pixel(target, &bounds, &clip, |target, x, y, _, _| target.blend(x, y, color));
        return;
    }
    for_each_pixel(target, &bounds, &clip, |target, x, y, px, py| {
        target.blend(x, y, quad_fragment(quad, &fill, border, px, py))
    });
}

fn draw_shadow(target: &mut Target, shadow: &Shadow) {
    let geometry = if shadow.inset != 0 {
        Rect::of(&shadow.element_bounds)
    } else {
        Rect::of(&shadow.bounds).grow(3.0 * shadow.blur_radius.0)
    };
    let clip = Rect::of(&shadow.content_mask.bounds);
    let color = hsla_to_rgba(shadow.color);
    for_each_pixel(target, &geometry, &clip, |target, x, y, px, py| {
        let mut out = color;
        out[3] *= shadow_alpha(shadow, px, py);
        target.blend(x, y, out);
    });
}

fn draw_underline(target: &mut Target, underline: &Underline) {
    const WAVE_FREQUENCY: f32 = 2.0;
    const WAVE_HEIGHT_RATIO: f32 = 0.8;
    let bounds = Rect::of(&underline.bounds);
    let clip = Rect::of(&underline.content_mask.bounds);
    let color = hsla_to_rgba(underline.color);
    let wavy = underline.wavy == PaddedBool32::from(true);
    let thickness = underline.thickness.0;
    for_each_pixel(target, &bounds, &clip, |target, x, y, px, py| {
        let mut out = color;
        if wavy && bounds.h > 0.0 {
            let half = thickness * 0.5;
            let st_x = (px - bounds.x) / bounds.h;
            let st_y = (py - bounds.y) / bounds.h - 0.5;
            let frequency = std::f32::consts::PI * WAVE_FREQUENCY * thickness / bounds.h;
            let amplitude = thickness * WAVE_HEIGHT_RATIO / bounds.h;
            let sine = (st_x * frequency).sin() * amplitude;
            let slope = (st_x * frequency).cos() * amplitude * frequency;
            let distance = (st_y - sine) / (1.0 + slope * slope).sqrt() * bounds.h;
            let alpha = saturate(0.5 - (-(distance + half)).max(distance - half));
            out[3] *= alpha;
        }
        target.blend(x, y, out);
    });
}

fn draw_poly(target: &mut Target, sprite: &PolychromeSprite, texture: &Texture) {
    let bounds = Rect::of(&sprite.bounds);
    let clip = Rect::of(&sprite.content_mask.bounds);
    let tile = &sprite.tile.bounds;
    let (tile_x, tile_y) = (tile.origin.x.0 as f32, tile.origin.y.0 as f32);
    let (tile_w, tile_h) = (tile.size.width.0 as f32, tile.size.height.0 as f32);
    let grayscale = sprite.grayscale == PaddedBool32::from(true);
    let rounded = sprite.corner_radii != Corners::default();
    for_each_pixel(target, &bounds, &clip, |target, x, y, px, py| {
        let (ux, uy) = ((px - bounds.x) / bounds.w, (py - bounds.y) / bounds.h);
        let (u, v) = (tile_x + ux * tile_w, tile_y + uy * tile_h);
        // BGRA in the atlas
        let mut color = [
            sample(texture, 4, u, v, 2),
            sample(texture, 4, u, v, 1),
            sample(texture, 4, u, v, 0),
            sample(texture, 4, u, v, 3),
        ];
        if grayscale {
            let g = color[0] * 0.2126 + color[1] * 0.7152 + color[2] * 0.0722;
            color = [g, g, g, color[3]];
        }
        let edge = if rounded {
            saturate(0.5 - quad_sdf(px, py, &bounds, &sprite.corner_radii))
        } else {
            1.0
        };
        color[3] *= sprite.opacity * edge;
        target.blend(x, y, color);
    });
}
