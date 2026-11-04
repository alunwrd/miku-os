// The character grid and its rendering
//
// The kernel keeps the text; cell size, margin, background, and caret are
// decided here. The grid is shared memory, four bytes per cell (character, then
// red, green, blue), read on every redraw with no local copy.

use super::conn::Pool;
use super::protocol;
use super::text;


/// Margin around the text, in pixels
pub const PAD: i32 = 8;
/// Cell size: the font's width, and two pixels of leading in height
pub const CELL_W: i32 = text::CELL_W as i32;
pub const LINE_H: i32 = text::CELL_H as i32 + 2;

/// Default background, `0x00RRGGBB` as kernel cells carry it
pub const BG: u32 = 0x0016_1216;
/// Default background opacity (`background_opacity 0.85`). Cells with their own
/// color and text stay opaque
pub const BG_ALPHA: u32 = 217;
/// Default background as ARGB8888
pub const BG_PX: u32 = BG | BG_ALPHA << 24;
/// Every other colour goes in whole
const SOLID: u32 = 0xFF00_0000;
/// Kernel console's caret color
pub const CARET: u32 = 0x00C8_DCDC;
/// Caret width
pub const CARET_W: i32 = 2;

/// Kernel grid limit. Larger requests are refused
pub const MAX_COLS: usize = 256;
pub const MAX_ROWS: usize = 96;

/// How many characters fit in a window this wide, and this tall
pub fn cols_for(width: i32) -> usize {
    (((width - 2 * PAD).max(0) / CELL_W) as usize).min(MAX_COLS)
}

pub fn rows_for(height: i32) -> usize {
    (((height - 2 * PAD).max(0) / LINE_H) as usize).min(MAX_ROWS)
}

/// One cell ready to draw: reverse applied, background resolved
#[derive(Clone, Copy)]
struct Painted {
    ch: char,
    fg: u32,
    /// `None` means the window's own background, which has just been painted
    bg: Option<u32>,
    attrs: u16,
}

/// A cell worth drawing a glyph for: not blank and not a control character.
/// `draw_char_on` draws nothing for missing glyphs
fn printable(ch: char) -> Option<char> {
    match ch {
        ' ' => None,
        c if (c as u32) < 0x20 => None,
        c if (0x7f..0xa0).contains(&(c as u32)) => None,
        c => Some(c),
    }
}

pub struct Grid {
    /// Mapping of the kernel's grid, or None before the first `grid` event
    pool: Option<Pool>,
    /// Handle of that mapping. It never changes, so a later `grid` event is a
    /// shape change only
    handle: u32,
    pub cols: usize,
    pub rows: usize,
    pub caret: (usize, usize),
}

impl Grid {
    pub const fn new() -> Self {
        Grid { pool: None, handle: 0, cols: 0, rows: 0, caret: (0, 0) }
    }

    pub fn ready(&self) -> bool {
        self.pool.is_some() && self.cols > 0 && self.rows > 0
    }

    /// Takes the grid or its new shape. The handle is allocated once per
    /// connection and resent on shape changes, so usually only two numbers
    /// change
    pub fn adopt(&mut self, handle: u32, size: u32, cols: u32, rows: u32) -> bool {
        let (cols, rows) = (cols as usize, rows as usize);
        if cols == 0 || rows == 0 || handle == 0 {
            return false;
        }
        // Refuses a pool too small for its shape
        if (size as usize) < cols * rows * protocol::console::CELL_BYTES {
            return false;
        }
        if self.handle != handle {
            let Some(p) = Pool::adopt(handle, size as usize) else { return false };
            // Frees the old mapping first; dropping a `Pool` unmaps nothing
            if let Some(old) = self.pool.as_mut() {
                old.free();
            }
            self.pool = Some(p);
            self.handle = handle;
        }
        self.cols = cols;
        self.rows = rows;
        true
    }

    /// Character in one cell, read at copy time by the selection
    pub fn char_at(&self, col: usize, row: usize) -> Option<char> {
        self.cell(col, row).map(|c| c.ch)
    }

