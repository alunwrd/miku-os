// Drawing into a pixel buffer: rectangles, rounded rectangles, lines, pictures
//
// Shared by `#[path]` between the kernel and clients, like `text.rs` and
// `protocol.rs`.
//
// The buffer is XRGB8888, four bytes a pixel, blue first in memory, so a color
// is a plain `0xRRGGBB`. No allocation and no floats. Curves are integer
// distance compares, sampled 4x4 at the edge.

/// A rectangle in buffer pixels. Half-open: `x..x+w`, `y..y+h`
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    pub const fn right(&self) -> i32 {
        self.x + self.w
    }

    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }

    pub fn contains(&self, px: i32, py: i32) -> bool {
        px >= self.x && px < self.right() && py >= self.y && py < self.bottom()
    }

    /// The same rectangle, pulled in by `d` on every side
    pub const fn inset(&self, d: i32) -> Self {
        Self { x: self.x + d, y: self.y + d, w: self.w - 2 * d, h: self.h - 2 * d }
    }

    /// The part of this rectangle inside `other`, or `None` if they miss
    pub fn intersect(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        if self.w <= 0 || self.h <= 0 { return *o; }
        if o.w <= 0 || o.h <= 0 { return *self; }
        let x = if self.x < o.x { self.x } else { o.x };
        let y = if self.y < o.y { self.y } else { o.y };
        let r = if self.right() > o.right() { self.right() } else { o.right() };
        let b = if self.bottom() > o.bottom() { self.bottom() } else { o.bottom() };
        Rect { x, y, w: r - x, h: b - y }
    }

    pub const fn centred(&self, w: i32, h: i32) -> Self {
        Self { x: self.x + (self.w - w) / 2, y: self.y + (self.h - h) / 2, w, h }
    }
}

/// A pixel buffer with its bounds
pub struct Canvas<'a> {
    pub px: &'a mut [u8],
    /// In pixels, not bytes
    pub stride: usize,
    pub w: i32,
    pub h: i32,
}

/// Blends one coverage value between two colors.
///
/// An edge pixel is part ink and part what is behind it. Per channel, rounding
/// with `+128` so coverage 255 lands exactly on the foreground.
///
/// All four channels, `0xAARRGGBB`, since a canvas may be a surface the
/// compositor blends, where the alpha byte is a channel.
///
/// Colors carry straight alpha, as this protocol's buffers do (see
/// `input::pointer::blend`). A per-channel lerp is exact when the alphas are
/// equal. When they differ and both are nonzero, the mix is premultiply, lerp,
/// and divide by the resulting alpha, as with a terminal background at 85%
/// opacity (`dev/mayland-window-shape.md`). Zero alpha often means "not given"
/// here, as in `0x00RRGGBB` constants drawn on opaque canvases, so it keeps the
/// simple path
#[inline]
pub fn blend(bg: u32, fg: u32, cov: u8) -> u32 {
    if cov == 0 {
        return bg;
    }
    if cov == 255 {
        return fg;
    }
    let (a, ia) = (cov as u32, 255 - cov as u32);
    let (ab, af) = (bg >> 24, fg >> 24);
    if ab != af && ab != 0 && af != 0 {
        // Alpha in 255² units and each channel premultiplied in 255³; the
        // quotient is the straight color, rounded to nearest
        let total = af * a + ab * ia;
        if total == 0 {
            return bg;
        }
        let mut out = div255(total) << 24;
        for shift in [0, 8, 16] {
            let b = (bg >> shift) & 0xff;
            let f = (fg >> shift) & 0xff;
            let premul = f * af * a + b * ab * ia;
            out |= ((premul + total / 2) / total).min(255) << shift;
        }
        return out;
    }
    let mut out = 0u32;
    for shift in [0, 8, 16, 24] {
        let b = (bg >> shift) & 0xff;
        let f = (fg >> shift) & 0xff;
        out |= div255(f * a + b * ia) << shift;
    }
    out
}

