// Clients in the kernel
//
// Each is a real client of the protocol: it binds globals from the registry,
// allocates a pool, creates two buffers from it, and draws in a Wayland
// client's loop: draw, attach, damage, request a frame callback, commit, wait
// to be told the frame happened.
//
// Two buffers because the client may only draw into a buffer the compositor has
// handed back. With one it would idle for a frame between every paint; with two
// it paints into the free one while the other is on screen. If the release rule
// in `server::surface` were wrong the square would tear, or the loop would
// stall.
//
// Several at once, because overlap shows stacking order, and each draws a
// bright border only while it is the active window. Nothing here asks the
// server who has focus: a client is told in the ACTIVATED state of its
// configure.
//
// Painting takes the pool lock and submitting takes the server lock, and this
// never holds one while taking the other. The compositor holds the server lock
// while asking a pool for its physical address, so the opposite order is live.

#![allow(dead_code)]

extern crate alloc;
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use spin::Mutex;

use crate::mayland::client::{ClientConn, Event};
// The requests themselves, shared with the userspace client half
use crate::mayland::requests::Requests;
use crate::mayland::protocol::{self, Interface};
use crate::mayland::server::{self, shm};

/// The size a window asks for when nobody tells it one
///
/// Wide enough for a line of text at nine pixels a character. Under a tiling
/// layout the client draws this for one frame, since the compositor's first
/// configure arrives before that frame does; under a floating one it is the
/// size for good
const W: i32 = 320;
const H: i32 = 140;
const BUFFERS: usize = 2;

/// The largest window this demo will draw
///
/// A tiled window can be as big as the screen. The pool is remade at every size
/// change rather than sized for the worst case: two full-screen buffers are
/// sixteen megabytes
const MAX_W: i32 = 3840;
const MAX_H: i32 = 2160;

/// The rows redrawn every frame: the title, the typed line, and the marker
///
/// Everything below this is painted once and left alone
const LIVE_H: i32 = 60;

/// The cursor this client asks for over its own window
///
/// A disc that is mostly transparent, with an opaque rim and an opaque cross at
/// its centre: the window showing through the body is the compositor blending,
/// and the cross sitting where the pointer is shows the hot spot honoured
const CUR_W: i32 = 24;
const CUR_H: i32 = 24;
const CUR_STRIDE: i32 = CUR_W * 4;
const CUR_BYTES: usize = (CUR_STRIDE * CUR_H) as usize;
const CUR_HOT: i32 = CUR_W / 2;

/// Enough to see stacking
const MAX_DEMOS: usize = 4;

/// A distinct body colour each
const COLOURS: [u32; MAX_DEMOS] = [
    0x0030_60c0, // blue
    0x0030_a060, // green
    0x00c0_7020, // amber
    0x0090_40b0, // violet
];

static RUNNING: AtomicBool = AtomicBool::new(false);
static SPAWNED: AtomicBool = AtomicBool::new(false);
/// Never reused, so two windows are never called the same thing
static NEXT_NAME: AtomicUsize = AtomicUsize::new(1);

