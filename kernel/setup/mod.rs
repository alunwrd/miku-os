// The installer, as a window
//
// `install plan` and `install go` are the mechanism: this asks the same
// questions with a pointer and calls the same code. Nothing here partitions,
// formats or copies anything: it collects a drive number and hands it to
// `install_cmds::start_background`, which is `install go` on its own thread.
//
// It is a client of the display server. It binds globals from the registry,
// allocates a pool, creates two buffers from it, and draws in a Wayland
// client's loop: draw, attach, damage, ask for a frame callback, commit, wait.
// The same shape as `mayland/shell/demo.rs`.
//
// In the kernel because partitioning, `mkfs` and the raw sector writes are
// kernel-side, and a userspace installer would need a privileged install
// syscall. The widgets are in `libmay` (`ui.rs`), so a userspace app gets the
// same buttons.

extern crate alloc;
use alloc::format;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};
use spin::Mutex;

use super::mayland::client::{ClientConn, Event};
// The requests themselves, shared with the userspace client half
use super::mayland::requests::Requests;
use super::mayland::protocol::{self, Interface};
use super::mayland::ui::Ui;
use super::mayland::server::{self, shm};

pub mod lang;
mod screens;
pub mod tz;

/// The window is drawn for this size and the compositor is asked for it. A
/// tiling layout will say something else and the window follows, so every
/// screen lays itself out from the size it was given
const W: i32 = 900;
const H: i32 = 640;
const MAX_W: i32 = 1920;
const MAX_H: i32 = 1200;
const BUFFERS: usize = 2;

/// Which screen is in front, and what it has collected so far
pub struct Model {
    pub screen: Screen,
    pub drives: Vec<crate::shell::commands::install_cmds::DriveSummary>,
    /// Which row is picked. `None` is the state this starts in and returns to:
    /// there is never a default disk
    pub selected: Option<usize>,
    /// True once the disk screen's Next was pressed, so the confirmation is a
    /// separate act from the choice
    pub confirming: bool,
    /// Which language every screen is written in. Chosen on the first one, and
    /// changing it changes the frame after
    pub lang: lang::Lang,
    /// Which zone the clock's dates will be written in, as an index into
    /// `tz::ZONES`
    pub tz: usize,
    /// Whether the zone above was chosen by the person rather than guessed from
    /// the language. Until then the language moves it
    pub tz_chosen: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Welcome,
    Disk,
    Progress,
    Done,
}

struct Setup {
    client: u32,
    conn: ClientConn,
    surface: u32,
    xdg_surface: u32,
    w: i32,
    h: i32,
    pool_handle: u32,
    pool_id: u32,
    buffers: [(u32, bool); BUFFERS],
    shm_obj: u32,
    frame_cb: u32,
    frames: u64,
    ui: Ui,
    model: Model,
    closing: bool,
    /// Something happened that the last frame does not show yet
    ///
    /// Otherwise the client would redraw and damage its whole window sixty
    /// times a second, and a compositor with no card copies those 436,000
    /// pixels with the processor each time. An immediate-mode interface redraws
    /// all of itself, but only when something happened
    dirty: bool,
    /// The compositor has released us to draw: no commit is outstanding, or the
    /// last one's frame callback has fired. Held across calls because a frame
    /// not drawn would otherwise lose the permission
    ready: bool,
    /// What the progress screen last drew, so the one screen that changes with
    /// no event of its own knows when it has something new to say
    last_progress: (u8, u64),
}

static SETUP: Mutex<Option<Setup>> = Mutex::new(None);
static RUNNING: AtomicBool = AtomicBool::new(false);
static SPAWNED: AtomicBool = AtomicBool::new(false);

pub fn running() -> bool {
    RUNNING.load(Ordering::Relaxed)
}

/// Open the installer window
pub fn start() {
    if RUNNING.load(Ordering::SeqCst) {
        crate::println!("  the installer is already open");
        return;
    }
    // A client runs whether or not anything is drawing it: on a machine with no
    // compositor this window would never appear. 'mayland demo' does the same
    if !super::mayland::render::running() {
        match super::mayland::render::start() {
            Ok(()) => crate::println!("  compositor started"),
            Err(e) => crate::println!(
                "  no compositor ({}) - drive frames with 'mayland frame'", e),
        }
    }
    match setup() {
        Ok(s) => {
            *SETUP.lock() = Some(s);
            RUNNING.store(true, Ordering::SeqCst);
            if !SPAWNED.swap(true, Ordering::SeqCst) {
                let pid = crate::sched::spawn_named(setup_main, "installer", 1);
                crate::serial_println!("[setup] installer client thread pid={}", pid);
            }
            crate::println!("  the installer is open, it is a window, not this shell");
        }
        Err(e) => crate::print_error!("  install: cannot open a window: {}", e),
    }
}

/// Close it, leaving whatever it started running
pub fn stop() {
    RUNNING.store(false, Ordering::SeqCst);
    if let Some(s) = SETUP.lock().take() {
        server::disconnect(s.client);
        let _ = shm::destroy(s.pool_handle);
    }
    // The surface is gone, so the compositor will not draw it again, but its
    // last frame is on the screen and nothing else would repaint those pixels
    super::mayland::render::claim_screen();
}

