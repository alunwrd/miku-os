extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;
use core::fmt;
use lazy_static::lazy_static;
use spin::Mutex;
use x86_64::instructions::interrupts;

lazy_static! {
    pub static ref WRITER: Mutex<Option<Console>> = Mutex::new(None);
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            let _ = w.write_fmt(args);
        }
    });
}

pub fn print_colored(r: u8, g: u8, b: u8, args: fmt::Arguments) {
    use core::fmt::Write;
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            let saved = w.fg_color;
            w.fg_color = [r, g, b];
            let _ = w.write_fmt(args);
            w.fg_color = saved;
        }
    });
}

pub fn backspace() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.backspace(); }
    });
}

pub fn clear_screen() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear(); }
    });
}

pub fn set_color(r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.fg_color = [r, g, b]; }
    });
}

pub fn reset_color() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.fg_color = COLOR_MIKU; }
    });
}

pub fn move_cursor_left() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            if w.x_pos > BORDER_PADDING { w.x_pos -= CHAR_WIDTH; }
        }
    });
}

pub fn move_cursor_right() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.x_pos += CHAR_WIDTH; }
    });
}

pub fn get_x() -> usize {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map_or(0, |w| w.x_pos)
    })
}

pub fn set_x(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.x_pos = x;
            w.cur_col = x.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
        }
    });
}

pub fn draw_cursor(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.paint_cursor(x, 200, 220, 220);
        }
    });
}

pub fn erase_cursor(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.paint_cursor(x, 0, 0, 0);
        }
    });
}

pub fn redraw_input_line(
    start_x: usize,
    buf: &[u8],
    len: usize,
    cursor: usize,
    old_len: usize,
    dirty_start: usize,
) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.redraw_input_fast(start_x, buf, len, cursor, old_len, dirty_start);
        }
    });
}

pub fn redraw_input_line_full(
    start_x: usize,
    buf: &[u8],
    len: usize,
    cursor: usize,
    old_len: usize,
) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.redraw_input_fast(start_x, buf, len, cursor, old_len, 0);
        }
    });
}

pub fn clear_from_x(start_x: usize, count: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear_columns(start_x, count); }
    });
}

pub fn fb_size() -> Option<(usize, usize)> {
    interrupts::without_interrupts(|| {
        WRITER.lock().as_ref().map(|w| (w.fb_width(), w.fb_height()))
    })
}

pub fn fill_rect(x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.fill_rect(x, y, w, h, r, g, b); }
    });
}

pub fn fill_hgradient(
    x: usize, y: usize, w: usize, h: usize,
    left: (u8, u8, u8), right: (u8, u8, u8),
) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.fill_hgradient(x, y, w, h, left, right); }
    });
}

pub fn write_pixel_at(x: usize, y: usize, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(c) = WRITER.lock().as_mut() { c.write_pixel(x, y, r, g, b); }
    });
}

pub fn hide_cursor() {}
pub fn show_cursor() {}

pub fn clear_char() {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() { w.clear_char_at_cursor(); }
    });
}

pub fn write_char_at_x(x: usize, ch: u8, r: u8, g: u8, b: u8) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.write_single_char_at(x, ch, r, g, b);
        }
    });
}

pub fn clear_char_at(x: usize) {
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            w.clear_rect(x, w.y_pos, CHAR_WIDTH, CHAR_HEIGHT);
        }
    });
}

pub const COLOR_MIKU: [u8; 3] = [57, 197, 187];
pub const COLOR_MIKU_DARK: [u8; 3] = [0, 150, 136];
pub const COLOR_MIKU_LIGHT: [u8; 3] = [128, 222, 217];
pub const COLOR_PINK: [u8; 3] = [255, 105, 140];
pub const COLOR_WHITE: [u8; 3] = [230, 240, 240];
pub const COLOR_GRAY: [u8; 3] = [120, 140, 140];
pub const COLOR_GREEN: [u8; 3] = [100, 220, 150];
pub const COLOR_YELLOW: [u8; 3] = [220, 220, 100];
pub const COLOR_CYAN: [u8; 3] = [0, 220, 220];

pub const BORDER_PADDING: usize = 10;
pub const CHAR_WIDTH: usize = 9;
const LINE_SPACING: usize = 2;
const CHAR_HEIGHT: usize = 16;
const LINE_HEIGHT: usize = CHAR_HEIGHT + LINE_SPACING;
const GLYPH_SIZE: usize = CHAR_WIDTH * CHAR_HEIGHT;

const MAX_COLS: usize = 160;
const MAX_ROWS: usize = 60;