/// Source-over: a translucent color on an opaque one, by the source's alpha.
/// `blend` mixes by coverage instead.
///
/// The result is declared opaque, since the bottom of every stack is the
/// wallpaper. Straight alpha, as in Mayland's `shm` buffers. Used by the cursor
/// and any surface committed with an alpha channel
#[inline]
pub fn over(dst: u32, src: u32, a: u32) -> u32 {
    match a {
        0 => dst,
        255 => src | 0xFF00_0000,
        _ => {
            let inv = 255 - a;
            let mut out = 0xFF00_0000u32;
            for shift in [0, 8, 16] {
                let s = (src >> shift) & 0xff;
                let d = (dst >> shift) & 0xff;
                out |= div255(s * a + d * inv) << shift;
            }
            out
        }
    }
}

/// `(v + 128) / 255` without the division, four per antialiased pixel. Exact
/// over the possible range (`v` is at most 255 * 255, the two coverages summing
/// to 255)
#[inline]
fn div255(v: u32) -> u32 {
    let t = v + 128;
    (t + (t >> 8) + 1) >> 8
}

impl<'a> Canvas<'a> {
    /// Wraps a buffer. `stride` is in pixels, not bytes: a row is `stride * 4`
    /// bytes apart, so a byte pitch passes `pitch / 4`. `at()` returns `None`
    /// past the end, so a byte stride would draw the top quarter silently
    pub fn new(px: &'a mut [u8], stride: usize, w: i32, h: i32) -> Self {
        Self { px, stride, w, h }
    }

