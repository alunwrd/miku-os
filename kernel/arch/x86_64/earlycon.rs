//! Boot progress before the real console exists.
//!
//! gdt, IDT, the heap and the scheduler all come up before anything can print,
//! so a hang there leaves a black screen and, without a serial port, nothing to
//! go on.
//!
//! This paints straight into the framebuffer GRUB handed over, with the font as
//! a plain static array. It depends on no heap, locks, console state or GDT of
//! our own, only the HHDM mapping the boot stub installs. The real console
//! clears the screen later.

use super::grub;
use crate::io::framebuffer::font;
use core::sync::atomic::{AtomicU32, Ordering};

static LINE: AtomicU32 = AtomicU32::new(0);

struct Fb {
    base:   *mut u8,
    pitch:  usize,
    width:  usize,
    height: usize,
    bytes:  usize,
}

fn fb() -> Option<Fb> {
    let i = grub::framebuffer()?;
    // fb_type 1 (RGB) is all 'grub::framebuffer' returns; an EGA-text or
    // absent framebuffer leaves us on the serial line alone
    if i.addr == 0 || i.pitch == 0 || i.width == 0 || i.height == 0 {
        return None;
    }
    if i.bpp != 32 && i.bpp != 24 {
        return None;
    }
    if !super::boot_entry::direct_mapped(i.addr, i.pitch as u64 * i.height as u64) {
        return None;
    }
    Some(Fb {
        base:   (i.addr + grub::HHDM_OFFSET) as *mut u8,
        pitch:  i.pitch as usize,
        width:  i.width as usize,
        height: i.height as usize,
        bytes:  (i.bpp / 8) as usize,
    })
}

#[inline]
fn put_px(f: &Fb, x: usize, y: usize, r: u8, g: u8, b: u8) {
    if x >= f.width || y >= f.height {
        return;
    }
    let off = y * f.pitch + x * f.bytes;
    if off + f.bytes > f.pitch * f.height {
        return;
    }
    unsafe {
        let p = f.base.add(off);
        // Byte order matches what the main console assumes (is_bgr: true)
        p.write_volatile(b);
        p.add(1).write_volatile(g);
        p.add(2).write_volatile(r);
    }
}

fn fill(f: &Fb, x0: usize, y0: usize, w: usize, h: usize, r: u8, g: u8, b: u8) {
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            put_px(f, x, y, r, g, b);
        }
    }
}

/// Draw one line of ASCII. Coverage from the antialiased font is folded into
/// the colour so the text looks the same as the console's
fn draw_text(f: &Fb, x0: usize, y0: usize, s: &str, r: u8, g: u8, b: u8) {
    let mut x = x0;
    for c in s.chars() {
        if x + font::CHAR_WIDTH >= f.width {
            break;
        }
        if let Some((glyph, _)) = font::get_glyph(c) {
            for gy in 0..font::CHAR_HEIGHT {
                for gx in 0..font::CHAR_WIDTH {
                    let cov = glyph[gy * font::CHAR_WIDTH + gx] as u32;
                    if cov == 0 {
                        continue;
                    }
                    put_px(
                        f,
                        x + gx,
                        y0 + gy,
                        (r as u32 * cov / 255) as u8,
                        (g as u32 * cov / 255) as u8,
                        (b as u32 * cov / 255) as u8,
                    );
                }
            }
        }
        x += font::CHAR_WIDTH;
    }
}

fn emit(msg: &str, r: u8, g: u8, b: u8) {
    let line = LINE.fetch_add(1, Ordering::Relaxed) as usize;
    let Some(f) = fb() else { return };
    // Bottom-up: the real console starts writing at the top and would overwrite
    // these lines
    let from_bottom = (line + 1) * font::CHAR_HEIGHT;
    if from_bottom > f.height {
        return;
    }
    let y = f.height - from_bottom;
    // Clear the row first: GRUB leaves its menu on screen
    fill(&f, 0, y, f.width, font::CHAR_HEIGHT, 0, 0, 0);
    draw_text(&f, 0, y, msg, r, g, b);
}

/// First thing the kernel does. Says which firmware path we came in on
pub fn init() {
    let via = if grub::booted_via_uefi() { "UEFI" } else { "BIOS" };
    crate::serial_println!("[early] handoff ok, firmware path: {}", via);
    match grub::framebuffer() {
        Some(i) => {
            crate::serial_println!(
                "[early] framebuffer {}x{} bpp={} pitch={} addr={:#x}",
                i.width, i.height, i.bpp, i.pitch, i.addr
            );
            emit("MikuOS: kernel entered", 120, 220, 255);
            let mut buf = [0u8; 64];
            let n = fmt_kv(&mut buf, "  firmware: ", via);
            emit(core::str::from_utf8(&buf[..n]).unwrap_or("  firmware: ?"), 150, 160, 170);
        }
        None => {
            // Nothing to draw on. Serial is the only channel left
            crate::serial_println!("[early] no RGB framebuffer from GRUB, screen stays blank");
        }
    }
}

/// One boot step done. Called from kernel_main between the phases before the
/// console; the last line on screen is the phase that hung
pub fn step(name: &str) {
    crate::serial_println!("[early] ok: {}", name);
    let mut buf = [0u8; 64];
    let n = fmt_kv(&mut buf, "  ok: ", name);
    emit(core::str::from_utf8(&buf[..n]).unwrap_or("  ok"), 120, 220, 150);
}

/// Concatenate into a stack buffer - no allocator this early
fn fmt_kv(buf: &mut [u8], prefix: &str, value: &str) -> usize {
    let mut n = 0;
    for b in prefix.bytes().chain(value.bytes()) {
        if n >= buf.len() {
            break;
        }
        buf[n] = b;
        n += 1;
    }
    n
}
