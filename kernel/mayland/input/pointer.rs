// The pointer: where the cursor is, and what it is over
//
// The mouse driver counts movement in the interrupt and this turns it into
// protocol events once a frame, from the compositor thread, as for the
// keyboard: sending an event takes the server lock, and an interrupt handler
// may not.
//
// Once a frame is the right rate. A PS/2 mouse reports a hundred times a second
// and the screen shows sixty; a client sent every packet would be woken to draw
// frames nobody sees.
//
// The head's own plane draws the cursor where there is one, on every boot
// without `nohwcursor`. This file then draws nothing:
// `render::cursor::hardware()` is true and `present` never asks for a patch.
//
// The patch runs everywhere else (in a virtual machine, on a head with no
// plane), here because the copy engine cannot blend: a small rectangle is
// composed by hand, screen content first and the arrow over it, and put down as
// one opaque copy.

extern crate alloc;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI32, AtomicU8, AtomicU64, Ordering};

use crate::mayland::server;
use crate::mayland::shell::layout;
use crate::drivers::input::mouse;
// Who owns the pointer when it is not the surface under it, in `grab.rs`
use super::grab::{self, Drag};
// The numbers a config set about the pointer: how far a movement goes and which
// way the wheel turns
use super::conf;

/// Button numbers as evdev names them, as Wayland carries them
pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;
pub const BTN_MIDDLE: u32 = 0x112;

/// The cursor image. A plain arrow: 1 is the outline, 2 the fill, 0 is whatever
/// is behind it
const CUR_W: usize = 12;
const CUR_H: usize = 19;
#[rustfmt::skip]
const CURSOR: [u8; CUR_W * CUR_H] = [
    1,0,0,0,0,0,0,0,0,0,0,0,
    1,1,0,0,0,0,0,0,0,0,0,0,
    1,2,1,0,0,0,0,0,0,0,0,0,
    1,2,2,1,0,0,0,0,0,0,0,0,
    1,2,2,2,1,0,0,0,0,0,0,0,
    1,2,2,2,2,1,0,0,0,0,0,0,
    1,2,2,2,2,2,1,0,0,0,0,0,
    1,2,2,2,2,2,2,1,0,0,0,0,
    1,2,2,2,2,2,2,2,1,0,0,0,
    1,2,2,2,2,2,2,2,2,1,0,0,
    1,2,2,2,2,2,2,2,2,2,1,0,
    1,2,2,2,2,2,2,1,1,1,1,1,
    1,2,2,2,1,2,2,1,0,0,0,0,
    1,2,2,1,1,2,2,1,0,0,0,0,
    1,2,1,0,0,1,2,2,1,0,0,0,
    1,1,0,0,0,1,2,2,1,0,0,0,
    1,0,0,0,0,0,1,2,2,1,0,0,
    0,0,0,0,0,0,1,2,2,1,0,0,
    0,0,0,0,0,0,0,1,1,0,0,0,
];
const OUTLINE: u32 = 0x0000_0000;
const FILL: u32 = 0x00ff_ffff;

static X: AtomicI32 = AtomicI32::new(0);
static Y: AtomicI32 = AtomicI32::new(0);
static BUTTONS: AtomicU8 = AtomicU8::new(0);
static VISIBLE: AtomicBool = AtomicBool::new(false);
/// Set once the cursor has been placed, so the first movement does not drag it
/// out of the top-left corner
static PLACED: AtomicBool = AtomicBool::new(false);

/// The smallest a window may be pulled down to
///
/// Not zero: a window with no width cannot be grabbed by any edge
const MIN_SIZE: i32 = 64;

/// Movement the pointer has made, for the shell to report
static MOVED: AtomicI32 = AtomicI32::new(0);

pub fn position() -> (i32, i32) {
    (X.load(Ordering::Relaxed), Y.load(Ordering::Relaxed))
}

pub fn visible() -> bool {
    VISIBLE.load(Ordering::Relaxed)
}

/// Which buttons are down, as the packet's own bitmap, from `BUTTONS`, which
/// the drain keeps
pub fn buttons() -> u8 {
    BUTTONS.load(Ordering::Relaxed)
}

pub fn moves() -> i32 {
    MOVED.load(Ordering::Relaxed)
}

/// Show the cursor and put it in the middle of the screen
pub fn start(screen_w: i32, screen_h: i32) {
    if !PLACED.swap(true, Ordering::Relaxed) {
        X.store(screen_w / 2, Ordering::Relaxed);
        Y.store(screen_h / 2, Ordering::Relaxed);
    }
    VISIBLE.store(true, Ordering::Relaxed);
}