    #[inline]
    fn at(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.w || y >= self.h {
            return None;
        }
        let i = (y as usize * self.stride + x as usize) * 4;
        if i + 4 <= self.px.len() {
            Some(i)
        } else {
            None
        }
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> u32 {
        match self.at(x, y) {
            Some(i) => u32::from_le_bytes([
                self.px[i],
                self.px[i + 1],
                self.px[i + 2],
                self.px[i + 3],
            ]),
            None => 0,
        }
    }

    #[inline]
    pub fn put(&mut self, x: i32, y: i32, colour: u32) {
        if let Some(i) = self.at(x, y) {
            self.px[i..i + 4].copy_from_slice(&colour.to_le_bytes());
        }
    }

    /// Put `colour` down with `cov` of it, over what is already there
    #[inline]
    pub fn blend_px(&mut self, x: i32, y: i32, colour: u32, cov: u8) {
        if cov == 0 {
            return;
        }
        if let Some(i) = self.at(x, y) {
            let under =
                u32::from_le_bytes([self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]);
            let v = blend(under, colour, cov);
            self.px[i..i + 4].copy_from_slice(&v.to_le_bytes());
        }
    }

    pub fn clear(&mut self, colour: u32) {
        let r = Rect::new(0, 0, self.w, self.h);
        self.fill_rect(r, colour);
    }

    /// A rectangle clipped to the canvas, as half-open pixel bounds, decided
    /// once per rectangle
    #[inline]
    fn clip(&self, r: Rect) -> Option<(i32, i32, i32, i32)> {
        let x0 = r.x.max(0);
        let y0 = r.y.max(0);
        let x1 = r.right().min(self.w);
        let y1 = r.bottom().min(self.h);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        Some((x0, y0, x1, y1))
    }

    /// One row of a clipped span: the bytes to write and how many pixels fell
    /// off the left edge. For a caller with a value per pixel, such as a
    /// glyph's coverage row; `skip` advances its source
    #[inline]
    pub fn row_span(&mut self, y: i32, x: i32, w: i32) -> Option<(usize, &mut [u8])> {
        if y < 0 || y >= self.h || w <= 0 {
            return None;
        }
        let x0 = x.max(0);
        let x1 = (x + w).min(self.w);
        if x1 <= x0 {
            return None;
        }
        let start = (y as usize * self.stride + x0 as usize) * 4;
        let end = start + ((x1 - x0) as usize) * 4;
        if end > self.px.len() {
            return None;
        }
        Some(((x0 - x) as usize, &mut self.px[start..end]))
    }

    pub fn fill_rect(&mut self, r: Rect, colour: u32) {
        let Some((x0, y0, x1, y1)) = self.clip(r) else { return };
        let bytes = colour.to_le_bytes();
        let run = (x1 - x0) as usize * 4;
        for y in y0..y1 {
            let start = (y as usize * self.stride + x0 as usize) * 4;
            let Some(row) = self.px.get_mut(start..start + run) else { break };
            // One clipped run per row. `chunks_exact_mut` over a known-length
            // slice lets the compiler emit a wide store
            for p in row.chunks_exact_mut(4) {
                p.copy_from_slice(&bytes);
            }
        }
    }

    /// A whole rectangle blended at one alpha, for a see-through panel
    pub fn blend_rect(&mut self, r: Rect, colour: u32, alpha: u8) {
        if alpha == 0 {
            return;
        }
        if alpha == 255 {
            self.fill_rect(r, colour);
            return;
        }
        let Some((x0, y0, x1, y1)) = self.clip(r) else { return };
        let run = (x1 - x0) as usize * 4;
        for y in y0..y1 {
            let start = (y as usize * self.stride + x0 as usize) * 4;
            let Some(row) = self.px.get_mut(start..start + run) else { break };
            for p in row.chunks_exact_mut(4) {
                let under = u32::from_le_bytes([p[0], p[1], p[2], p[3]]);
                p.copy_from_slice(&blend(under, colour, alpha).to_le_bytes());
            }
        }
    }

    pub fn fill_round_rect(&mut self, r: Rect, radius: i32, colour: u32) {
        let rad = radius.min(r.w / 2).min(r.h / 2).max(0);
        if rad == 0 {
            self.fill_rect(r, colour);
            return;
        }

        // Exact straight parts: a band across the middle, and bands at the top
        // and bottom between the corners
        self.fill_rect(Rect::new(r.x, r.y + rad, r.w, r.h - 2 * rad), colour);
        self.fill_rect(Rect::new(r.x + rad, r.y, r.w - 2 * rad, rad), colour);
        self.fill_rect(
            Rect::new(r.x + rad, r.bottom() - rad, r.w - 2 * rad, rad),
            colour,
        );

        for (cx, cy, ox, oy) in self.corners(r, rad) {
            for dy in 0..rad {
                for dx in 0..rad {
                    let (x, y) = (ox + dx, oy + dy);
                    let cov = coverage(x, y, cx, cy, rad);
                    self.blend_px(x, y, colour, cov);
                }
            }
        }
    }

    /// An outline `width` pixels thick, drawn inside `r`
    pub fn stroke_round_rect(&mut self, r: Rect, radius: i32, width: i32, colour: u32) {
        let w = width.max(1);
        let rad = radius.min(r.w / 2).min(r.h / 2).max(0);

        // The four straight runs, each stopping short of the corners
        self.fill_rect(Rect::new(r.x + rad, r.y, r.w - 2 * rad, w), colour);
        self.fill_rect(Rect::new(r.x + rad, r.bottom() - w, r.w - 2 * rad, w), colour);
        self.fill_rect(Rect::new(r.x, r.y + rad, w, r.h - 2 * rad), colour);
        self.fill_rect(Rect::new(r.right() - w, r.y + rad, w, r.h - 2 * rad), colour);

        if rad == 0 {
            return;
        }
        let inner = (rad - w).max(0);
        for (cx, cy, ox, oy) in self.corners(r, rad) {
            for dy in 0..rad {
                for dx in 0..rad {
                    let (x, y) = (ox + dx, oy + dy);
                    let outside = coverage(x, y, cx, cy, rad);
                    let hole = coverage(x, y, cx, cy, inner);
                    self.blend_px(x, y, colour, outside.saturating_sub(hole));
                }
            }
        }
    }

    /// The four corner squares of a rounded rectangle, as (center x, center y,
    /// square origin x, square origin y). Centers are 16.16 fixed point, since
    /// a corner's center is on a pixel boundary
    fn corners(&self, r: Rect, rad: i32) -> [(i32, i32, i32, i32); 4] {
        [
            (r.x + rad, r.y + rad, r.x, r.y),
            (r.right() - rad, r.y + rad, r.right() - rad, r.y),
            (r.x + rad, r.bottom() - rad, r.x, r.bottom() - rad),
            (r.right() - rad, r.bottom() - rad, r.right() - rad, r.bottom() - rad),
        ]
    }

    pub fn fill_circle(&mut self, cx: i32, cy: i32, rad: i32, colour: u32) {
        for y in (cy - rad)..(cy + rad) {
            for x in (cx - rad)..(cx + rad) {
                let cov = coverage(x, y, cx, cy, rad);
                self.blend_px(x, y, colour, cov);
            }
        }
    }

    /// Copies a picture in, blending over what is there. `src` is premultiplied
    /// BGRA in framebuffer byte order, as `imagegen.rs` writes, so a pixel is
    /// an add and a scale
    pub fn blit(&mut self, src: &[u8], sw: usize, sh: usize, x: i32, y: i32) {
        for row in 0..sh as i32 {
            let py = y + row;
            if py < 0 || py >= self.h {
                continue;
            }
            for col in 0..sw as i32 {
                let pxx = x + col;
                if pxx < 0 || pxx >= self.w {
                    continue;
                }
                let s = (row as usize * sw + col as usize) * 4;
                if s + 4 > src.len() {
                    continue;
                }
                let a = src[s + 3] as u32;
                if a == 0 {
                    continue;
                }
                let Some(d) = self.at(pxx, py) else { continue };
                if a == 255 {
                    self.px[d..d + 3].copy_from_slice(&src[s..s + 3]);
                    continue;
                }
                let ia = 255 - a;
                for ch in 0..3 {
                    let under = self.px[d + ch] as u32;
                    self.px[d + ch] = (src[s + ch] as u32 + (under * ia + 127) / 255) as u8;
                }
            }
        }
    }

    /// The same for a picture with straight alpha. `imagegen` premultiplies at
    /// build time; `qoi::decode` does not, to round-trip. The call's name
    /// carries the convention, since mixing them gives washed-out or too-bright
    /// soft edges.
    ///
    /// Scaled to `dw` by `dh`, nearest-neighbour, in fixed point
    pub fn blit_over(&mut self, src: &[u8], sw: usize, sh: usize,
                     x: i32, y: i32, dw: i32, dh: i32) {
        if sw == 0 || sh == 0 || dw <= 0 || dh <= 0 {
            return;
        }
        let step_x = ((sw as u32) << 16) / dw as u32;
        let step_y = ((sh as u32) << 16) / dh as u32;
        for row in 0..dh {
            let py = y + row;
            if py < 0 || py >= self.h {
                continue;
            }
            let sy = ((row as u32 * step_y) >> 16) as usize;
            if sy >= sh {
                continue;
            }
            for col in 0..dw {
                let pxx = x + col;
                if pxx < 0 || pxx >= self.w {
                    continue;
                }
                let sx = ((col as u32 * step_x) >> 16) as usize;
                if sx >= sw {
                    continue;
                }
                let o = (sy * sw + sx) * 4;
                if o + 4 > src.len() {
                    continue;
                }
                let a = src[o + 3] as u32;
                if a == 0 {
                    continue;
                }
                let Some(d) = self.at(pxx, py) else { continue };
                let s = u32::from_le_bytes([src[o], src[o + 1], src[o + 2], src[o + 3]]);
                let under = u32::from_le_bytes(
                    [self.px[d], self.px[d + 1], self.px[d + 2], self.px[d + 3]]);
                // Source-over, keeping the destination's alpha, so an icon does
                // not make a translucent card opaque
                let out = (over(under, s, a) & 0x00FF_FFFF) | (under & 0xFF00_0000);
                self.px[d..d + 4].copy_from_slice(&out.to_le_bytes());
            }
        }
    }
}

// Curves and edges, for the icons
//
// Sampled on the same 4x4 grid as the corners, in integer arithmetic.
//
// Angles are whole degrees from east, increasing clockwise on a screen where y
// grows downward.

/// sin and cos of an angle in degrees, in 1024ths. One quadrant of table plus
/// symmetry
pub fn sin_cos_deg(deg: i32) -> (i32, i32) {
    #[rustfmt::skip]
    const SIN: [i16; 91] = [
        0, 18, 36, 54, 71, 89, 107, 125, 143, 160,
        178, 195, 213, 230, 248, 265, 282, 299, 316, 333,
        350, 367, 384, 400, 416, 433, 449, 465, 481, 496,
        512, 527, 543, 558, 573, 587, 602, 616, 630, 644,
        658, 672, 685, 698, 711, 724, 737, 749, 761, 773,
        784, 796, 807, 818, 828, 839, 849, 859, 868, 878,
        887, 896, 904, 912, 920, 928, 935, 943, 949, 956,
        962, 968, 974, 979, 984, 989, 994, 998, 1002, 1005,
        1008, 1011, 1014, 1016, 1018, 1020, 1022, 1023, 1023, 1024,
        1024,
    ];
    let sin_of = |d: i32| -> i32 {
        let d = d.rem_euclid(360);
        match d {
            0..=90 => SIN[d as usize] as i32,
            91..=180 => SIN[(180 - d) as usize] as i32,
            181..=270 => -(SIN[(d - 180) as usize] as i32),
            _ => -(SIN[(360 - d) as usize] as i32),
        }
    };
    (sin_of(deg), sin_of(deg + 90))
}

/// Whether a direction lies in the sector starting at `start` and turning
/// clockwise by `sweep`, from cross products with the two edges. Only the signs
/// matter
#[inline]
fn in_sector(dx: i64, dy: i64, start: (i32, i32), end: (i32, i32), reflex: bool) -> bool {
    // Positive when the second vector is clockwise of the first, with y down
    let from_start = start.0 as i64 * dy - start.1 as i64 * dx;
    let to_end = dx * end.1 as i64 - dy * end.0 as i64;
    if reflex {
        from_start >= 0 || to_end >= 0
    } else {
        from_start >= 0 && to_end >= 0
    }
}

impl<'a> Canvas<'a> {
    /// A slice of a ring: between `r_in` and `r_out` from (cx, cy), from
    /// `start_deg`, clockwise by `sweep_deg`. `r_in` of 0 makes a pie slice, a
    /// sweep of 360 a whole ring. Draws the round parts of the icons
    pub fn arc(
        &mut self,
        cx: i32,
        cy: i32,
        r_out: i32,
        r_in: i32,
        start_deg: i32,
        sweep_deg: i32,
        colour: u32,
    ) {
        if r_out <= 0 || sweep_deg == 0 {
            return;
        }
        let full = sweep_deg.abs() >= 360;
        let (ss, sc) = sin_cos_deg(start_deg);
        let (es, ec) = sin_cos_deg(start_deg + sweep_deg);
        let (start, end) = ((sc, ss), (ec, es));
        let reflex = sweep_deg > 180;

        // In 1/8 pixel units, where the sample centers are
        let out8 = (r_out * 8) as i64;
        let in8 = (r_in.max(0) * 8) as i64;
        for y in (cy - r_out - 1)..=(cy + r_out + 1) {
            for x in (cx - r_out - 1)..=(cx + r_out + 1) {
                // The pixel's center, to reject anything not near the band
                // before taking sixteen samples
                let pdx = ((x - cx) * 8 + 4) as i64;
                let pdy = ((y - cy) * 8 + 4) as i64;
                let d2 = pdx * pdx + pdy * pdy;
                if d2 > (out8 + 12) * (out8 + 12) || d2 < (in8 - 12).max(0) * (in8 - 12).max(0) {
                    continue;
                }
                let mut hits = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let dx = ((x - cx) * 8 + 1 + sx * 2) as i64;
                        let dy = ((y - cy) * 8 + 1 + sy * 2) as i64;
                        let d2 = dx * dx + dy * dy;
                        if d2 > out8 * out8 || d2 < in8 * in8 {
                            continue;
                        }
                        if full || in_sector(dx, dy, start, end, reflex) {
                            hits += 1;
                        }
                    }
                }
                self.blend_px(x, y, colour, (hits * 255 / 16) as u8);
            }
        }
    }

    /// A ring: the outline of a circle, `width` thick, centered on the radius
    pub fn stroke_circle(&mut self, cx: i32, cy: i32, rad: i32, width: i32, colour: u32) {
        self.arc(cx, cy, rad, rad - width.max(1), 0, 360, colour);
    }

    /// Strokes a polyline as a distance field, in sixty-fourths of a pixel.
    ///
    /// One pass over the reachable pixels, each one's coverage from its
    /// distance to the nearest point on the path. Unlike chained `stroke`
    /// calls, joints are not blended twice. Sub-pixel endpoints and fractional
    /// width are supported, with round caps and joins following from "within
    /// half a width of the path".
    ///
    /// Coordinates and `half`, half the width, are in sixty-fourths of a pixel.
    /// Cost is the bounding box times the segment count
    pub fn stroke_path(&mut self, pts: &[(i32, i32)], half: i32, colour: u32) {
        const Q: i32 = 64;
        if pts.len() < 2 || half <= 0 {
            return;
        }
        // Everything within half a width of the path, plus the pixel the edge
        // fades over
        let reach = half + Q;
        let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
        for &(x, y) in pts {
            x0 = x0.min(x); y0 = y0.min(y);
            x1 = x1.max(x); y1 = y1.max(y);
        }
        let bx0 = ((x0 - reach).div_euclid(Q)).max(0);
        let by0 = ((y0 - reach).div_euclid(Q)).max(0);
        let bx1 = ((x1 + reach).div_euclid(Q) + 1).min(self.w);
        let by1 = ((y1 + reach).div_euclid(Q) + 1).min(self.h);

        for py in by0..by1 {
            for px in bx0..bx1 {
                // The pixel's centre, in the same sixty-fourths
                let (cx, cy) = (px * Q + Q / 2, py * Q + Q / 2);
                let mut best = i64::MAX;
                for seg in pts.windows(2) {
                    let (ax, ay) = seg[0];
                    let (bx, by) = seg[1];
                    let (vx, vy) = ((bx - ax) as i64, (by - ay) as i64);
                    let (wx, wy) = ((cx - ax) as i64, (cy - ay) as i64);
                    let len2 = vx * vx + vy * vy;
                    // Foot of the perpendicular on the segment, clamped to its
                    // ends, which makes caps and joins
                    let (dx, dy) = if len2 == 0 {
                        (wx, wy)
                    } else {
                        let t = ((wx * vx + wy * vy) * Q as i64 / len2).clamp(0, Q as i64);
                        (wx - vx * t / Q as i64, wy - vy * t / Q as i64)
                    };
                    let d2 = dx * dx + dy * dy;
                    if d2 < best {
                        best = d2;
                        if best == 0 {
                            break;
                        }
                    }
                }
                let d = isqrt64(best) as i32;
                // Solid to half a pixel inside the edge, gone half a pixel
                // outside, with a linear ramp between
                let cov = (((half + Q / 2 - d) * 255) / Q).clamp(0, 255) as u8;
                self.blend_px(px, py, colour, cov);
            }
        }
    }

    /// A straight stroke with round ends, `thick` pixels across, and the one
    /// segment drawer. Every sample within half the thickness is a capsule, so
    /// two strokes meet with no notch
    pub fn stroke(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, colour: u32) {
        let half = (thick.max(1) * 4) as i64; // half the width, in 1/8 pixels
        let pad = thick.max(1) / 2 + 2;
        let (ax, bx) = (x0.min(x1) - pad, x0.max(x1) + pad);
        let (ay, by) = (y0.min(y1) - pad, y0.max(y1) + pad);
        let (px0, py0) = ((x0 * 8 + 4) as i64, (y0 * 8 + 4) as i64);
        let (vx, vy) = (((x1 - x0) * 8) as i64, ((y1 - y0) * 8) as i64);
        let len2 = vx * vx + vy * vy;

        for y in ay..=by {
            for x in ax..=bx {
                let mut hits = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let qx = (x * 8 + 1 + sx * 2) as i64 - px0;
                        let qy = (y * 8 + 1 + sy * 2) as i64 - py0;
                        // The nearest point on the segment, clamped to its ends
                        let (dx, dy) = if len2 == 0 {
                            (qx, qy)
                        } else {
                            let t = ((qx * vx + qy * vy) * 256 / len2).clamp(0, 256);
                            (qx - vx * t / 256, qy - vy * t / 256)
                        };
                        if dx * dx + dy * dy <= half * half {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.blend_px(x, y, colour, (hits * 255 / 16) as u8);
                }
            }
        }
    }

    /// A filled convex shape: an arrow head or a gear tooth. One pass over the
    /// outline, since two triangles sharing an edge would blend half coverage
    /// twice and leave a pale seam
    pub fn fill_poly(&mut self, pts: &[(i32, i32)], colour: u32) {
        if pts.len() < 3 {
            return;
        }
        let (mut ax, mut bx) = (i32::MAX, i32::MIN);
        let (mut ay, mut by) = (i32::MAX, i32::MIN);
        // Twice the signed area, giving the winding
        let mut area = 0i64;
        for (i, p) in pts.iter().enumerate() {
            ax = ax.min(p.0 - 1);
            bx = bx.max(p.0 + 1);
            ay = ay.min(p.1 - 1);
            by = by.max(p.1 + 1);
            let q = pts[(i + 1) % pts.len()];
            area += p.0 as i64 * q.1 as i64 - q.0 as i64 * p.1 as i64;
        }
        if area == 0 {
            return;
        }
        let sign = if area > 0 { 1i64 } else { -1 };
        // An edge function, positive on the inside once the winding is taken
        // out
        let edge = |a: (i32, i32), b: (i32, i32), x: i64, y: i64| -> i64 {
            sign * ((b.0 - a.0) as i64 * (y - (a.1 as i64 * 8 + 4))
                - (b.1 - a.1) as i64 * (x - (a.0 as i64 * 8 + 4)))
        };

        for y in ay..=by {
            for x in ax..=bx {
                let mut hits = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let qx = (x * 8 + 1 + sx * 2) as i64;
                        let qy = (y * 8 + 1 + sy * 2) as i64;
                        let inside = pts
                            .iter()
                            .enumerate()
                            .all(|(i, p)| edge(*p, pts[(i + 1) % pts.len()], qx, qy) >= 0);
                        if inside {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    self.blend_px(x, y, colour, (hits * 255 / 16) as u8);
                }
            }
        }
    }

    /// A filled triangle
    pub fn fill_tri(&mut self, p0: (i32, i32), p1: (i32, i32), p2: (i32, i32), colour: u32) {
        self.fill_poly(&[p0, p1, p2], colour);
    }
}