struct Demo {
    name: usize,
    slot: usize,
    client: u32,
    conn: ClientConn,
    surface: u32,
    xdg_surface: u32,
    /// The pointer object, and the surface offered as the cursor over this
    /// window. Sent on every enter, which is when the compositor will take it
    pointer: u32,
    cursor: u32,
    /// The size the compositor last configured, and what follows from it
    w: i32,
    h: i32,
    /// The window's pixels: one shared-memory pool, one protocol object over
    /// it, and `BUFFERS` buffers cut out of it. All three are remade whenever
    /// the size changes, which is what `resize` is
    pool_handle: u32,
    pool_id: u32,
    /// Buffer object id and whether the compositor has given it back
    buffers: [(u32, bool); BUFFERS],
    /// The pool the window had before the last resize
    ///
    /// Kept one frame longer. The old buffer is what the surface shows until
    /// the resized one is committed, and the copy engine may read it after
    /// that. It goes at the frame callback after the first commit at the new
    /// size
    retired: Option<Retired>,
    /// The cursor lives in a pool of its own, so that remaking the window's
    /// costs nothing else. It is painted once and never resized
    cursor_handle: u32,
    /// The may_shm global, kept because every resize makes another pool out
    /// of it
    shm_obj: u32,
    frame_cb: u32,
    /// What the last configure said: whether this is the window being typed at
    active: bool,
    /// What has been typed into this window, and which modifiers were held when
    /// the last key arrived. A fixed line, oldest character dropped
    typed: [u8; TYPED_MAX],
    typed_len: usize,
    mods: u32,
    /// The keyboard layout in use, from the same event
    group: u32,
    /// The layouts the compositor sent, as (pool handle, bytes). Read on every
    /// keystroke rather than copied: it is the compositor's memory, and a
    /// config that changes the layouts rewrites it in place
    keymap: Option<(u32, usize)>,
    /// Where the pointer is inside this window, when it is over it. Printed in
    /// the title line, which makes motion events and their coordinates visible
    hover: Option<(i32, i32)>,
    clicks: u64,
    /// The body below the live band is painted only when there is a reason: the
    /// first frame, a resize, and the border changing colour because the
    /// keyboard came or went
    ///
    /// One per buffer: a full paint writes only the buffer it draws into, and a
    /// single flag would leave the second with `shm::create`'s zeros below the
    /// band. The client's own version of the debt `render::damage` keeps per
    /// scanout surface
    full_repaint: [bool; BUFFERS],
    painted_active: [bool; BUFFERS],
    frames: u64,
    /// Frames spent waiting because both buffers were with the compositor
    starved: u64,
    tint: u32,
    keys: u64,
    /// The window manager has asked this window to go away, and it has agreed
    ///
    /// A flag rather than the disconnection: the request arrives inside `step`,
    /// which holds the list the window would have to leave
    closing: bool,
}

/// A pool that has been replaced and is waiting to be safe to free
struct Retired {
    handle: u32,
    pool_id: u32,
    buffers: [u32; BUFFERS],
    /// The frame count when it was retired
    ///
    /// A frame callback alone does not say it is safe: the configure causing
    /// the resize and the callback for the frame before it can arrive in the
    /// same batch. What must have happened is a frame after the first commit at
    /// the new size
    since: u64,
}

static DEMOS: Mutex<Vec<Demo>> = Mutex::new(Vec::new());

pub fn running() -> bool { RUNNING.load(Ordering::Relaxed) }

pub struct DemoStats {
    pub name: usize,
    pub frames: u64,
    pub starved: u64,
    pub keys: u64,
    pub active: bool,
}

pub fn stats() -> Vec<DemoStats> {
    DEMOS.lock().iter().map(|d| DemoStats {
        name: d.name,
        frames: d.frames,
        starved: d.starved,
        keys: d.keys,
        active: d.active,
    }).collect()
}

pub fn count() -> usize { DEMOS.lock().len() }

/// Connect one more window
pub fn start() -> Result<usize, &'static str> {
    if DEMOS.lock().len() >= MAX_DEMOS {
        return Err("that is as many demo windows as there are");
    }
    let name = NEXT_NAME.fetch_add(1, Ordering::Relaxed);
    let slot = DEMOS.lock().len();
    let demo = setup(name, slot)?;
    DEMOS.lock().push(demo);
    RUNNING.store(true, Ordering::SeqCst);

    if !SPAWNED.swap(true, Ordering::SeqCst) {
        let pid = crate::sched::spawn_named(demo_main, "mayclient", 1);
        crate::serial_println!("[mayland] demo client thread pid={}", pid);
    }
    Ok(name)
}

/// Disconnect every demo window
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    let taken: Vec<Demo> = core::mem::take(&mut *DEMOS.lock());
    for d in taken {
        server::disconnect(d.client);
        let _ = shm::destroy(d.pool_handle);
        let _ = shm::destroy(d.cursor_handle);
        if let Some(r) = d.retired { let _ = shm::destroy(r.handle); }
    }
    // The surfaces are gone, so the compositor will not draw them again, but
    // their last frames are on the screen and nothing else would repaint those
    // pixels
    crate::mayland::render::claim_screen();
}

/// How much memory a window of this size needs, for all its buffers
fn window_bytes(w: i32, h: i32) -> usize {
    (w * 4 * h) as usize * BUFFERS
}