/// Put the pointer somewhere without a mouse moving it
///
/// For the shell's `mayland pointer`, to exercise a client's enter on a machine
/// that is not compositing
pub fn place(x: i32, y: i32) {
    PLACED.store(true, Ordering::Relaxed);
    X.store(x, Ordering::Relaxed);
    Y.store(y, Ordering::Relaxed);
}

pub fn stop() {
    VISIBLE.store(false, Ordering::Relaxed);
    grab::clear();
}

/// Everything the mouse has done since the last frame, turned into events.
/// Called from the compositor loop, next to the keyboard's drain
pub fn drain(screen_w: i32, screen_h: i32, time_ms: u32) {
    if !VISIBLE.load(Ordering::Relaxed) {
        return;
    }
    let (dx, dy, wheel, raw) = mouse::take();

    // An absolute device states where the pointer is and a relative one how far
    // it went, taken in that order and not added: a delta on top of a stated
    // position would be a second opinion about one pointer.
    //
    // The drag below works on deltas, so an absolute move contributes the
    // distance it covered.
    //
    // Sensitivity comes first, only on the relative device: a position
    // multiplied by a factor is not a slower pointer but one elsewhere than the
    // pen. The branch below discards these two when there is an absolute
    // reading
    let (dx, dy) = conf::scale(dx, dy);
    let (dx, dy) = match mouse::take_abs() {
        Some((ax, ay)) => {
            // 0..=0xFFFF onto the screen. The multiply is in i64: 0xFFFF times
            // a 4K width overflows an i32
            let nx = (ax as i64 * (screen_w - 1) as i64 / 0xFFFF) as i32;
            let ny = (ay as i64 * (screen_h - 1) as i64 / 0xFFFF) as i32;
            (nx - X.load(Ordering::Relaxed), ny - Y.load(Ordering::Relaxed))
        }
        None => (dx, dy),
    };

    let moved = dx != 0 || dy != 0;
    if moved {
        // Clamped to the screen, not wrapped. One pixel short of the far edge,
        // so the hot spot is on a pixel that exists
        let x = (X.load(Ordering::Relaxed) + dx).clamp(0, screen_w - 1);
        let y = (Y.load(Ordering::Relaxed) + dy).clamp(0, screen_h - 1);
        X.store(x, Ordering::Relaxed);
        Y.store(y, Ordering::Relaxed);
        MOVED.fetch_add(1, Ordering::Relaxed);
    }

    // A grab moves or stretches the window and sends nothing: the client is
    // being carried, not pointed at
    let held = grab::drag();
    if let Some(mut g) = held {
        if moved {
            if g.edges == 0 {
                layout::move_window(g.client, g.surface, dx, dy);
            } else {
                resize_by(&mut g, dx, dy);
                grab::update_drag(g);
            }
        }
    } else if moved {
        server::pointer_motion(X.load(Ordering::Relaxed), Y.load(Ordering::Relaxed), time_ms);
    }

    // Buttons are a level in the packet and events are edges, so the change
    // matters
    let was = BUTTONS.swap(raw, Ordering::Relaxed);
    if was != raw {
        for (bit, code) in [
            (mouse::BTN_LEFT, BTN_LEFT),
            (mouse::BTN_RIGHT, BTN_RIGHT),
            (mouse::BTN_MIDDLE, BTN_MIDDLE),
        ] {
            let now = raw & bit != 0;
            if now == (was & bit != 0) {
                continue;
            }
            if !button_is_the_desktop_s(code, now) {
                continue;
            }
            server::pointer_button(code, now, time_ms);
        }
    }

    let wheel = conf::wheel(wheel);
    if wheel != 0 {
        server::pointer_axis(wheel, time_ms);
    }
}

/// Pull one corner of a window towards the pointer
///
/// The edges say which corner is held, and the others do not move: dragging the
/// right edge changes the width and leaves the left. A left or top edge does
/// two things (the window grows and its origin moves the other way), so the
/// move is asked for as well
fn resize_by(g: &mut Drag, dx: i32, dy: i32) {
    use crate::mayland::protocol::wm::edge;

    let (mut ox, mut oy) = (0, 0);
    if g.edges & edge::RIGHT != 0 {
        g.w += dx;
    } else if g.edges & edge::LEFT != 0 {
        // Clamped before use as an offset, or a window at the minimum would
        // keep sliding while its width refused to shrink
        let w = (g.w - dx).max(MIN_SIZE);
        ox = g.w - w;
        g.w = w;
    }
    if g.edges & edge::BOTTOM != 0 {
        g.h += dy;
    } else if g.edges & edge::TOP != 0 {
        let h = (g.h - dy).max(MIN_SIZE);
        oy = g.h - h;
        g.h = h;
    }
    g.w = g.w.max(MIN_SIZE);
    g.h = g.h.max(MIN_SIZE);
    if ox != 0 || oy != 0 {
        layout::move_window(g.client, g.surface, ox, oy);
    }
    server::resize_window(g.client, g.surface, g.w, g.h);
}