/// Integer square root, floor: Newton, seeded by halving the bit length.
/// `libmay` is `no_std` with no libm
#[inline]
pub fn isqrt(n: u32) -> u32 {
    if n < 2 { return n; }
    let mut x = 1u32 << ((32 - n.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + n / x) / 2;
        if y >= x { return x; }
        x = y;
    }
}

/// How far the pixel at (x, y) lies outside `r` with corners rounded by
/// `radius`, in pixels; zero inside. Same rounded-box distance as the shader
/// (`shaders/composite.cu::rounded_box`): shrink the box by the radius, measure
/// to it, and subtract the radius
pub fn round_rect_outside_dist(x: i32, y: i32, r: Rect, radius: i32) -> u32 {
    let rad = radius.min(r.w / 2).min(r.h / 2).max(0);
    // The box shrunk by the radius
    let (ix0, iy0) = (r.x + rad, r.y + rad);
    let (ix1, iy1) = (r.right() - rad, r.bottom() - rad);
    let dx = if x < ix0 { ix0 - x } else if x >= ix1 { x - ix1 + 1 } else { 0 };
    let dy = if y < iy0 { iy0 - y } else if y >= iy1 { y - iy1 + 1 } else { 0 };
    if dx == 0 && dy == 0 { return 0; }
    let d = isqrt((dx * dx + dy * dy) as u32);
    d.saturating_sub(rad as u32)
}