/// A pool object over a shared-memory handle, cut into `BUFFERS` buffers
///
/// The one place the offsets and the stride are worked out, since a resize does
/// what the first setup did
fn make_pool(conn: &mut ClientConn, shm_obj: u32, handle: u32, w: i32, h: i32)
    -> (u32, [(u32, bool); BUFFERS])
{
    let bytes = window_bytes(w, h);
    let one = bytes / BUFFERS;
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    let mut buffers = [(0u32, true); BUFFERS];
    for (i, buf) in buffers.iter_mut().enumerate() {
        buf.0 = conn.create_buffer(pool, (i * one) as i32, w, h, w * 4,
                                   protocol::shm::FORMAT_XRGB8888);
    }
    (pool, buffers)
}

/// Redraws at the size the compositor asked for
///
/// A client cannot draw a bigger picture into its buffer, so a resize is a new
/// pool, new buffers, and the old ones kept until the new content is on the
/// screen. The freeing is explicit because the memory is the kernel's.
///
/// Refused sizes are not an error: zero means "you choose", and past
/// `MAX_W`/`MAX_H` the client carries on at the size it has
fn resize(d: &mut Demo, w: i32, h: i32) {
    if w <= 0 || h <= 0 || w > MAX_W || h > MAX_H || (w, h) == (d.w, d.h) {
        return;
    }
    let Ok(handle) = shm::create(0, window_bytes(w, h)) else {
        crate::serial_println!("[mayland] demo {} cannot afford {}x{}", d.name, w, h);
        return;
    };

    // The pool being replaced goes on the shelf. Anything still there from an
    // earlier resize has had its frame and can go
    if let Some(old) = d.retired.take() {
        release(d, old);
    }
    let shm_obj = d.shm_obj;
    let (pool, buffers) = make_pool(&mut d.conn, shm_obj, handle, w, h);
    d.retired = Some(Retired {
        handle: d.pool_handle,
        pool_id: d.pool_id,
        buffers: [d.buffers[0].0, d.buffers[1].0],
        since: d.frames,
    });
    d.pool_handle = handle;
    d.pool_id = pool;
    d.buffers = buffers;
    d.w = w;
    d.h = h;
    // New memory holds nothing, and both buffers of it
    d.full_repaint = [true; BUFFERS];
}

/// Give back a pool nothing is reading any more
fn release(d: &mut Demo, old: Retired) {
    for b in old.buffers {
        d.conn.destroy_buffer(b);
    }
    d.conn.destroy_pool(old.pool_id);
    let _ = shm::destroy(old.handle);
}

