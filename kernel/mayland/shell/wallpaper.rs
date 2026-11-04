// The wallpaper, as a window
//
// Something has to be at the back, or every pixel no window covers keeps
// whatever was there before.
//
// It is an ordinary Mayland client in the kernel, like `demo`, with no special
// case in the frame loop: a surface at the bottom of the stack is what
// `present` needs.
//
// The memory
//
// A screen of XRGB is eight megabytes, physically contiguous, an allocation the
// frame allocator can refuse after a firmware load. So it is asked for at boot
// in `mayland::init` and kept whether or not anything composes. Failing to get
// it costs the wallpaper and nothing else.
//
// The wallpaper owns that memory, not the pool table. Each `start` lends it to
// the server as a borrowed pool (`shm::adopt`), and the pool goes with the
// connection at `stop`, freeing nothing. An ordinary pool would be destroyed by
// the disconnect sweep at the first `mayland stop`, and the next `start` would
// find a dead handle

#![allow(dead_code)]

extern crate alloc;
use spin::Mutex;

use crate::mayland::client::{ClientConn, Event};
// The requests themselves, shared with the userspace client half
use crate::mayland::requests::Requests;
use crate::mayland::protocol::{self, Interface};
use crate::mayland::server::{self, shm};
use crate::mm::dma::DmaBuffer;

/// The memory, reserved at boot and never given back
struct Reserved {
    mem: DmaBuffer,
    /// The screen it was sized for
    width: usize,
    height: usize,
}

static RESERVED: Mutex<Option<Reserved>> = Mutex::new(None);

/// The picture the config chose, if it chose one
///
/// A path rather than the pixels: the file is read again on every repaint (when
/// the config is read and when the screen changes size), so eight megabytes of
/// decoded picture are not held for those moments
static IMAGE: Mutex<Option<alloc::string::String>> = Mutex::new(None);

/// Whether the picture on the screen is still the one `IMAGE` names
///
/// A flag rather than a repaint: `set_image` is called from inside a `may_wm`
/// request with the server locked, and putting a picture up means submitting to
/// the server. So the request says what the wallpaper should be, and `pump`,
/// called from the frame loop with nothing held, makes it so
static DIRTY: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

struct Wallpaper {
    client: u32,
    /// The pool lent to the server out of `RESERVED`, for this run only
    handle: u32,
    conn: ClientConn,
    surface: u32,
    xdg_surface: u32,
    width: usize,
    height: usize,
}

static WIN: Mutex<Option<Wallpaper>> = Mutex::new(None);

pub fn running() -> bool { WIN.lock().is_some() }

/// The size the reserved pool can cover, if there is one
pub fn reserved() -> Option<(usize, usize)> {
    RESERVED.lock().as_ref().map(|r| (r.width, r.height))
}

/// Asks for the wallpaper's memory, once, before anything else has taken it
///
/// Called from `mayland::init`, before any firmware image is loaded. The panel
/// the card ends up driving is not known this early, so the size is the larger
/// of the current framebuffer and full HD, and a bigger panel goes without
pub fn reserve() {
    if RESERVED.lock().is_some() {
        return;
    }
    let (fw, fh) = crate::io::console::fb_size().unwrap_or((0, 0));
    let (w, h) = (fw.max(1920), fh.max(1080));
    let bytes = w * h * 4;
    match DmaBuffer::alloc(bytes.div_ceil(4096)) {
        Ok(mut mem) => {
            mem.zero();
            *RESERVED.lock() = Some(Reserved { mem, width: w, height: h });
            crate::serial_println!(
                "[mayland] wallpaper reserved {} KiB for up to {}x{}", bytes / 1024, w, h);
        }
        Err(e) => crate::serial_println!(
            "[mayland] no wallpaper: {} KiB refused ({:?}) - the console stays the backdrop",
            bytes / 1024, e),
    }
}

/// What the config says the wallpaper is, or nothing for the gradient
///
/// Sent by whoever holds `may_wm` from its config, and applied at once if the
/// wallpaper is up. An empty path means no picture, so a deleted `wallpaper`
/// line puts the gradient back
pub fn set_image(path: Option<&str>) {
    let next = path.map(alloc::string::String::from);
    let mut cur = IMAGE.lock();
    if *cur == next {
        return;
    }
    *cur = next;
    DIRTY.store(true, core::sync::atomic::Ordering::Release);
}

/// Tells the server the whole pool is new and puts it on the screen
///
/// Called from `pump`, with the server unlocked, once the pool holds the whole
/// new picture (see `Job`)
fn show() {
    let mut guard = WIN.lock();
    let Some(w) = guard.as_mut() else { return };
    let (width, height) = (w.width, w.height);
    w.conn.damage(w.surface, 0, 0, width as i32, height as i32);
    let surface = w.surface;
    w.conn.commit(surface);
    let bytes = w.conn.take();
    let client = w.client;
    drop(guard);
    server::submit(client, &bytes);
    let _ = server::drain(client);
}