/// How much of the pixel at (x, y) is inside `r` with corners rounded by
/// `radius`, as 0..=255; outside is 0. Used by `fill_round_rect` above and the
/// compositor's CPU compose pass (`kernel/mayland/render/compose.rs`), so both
/// draw the same corner
#[inline]
pub fn round_rect_coverage(x: i32, y: i32, r: Rect, radius: i32) -> u8 {
    if x < r.x || y < r.y || x >= r.right() || y >= r.bottom() {
        return 0;
    }
    let rad = radius.min(r.w / 2).min(r.h / 2).max(0);
    if rad == 0 {
        return 255;
    }
    // The straight parts, everything but the corner squares, are fully inside:
    // one compare each
    let left   = x < r.x + rad;
    let right  = x >= r.right() - rad;
    let top    = y < r.y + rad;
    let bottom = y >= r.bottom() - rad;
    if !(left || right) || !(top || bottom) {
        return 255;
    }
    let cx = if left { r.x + rad } else { r.right() - rad };
    let cy = if top  { r.y + rad } else { r.bottom() - rad };
    coverage(x, y, cx, cy, rad)
}

/// How much of the pixel at (x, y) is inside a circle of `rad` about (cx, cy),
/// as 0..=255. Sampled on a 4x4 grid in integers; a pixel well inside or
/// outside answers on its first compare
#[inline]
fn coverage(x: i32, y: i32, cx: i32, cy: i32, rad: i32) -> u8 {
    if rad <= 0 {
        return 0;
    }
    // Distance from pixel center to circle center, in quarter pixels, for an
    // exact accept or reject
    let dx = (x * 4 + 2) - cx * 4;
    let dy = (y * 4 + 2) - cy * 4;
    let d2 = dx * dx + dy * dy;
    let r4 = rad * 4;
    // More than a pixel inside, or more than a pixel outside
    if d2 <= (r4 - 4) * (r4 - 4) {
        return 255;
    }
    if d2 >= (r4 + 4) * (r4 + 4) {
        return 0;
    }

    let mut hits = 0u32;
    for sy in 0..4 {
        for sx in 0..4 {
            // Sample centres at 1/8, 3/8, 5/8, 7/8 of the pixel
            let px = x * 8 + 1 + sx * 2;
            let py = y * 8 + 1 + sy * 2;
            let ddx = px - cx * 8;
            let ddy = py - cy * 8;
            if ddx * ddx + ddy * ddy <= (rad * 8) * (rad * 8) {
                hits += 1;
            }
        }
    }
    (hits * 255 / 16) as u8
}