/// Whether a button press belongs to the client under the pointer
///
/// Two things can take it away. A mouse binding (`bindm` in the config) is a
/// chord delivered to the window manager; the button numbers are evdev's, the
/// space the chord table already holds keycodes in, so `mouse:272` needs no
/// second mechanism. And a release ending a grab belongs to the compositor, as
/// the client never saw the press.
///
/// False means the client is not told: a press it never saw followed by a
/// release would leave the button stuck down
fn button_is_the_desktop_s(code: u32, pressed: bool) -> bool {
    if !pressed {
        // Any button coming up ends a grab, not only the one that started it:
        // the window manager may have begun the drag from a chord while another
        // button was held, and the window would otherwise follow the pointer
        // for ever
        if grab::end_drag() {
            return false;
        }
        return true;
    }
    // Super and a click is not a tap of Super
    crate::mayland::input::spoil_held(0);
    match crate::mayland::input::chord_for_button(code) {
        Some(id) => {
            // Directly, not into the key ring: this runs on the compositor
            // thread, where taking the server lock is normal
            server::wm_chord(id);
            false
        }
        None => true,
    }
}

/// The position the whole of one frame agrees the pointer is at
///
/// The mouse moves in an interrupt, so `X` and `Y` change between any two
/// reads. A frame reads the pointer twice, to work out which rectangles to
/// repair and to compose the patch; with two different positions the patch
/// lands where nothing was damaged, the rectangle recorded as "where the cursor
/// was" is a third place, and a mark stays on the screen along the pointer's
/// path.
///
/// So the frame latches one position and everything in it uses that: latching
/// input at the top of a repaint
static FRAME_POS: AtomicU64 = AtomicU64::new(0);

fn pack(x: i32, y: i32) -> u64 {
    ((x as u32 as u64) << 32) | y as u32 as u64
}

fn latched() -> (i32, i32) {
    let v = FRAME_POS.load(Ordering::Relaxed);
    ((v >> 32) as u32 as i32, v as u32 as i32)
}

/// The cursor image this frame agrees on, latched with the position
///
/// `FRAME_POS` fixes the moment; this fixes what is drawn at it. The frame asks
/// twice, for the rectangle to repair and for the pixels to put in it, and a
/// client may commit a different cursor in between. With a patch sized from the
/// new image and damage from the old, the part outside the damage stays as a
/// sliver of stale arrow, at every size change, which happens as the pointer
/// crosses a window's edge
static FRAME_IMG: spin::Mutex<Option<Option<server::CursorImage>>> =
    spin::Mutex::new(None);

/// The rectangle a cursor image occupies with its hot spot at `(x, y)`. One
/// place, so the frame's damage and patch agree
fn rect_of(img: &Option<Option<server::CursorImage>>, x: i32, y: i32)
    -> Option<(i32, i32, i32, i32)>
{
    if !VISIBLE.load(Ordering::Relaxed) {
        return None;
    }
    match img {
        // Nobody asked for anything: the compositor's own arrow
        None => Some((x, y, CUR_W as i32, CUR_H as i32)),
        // A client asked for no cursor at all
        Some(None) => None,
        Some(Some(i)) => {
            if !usable(i) {
                return None;
            }
            // The hot spot is the point; the image hangs off it, so a cursor's
            // rectangle starts left of and above the pointer
            Some((x - i.hot_x, y - i.hot_y, i.width, i.height))
        }
    }
}

/// Take the position this frame will use, and return the rectangle it puts the
/// cursor in. Called once, where the frame starts deciding what to repair;
/// `patch` reads the same latch
pub fn latch_rect() -> Option<(i32, i32, i32, i32)> {
    let (x, y) = (X.load(Ordering::Relaxed), Y.load(Ordering::Relaxed));
    FRAME_POS.store(pack(x, y), Ordering::Relaxed);
    let img = server::cursor_image();
    let r = rect_of(&img, x, y);
    *FRAME_IMG.lock() = img;
    r
}

/// The rectangle the cursor occupies now, as (x, y, w, h)
///
/// The live position, for anything outside a frame: the selftest asks where the
/// cursor is after moving it
pub fn rect() -> Option<(i32, i32, i32, i32)> {
    rect_at(X.load(Ordering::Relaxed), Y.load(Ordering::Relaxed))
}

fn rect_at(x: i32, y: i32) -> Option<(i32, i32, i32, i32)> {
    rect_of(&server::cursor_image(), x, y)
}

