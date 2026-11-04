// The cursor, composed by hand
//
// The copy engine cannot blend, and the cursor is drawn over a finished frame.
// So the processor composes a small patch (what is underneath, then the image
// on top), put down as one opaque rectangle.
//
// Not on a head with a cursor plane, the normal case: the plane composites over
// the finished viewport from its own surface in video memory. This file is the
// path for everything else.
//
// Both allocations here are made once. The patch is at most 64 by 64, filled
// and drained inside one frame

use alloc::vec::Vec;
use spin::Mutex;

use super::damage::{Rect};

/// Where the cursor was last frame, so the frame moving it repaints what it
/// left behind
pub(super) static CURSOR_LAST: Mutex<Option<Rect>> = Mutex::new(None);

/// A page of contiguous memory the copy engine can read the cursor patch out
/// of, allocated once
pub(super) static CURSOR_DMA: Mutex<Option<crate::mm::dma::DmaBuffer>> = Mutex::new(None);

/// Scratch the composed cursor is built in, kept between frames
///
/// 64 by 64 at most, filled and drained inside one frame. `patch` resizes it,
/// keeping the capacity, so after the first frame this asks the allocator for
/// nothing
pub(super) static CURSOR_PX: Mutex<Vec<u32>> = Mutex::new(Vec::new());

/// Compose the cursor over what is under it and put it where the engine can
/// reach it. Returns the rectangle to blit
///
/// The background comes from `compose::compose_into`, the composer of the frame
/// this patch is laid on, asked as `Dress::Patch` so it reproduces the shadow,
/// the corner and the ring whoever drew them
pub(super) fn cursor_patch(
    screen: &Rect,
    list: &[crate::mayland::server::Presentable],
) -> Option<(i32, i32, i32, i32)> {
    let (x, y, w, h) = crate::mayland::input::pointer::frame_rect()?;
    let area = Rect::new(x, y, w, h);
    // Clipped: the cursor may sit against the right and bottom edges, and the
    // engine takes an address at its word
    if area.intersect(screen)? != area {
        // Partly off the edge. Copying only the visible columns would need the
        // patch re-packed at a different pitch; it is skipped for the frame it
        // is half outside
        return None;
    }

    let mut px = CURSOR_PX.lock();
    px.clear();
    px.resize((w * h) as usize, 0);
    {
        // SAFETY: a `Vec<u32>` is `w * h` four-byte words end to end, the
        // `XRGB8888` rows with no padding that `compose_into` writes. The alias
        // lasts for the call and nothing else reads `px` meanwhile
        let bytes = unsafe {
            core::slice::from_raw_parts_mut(
                px.as_mut_ptr() as *mut u8, px.len() * 4)
        };
        super::compose::compose_into(
            bytes, w as usize, area, list, screen, super::compose::Dress::Patch);
    }
    crate::mayland::input::pointer::draw_over(&mut px);

    let mut dma = CURSOR_DMA.lock();
    if dma.is_none() {
        // Four pages: a client cursor at the largest size the pointer draws (64
        // x 64 x 4 bytes)
        *dma = crate::mm::dma::DmaBuffer::alloc(4).ok();
    }
    let buf = dma.as_mut()?;
    let bytes = buf.as_mut_slice();
    if bytes.len() < px.len() * 4 {
        return None;
    }
    for (i, p) in px.iter().enumerate() {
        bytes[i * 4..i * 4 + 4].copy_from_slice(&p.to_le_bytes());
    }
    Some((x, y, w, h))
}

// The cursor the head draws

// The head has a cursor plane that composites over the finished viewport from
// its own small surface in video memory, fed by a PIO channel that takes a
// position at any point in any frame.
//
// With the plane up, none of the work above happens. A mouse moving over a
// still desktop damages, composes and copies nothing (`present` returns early
// with no frame), and the pointer moves through two stores into BAR0 from the
// input path.
//
// `nohwcursor` on the boot line selects the software path, which also runs on
// any output with no plane.

use core::sync::atomic::{AtomicU32, Ordering};