/// Integer square root for the distance field, taking the `i64` squared
/// distance in sixty-fourths of a pixel
fn isqrt64(v: i64) -> i64 {
    if v <= 0 {
        return 0;
    }
    (v as u64).isqrt() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One alpha, or a zero alpha on either side: the simple lerp
    #[test]
    fn equal_alphas_unchanged() {
        for &(bg, fg) in &[(0xFF10_2030u32, 0xFFF0_E0D0u32), (0x0000_0000, 0x0012_3456),
                           (0x80AA_BBCC, 0x8011_2233), (0xFF10_2030, 0x00F0_E0D0), (0x0010_2030, 0xFFF0_E0D0)] {
            for cov in 1..255u32 {
                let mut want = 0u32;
                for shift in [0, 8, 16, 24] {
                    let b = (bg >> shift) & 0xff;
                    let f = (fg >> shift) & 0xff;
                    want |= div255(f * cov + b * (255 - cov)) << shift;
                }
                assert_eq!(blend(bg, fg, cov as u8), want);
            }
        }
    }

    /// An opaque glyph over an 85% background: premultiplied blending's result,
    /// within a step
    #[test]
    fn translucent_background_is_exact() {
        let (bg, fg) = (0xD916_1216u32, 0xFFE6_E1E9u32);
        for cov in 1..255u32 {
            let v = blend(bg, fg, cov as u8);
            let c = cov as f64 / 255.0;
            let (ab, af) = (217.0 / 255.0, 1.0);
            let a = af * c + ab * (1.0 - c);
            assert!(((v >> 24) as f64 - a * 255.0).abs() <= 1.0);
            for shift in [0, 8, 16] {
                let b = ((bg >> shift) & 0xff) as f64;
                let f = ((fg >> shift) & 0xff) as f64;
                let want = (f * af * c + b * ab * (1.0 - c)) / a;
                let got = ((v >> shift) & 0xff) as f64;
                assert!((got - want).abs() <= 1.0, "cov {cov} shift {shift}: {got} vs {want}");
            }
        }
    }
}