/// The handshake, once: registry, globals, pool, buffers, surface, window
fn setup(name: usize, slot: usize) -> Result<Demo, &'static str> {
    let handle = shm::create(0, window_bytes(W, H))
        .map_err(|_| "no memory for a client pool")?;
    let cursor_handle = match shm::create(0, CUR_BYTES) {
        Ok(h) => h,
        Err(_) => { let _ = shm::destroy(handle); return Err("no memory for a cursor pool") }
    };

    let client = server::connect(0);
    let mut conn = ClientConn::new();

    let registry = conn.get_registry();
    let bytes = conn.take();
    server::submit(client, &bytes);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);

    let name_of = |want: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (shm_name, comp_name, xdg_name) = match (
        name_of("may_shm"), name_of("may_compositor"), name_of("may_xdg"),
    ) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            server::disconnect(client);
            let _ = shm::destroy(handle);
            let _ = shm::destroy(cursor_handle);
            return Err("the registry is missing a global the demo needs");
        }
    };

    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    let comp = conn.bind(registry, comp_name, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xdg_name, Interface::Xdg, 1);
    let seat_obj = name_of("may_seat").map(|n| conn.bind(registry, n, Interface::Seat, 1));
    let mut pointer = 0;
    if let Some(seat) = seat_obj {
        conn.get_keyboard(seat);
        pointer = conn.get_pointer(seat);
    }
    let (pool, buffers) = make_pool(&mut conn, shm_obj, handle, W, H);
    let surface = conn.create_surface(comp);

    // The cursor: its own pool, its own surface, and the only ARGB buffer this
    // client makes. Painted once, and apart from the window's pool so a resize
    // does not disturb it
    let cursor_pool = conn.create_pool(shm_obj, cursor_handle, CUR_BYTES as i32);
    let cursor_buf = conn.create_buffer(cursor_pool, 0,
                                        CUR_W, CUR_H, CUR_STRIDE,
                                        protocol::shm::FORMAT_ARGB8888);
    let cursor = conn.create_surface(comp);
    paint_cursor(cursor_handle, COLOURS[slot % COLOURS.len()]);
    conn.attach(cursor, cursor_buf, 0, 0);
    conn.damage(cursor, 0, 0, CUR_W, CUR_H);
    conn.commit(cursor);

    // The window role. Without it the surface is pixels nobody has claimed and
    // the compositor does not present it
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    let toplevel = conn.get_toplevel(xdg_surface);
    conn.set_title(toplevel, &format!("mayland demo {}", name));

    let bytes = conn.take();
    server::submit(client, &bytes);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);
    if let Some(Event::Error { message, .. }) = events.iter()
        .find(|e| matches!(e, Event::Error { .. }))
    {
        crate::serial_println!("[mayland] demo setup refused: {}", message);
        server::disconnect(client);
        let _ = shm::destroy(handle);
        let _ = shm::destroy(cursor_handle);
        return Err("the server refused the demo's setup");
    }

    // The compositor has proposed a size; it is acknowledged before drawing
    // anything. A buffer committed before this is refused by the server
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial),
        _ => None,
    });
    let active = events.iter().any(|e| matches!(
        e, Event::ToplevelConfigure { activated: true, .. }));
    // A size, if the compositor had one to propose. Under a tiling layout it
    // always does, and it arrives here: the loop only sees configures for
    // changes, so a window ignoring this first one would draw at its own size
    // until something else opened
    let told = events.iter().rev().find_map(|e| match e {
        Event::ToplevelConfigure { width, height, .. } if *width > 0 && *height > 0 =>
            Some((*width, *height)),
        _ => None,
    });
    match serial {
        Some(serial) => {
            conn.ack_configure(xdg_surface, serial);
            let bytes = conn.take();
            server::submit(client, &bytes);
            let _ = server::drain(client);
        }
        None => {
            server::disconnect(client);
            let _ = shm::destroy(handle);
            let _ = shm::destroy(cursor_handle);
            return Err("the server never configured the window");
        }
    }

    // Where this lands, and how big it is, is the compositor's business
    let mut demo = Demo {
        name,
        slot,
        client,
        conn,
        surface,
        xdg_surface,
        pointer,
        cursor,
        w: W,
        h: H,
        pool_handle: handle,
        pool_id: pool,
        buffers,
        retired: None,
        cursor_handle,
        shm_obj,
        frame_cb: 0,
        active,
        full_repaint: [true; BUFFERS],
        painted_active: [!active; BUFFERS],
        frames: 0,
        starved: 0,
        tint: 0,
        keys: 0,
        closing: false,
        typed: [0; TYPED_MAX],
        typed_len: 0,
        mods: 0,
        group: 0,
        keymap: None,
        hover: None,
        clicks: 0,
    };

    // Built at the asking size and then resized before drawing, so there is one
    // path for resizing
    if let Some((w, h)) = told {
        resize(&mut demo, w, h);
        let bytes = demo.conn.take();
        let client = demo.client;
        server::submit(client, &bytes);
        let _ = server::drain(client);
        // Nothing has been drawn yet, so there is nothing on the shelf that
        // anything is reading
        if let Some(old) = demo.retired.take() {
            release(&mut demo, old);
            let bytes = demo.conn.take();
            server::submit(client, &bytes);
            let _ = server::drain(client);
        }
    }
    Ok(demo)
}

/// Draws the cursor image into the tail of the pool, once
///
/// Straight alpha, not premultiplied, which is what this protocol's buffers
/// carry: the compositor scales the source when it blends
fn paint_cursor(handle: u32, colour: u32) {
    let _ = shm::with_bytes_mut(handle, 0, CUR_BYTES, |b| {
        let c = CUR_HOT;
        for y in 0..CUR_H {
            for x in 0..CUR_W {
                let (dx, dy) = (x - c, y - c);
                let d2 = dx * dx + dy * dy;
                // The cross at the hot spot is opaque, so where the pointer is
                // can be read off the screen
                let px = if dx.abs() <= 3 && dy.abs() <= 0 || dy.abs() <= 3 && dx.abs() <= 0 {
                    0xff00_0000
                } else if d2 >= 81 && d2 <= 121 {
                    0xffff_ffff                       // the rim
                } else if d2 < 81 {
                    0x6000_0000 | (colour & 0x00ff_ffff)   // the body, mostly see-through
                } else {
                    0                                  // outside the disc
                };
                let at = (y * CUR_STRIDE + x * 4) as usize;
                b[at..at + 4].copy_from_slice(&px.to_le_bytes());
            }
        }
    });
}

