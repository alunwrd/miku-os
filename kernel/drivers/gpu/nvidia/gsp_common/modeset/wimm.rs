// The window-immediate channel (NVCA7B / NVC37B).

use crate::drivers::gpu::nvidia::le;
use super::super::pushbuf::{PushBuf, PushError};

// The modeset modules are re-exported flat from `mod.rs`

// Window-immediate channel (NVCA7B / NVC37B)

// Needed even for a static image: whatever position the firmware left stays in
// effect otherwise

pub const M_WIMM_UPDATE: u32 = 0x0200;
pub const M_WIMM_SET_POINT_OUT: u32 = 0x0208;

/// Place the window's top-left corner on the head, then commit
pub fn wimm_set_point(push: &mut PushBuf, x: u32, y: u32) -> Result<(), PushError> {
    push.mthd1(M_WIMM_SET_POINT_OUT, (x & 0xffff) | (y << 16))?;
    // Bit 0 commits; INTERLOCK_WITH_WINDOW (bit 1) stays clear: this update
    // stands on its own
    push.mthd1(M_WIMM_UPDATE, 1)
}

/// Paint a test pattern, where each way of being wrong looks different.
///
///   Vertical colour bars in a known order. Red where blue is drawn means the component order is reversed: A8R8G8B8 against A8B8G8R8.
///   A one-pixel white border. Missing on one edge means the surface does not cover the raster.
///   A diagonal from corner to corner. A wrong pitch leaves the image filling the screen, sheared; a straight diagonal says the stride is right
pub fn paint_test_pattern(fb: &mut [u8], pitch: u32, width: u32, height: u32) {
    const BARS: [u32; 8] = [
        0xff00_0000, // black
        0xffff_0000, // red
        0xff00_ff00, // green
        0xff00_00ff, // blue
        0xffff_ff00, // yellow
        0xffff_00ff, // magenta
        0xff00_ffff, // cyan
        0xffff_ffff, // white
    ];
    const WHITE: u32 = 0xffff_ffff;

    let pitch = pitch as usize;
    let put = |fb: &mut [u8], x: u32, y: u32, c: u32| {
        let off = y as usize * pitch + x as usize * 4;
        if off + 4 <= fb.len() {
            le::put_u32(fb, off, c);
        }
    };

    for y in 0..height {
        let row = y as usize * pitch;
        if row + width as usize * 4 > fb.len() {
            break;
        }
        for x in 0..width {
            let bar = (x * BARS.len() as u32 / width.max(1)) as usize;
            let c = BARS[bar.min(BARS.len() - 1)];
            fb[row + x as usize * 4..row + x as usize * 4 + 4]
                .copy_from_slice(&c.to_le_bytes());
        }
    }

    for x in 0..width {
        put(fb, x, 0, WHITE);
        put(fb, x, height - 1, WHITE);
        // Two pixels wide, so a slightly wrong pitch still shows a line
        let y = x * height / width.max(1);
        put(fb, x, y.min(height - 1), WHITE);
        put(fb, x, (y + 1).min(height - 1), WHITE);
    }
    for y in 0..height {
        put(fb, 0, y, WHITE);
        put(fb, width - 1, y, WHITE);
    }
}

/// The pixels the boot's memory checks read back, and what they should hold.
///
/// Four single pixels, none the colour of the surface around it or of memory
/// nobody wrote, so a blank surface keeps the check
pub fn late_probe_points(pitch: u32) -> [(u64, u32); 4] {
    [
        (0, 0xffff_ffff),
        (pitch as u64 * 100 + 4 * 20, 0xff00_0000),
        (pitch as u64 * 100 + 4 * 300, 0xffff_0000),
        (pitch as u64 * 100 + 4 * 700, 0xffff_ff00),
    ]
}

/// A blank surface: opaque black, plus the probe pixels above.
///
/// Opaque: video memory nobody wrote reads as zero, and transparent black would
/// look like a copy that never happened, what 'R fbmem' detects
pub fn paint_blank(fb: &mut [u8], pitch: u32, width: u32, height: u32) {
    const BLACK: u32 = 0xff00_0000;
    let pitch = pitch as usize;
    for y in 0..height as usize {
        let row = y * pitch;
        let end = row + width as usize * 4;
        if end > fb.len() {
            break;
        }
        for px in fb[row..end].chunks_exact_mut(4) {
            px.copy_from_slice(&BLACK.to_le_bytes());
        }
    }
    for (off, want) in late_probe_points(pitch as u32) {
        let o = off as usize;
        if o + 4 <= fb.len() {
            le::put_u32(fb, o, want);
        }
    }
}


////////////////////////////////////////////////////////////////////////