const SHADOW_LINE_PIXELS: usize = MAX_COLS * CHAR_WIDTH;
const SHADOW_LINE_BYTES: usize = SHADOW_LINE_PIXELS * 4;

#[derive(Clone, Copy)]
struct Cell {
    ch: u8,
    r: u8,
    g: u8,
    b: u8,
}

impl Cell {
    const fn blank() -> Self { Self { ch: b' ', r: 0, g: 0, b: 0 } }
}

#[derive(Debug, Clone, Copy)]
pub struct FrameBufferConfig {
    pub width: usize,
    pub height: usize,
    pub stride: usize,
    pub bytes_per_pixel: usize,
    pub is_bgr: bool,
}

pub struct Console {
    fb_ptr: *mut u8,
    fb_len: usize,
    cells: Vec<Cell>,
    cols: usize,
    rows: usize,
    cur_col: usize,
    cur_row: usize,
    width: usize,
    height: usize,
    stride_bytes: usize,
    bpp: usize,
    pub x_pos: usize,
    pub y_pos: usize,
    pub fg_color: [u8; 3],
    pub is_bgr: bool,
    shadow_line: Vec<u32>,
}

unsafe impl Send for Console {}
unsafe impl Sync for Console {}

impl Console {
    pub fn new_limine(framebuffer: &'static mut [u8], config: FrameBufferConfig) -> Self {
        let cols = ((config.width.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH).min(MAX_COLS);
        let rows = ((config.height.saturating_sub(BORDER_PADDING)) / LINE_HEIGHT).min(MAX_ROWS);
        let cells = vec![Cell::blank(); cols * rows];
        let stride_bytes = config.stride * config.bytes_per_pixel;
        let fb_len = framebuffer.len();
        let fb_ptr = framebuffer.as_mut_ptr();
        let fill_end = (config.height * stride_bytes).min(fb_len);
        unsafe { core::ptr::write_bytes(fb_ptr, 0, fill_end); }

        let shadow_width = config.width;
        let shadow_line = vec![0u32; shadow_width];

        Self {
            fb_ptr,
            fb_len,
            cells,
            cols,
            rows,
            cur_col: 0,
            cur_row: 0,
            width: config.width,
            height: config.height,
            stride_bytes,
            bpp: config.bytes_per_pixel,
            x_pos: BORDER_PADDING,
            y_pos: BORDER_PADDING,
            fg_color: COLOR_MIKU,
            is_bgr: config.is_bgr,
            shadow_line,
        }
    }

    pub fn clear(&mut self) {
        for c in self.cells.iter_mut() { *c = Cell::blank(); }
        let fill_end = (self.height * self.stride_bytes).min(self.fb_len);
        unsafe { core::ptr::write_bytes(self.fb_ptr, 0, fill_end); }
        self.x_pos = BORDER_PADDING;
        self.y_pos = BORDER_PADDING;
        self.cur_col = 0;
        self.cur_row = 0;
    }

    #[inline]
    fn new_line(&mut self) {
        self.cur_col = 0;
        self.cur_row += 1;
        self.x_pos = BORDER_PADDING;
        self.y_pos += LINE_HEIGHT;
        if self.cur_row >= self.rows {
            self.scroll_up();
        }
    }

    fn scroll_up(&mut self) {
        let cols = self.cols;
        let rows = self.rows;
        let sb = self.stride_bytes;
        let cell_count = (rows - 1) * cols;
        self.cells.copy_within(cols..cols + cell_count, 0);
        let last_row_start = (rows - 1) * cols;
        for i in 0..cols {
            self.cells[last_row_start + i] = Cell::blank();
        }
        let top = BORDER_PADDING * sb;
        let src_start = top + LINE_HEIGHT * sb;
        let copy_len = (rows - 1) * LINE_HEIGHT * sb;
        let src_end = src_start + copy_len;
        if src_end <= self.fb_len {
            unsafe {
                core::ptr::copy(self.fb_ptr.add(src_start), self.fb_ptr.add(top), copy_len);
            }
        }
        let clear_start = (top + (rows - 1) * LINE_HEIGHT * sb).min(self.fb_len);
        let clear_end = (top + rows * LINE_HEIGHT * sb).min(self.fb_len);
        if clear_start < clear_end {
            unsafe { core::ptr::write_bytes(self.fb_ptr.add(clear_start), 0, clear_end - clear_start); }
        }
        self.cur_row = rows - 1;
        self.y_pos = BORDER_PADDING + self.cur_row * LINE_HEIGHT;
    }

    pub fn backspace(&mut self) {
        if self.x_pos > BORDER_PADDING {
            self.x_pos -= CHAR_WIDTH;
            if self.cur_col > 0 { self.cur_col -= 1; }
            self.clear_char_at_cursor();
            if self.cur_col < self.cols && self.cur_row < self.rows {
                self.cells[self.cur_row * self.cols + self.cur_col] = Cell::blank();
            }
        }
    }

    #[inline]
    pub fn clear_char_at_cursor(&mut self) {
        self.clear_rect(self.x_pos, self.y_pos, CHAR_WIDTH, CHAR_HEIGHT);
    }

    #[inline]
    fn clear_rect(&mut self, x: usize, y: usize, w: usize, h: usize) {
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let byte_w = w * bpp;
        let base = sb * y + x * bpp;
        if base + sb * (h.saturating_sub(1)) + byte_w <= self.fb_len {
            let mut p = unsafe { self.fb_ptr.add(base) };
            for _ in 0..h {
                unsafe {
                    core::ptr::write_bytes(p, 0, byte_w);
                    p = p.add(sb);
                }
            }
        }
    }

    fn clear_columns(&mut self, start_x: usize, count: usize) {
        if count == 0 { return; }
        let pixel_w = count * CHAR_WIDTH;
        self.clear_rect(start_x, self.y_pos, pixel_w, CHAR_HEIGHT);
        let start_col = start_x.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
        let end_col = (start_col + count).min(self.cols);
        if self.cur_row < self.rows && start_col < end_col {
            let row_off = self.cur_row * self.cols;
            self.cells[row_off + start_col..row_off + end_col].fill(Cell::blank());
        }
    }

    fn paint_cursor(&mut self, x: usize, r: u8, g: u8, b: u8) {
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let pixel = self.make_pixel(r, g, b);
        let y0 = self.y_pos;
        for y in 1..CHAR_HEIGHT - 1 {
            let row_off = sb * (y0 + y) + x * bpp;
            self.put_pixel_unchecked(row_off, pixel);
            self.put_pixel_unchecked(row_off + bpp, pixel);
        }
    }

    fn redraw_input_fast(
        &mut self,
        start_x: usize,
        buf: &[u8],
        len: usize,
        cursor: usize,
        old_len: usize,
        dirty_start: usize,
    ) {
        let saved = self.fg_color;
        self.fg_color = [255, 255, 255];

        let redraw_from = dirty_start.min(len);
        let clear_from_col = redraw_from;
        let clear_count = if old_len > redraw_from { old_len - redraw_from } else { 0 };
        let clear_x = start_x + clear_from_col * CHAR_WIDTH;

        let total_clear = if old_len > len { old_len - redraw_from } else { len - redraw_from };
        let total_clear = total_clear.max(clear_count);
        if total_clear > 0 {
            self.clear_columns(clear_x, total_clear + 1);
        }

        self.x_pos = start_x + redraw_from * CHAR_WIDTH;
        self.cur_col = (self.x_pos.saturating_sub(BORDER_PADDING)) / CHAR_WIDTH;
        for i in redraw_from..len {
            self.write_char(buf[i] as char);
        }

        self.fg_color = saved;

        let cx = start_x + cursor * CHAR_WIDTH;
        self.x_pos = cx;
        self.cur_col = cx.saturating_sub(BORDER_PADDING) / CHAR_WIDTH;
    }

    fn write_single_char_at(&mut self, x: usize, ch: u8, r: u8, g: u8, b: u8) {
        let color = [r, g, b];
        let c = ch as char;
        self.clear_rect(x, self.y_pos, CHAR_WIDTH, CHAR_HEIGHT);

        if let Some((glyph, _)) = crate::font::get_glyph(c) {
            self.render_glyph(glyph, x, self.y_pos, color);
        } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
            c,
            noto_sans_mono_bitmap::FontWeight::Regular,
            noto_sans_mono_bitmap::RasterHeight::Size16,
        ) {
            self.render_raster(&raster, x, self.y_pos, color);
        }
    }