/// As many characters as fit on one line of the window
const TYPED_MAX: usize = 28;

/// Adds one keystroke to what this window has been typed
///
/// Backspace deletes, and the line scrolls rather than stopping
fn type_char(d: &mut Demo, c: char) {
    match c {
        '\x08' => { d.typed_len = d.typed_len.saturating_sub(1); }
        '\n' | '\t' | '\x1b' => {}
        c if (c as u32) < 0x20 => {}
        c => {
            if d.typed_len == TYPED_MAX {
                d.typed.copy_within(1.., 0);
                d.typed_len -= 1;
            }
            // A byte per cell, so a letter from another layout shows as a
            // question mark rather than its low byte
            d.typed[d.typed_len] = if c.is_ascii() { c as u8 } else { b'?' };
            d.typed_len += 1;
        }
    }
}

fn demo_main() -> ! {
    loop {
        if !RUNNING.load(Ordering::Relaxed) {
            crate::sched::sleep(25);
            continue;
        }
        let n = DEMOS.lock().len();
        for i in 0..n {
            step(i);
        }
        reap();
        // Driven by frame callbacks, not by this sleep; one tick is how often
        // it looks for one
        crate::sched::sleep(1);
    }
}

/// Lets go of every window that agreed to close
///
/// The same ending as `stop`, one window at a time: the connection and its
/// memory go, and the screen is claimed because nothing else would repaint the
/// pixels left behind
fn reap() {
    loop {
        let mut guard = DEMOS.lock();
        let Some(i) = guard.iter().position(|d| d.closing) else { return };
        let d = guard.remove(i);
        let left = guard.len();
        drop(guard);

        crate::serial_println!("[mayland] demo {} closed at the window manager's request",
                               d.name);
        server::disconnect(d.client);
        let _ = shm::destroy(d.pool_handle);
        let _ = shm::destroy(d.cursor_handle);
        if let Some(r) = d.retired { let _ = shm::destroy(r.handle); }
        if left == 0 {
            RUNNING.store(false, Ordering::SeqCst);
        }
        crate::mayland::render::claim_screen();
    }
}

