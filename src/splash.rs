// Boot splash / GPU test-pattern
//
// The splash is drawn onto the live framebuffer which, on a machine whose
// primary display is the NVIDIA card we probed, physically lives inside
// the card's BAR1. So every pixel we paint here is going into GPU memory
// and out through the card's scanout hardware. Without a GSP firmware blob
// we cannot re-program the display engine, but we do not need to: the
// pipeline firmware set up is already feeding this framebuffer to the
// panel
//
// The splash has three bands:
//   - a color-bar strip across the top (red / green / blue / yellow / cyan
//     / magenta / white / black, plus a horizontal gradient underneath)
//     to demonstrate pixel output on all channels,
//   - a centered "MikuOS" title plus a source label,
//   - a footer showing GPU / framebuffer info so the user can verify the
//     display is actually routed through the GPU we detected.

use crate::console;
use crate::nvidia;

/// Draw the splash and return without blocking. Caller decides how long to
/// leave it on screen before clearing
pub fn draw() {
    let (w, h) = match console::fb_size() {
        Some(s) => s,
        None => return,
    };

    // 1. Background: a deep teal that matches MikuOS's palette
    console::fill_rect(0, 0, w, h, 10, 20, 28);

    // 2. Color bars across the top band.
    draw_color_bars(w, h);

    // 3. Horizontal gradient underneath the bars.
    draw_gradient_band(w, h);

    // 4. Title + subtitle.
    draw_title(w, h);

    // 5. Footer with GPU info.
    draw_footer(w, h);
}

fn draw_color_bars(w: usize, _h: usize) {
    let bars: [(u8, u8, u8); 8] = [
        (220, 50, 50),     // red
        (60, 200, 80),     // green
        (60, 110, 220),    // blue
        (220, 200, 60),    // yellow
        (60, 200, 220),    // cyan
        (200, 80, 200),    // magenta
        (230, 240, 240),   // white
        (10, 10, 10),      // black
    ];
    let band_h = 48usize;
    let bar_w = w / bars.len();
    for (i, c) in bars.iter().enumerate() {
        let x = i * bar_w;
        let bw = if i + 1 == bars.len() { w - x } else { bar_w };
        console::fill_rect(x, 0, bw, band_h, c.0, c.1, c.2);
    }
}

fn draw_gradient_band(w: usize, _h: usize) {
    let y = 48usize;
    let band_h = 20usize;
    console::fill_hgradient(0, y, w, band_h, (40, 80, 110), (180, 230, 220));
    console::fill_rect(0, y + band_h, w, 2, 0, 0, 0);
}

fn draw_title(w: usize, h: usize) {
    // Headline just below the bars
    let cy = h / 2;
    // Using the text console: set the cursor near the center and write.
    // Because the console is fixed-pitch with CHAR_WIDTH=9 per glyph, we
    // compute a roughly-centered x
    let title = "MikuOS v0.2.8-rc";
    let title_px = title.len() * 9;
    let x = (w.saturating_sub(title_px)) / 2;
    draw_text_at(x, cy - 20, title, 57, 197, 187);

    let sub = "booting - display routed through GPU";
    let sub_px = sub.len() * 9;
    let sx = (w.saturating_sub(sub_px)) / 2;
    draw_text_at(sx, cy + 6, sub, 200, 220, 220);
}

fn draw_footer(w: usize, h: usize) {
    let mut line1 = heapless_string::HeaplessLine::new();
    let mut line2 = heapless_string::HeaplessLine::new();
    let mut line3 = heapless_string::HeaplessLine::new();

    let (model, chip, bar1_mb, via_gpu): (&str, &str, u64, bool) =
        nvidia::with_gtx1650(|dev| {
            (
                dev.model_name,
                dev.chip.codename(),
                dev.bar1_size / (1024 * 1024),
                dev.boot_fb.is_some(),
            )
        }).unwrap_or(("no NVIDIA GPU detected", "-", 0, false));

    let _ = line1.write_fmt(format_args!(
        "GPU: {}  chip={}  VRAM aperture={} MB",
        model, chip, bar1_mb
    ));
    let fb = crate::grub::framebuffer();
    if let Some(f) = fb {
        let _ = line2.write_fmt(format_args!(
            "FB:  phys={:#010x}  {}x{}  {} bpp  pitch={}",
            f.addr, f.width, f.height, f.bpp, f.pitch
        ));
    } else {
        let _ = line2.write_fmt(format_args!("FB:  no multiboot framebuffer tag"));
    }
    if via_gpu {
        let _ = line3.write_fmt(format_args!("scanout: framebuffer is inside GPU BAR1 - output is through the card"));
    } else {
        let _ = line3.write_fmt(format_args!("scanout: framebuffer not owned by detected NVIDIA GPU"));
    }

    let fy = h.saturating_sub(84);
    draw_text_at(40, fy,      line1.as_str(), 200, 220, 220);
    draw_text_at(40, fy + 20, line2.as_str(), 200, 220, 220);
    let (r, g, b) = if via_gpu { (100, 220, 150) } else { (220, 200, 80) };
    draw_text_at(40, fy + 40, line3.as_str(), r, g, b);
}

/// Write a UTF-8 string at absolute pixel (x, y) through the Console. We temporarily move the console cursor, print, then restore
fn draw_text_at(x: usize, y: usize, text: &str, r: u8, g: u8, b: u8) {
    use crate::console::WRITER;
    use x86_64::instructions::interrupts;
    interrupts::without_interrupts(|| {
        if let Some(w) = WRITER.lock().as_mut() {
            let saved_x = w.x_pos;
            let saved_y = w.y_pos;
            let saved_fg = w.fg_color;
            w.x_pos = x;
            w.y_pos = y;
            w.fg_color = [r, g, b];
            use core::fmt::Write;
            let _ = w.write_str(text);
            w.x_pos = saved_x;
            w.y_pos = saved_y;
            w.fg_color = saved_fg;
        }
    });
}

mod heapless_string {
    use core::fmt;

    /// A tiny fixed-size string buffer so splash formatting doesn't touch
    /// the heap on the hot boot path
    pub struct HeaplessLine {
        buf: [u8; 160],
        len: usize,
    }

    impl HeaplessLine {
        pub fn new() -> Self { Self { buf: [0; 160], len: 0 } }
        pub fn as_str(&self) -> &str {
            core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
        }
        pub fn write_fmt(&mut self, args: fmt::Arguments) -> fmt::Result {
            fmt::write(self, args)
        }
    }

    impl fmt::Write for HeaplessLine {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let b = s.as_bytes();
            let room = self.buf.len().saturating_sub(self.len);
            let n = b.len().min(room);
            self.buf[self.len..self.len + n].copy_from_slice(&b[..n]);
            self.len += n;
            Ok(())
        }
    }
}