fn window_bytes(w: i32, h: i32) -> usize {
    (w * 4 * h) as usize * BUFFERS
}

fn make_pool(conn: &mut ClientConn, shm_obj: u32, handle: u32, w: i32, h: i32)
    -> (u32, [(u32, bool); BUFFERS])
{
    let bytes = window_bytes(w, h);
    let one = bytes / BUFFERS;
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    let mut buffers = [(0u32, true); BUFFERS];
    for (i, buf) in buffers.iter_mut().enumerate() {
        buf.0 = conn.create_buffer(
            pool, (i * one) as i32, w, h, w * 4, protocol::shm::FORMAT_XRGB8888);
    }
    (pool, buffers)
}

/// The handshake: registry, globals, pool, buffers, surface, window
fn setup() -> Result<Setup, &'static str> {
    let handle = shm::create(0, window_bytes(W, H)).map_err(|_| "no memory for a pool")?;

    let client = server::connect(0);
    let mut conn = ClientConn::new();

    let registry = conn.get_registry();
    let bytes = conn.take();
    server::submit(client, &bytes);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);

    let name_of = |want: &str| {
        events.iter().find_map(|e| match e {
            Event::Global { name, interface, .. } if interface == want => Some(*name),
            _ => None,
        })
    };
    let (shm_name, comp_name, xdg_name) = match (
        name_of("may_shm"),
        name_of("may_compositor"),
        name_of("may_xdg"),
    ) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            server::disconnect(client);
            let _ = shm::destroy(handle);
            return Err("the registry is missing a global the installer needs");
        }
    };

    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    let comp = conn.bind(registry, comp_name, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xdg_name, Interface::Xdg, 1);
    // Both halves of the seat: the pointer clicks the rows and the keyboard
    // walks them
    if let Some(seat_name) = name_of("may_seat") {
        let seat = conn.bind(registry, seat_name, Interface::Seat, 1);
        conn.get_keyboard(seat);
        conn.get_pointer(seat);
    }

    let (pool, buffers) = make_pool(&mut conn, shm_obj, handle, W, H);
    let surface = conn.create_surface(comp);
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    let toplevel = conn.get_toplevel(xdg_surface);
    conn.set_title(toplevel, "Install MikuOS");

    let bytes = conn.take();
    server::submit(client, &bytes);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);
    if events.iter().any(|e| matches!(e, Event::Error { .. })) {
        server::disconnect(client);
        let _ = shm::destroy(handle);
        return Err("the server refused the installer's setup");
    }

    let Some(serial) = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial),
        _ => None,
    }) else {
        server::disconnect(client);
        let _ = shm::destroy(handle);
        return Err("the server never configured the window");
    };
    conn.ack_configure(xdg_surface, serial);
    let told = events.iter().rev().find_map(|e| match e {
        Event::ToplevelConfigure { width, height, .. } if *width > 0 && *height > 0 => {
            Some((*width, *height))
        }
        _ => None,
    });
    let bytes = conn.take();
    server::submit(client, &bytes);
    let _ = server::drain(client);

    let mut s = Setup {
        client,
        conn,
        surface,
        xdg_surface,
        w: W,
        h: H,
        pool_handle: handle,
        pool_id: pool,
        buffers,
        shm_obj,
        frame_cb: 0,
        frames: 0,
        ui: Ui::new(),
        model: Model {
            screen: Screen::Welcome,
            // Empty until the welcome screen is left: the screen showing the
            // disks reads them on the way in
            drives: alloc::vec::Vec::new(),
            selected: None,
            confirming: false,
            lang: lang::Lang::En,
            tz: tz::default_for(lang::Lang::En),
            tz_chosen: false,
        },
        closing: false,
        dirty: true,
        ready: true,
        last_progress: (0xff, u64::MAX),
    };
    if let Some((w, h)) = told {
        resize(&mut s, w, h);
    }
    Ok(s)
}

/// Redraws at the size the compositor asked for.
///
/// A client cannot draw a bigger picture into its buffer, so a resize is a new
/// pool and new buffers. The old ones go at once: this window draws every pixel
/// of every frame
fn resize(s: &mut Setup, w: i32, h: i32) {
    if w <= 0 || h <= 0 || w > MAX_W || h > MAX_H || (w, h) == (s.w, s.h) {
        return;
    }
    let Ok(handle) = shm::create(0, window_bytes(w, h)) else {
        crate::serial_println!("[setup] cannot afford {}x{}", w, h);
        return;
    };
    for b in s.buffers {
        s.conn.destroy_buffer(b.0);
    }
    s.conn.destroy_pool(s.pool_id);
    let _ = shm::destroy(s.pool_handle);

    let shm_obj = s.shm_obj;
    let (pool, buffers) = make_pool(&mut s.conn, shm_obj, handle, w, h);
    s.pool_handle = handle;
    s.pool_id = pool;
    s.buffers = buffers;
    s.w = w;
    s.h = h;
}

