// Drawing text into a pixel buffer
//
// Shared by `#[path]` between the kernel and clients, like `wire.rs` and
// `protocol.rs`.
//
// The buffer is XRGB8888 everywhere here: four bytes a pixel, blue first in
// memory. This takes a slice and a stride, whatever the buffer is. No
// allocation.

use super::font;
use super::paint::{blend, Canvas, Rect};

/// Cell size for laying text out in rows and columns: the box a glyph is
/// rasterized into, and what a terminal steps by. Not the advance `draw_char`
/// returns, which is `CELL_W + 1` so `draw_text` leaves a gap between letters.
/// A grid ignores that return value, as `mayterm` and the kernel console do
pub const CELL_W: usize = font::CHAR_WIDTH;
pub const CELL_H: usize = font::CHAR_HEIGHT;

/// Wraps a raw buffer and its stride as a canvas, so clipping has one
/// implementation. The visible width is the stride; a zero stride has no rows.
///
/// Public so a caller drawing a whole screen can build one and pass it to
/// `draw_char_on` and `fill_on`. The `px`/`stride` wrappers repeat this
/// division per cell
#[inline]
pub fn canvas_of(px: &mut [u8], stride: usize) -> Canvas<'_> {
    let h = if stride == 0 { 0 } else { px.len() / (stride * 4) };
    Canvas::new(px, stride, stride as i32, h as i32)
}

// The console's face: one size, fixed cells, indexed by column

/// Draws one character with its top-left at (x, y). Returns the distance to the
/// next.
///
/// `bg` is what partial pixels blend toward. `None` reads the buffer, for
/// drawing over a picture; a color is faster over a solid panel
pub fn draw_char(
    px: &mut [u8],
    stride: usize,
    x: i32,
    y: i32,
    c: char,
    fg: u32,
    bg: Option<u32>,
) -> i32 {
    draw_char_on(&mut canvas_of(px, stride), x, y, c, fg, bg)
}

/// The same, onto a caller's canvas. A row at a time: the clip is decided once
/// per row
pub fn draw_char_on(
    cv: &mut Canvas,
    x: i32,
    y: i32,
    c: char,
    fg: u32,
    bg: Option<u32>,
) -> i32 {
    let Some((cell, advance)) = font::get_glyph(c) else {
        // Not a glyph we have; moves on by one cell
        return CELL_W as i32;
    };

    for row in 0..CELL_H {
        let Some((skip, span)) = cv.row_span(y + row as i32, x, CELL_W as i32) else {
            continue;
        };
        let cov_row = &cell[row * CELL_W..row * CELL_W + CELL_W];
        for (i, p) in span.chunks_exact_mut(4).enumerate() {
            let cov = cov_row[skip + i];
            if cov == 0 {
                continue;
            }
            let v = match bg {
                // Known background: no read back, one blend and one write
                Some(b) => blend(b, fg, cov),
                // Over what is already there, for a picture underneath
                None => blend(u32::from_le_bytes([p[0], p[1], p[2], p[3]]), fg, cov),
            };
            p.copy_from_slice(&v.to_le_bytes());
        }
    }
    *advance as i32
}

/// Draws a string on one line. Returns where the next character would go.
/// Nothing wraps or scrolls; `draw_char` clips at the right edge
pub fn draw_text(
    px: &mut [u8],
    stride: usize,
    x: i32,
    y: i32,
    s: &str,
    fg: u32,
    bg: Option<u32>,
) -> i32 {
    let mut pen = x;
    for c in s.chars() {
        pen += draw_char(px, stride, pen, y, c, fg, bg);
    }
    pen
}

/// How wide a string will be
pub fn text_width(s: &str) -> i32 {
    s.chars()
        .map(|c| font::get_glyph(c).map(|(_, a)| *a as i32).unwrap_or(CELL_W as i32))
        .sum()
}

// The interface faces: several sizes, two weights, proportional
//
// These take a face index and a `Canvas`. Indices are from `fontgen.rs`, and
// `ui.rs` names them