/// The largest client cursor that will be drawn. Not a protocol limit: the size
/// of the staging buffer the patch is copied through. A cursor not fitting is
/// skipped, not truncated
pub const MAX_CURSOR: i32 = 64;

fn usable(img: &server::CursorImage) -> bool {
    img.width > 0 && img.height > 0 && img.width <= MAX_CURSOR && img.height <= MAX_CURSOR
}

/// The rectangle this frame's cursor patch covers
///
/// From the latch, not the live pointer: the rectangle repaired is the
/// rectangle the patch lands in
pub fn frame_rect() -> Option<(i32, i32, i32, i32)> {
    let (px, py) = latched();
    let img = FRAME_IMG.lock();
    rect_of(&img, px, py)
}

/// Put the pointer over the screen content already in `out`
///
/// `out` is the rectangle `frame_rect` returned, holding what is underneath,
/// composed by `render::compose`, the frame's own composer (drop shadow,
/// rounded corner and border ring included). This only lays the arrow on top
pub fn draw_over(out: &mut [u32]) {
    match &*FRAME_IMG.lock() {
        Some(Some(i)) => draw_client_cursor(i, out),
        _ => {
            for i in 0..CUR_W * CUR_H {
                match CURSOR[i] {
                    1 => out[i] = OUTLINE,
                    2 => out[i] = FILL,
                    _ => {}
                }
            }
        }
    }
}

/// Put a client's cursor image over the screen content already in `out`
///
/// The copy engine cannot blend, so anything with a shaped transparency is
/// composed by the processor, and a cursor is at most 64 by 64.
///
/// `render::compose` does the same arithmetic over a damage rectangle for a
/// surface committed with an alpha channel. Both go through `paint::over`, the
/// one source-over in the system
fn draw_client_cursor(img: &server::CursorImage, out: &mut [u32]) {
    let (w, h) = (img.width as usize, img.height as usize);
    let opaque = img.format == crate::mayland::protocol::shm::FORMAT_XRGB8888;
    for row in 0..h {
        let src = img.offset + row * img.stride as usize;
        let _ = crate::mayland::server::shm::with_bytes(img.pool, src, w * 4, |px| {
            for col in 0..w {
                let s = u32::from_le_bytes(
                    px[col * 4..col * 4 + 4].try_into().unwrap_or([0; 4]));
                let a = if opaque { 255 } else { s >> 24 };
                let i = row * w + col;
                out[i] = libmay::paint::over(out[i], s, a);
            }
        });
    }
}



/// The cursor as an image in its own right, alpha and all, with nothing of the
/// screen in it.
///
/// What `patch` would produce if the copy engine could blend, and what the
/// head's cursor plane wants: the plane composites over the viewport itself, so
/// screen content underneath would paint a stale rectangle of desktop around
/// the arrow.
///
/// Fills `out` with `w * h` pixels and returns `(w, h, hot_x, hot_y)`. None
/// means no pointer on the screen: hidden, or a client that asked for no cursor
pub fn image_argb(out: &mut Vec<u32>) -> Option<(i32, i32, i32, i32)> {
    if !VISIBLE.load(Ordering::Relaxed) {
        return None;
    }
    match server::cursor_image() {
        None => {
            // The compositor's own arrow. Two colours and a hole, so the alpha
            // channel is the shape
            out.clear();
            out.resize(CUR_W * CUR_H, 0);
            for i in 0..CUR_W * CUR_H {
                match CURSOR[i] {
                    1 => out[i] = OUTLINE | 0xff00_0000,
                    2 => out[i] = FILL | 0xff00_0000,
                    _ => {}
                }
            }
            Some((CUR_W as i32, CUR_H as i32, 0, 0))
        }
        Some(None) => None,
        Some(Some(img)) => {
            if !usable(&img) {
                return None;
            }
            let (w, h) = (img.width as usize, img.height as usize);
            out.clear();
            out.resize(w * h, 0);
            // XRGB has no alpha, so it is forced opaque, as in
            // `draw_client_cursor`
            let opaque = img.format == crate::mayland::protocol::shm::FORMAT_XRGB8888;
            for row in 0..h {
                let src = img.offset + row * img.stride as usize;
                let _ = crate::mayland::server::shm::with_bytes(img.pool, src, w * 4, |px| {
                    for col in 0..w {
                        let s = u32::from_le_bytes(
                            px[col * 4..col * 4 + 4].try_into().unwrap_or([0; 4]));
                        out[row * w + col] = if opaque { s | 0xff00_0000 } else { s };
                    }
                });
            }
            Some((img.width, img.height, img.hot_x, img.hot_y))
        }
    }
}