fn setup_main() -> ! {
    loop {
        if !RUNNING.load(Ordering::Relaxed) {
            crate::sched::sleep(25);
            continue;
        }
        step();
        if SETUP.lock().as_ref().is_some_and(|s| s.closing) {
            stop();
        }
        // Driven by frame callbacks; one tick is how often it looks for one
        crate::sched::sleep(1);
    }
}

fn step() {
    let mut guard = SETUP.lock();
    let Some(s) = guard.as_mut() else { return };

    let reply = server::drain(s.client);
    let (events, _) = s.conn.decode(&reply);

    for e in &events {
        match e {
            Event::BufferRelease { id } => {
                for b in s.buffers.iter_mut() {
                    if b.0 == *id {
                        b.1 = true;
                    }
                }
            }
            Event::Done { id, .. } if *id == s.frame_cb => s.ready = true,
            // The pointer, in surface pixels: where it is, and whether the
            // button is down. Motion only asks for a frame when it crossed into
            // or out of a control (see Ui::pointer_moved)
            Event::PointerEnter { x, y, .. } => s.dirty |= s.ui.pointer_moved(*x, *y),
            Event::PointerMotion { x, y } => s.dirty |= s.ui.pointer_moved(*x, *y),
            Event::PointerLeave { .. } => s.dirty |= s.ui.pointer_moved(-1, -1),
            Event::PointerButton { button, pressed, .. } => {
                s.ui.pointer_button(*button, *pressed);
                s.dirty = true;
            }
            // Keycodes, not letters: the widgets navigate with Tab, the arrows
            // and Enter, and none of those depend on a layout
            Event::Key { keycode, pressed, .. } => {
                s.ui.key(*keycode as u32, *pressed);
                s.dirty |= *pressed;
            }
            Event::ToplevelConfigure { width, height, .. } => {
                let (w, h) = (*width, *height);
                resize(s, w, h);
                s.dirty = true;
            }
            Event::XdgConfigure { serial } => {
                let (xs, sn) = (s.xdg_surface, *serial);
                s.conn.ack_configure(xs, sn);
            }
            Event::ToplevelClose => s.closing = true,
            Event::Error { object, message, .. } => {
                crate::serial_println!("[setup] the display server objected: object {} - {}",
                                       object, message);
                s.closing = true;
                return;
            }
            _ => {}
        }
    }

    // The one screen changing with nothing arriving: the install runs on its
    // own thread and the bar follows it. Asked once a frame, and only a step or
    // another whole megabyte gets a repaint
    if matches!(s.model.screen, Screen::Progress) {
        let st = crate::shell::commands::install_cmds::progress::get();
        let now = (st.step as u8, st.bytes_done / (1024 * 1024));
        if now != s.last_progress {
            s.last_progress = now;
            s.dirty = true;
        }
    }

    if !s.ready || !s.dirty {
        let bytes = s.conn.take();
        if !bytes.is_empty() {
            let client = s.client;
            server::submit(client, &bytes);
        }
        return;
    }

    let Some(bi) = s.buffers.iter().position(|b| b.1) else {
        // Both buffers are still with the compositor: drawing faster than the
        // screen refreshes, which the release event is for
        return;
    };

    // Draw
    //
    // No server lock held. The pixels land in this client's own pool and the
    // compositor will not read them until the commit below
    let (w, h) = (s.w, s.h);
    let one = window_bytes(w, h) / BUFFERS;
    let offset = bi * one;
    let handle = s.pool_handle;
    let mut action = None;
    let was_screen = s.model.screen;
    let mut again = false;
    let _ = shm::with_bytes_mut(handle, offset, one, |px| {
        let mut c = super::mayland::paint::Canvas::new(px, w as usize, w, h);
        s.ui.begin();
        action = screens::draw(&mut c, &mut s.ui, &mut s.model);
        again = s.ui.end();
    });
    // A screen that changed itself while being drawn (the progress screen
    // becomes the done screen when the install finishes) drew the old one, so
    // the new one is owed a frame
    again |= s.model.screen != was_screen;

    match action {
        Some(screens::Action::Quit) => s.closing = true,
        // Answered by the screen that drew the header, and never seen here
        Some(screens::Action::Back) => {}
        Some(screens::Action::Install { drive, mb }) => {
            crate::shell::commands::install_cmds::start_background(drive, mb);
            s.model.screen = Screen::Progress;
            again = true;
        }
        None => {}
    }

    let (bid, surface) = (s.buffers[bi].0, s.surface);
    s.conn.attach(surface, bid, 0, 0);
    s.conn.damage(surface, 0, 0, w, h);
    s.frame_cb = s.conn.frame(surface);
    s.conn.commit(surface);
    s.buffers[bi].1 = false;
    // Drawn, and owed a callback before the next one
    s.dirty = again;
    s.ready = false;

    let bytes = s.conn.take();
    let client = s.client;
    server::submit(client, &bytes);
    s.frames += 1;
}