/// How tall one line of this face is, for a caller stacking lines
pub fn face_line_height(face: usize) -> i32 {
    font::UI_FACES.get(face).map(|f| f.line_height as i32).unwrap_or(CELL_H as i32)
}

/// Where the baseline sits inside that line
pub fn face_ascent(face: usize) -> i32 {
    font::UI_FACES.get(face).map(|f| f.ascent as i32).unwrap_or(CELL_H as i32 - 3)
}

/// Draws a string with its line box starting at (x, y). Returns where the next
/// character would go. `y` is the top of the line, not the baseline
pub fn draw_face(c: &mut Canvas, face: usize, x: i32, y: i32, s: &str, colour: u32) -> i32 {
    draw_face_within(c, face, x, y, i32::MAX, s, colour)
}

/// The same, stopping before `limit`, with an ellipsis if it did not fit. No
/// allocation: characters are measured as drawn, and the ellipsis uses room
/// kept for it
pub fn draw_face_within(
    c: &mut Canvas, face: usize, x: i32, y: i32, limit: i32, s: &str, colour: u32,
) -> i32 {
    let dots = if limit == i32::MAX { 0 } else { face_width(face, "...") };
    let baseline = y + face_ascent(face);
    let mut pen = x;
    for ch in s.chars() {
        if limit != i32::MAX {
            let advance = font::ui_glyph(face, ch).map(|g| g.advance as i32).unwrap_or(4);
            if pen + advance > limit {
                // Room was kept by refusing the character crossing the limit
                return draw_glyphs(c, face, pen.min(limit - dots), baseline, "...", colour);
            }
        }
        pen = draw_glyphs(c, face, pen, baseline, ch.encode_utf8(&mut [0u8; 4]), colour);
    }
    pen
}

fn draw_glyphs(
    c: &mut Canvas, face: usize, x: i32, baseline: i32, s: &str, colour: u32,
) -> i32 {
    let mut pen = x;
    for ch in s.chars() {
        let Some(g) = font::ui_glyph(face, ch) else {
            // Not in this font; moves on by a space
            pen += font::ui_glyph(face, ' ').map(|g| g.advance as i32).unwrap_or(4);
            continue;
        };
        let (gw, gh) = (g.w as usize, g.h as usize);
        let top = baseline - g.top as i32;
        for row in 0..gh {
            for col in 0..gw {
                let cov = font::UI_PIXELS.get(g.off + row * gw + col).copied().unwrap_or(0);
                if cov != 0 {
                    c.blend_px(pen + g.left as i32 + col as i32, top + row as i32, colour, cov);
                }
            }
        }
        pen += g.advance as i32;
    }
    pen
}

/// How wide a string will be in this face
pub fn face_width(face: usize, s: &str) -> i32 {
    s.chars()
        .map(|ch| {
            font::ui_glyph(face, ch)
                .or_else(|| font::ui_glyph(face, ' '))
                .map(|g| g.advance as i32)
                .unwrap_or(4)
        })
        .sum()
}

/// Draw a string centred in `width` starting at `x`
pub fn draw_face_centred(
    c: &mut Canvas,
    face: usize,
    x: i32,
    y: i32,
    width: i32,
    s: &str,
    colour: u32,
) -> i32 {
    let w = face_width(face, s);
    draw_face(c, face, x + (width - w) / 2, y, s, colour)
}

/// Fills a rectangle, clipped to the buffer, using the rectangle fill in
/// `paint.rs`
pub fn fill(px: &mut [u8], stride: usize, x: i32, y: i32, w: i32, h: i32, colour: u32) {
    canvas_of(px, stride).fill_rect(Rect::new(x, y, w, h), colour);
}

/// The same, onto a canvas the caller already has. See `canvas_of`
pub fn fill_on(cv: &mut Canvas, x: i32, y: i32, w: i32, h: i32, colour: u32) {
    cv.fill_rect(Rect::new(x, y, w, h), colour);
}