    fn write_char(&mut self, c: char) {
        match c {
            '\n' => self.new_line(),
            '\x08' => self.backspace(),
            ch => {
                if self.x_pos + CHAR_WIDTH >= self.width {
                    self.new_line();
                }
                self.clear_char_at_cursor();
                if self.cur_col < self.cols && self.cur_row < self.rows {
                    let [r, g, b] = self.fg_color;
                    self.cells[self.cur_row * self.cols + self.cur_col] = Cell { ch: ch as u8, r, g, b };
                }
                if let Some((glyph, _)) = crate::font::get_glyph(ch) {
                    self.render_glyph(glyph, self.x_pos, self.y_pos, self.fg_color);
                } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
                    ch,
                    noto_sans_mono_bitmap::FontWeight::Regular,
                    noto_sans_mono_bitmap::RasterHeight::Size16,
                ) {
                    self.render_raster(&raster, self.x_pos, self.y_pos, self.fg_color);
                }
                self.x_pos += CHAR_WIDTH;
                self.cur_col += 1;
            }
        }
    }

    #[inline]
    fn render_glyph(&mut self, glyph: &[u8; GLYPH_SIZE], px: usize, py: usize, color: [u8; 3]) {
        let [r, g, b] = color;
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let base = sb * py + px * bpp;
        if base + sb * (CHAR_HEIGHT - 1) + CHAR_WIDTH * bpp > self.fb_len { return; }
        let is_bgr = self.is_bgr;
        let solid = Self::pixel_value(r, g, b, is_bgr);

        if bpp == 4 {
            let shadow = &mut self.shadow_line[..CHAR_WIDTH];
            let fb = self.fb_ptr;

            for y in 0..CHAR_HEIGHT {
                let row_base = base + sb * y;
                let glyph_row = y * CHAR_WIDTH;
                let mut any_pixel = false;

                for x in 0..CHAR_WIDTH {
                    let i = glyph[glyph_row + x];
                    if i == 255 {
                        shadow[x] = solid;
                        any_pixel = true;
                    } else if i > 0 {
                        let i16 = i as u16;
                        let pr = ((r as u16 * i16 + 127) >> 8) as u8;
                        let pg = ((g as u16 * i16 + 127) >> 8) as u8;
                        let pb = ((b as u16 * i16 + 127) >> 8) as u8;
                        shadow[x] = Self::pixel_value(pr, pg, pb, is_bgr);
                        any_pixel = true;
                    } else {
                        shadow[x] = 0;
                    }
                }

                if any_pixel {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            shadow.as_ptr() as *const u8,
                            fb.add(row_base),
                            CHAR_WIDTH * 4,
                        );
                    }
                }
            }
        } else {
            for y in 0..CHAR_HEIGHT {
                let row_base = base + sb * y;
                let glyph_row = y * CHAR_WIDTH;
                let mut p = unsafe { self.fb_ptr.add(row_base) };
                for x in 0..CHAR_WIDTH {
                    let i = glyph[glyph_row + x];
                    if i > 0 {
                        let i16 = i as u16;
                        let pr = ((r as u16 * i16 + 127) >> 8) as u8;
                        let pg = ((g as u16 * i16 + 127) >> 8) as u8;
                        let pb = ((b as u16 * i16 + 127) >> 8) as u8;
                        unsafe {
                            if is_bgr {
                                *p = pb; *p.add(1) = pg; *p.add(2) = pr;
                            } else {
                                *p = pr; *p.add(1) = pg; *p.add(2) = pb;
                            }
                        }
                    }
                    p = unsafe { p.add(3) };
                }
            }
        }
    }

    #[inline]
    fn render_raster(&mut self, raster: &noto_sans_mono_bitmap::RasterizedChar, px: usize, py: usize, color: [u8; 3]) {
        let [r, g, b] = color;
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        let is_bgr = self.is_bgr;
        let fb = self.fb_ptr;
        for (y, row) in raster.raster().iter().enumerate() {
            let row_base = sb * (py + y) + px * bpp;
            if bpp == 4 {
                let rlen = row.len().min(self.shadow_line.len());
                let shadow = &mut self.shadow_line[..rlen];
                let mut any = false;
                for (x, byte) in row.iter().enumerate() {
                    if x >= rlen { break; }
                    if *byte > 0 {
                        let i = *byte as u16;
                        let pr = ((r as u16 * i + 127) >> 8) as u8;
                        let pg = ((g as u16 * i + 127) >> 8) as u8;
                        let pb = ((b as u16 * i + 127) >> 8) as u8;
                        shadow[x] = Self::pixel_value(pr, pg, pb, is_bgr);
                        any = true;
                    } else {
                        shadow[x] = 0;
                    }
                }
                if any {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            shadow.as_ptr() as *const u8,
                            fb.add(row_base),
                            rlen * 4,
                        );
                    }
                }
            } else {
                let mut p = unsafe { fb.add(row_base) };
                for byte in row.iter() {
                    if *byte > 0 {
                        let i = *byte as u16;
                        let pr = ((r as u16 * i + 127) >> 8) as u8;
                        let pg = ((g as u16 * i + 127) >> 8) as u8;
                        let pb = ((b as u16 * i + 127) >> 8) as u8;
                        unsafe {
                            if is_bgr {
                                *p = pb; *p.add(1) = pg; *p.add(2) = pr;
                            } else {
                                *p = pr; *p.add(1) = pg; *p.add(2) = pb;
                            }
                        }
                    }
                    p = unsafe { p.add(3) };
                }
            }
        }
    }

    pub fn render_char_at(&mut self, c: char, px: usize, py: usize, r: u8, g: u8, b: u8) {
        let color = [r, g, b];
        if let Some((glyph, _)) = crate::font::get_glyph(c) {
            self.render_glyph(glyph, px, py, color);
        } else if let Some(raster) = noto_sans_mono_bitmap::get_raster(
            c,
            noto_sans_mono_bitmap::FontWeight::Regular,
            noto_sans_mono_bitmap::RasterHeight::Size16,
        ) {
            self.render_raster(&raster, px, py, color);
        }
    }

    pub fn write_pixel(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        self.write_pixel_direct(x, y, r, g, b);
    }

    pub fn fb_width(&self) -> usize { self.width }
    pub fn fb_height(&self) -> usize { self.height }
    pub fn fb_is_bgr(&self) -> bool { self.is_bgr }

    /// Paint a filled rectangle at (x, y) with the given dimensions. Pixels
    /// outside the framebuffer are clipped. Uses memset for the common 32bpp
    /// case when the colour bytes repeat, otherwise a row-by-row fill.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
        if w == 0 || h == 0 || x >= self.width || y >= self.height { return; }
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        let pixel = self.make_pixel(r, g, b);
        let bpp = self.bpp;
        let sb = self.stride_bytes;
        for row in 0..h {
            let base = sb * (y + row) + x * bpp;
            if base + w * bpp > self.fb_len { break; }
            unsafe {
                let mut p = self.fb_ptr.add(base);
                if bpp == 4 {
                    for _ in 0..w {
                        (p as *mut u32).write_unaligned(pixel);
                        p = p.add(4);
                    }
                } else {
                    for _ in 0..w {
                        *p         = pixel as u8;
                        *p.add(1)  = (pixel >> 8) as u8;
                        *p.add(2)  = (pixel >> 16) as u8;
                        p = p.add(bpp);
                    }
                }
            }
        }
    }

    /// Paint a horizontal gradient from `left` to `right` across the given
    /// rectangle. Useful for colour-bar tests.
    pub fn fill_hgradient(
        &mut self,
        x: usize, y: usize, w: usize, h: usize,
        left: (u8, u8, u8), right: (u8, u8, u8),
    ) {
        if w == 0 || h == 0 { return; }
        for col in 0..w {
            let t = col as u32;
            let denom = (w.max(1) - 1).max(1) as u32;
            let lerp = |a: u8, b: u8| -> u8 {
                let a = a as i32;
                let b = b as i32;
                (a + (b - a) * t as i32 / denom as i32) as u8
            };
            let r = lerp(left.0, right.0);
            let g = lerp(left.1, right.1);
            let b = lerp(left.2, right.2);
            self.fill_rect(x + col, y, 1, h, r, g, b);
        }
    }

    #[inline(always)]
    pub fn write_pixel_direct(&mut self, x: usize, y: usize, r: u8, g: u8, b: u8) {
        if x >= self.width || y >= self.height { return; }
        let off = self.stride_bytes * y + x * self.bpp;
        let pixel = self.make_pixel(r, g, b);
        self.put_pixel_unchecked(off, pixel);
    }

    #[inline(always)]
    fn pixel_value(r: u8, g: u8, b: u8, is_bgr: bool) -> u32 {
        if is_bgr {
            0xFF00_0000 | (r as u32) << 16 | (g as u32) << 8 | (b as u32)
        } else {
            0xFF00_0000 | (b as u32) << 16 | (g as u32) << 8 | (r as u32)
        }
    }

    #[inline(always)]
    fn make_pixel(&self, r: u8, g: u8, b: u8) -> u32 {
        Self::pixel_value(r, g, b, self.is_bgr)
    }

    #[inline(always)]
    fn put_pixel_unchecked(&mut self, off: usize, pixel: u32) {
        if self.bpp == 4 {
            if off + 4 <= self.fb_len {
                unsafe { (self.fb_ptr.add(off) as *mut u32).write_unaligned(pixel); }
            }
        } else {
            if off + 3 <= self.fb_len {
                unsafe {
                    let p = self.fb_ptr.add(off);
                    *p = pixel as u8;
                    *p.add(1) = (pixel >> 8) as u8;
                    *p.add(2) = (pixel >> 16) as u8;
                }
            }
        }
    }
}

impl fmt::Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            self.write_char(c);
        }
        Ok(())
    }
}
