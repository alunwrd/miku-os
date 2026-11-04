// A surface in video memory, and drawing into it with the CPU
//
// Software rasteriser: rectangles, blits, glyphs, a test pattern. It addresses
// pixels through a `pramin::Window` and knows nothing of which GPU scans the
// surface, if any.
//
// Everything here writes as few pixels as possible: the write goes through
// PRAMIN, the 1 MiB BAR0 window, at one MMIO write per pixel, the only path to
// video memory once GSP-RM has remapped BAR1 to virtual mode. Hence rectangles,
// and glyphs that skip their blank pixels.

use super::mmio::MmioRegion;
use super::pramin;
use crate::io::framebuffer::font;

extern crate alloc;

/// A surface being scanned out, and the geometry needed to address it
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// Physical video-memory address of pixel (0, 0)
    pub vram: u64,
    /// Bytes per scanline
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
}

/// Packed 8-bit-per-channel colour, as the surface stores it: A8R8G8B8
pub const fn rgb(r: u8, g: u8, b: u8) -> u32 {
    0xff00_0000 | ((r as u32) << 16) | ((g as u32) << 8) | b as u32
}

impl Surface {
    fn px(&self, x: u32, y: u32) -> u64 {
        self.vram + (y as u64) * (self.pitch as u64) + (x as u64) * 4
    }

    /// One horizontal run of already-packed pixels.
    ///
    /// The window is passed in: a caller pushing a rectangle re-uses one window
    /// for every row, and re-basing the aperture is the only part of a PRAMIN
    /// write that is not a single store
    pub fn write_span(&self, win: &mut pramin::Window, x: u32, y: u32, pixels: &[u32]) {
        if y >= self.height || x >= self.width {
            return;
        }
        let n = pixels.len().min((self.width - x) as usize);
        let mut addr = self.px(x, y);
        for p in &pixels[..n] {
            win.write32(addr, *p);
            addr += 4;
        }
    }

    /// Clip a rectangle to the surface. Returns nothing when it falls entirely
    /// outside
    fn clip(&self, x: u32, y: u32, w: u32, h: u32) -> Option<(u32, u32, u32, u32)> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let w = w.min(self.width - x);
        let h = h.min(self.height - y);
        (w != 0 && h != 0).then_some((x, y, w, h))
    }

    /// One rectangle in one colour. A whole scanline is a run of consecutive
    /// addresses, so `fill32` re-points the aperture at most once per megabyte
    pub fn fill_rect(&self, bar0: &MmioRegion, x: u32, y: u32, w: u32, h: u32, color: u32) {
        let Some((x, y, w, h)) = self.clip(x, y, w, h) else {
            return;
        };
        let mut win = pramin::Window::new(bar0);
        for row in y..y + h {
            win.fill32(self.px(x, row), color, w as usize);
        }
    }

    /// The whole surface in one colour. 1920x1080 is two million MMIO writes: a
    /// setup operation, not per frame
    pub fn fill(&self, bar0: &MmioRegion, color: u32) {
        self.fill_rect(bar0, 0, 0, self.width, self.height, color);
    }

    /// Copy a CPU-side image, one scanline at a time. `src_pitch` is the
    /// source's stride in bytes, which need not match the surface's. Nothing
    /// calls it yet
    #[allow(dead_code)]
    pub fn blit(
        &self,
        bar0: &MmioRegion,
        x: u32,
        y: u32,
        w: u32,
        h: u32,
        src: &[u8],
        src_pitch: u32,
    ) {
        let Some((x, y, w, h)) = self.clip(x, y, w, h) else {
            return;
        };
        let mut win = pramin::Window::new(bar0);
        for row in 0..h {
            let base = (row as usize) * (src_pitch as usize);
            for col in 0..w as usize {
                let i = base + col * 4;
                let Some(b) = src.get(i..i + 4) else { return };
                win.write32(
                    self.px(x + col as u32, y + row),
                    u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                );
            }
        }
    }

    /// One character at a pixel position, in the console's own font.
    ///
    /// Blank glyph pixels are skipped: most of a glyph is empty, and this cuts
    /// the traffic per line by roughly two thirds. The caller clears the row
    /// first if needed
    pub fn glyph(&self, win: &mut pramin::Window, c: char, x: u32, y: u32, color: u32) {
        let Some((glyph, _)) = font::get_glyph(c) else {
            return;
        };
        let (cw, ch) = (font::CHAR_WIDTH as u32, font::CHAR_HEIGHT as u32);
        if x + cw > self.width || y + ch > self.height {
            return;
        }
        let (r, g, b) = ((color >> 16) & 0xff, (color >> 8) & 0xff, color & 0xff);
        for gy in 0..ch {
            for gx in 0..cw {
                // The font is antialiased: the byte is coverage, not a bit
                let cov = glyph[(gy * cw + gx) as usize] as u32;
                if cov == 0 {
                    continue;
                }
                let mix = |v: u32| ((v * cov) / 255) & 0xff;
                win.write32(
                    self.px(x + gx, y + gy),
                    0xff00_0000 | (mix(r) << 16) | (mix(g) << 8) | mix(b),
                );
            }
        }
    }

    /// A string at a pixel position. Returns the x it ended at
    pub fn text(&self, bar0: &MmioRegion, x: u32, y: u32, s: &str, color: u32) -> u32 {
        let mut win = pramin::Window::new(bar0);
        let cw = font::CHAR_WIDTH as u32;
        let mut px = x;
        for c in s.chars() {
            if px + cw > self.width {
                break;
            }
            self.glyph(&mut win, c, px, y, color);
            px += cw;
        }
        px
    }

    /// Make everything written so far visible to the display engine. False if
    /// the flush register never reported completion: the picture may be stale
    pub fn flush(&self, bar0: &MmioRegion) -> bool {
        pramin::flush_fb(bar0)
    }

    /// A test image with no memory of its own: four colour bars, a white border
    /// and a diagonal. Each is a different code path (runs, single pixels,
    /// clipping), so a partly working write path looks wrong
    pub fn draw_test_pattern(&self, bar0: &MmioRegion) {
        const BARS: [u32; 4] = [rgb(0, 0, 0), rgb(255, 0, 0), rgb(0, 255, 0), rgb(0, 0, 255)];
        let bar_w = self.width / BARS.len() as u32;
        for (i, c) in BARS.iter().enumerate() {
            self.fill_rect(bar0, i as u32 * bar_w, 0, bar_w, self.height, *c);
        }
        // Border
        self.fill_rect(bar0, 0, 0, self.width, 2, rgb(255, 255, 255));
        self.fill_rect(bar0, 0, self.height - 2, self.width, 2, rgb(255, 255, 255));
        self.fill_rect(bar0, 0, 0, 2, self.height, rgb(255, 255, 255));
        self.fill_rect(bar0, self.width - 2, 0, 2, self.height, rgb(255, 255, 255));
        // Diagonal, one pixel per column
        let mut win = pramin::Window::new(bar0);
        for x in 0..self.width {
            let y = x * self.height / self.width;
            if y < self.height {
                win.write32(self.px(x, y), rgb(255, 255, 255));
            }
        }
    }
}