fn step(i: usize) {
    let mut guard = DEMOS.lock();
    let Some(d) = guard.get_mut(i) else { return };

    // Read what the server has to say: buffer releases, the frame callback,
    // keys, and whether this window is still the active one
    let reply = server::drain(d.client);
    let (events, _) = d.conn.decode(&reply);
    let mut fired = d.frames == 0; // the first paint is not waiting for anything

    for e in &events {
        match e {
            Event::BufferRelease { id } => {
                for b in d.buffers.iter_mut() {
                    if b.0 == *id { b.1 = true; }
                }
            }
            Event::Done { id, .. } if *id == d.frame_cb => {
                fired = true;
                // A frame has been presented since the resize, so what the
                // window looked like before it is on no screen and in no
                // engine's queue
                if d.retired.as_ref().is_some_and(|r| d.frames > r.since) {
                    if let Some(old) = d.retired.take() {
                        release(d, old);
                    }
                }
            }
            // The modifier state arrives before the key typed under it, as
            // specified
            Event::Modifiers { mask, group } => { d.mods = *mask; d.group = *group; }
            // The layout, in a pool. An in-kernel client reads it through the
            // same shm the pixels go through, where a process would map it
            Event::KeyboardKeymap { format, handle, size }
                if *format == crate::mayland::protocol::keyboard::KEYMAP_MAY_V2 =>
            {
                d.keymap = Some((*handle, *size as usize));
            }
            // The pointer half of the seat. Enter and leave say the cursor is
            // over this window; motion only arrives while it is, so the last
            // position is all there is to keep
            //
            // The cursor is this client's to decide only while it is over this
            // client's window, so it is set on enter
            Event::PointerEnter { x, y, .. } => {
                d.hover = Some((*x, *y));
                if d.pointer != 0 {
                    d.conn.set_cursor(d.pointer, d.cursor, CUR_HOT, CUR_HOT);
                }
            }
            Event::PointerLeave { .. } => d.hover = None,
            Event::PointerMotion { x, y } => d.hover = Some((*x, *y)),
            Event::PointerButton { pressed: true, .. } => d.clicks += 1,
            // Shows that a key reached the focused window rather than the
            // shell, and that a client can turn a keycode into a letter with
            // the shared layout alone
            Event::Key { keycode, pressed: true, .. } => {
                d.keys += 1;
                // The table the compositor sent, if it sent one. The
                // compiled-in layout is the fallback
                let kc = *keycode as u16;
                let (mods, group) = (d.mods, d.group);
                let c = match d.keymap {
                    Some((h, n)) => crate::mayland::server::shm::with_bytes(h, 0, n, |b| {
                        crate::mayland::keymap::Table::new(b).and_then(|t| t.to_char_in(kc, mods, group))
                    }).ok().flatten(),
                    None => crate::mayland::keymap::to_char(kc, mods),
                };
                if let Some(c) = c {
                    type_char(d, c);
                }
            }
            // The compositor decides how big this window is and whether it is
            // being typed at, and says both here. Acknowledged like any other
            // configure, and the size is obeyed: a client drawing something
            // else cannot be tiled
            Event::ToplevelConfigure { width, height, activated } => {
                d.active = *activated;
                let (w, h) = (*width, *height);
                resize(d, w, h);
            }
            Event::XdgConfigure { serial } => {
                let (xs, s) = (d.xdg_surface, *serial);
                d.conn.ack_configure(xs, s);
            }
            // A request, which this window accepts at once. A client may ignore
            // it, so the window manager can only ask
            Event::ToplevelClose => d.closing = true,

            Event::Error { object, message, .. } => {
                crate::serial_println!("[mayland] demo {} died: object {} - {}",
                                       d.name, object, message);
                RUNNING.store(false, Ordering::Relaxed);
                return;
            }
            _ => {}
        }
    }
    if !fired {
        // Anything queued above (an ack, most likely) still has to go
        let bytes = d.conn.take();
        if !bytes.is_empty() {
            let client = d.client;
            server::submit(client, &bytes);
        }
        return;
    }

    let Some(bi) = d.buffers.iter().position(|b| b.1) else {
        // Both buffers are still with the compositor. Not an error: the client
        // draws faster than the screen refreshes, which the release event
        // exists to stop
        d.starved += 1;
        return;
    };

    // Draw
    //
    // Before anything is sent, with no server lock held. The pixels land in the
    // client's own pool, and the compositor will not read this buffer until the
    // commit below
    let tint = d.tint;
    let body = COLOURS[d.slot % COLOURS.len()];
    let active = d.active;
    let (w, h) = (d.w, d.h);
    let one = window_bytes(w, h) / BUFFERS;
    let offset = bi * one;
    let handle = d.pool_handle;
    let name = d.name;
    let typed = d.typed;
    let typed_len = d.typed_len;
    let hover = d.hover;
    // The whole window only when there is a reason: everything below the band
    // is unchanged
    let full = d.full_repaint[bi] || d.painted_active[bi] != active;
    let band = LIVE_H.min(h);
    let _ = shm::with_bytes_mut(handle, offset, one, |px| {
        paint(px, w, h, if full { 0..h } else { 0..band }, body, active);
        live_band(px, w, h, band, tint, active);
        label(px, w, name, &typed[..typed_len], active, hover);
    });
    d.tint = d.tint.wrapping_add(3);
    d.full_repaint[bi] = false;
    d.painted_active[bi] = active;

    // Attach, damage, ask for the next frame, commit
    //
    // Nothing about where the window is: a window moving itself could not be
    // tiled, dragged or animated by anybody else
    let (bid, surface) = (d.buffers[bi].0, d.surface);
    d.conn.attach(surface, bid, 0, 0);
    d.conn.damage(surface, 0, 0, w, if full { h } else { band });
    d.frame_cb = d.conn.frame(surface);
    d.conn.commit(surface);
    d.buffers[bi].1 = false;

    let bytes = d.conn.take();
    let client = d.client;
    server::submit(client, &bytes);
    d.frames += 1;
}