/// Is the head drawing the pointer? When true, nothing here composes one and
/// `present` skips the two rectangles it would have repaired.
///
/// Asked every time, not cached. The plane is torn down with the display
/// (`gb206-reset` clears it before the FLR), and a compositor on a cached "yes"
/// would draw no pointer at all
pub(crate) fn hardware() -> bool {
    super::backend().map(|b| b.cursor_available()).unwrap_or(false)
}

/// The image last handed to the plane, so an unchanged cursor is not copied
/// into video memory every frame
static HW_IMAGE: Mutex<Option<(Vec<u32>, i32, i32, i32, i32)>> = Mutex::new(None);

/// Put the pointer where it is now.
///
/// Called from the compositor iteration right after the mouse is drained, not
/// from a composed frame: the position lands whether or not this iteration
/// composes, so a mouse over a still desktop costs a drain and two stores
pub(crate) fn hw_move() {
    if !hardware() {
        return;
    }
    let (x, y) = crate::mayland::input::pointer::position();
    if super::backend().map(|b| b.cursor_move(x, y)).unwrap_or(false) {
        MISSES.store(0, Ordering::Relaxed);
        return;
    }
    // A channel refusing a method is a pointer that stopped moving, which makes
    // the machine look hung. One refusal is a frame; a run is the plane gone,
    // so it is given up and the compositor draws the pointer from the next
    // frame, which `force_full_frame` ensures on a still desktop.
    //
    // Counted in frames, scaled by the panel's rate, so the wall-clock patience
    // is the same on any panel
    if MISSES.fetch_add(1, Ordering::Relaxed) + 1 >= miss_limit() {
        if let Some(b) = super::backend() { b.cursor_release(); }
        *HW_IMAGE.lock() = None;
        MISSES.store(0, Ordering::Relaxed);
        super::force_full_frame();
        crate::serial_println!(
            "[mayland] the cursor channel stopped taking methods - \
             the compositor draws the pointer again");
    }
}

/// How many refused moves in a row mean the plane is gone.
///
/// Eight frames at 60 Hz, an eighth of a second, as however many frames that is
/// at the panel's rate. At 240 Hz, thirty-two
fn miss_limit() -> u32 {
    let hz = super::backend()
        .and_then(|b| b.output(0))
        .map(|o| (o.refresh_mhz / 1000).max(1))
        .unwrap_or(60);
    (8 * hz.max(1) / 60).clamp(8, 256)
}

/// Consecutive moves the channel would not take
static MISSES: AtomicU32 = AtomicU32::new(0);

/// Make the plane's image agree with what the pointer should look like.
///
/// Called once per compositor iteration, composing or not: a client can commit
/// a new cursor into the same buffer, so the pixels are compared, not the
/// buffer's identity. Building them costs at most 4096 words; copying into
/// video memory costs a 16 KiB PRAMIN write, the conditional one
pub(crate) fn hw_sync() {
    if !hardware() {
        return;
    }
    let mut px: Vec<u32> = Vec::new();
    let shape = crate::mayland::input::pointer::image_argb(&mut px);
    let mut last = HW_IMAGE.lock();
    match shape {
        None => {
            if last.is_some() {
                if let Some(b) = super::backend() { b.cursor_hide(); }
                *last = None;
            }
        }
        Some((w, h, hx, hy)) => {
            let same = matches!(
                last.as_ref(),
                Some((old, ow, oh, ox, oy))
                    if *ow == w && *oh == h && *ox == hx && *oy == hy && *old == px
            );
            if !same {
                if !super::backend()
                    .map(|b| b.cursor_set_image(&px, w as u32, h as u32, hx, hy))
                    .unwrap_or(false)
                {
                    return;
                }
                *last = Some((px, w, h, hx, hy));
            }
            // A cursor hidden and now visible goes back on the screen:
            // `cursor_hide` parked it past the raster, and nothing else moves
            // it until the mouse does
            drop(last);
            hw_move();
        }
    }
}

/// Take the pointer off the screen for good: the compositor is stopping and
/// nothing will draw over the plane
pub(crate) fn hw_hide() {
    if !hardware() {
        return;
    }
    if let Some(b) = super::backend() { b.cursor_hide(); }
    *HW_IMAGE.lock() = None;
}