    /// One cell's character and color, the only place a row and column become
    /// an offset, where the bound is checked
    fn cell(&self, col: usize, row: usize) -> Option<Painted> {
        let p = self.pool.as_ref()?;
        let at = (row * self.cols + col) * protocol::console::CELL_BYTES;
        let b = p.as_slice();
        if at + protocol::console::CELL_BYTES > b.len() {
            return None;
        }
        use protocol::console as pc;
        // Little-endian u32 character, fg rgb, bg rgb, little-endian u16 attrs
        let cp = u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
        let fg = (b[at + 4] as u32) << 16 | (b[at + 5] as u32) << 8 | b[at + 6] as u32;
        let bg = (b[at + 7] as u32) << 16 | (b[at + 8] as u32) << 8 | b[at + 9] as u32;
        let attrs = u16::from_le_bytes([b[at + 10], b[at + 11]]);

        // Reverse is applied here rather than stored swapped, so `\x1b[27m` can
        // restore it
        let bg_set = attrs & pc::ATTR_BG_SET != 0;
        let (fg, bg) = if attrs & pc::ATTR_REVERSE != 0 {
            (if bg_set { bg } else { BG }, Some(fg))
        } else {
            (fg, if bg_set { Some(bg) } else { None })
        };
        // Shared memory: a non-scalar value draws nothing
        Some(Painted { ch: char::from_u32(cp).unwrap_or(' '), fg, bg, attrs })
    }

    /// Repaints rows `r0..r1`. One `Canvas` per call and one background fill
    /// per row. Cells in `sel` draw with swapped colors
    pub fn draw_rows(&self, px: &mut [u8], stride_px: usize, r0: usize, r1: usize,
                     sel: Option<super::menu::Span>) {
        let r1 = r1.min(self.rows);
        let mut cv = text::canvas_of(px, stride_px);
        let span = self.cols as i32 * CELL_W;
        for row in r0..r1 {
            let y = PAD + row as i32 * LINE_H;
            // Paints the whole cell box as one rectangle per row, so old text
            // leaves no stripe
            text::fill_on(&mut cv, PAD, y, span, LINE_H, BG_PX);
            for col in 0..self.cols {
                let marked = sel.is_some_and(|s| s.covers(col, row));
                let x = PAD + col as i32 * CELL_W;
                let Some(cell) = self.cell(col, row) else { continue };
                use protocol::console as pc;
                // Selection overrides cell colors, and blanks in it are filled
                let (fg, bg) = if marked {
                    (BG, cell.fg.max(0x0080_8080))
                } else {
                    (cell.fg, cell.bg.unwrap_or(BG))
                };
                // The row was painted in BG, so only other colors cost a
                // rectangle. A cell in the default color stays translucent
                let own = marked || bg != BG;
                let bg_px = if own { bg | SOLID } else { BG_PX };
                if own {
                    text::fill_on(&mut cv, x, y, CELL_W, LINE_H, bg_px);
                }
                let fg = fg | SOLID;
                if cell.attrs & pc::ATTR_UNDERLINE != 0 {
                    text::fill_on(&mut cv, x, y + LINE_H - 2, CELL_W, 1, fg);
                }
                if cell.attrs & pc::ATTR_STRIKE != 0 {
                    text::fill_on(&mut cv, x, y + LINE_H / 2, CELL_W, 1, fg);
                }
                // Hidden: background without a glyph
                if cell.attrs & pc::ATTR_HIDDEN != 0 { continue; }
                let Some(c) = printable(cell.ch) else { continue };
                text::draw_char_on(&mut cv, x, y, c, fg, Some(bg_px));
            }
        }
    }

    /// Margin and any area past the grid at the right and bottom. Only on a
    /// buffer's first frame, since these bands change only with the window or
    /// grid
    pub fn draw_surround(&self, px: &mut [u8], stride_px: usize, w: i32, h: i32) {
        for (x, y, bw, bh) in self.surround_rects(w, h) {
            text::fill(px, stride_px, x, y, bw, bh, BG_PX);
        }
    }

    /// Those bands as rectangles, for painting and for damage
    pub fn surround_rects(&self, w: i32, h: i32) -> [(i32, i32, i32, i32); 4] {
        let text_w = self.cols as i32 * CELL_W;
        let text_h = self.rows as i32 * LINE_H;
        [
            (0, 0, w, PAD),
            (0, PAD, PAD, text_h),
            (PAD + text_w, PAD, w - PAD - text_w, text_h),
            (0, PAD + text_h, w, h - PAD - text_h),
        ]
    }

    /// Caret: a two-pixel bar down the left of the pen's cell, inset a row top
    /// and bottom
    pub fn draw_caret(&self, px: &mut [u8], stride_px: usize) {
        let (col, row) = self.caret;
        if col >= self.cols || row >= self.rows {
            return;
        }
        let x = PAD + col as i32 * CELL_W;
        let y = PAD + row as i32 * LINE_H;
        text::fill(px, stride_px, x, y + 1, CARET_W, LINE_H - 4, CARET | SOLID);
    }
}