/// What the pool holds now: the picture's path, or `None` for the gradient
///
/// A window manager starting up says `reset` and names the picture again, both
/// marking the wallpaper dirty. When the name is the one on the screen there is
/// nothing to do
static PAINTED: Mutex<Option<alloc::string::String>> = Mutex::new(None);

/// The picture being decoded between frames, if one is
///
/// Decoded into its own memory and copied into the pool only when whole. The
/// pool is what the screen is made from (its copy in video memory is refreshed
/// from it, and the processor blends a see-through panel over it), so a
/// half-new pool would be on the screen for as long as the decode took
struct Job {
    path: alloc::string::String,
    file: alloc::vec::Vec<u8>,
    dec: libmay::qoi::Decoder,
    img: alloc::vec::Vec<u8>,
}

static JOB: Mutex<Option<Job>> = Mutex::new(None);

/// How long `pump` may decode for in one frame: a quarter of a 240 Hz frame. A
/// 1920x1080 picture is then up in about fifteen frames
const DECODE_BUDGET_US: u64 = 1_000;
/// Rows decoded between looks at the clock
const DECODE_ROWS: usize = 8;

impl Job {
    /// Read the file and get ready to decode it. Nothing is decoded yet
    fn open(path: &str) -> Result<Self, &'static str> {
        let file = crate::fs::vfs_read::read_file(path).ok_or("no such file")?;
        let dec = libmay::qoi::Decoder::new(&file)
            .ok_or("not a QOI image - the build converts assets/wallpapers/ into one")?;
        let (iw, ih) = dec.size();
        let img = alloc::vec![0u8; iw * ih * 4];
        Ok(Self { path: alloc::string::String::from(path), file, dec, img })
    }

    /// Decode for at most `budget_us`, or to the end when it is `None`.
    /// True once there is nothing left to decode
    fn step(&mut self, budget_us: Option<u64>) -> bool {
        let sw = crate::kcore::time::Stopwatch::start();
        while !self.dec.done() {
            let rows = if budget_us.is_some() { DECODE_ROWS } else { usize::MAX };
            self.dec.rows(&self.file, &mut self.img, rows);
            if budget_us.is_some_and(|b| sw.elapsed_us() >= b) {
                break;
            }
        }
        self.dec.done()
    }

    /// The decoded picture into the pool, scaled to cover the screen
    fn into_pool(&self, handle: u32, width: usize, height: usize) -> Result<(), &'static str> {
        if !self.dec.complete() {
            return Err("the picture ended early");
        }
        let (iw, ih) = self.dec.size();
        cover(&self.img, iw, ih, handle, width, height)
    }
}

/// Whatever the wallpaper should look like, into the pool, all at once
///
/// For `start`, which has no frame to keep moving yet. A picture chosen once
/// the desktop is up goes through `begin` and `advance`, a little each frame.
///
/// The picture if the config named one and it could be read; the gradient
/// otherwise. A mistyped path costs the picture and not the desktop, and the
/// console says which path it was
fn paint(handle: u32, width: usize, height: usize) -> bool {
    let chosen = IMAGE.lock().clone();
    if let Some(path) = chosen {
        let done = Job::open(&path).and_then(|mut job| {
            job.step(None);
            job.into_pool(handle, width, height)
        });
        match done {
            Ok(()) => {
                *PAINTED.lock() = Some(path);
                return true;
            }
            Err(why) => crate::serial_println!(
                "[mayland] wallpaper '{}': {} - the gradient stays", path, why),
        }
    }
    *PAINTED.lock() = None;
    paint_gradient(handle, width, height)
}

/// A decoded picture into the pool, scaled to cover the screen
///
/// Cover, not fit: the larger of the two ratios wins, and the overhang is
/// cropped evenly from both sides.
///
/// The build has already scaled the picture to the size the pool was reserved
/// for, so at that size this is one copy. Scaling is for another screen size: a
/// virtual machine at 1024x768, a panel the card reports late
fn cover(img: &[u8], iw: usize, ih: usize, handle: u32, width: usize, height: usize)
    -> Result<(), &'static str>
{
    let stride = width * 4;

    // The screen's own size: one copy and stop
    if iw == width && ih == height {
        return shm::with_bytes_mut(handle, 0, stride * height, |b| {
            b.copy_from_slice(&img[..stride * height]);
        }).map_err(|_| "the wallpaper pool could not be written");
    }

    // Cover: one source pixel step per destination pixel, at the ratio that
    // leaves no gap, with the overhang taken off both sides
    let sx = iw as u64 * 1024 / width as u64;
    let sy = ih as u64 * 1024 / height as u64;
    let step = sx.min(sy).max(1);
    let off_x = (iw as i64 - (width as u64 * step / 1024) as i64) / 2;
    let off_y = (ih as i64 - (height as u64 * step / 1024) as i64) / 2;

    let ok = shm::with_bytes_mut(handle, 0, stride * height, |b| {
        for y in 0..height {
            let syp = (off_y + (y as u64 * step / 1024) as i64)
                .clamp(0, ih as i64 - 1) as usize;
            let src = syp * iw * 4;
            let dst = y * stride;
            for x in 0..width {
                let sxp = (off_x + (x as u64 * step / 1024) as i64)
                    .clamp(0, iw as i64 - 1) as usize;
                let s = src + sxp * 4;
                b[dst + x * 4..dst + x * 4 + 4].copy_from_slice(&img[s..s + 4]);
            }
        }
    });
    ok.map(|_| ()).map_err(|_| "the wallpaper pool could not be written")
}