/// The one thing that moves every frame
///
/// A marker sliding along the band under the title, showing the loop is still
/// running. Besides the text it is the only thing a frame writes
fn live_band(px: &mut [u8], w: i32, h: i32, band: i32, tint: u32, active: bool) {
    let stride = (w * 4) as usize;
    let size = 12i32;
    let y = (band - size - 6).clamp(0, h - size);
    // Across the width of the window and back, so it never leaves the band
    let span = (w - 2 * size - 16).max(1);
    // Four pixels a frame, so a sweep of a half-screen window takes a second
    // or two - fast enough to read as motion, slow enough to see it stop
    let phase = (tint * 4) % (2 * span as u32);
    let x = if phase < span as u32 { phase as i32 } else { 2 * span - phase as i32 };
    let x = (x + size + 8).clamp(0, w - size);

    let colour: u32 = if active { 0x00ff_ffff } else { 0x0090_9098 };
    for row in y..(y + size).min(h) {
        let line = &mut px[row as usize * stride..(row as usize + 1) * stride];
        for col in x..(x + size).min(w) {
            let at = col as usize * 4;
            line[at..at + 4].copy_from_slice(&colour.to_le_bytes());
        }
    }
}

/// The window's name and whatever has been typed into it
///
/// Drawn with the console's glyphs, through the same `draw_text` a userspace
/// client compiles: a client needs only a buffer and the shared font
fn label(px: &mut [u8], w: i32, name: usize, typed: &[u8], active: bool,
         hover: Option<(i32, i32)>) {
    use crate::mayland::text;

    let ink = if active { 0x00ff_ffff } else { 0x00b0_b0b8 };
    let mut title = [0u8; 24];
    let head = b"mayland demo ";
    title[..head.len()].copy_from_slice(head);
    // One digit: there are four of these at most
    title[head.len()] = b'0' + (name % 10) as u8;
    let mut n = head.len() + 1;

    // The pointer's position inside this window, when it is in it. Two
    // three-digit numbers, written by hand because no formatter writes into a
    // fixed buffer
    if let Some((hx, hy)) = hover {
        title[n] = b' '; n += 1;
        n += write_num(&mut title[n..], hx);
        title[n] = b','; n += 1;
        n += write_num(&mut title[n..], hy);
    }
    let title = core::str::from_utf8(&title[..n]).unwrap_or("demo");

    text::draw_text(px, w as usize, 10, 12, title, ink, None);

    // A caret, so an empty line still shows the window is taking keys
    let line = core::str::from_utf8(typed).unwrap_or("");
    let pen = text::draw_text(px, w as usize, 10, 40, line, 0x0000_0000, None);
    if active {
        text::fill(px, w as usize, pen + 1, 40, 2, text::CELL_H as i32, 0x0000_0000);
    }
}

/// A number into a byte buffer, returning how many bytes it took
fn write_num(out: &mut [u8], v: i32) -> usize {
    let v = v.clamp(0, 9999) as usize;
    let mut digits = [0u8; 4];
    let mut n = 0;
    let mut rest = v;
    loop {
        digits[n] = b'0' + (rest % 10) as u8;
        rest /= 10;
        n += 1;
        if rest == 0 || n == digits.len() { break; }
    }
    for i in 0..n.min(out.len()) {
        out[i] = digits[n - 1 - i];
    }
    n.min(out.len())
}

/// The body and the border, over a range of rows
///
/// A range because the whole window can be a megapixel, which is four megabytes
/// to write. The body is painted on the first frame, on a resize, and when the
/// border changes colour because the keyboard arrived or left. `live_band`
/// moves every frame
fn paint(px: &mut [u8], w: i32, h: i32, rows: core::ops::Range<i32>,
         body: u32, active: bool) {
    let edge: u32 = if active { 0x00ff_ffff } else { 0x0050_5058 };
    let colour = body;
    let stride = (w * 4) as usize;
    for row in rows.start.max(0) as usize..rows.end.min(h) as usize {
        let line = &mut px[row * stride..(row + 1) * stride];
        for col in 0..w as usize {
            let border = row < 4 || col < 4
                || row >= h as usize - 4 || col >= w as usize - 4;
            let c = if border { edge } else { colour };
            line[col * 4..col * 4 + 4].copy_from_slice(&c.to_le_bytes());
        }
    }
}