/// The wallpaper changed: start on what it is now
///
/// The gradient is drawn at once (rows of one colour). A picture is opened and
/// left for `advance`
fn begin() {
    let want = IMAGE.lock().clone();
    if *PAINTED.lock() == want {
        // Already on the screen (`reset` and then the same name again), and
        // whatever was being decoded is no longer wanted
        *JOB.lock() = None;
        return;
    }
    let Some(path) = want else {
        *JOB.lock() = None;
        repaint_gradient();
        return;
    };
    if JOB.lock().as_ref().is_some_and(|j| j.path == path) {
        return;
    }
    match Job::open(&path) {
        Ok(job) => *JOB.lock() = Some(job),
        Err(why) => {
            crate::serial_println!("[mayland] wallpaper '{}': {} - the gradient stays", path, why);
            *JOB.lock() = None;
            repaint_gradient();
        }
    }
}

/// A frame's share of the decode, and the picture onto the screen once it is
/// whole
fn advance() {
    let mut slot = JOB.lock();
    let Some(job) = slot.as_mut() else { return };
    if !job.step(Some(DECODE_BUDGET_US)) {
        return;
    }
    let job = slot.take().expect("checked just above");
    drop(slot);
    let Some((handle, width, height)) = WIN.lock().as_ref().map(|w| (w.handle, w.width, w.height))
    else { return };
    match job.into_pool(handle, width, height) {
        Ok(()) => {
            *PAINTED.lock() = Some(job.path);
            show();
        }
        Err(why) => {
            crate::serial_println!("[mayland] wallpaper '{}': {} - the gradient stays", job.path, why);
            repaint_gradient();
        }
    }
}

/// The gradient into the pool and onto the screen
fn repaint_gradient() {
    let Some((handle, width, height)) = WIN.lock().as_ref().map(|w| (w.handle, w.width, w.height))
    else { return };
    if paint_gradient(handle, width, height) {
        *PAINTED.lock() = None;
        show();
    }
}

/// A vertical gradient, drawn into the pool
///
/// Deep teal at the top falling to almost black: the console's own colour taken
/// down until text on it reads. Rows rather than pixels, since the colour only
/// changes down the screen
fn paint_gradient(handle: u32, width: usize, height: usize) -> bool {
    const TOP: (u32, u32, u32) = (12, 34, 38);
    const BOTTOM: (u32, u32, u32) = (3, 6, 9);
    let stride = width * 4;
    shm::with_bytes_mut(handle, 0, stride * height, |b| {
        let denom = (height.max(2) - 1) as i64;
        let lerp = |a: u32, c: u32, t: i64| -> u32 {
            (a as i64 + (c as i64 - a as i64) * t / denom) as u32
        };
        for y in 0..height {
            let t = y as i64;
            let px = 0xFF00_0000u32
                | (lerp(TOP.0, BOTTOM.0, t) << 16)
                | (lerp(TOP.1, BOTTOM.1, t) << 8)
                | lerp(TOP.2, BOTTOM.2, t);
            let row = &mut b[y * stride..y * stride + stride];
            for p in row.chunks_exact_mut(4) {
                p.copy_from_slice(&px.to_le_bytes());
            }
        }
    }).is_ok()
}

/// Put the wallpaper up at whatever size the output is
pub fn start_for_output() -> Result<(), &'static str> {
    let (w, h) = server::output_size().ok_or("the output has no size")?;
    start(w as usize, h as usize)
}

/// Put the wallpaper on the screen, at the back
pub fn start(width: usize, height: usize) -> Result<(), &'static str> {
    if WIN.lock().is_some() {
        return Err("the wallpaper is already up");
    }
    let (phys, virt, bytes) = {
        let r = RESERVED.lock();
        let Some(r) = r.as_ref() else {
            return Err("there is no wallpaper memory - it was refused at boot");
        };
        if width > r.width || height > r.height {
            return Err("the screen is bigger than the wallpaper reserved at boot");
        }
        (r.mem.phys(), r.mem.virt_base(), r.width * r.height * 4)
    };
    if width == 0 || height == 0 {
        return Err("the screen has no size yet");
    }
    // Safety: `RESERVED` is physically contiguous (`DmaBuffer`), at least
    // `bytes` long, and never freed
    let handle = unsafe { shm::adopt(0, phys, virt, bytes) }
        .map_err(|_| "the pool table is full")?;
    if !paint(handle, width, height) {
        let _ = shm::destroy(handle);
        return Err("the wallpaper could not be drawn");
    }

    let client = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let bytes_out = conn.take();
    server::submit(client, &bytes_out);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);
    let name_of = |want: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (Some(sn), Some(cn), Some(ln)) =
        (name_of("may_shm"), name_of("may_compositor"), name_of("may_layer"))
    else {
        server::disconnect(client);
        // No pool object was made, so the disconnect did not take the loan back
        let _ = shm::destroy(handle);
        return Err("the registry is missing a global the wallpaper needs");
    };

    let shm_obj = conn.bind(registry, sn, Interface::Shm, 1);
    let comp = conn.bind(registry, cn, Interface::Compositor, 1);
    let layer = conn.bind(registry, ln, Interface::Layer, 1);
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    // The screen's rectangle out of a pool reserved for a possibly larger one
    let buffer = conn.create_buffer(pool, 0, width as i32, height as i32,
                                    (width * 4) as i32,
                                    protocol::shm::FORMAT_XRGB8888);
    let surface = conn.create_surface(comp);

    // The background band, stretched across the screen, keeping none of it: the
    // band puts it behind everything, a layer surface is on every workspace,
    // and one that has not asked for the keyboard never gets it
    let xdg_surface = conn.get_layer_surface(layer, surface, protocol::layer::BACKGROUND);
    conn.layer_set_anchor(xdg_surface, protocol::layer_surface::ANCHOR_ALL);
    conn.layer_set_exclusive(xdg_surface, -1);
    let bytes_out = conn.take();
    server::submit(client, &bytes_out);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);
    if let Some(Event::Error { message, .. }) = events.iter()
        .find(|e| matches!(e, Event::Error { .. }))
    {
        crate::serial_println!("[mayland] the wallpaper was refused: {}", message);
        server::disconnect(client);
        // Gone already if the pool object was made; a no-op then
        let _ = shm::destroy(handle);
        return Err("the server refused the wallpaper");
    }
    if let Some(serial) = events.iter().find_map(|e| match e {
        Event::LayerConfigure { serial, .. } => Some(*serial), _ => None,
    }) {
        conn.layer_ack_configure(xdg_surface, serial);
    }

    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, width as i32, height as i32);
    conn.commit(surface);
    let bytes_out = conn.take();
    server::submit(client, &bytes_out);
    let _ = server::drain(client);

    *WIN.lock() = Some(Wallpaper { client, handle, conn, surface, xdg_surface, width, height });
    crate::serial_println!(
        "[mayland] the wallpaper is client {} surface {} at {}x{}",
        client, surface, width, height);
    Ok(())
}

/// Takes the wallpaper down. The memory stays reserved, since that allocation
/// may not succeed twice, and the pool lent from it goes with the connection,
/// which frees nothing for a borrowed pool
pub fn stop() {
    *JOB.lock() = None;
    let taken = WIN.lock().take();
    if let Some(w) = taken {
        server::disconnect(w.client);
    }
}

/// Reads the events the server has for it, and drops them
///
/// It never redraws, but a queue nothing reads grows without limit, and a
/// protocol error nobody reads looks like a blank screen
pub fn pump() {
    // The picture the config chose, put up here rather than where it was
    // chosen: see `DIRTY`
    if DIRTY.swap(false, core::sync::atomic::Ordering::AcqRel) {
        begin();
    }
    advance();
    let mut guard = WIN.lock();
    let Some(w) = guard.as_mut() else { return };
    let reply = server::drain(w.client);
    let (events, _) = w.conn.decode(&reply);
    for e in &events {
        match e {
            Event::LayerConfigure { serial, .. } => {
                // The screen changed size. The wallpaper is drawn once into a
                // pool reserved at boot and does not redraw, but an unacked
                // configure is a client the server waits on
                let (xs, s) = (w.xdg_surface, *serial);
                w.conn.layer_ack_configure(xs, s);
            }
            Event::Error { object, message, .. } => {
                crate::serial_println!(
                    "[mayland] the wallpaper died: object {} - {}", object, message);
            }
            _ => {}
        }
    }
    let bytes = w.conn.take();
    if !bytes.is_empty() {
        let client = w.client;
        server::submit(client, &bytes);
    }
}

pub fn size() -> Option<(usize, usize)> {
    WIN.lock().as_ref().map(|w| (w.width, w.height))
}
