// The display server
//
// Connections, the objects behind their ids, and the lifetime of the surfaces
// and buffers those objects name. This layer sends and receives and decides
// nothing. Where a request needs a decision (where a window goes, which
// workspace is being looked at, how long something takes to fade) it calls
// `shell`.
//
// There is one `Server` behind one lock, and everything a client can see hangs
// off it. Nothing holding the server may call a function that takes the lock
// again: that spins for ever on a machine with no preemption inside a spinlock.
// So internal functions take `&mut Server`, and the ones reaching for the lock
// are the entry points at the edges of this file.
//
//      state     the types behind the ids, and the leases that are not types
//      dispatch  bytes to a call: the interface table as one match
//      iface     one file per interface: what each object may be asked
//      surface   commit, configure, and the two ends of a frame
//      scene     what the compositor reads, flattened and handed over
//      focus     who gets the input, and what is in front
//      shm       physically contiguous pools the copy engine can read
//
// Protocol errors are fatal
//
// A handler returning `Err` has decided the client is broken. The error is
// queued against the object it came in on and the connection is marked dead;
// nothing further is dispatched for it, and the transport notices in its own
// time. A request that legitimately does not apply returns `Ok(())` and does
// nothing.

#![allow(dead_code)]

extern crate alloc;
use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::shell::layout;
use crate::mayland::wire::Arg;

pub mod access;
pub mod dispatch;
pub mod focus;
pub mod held;
pub mod iface;
pub mod scene;
pub mod selection;
pub mod shm;
pub mod state;
pub mod surface;

// The server's own vocabulary, under the name the rest of the kernel uses.
// Listed rather than globbed, as the answer to what may be asked of the display
// server
pub use state::{workspace, BufferObj, Client, CursorImage, Global, Role, Server, Surface};
pub(in crate::mayland) use state::lock;
pub use surface::{frame_composed, frame_done};
pub use scene::{cursor_image, open_windows, presentable, presentable_for_frame, Presentable};
pub use focus::{active_key, focus, focus_client, focus_key, focus_next, focus_window, key,
                keyboard_layout_changed, lower_window, modifiers, pointer_axis, pointer_button, pointer_focus, pointer_motion,
                set_backdrop, set_no_focus};
pub use iface::console::{console_client_pid, console_pump, console_window, focused_terminal};

/// Checks the keyboard lock against whoever holds it. Called once a frame
///
/// Here rather than in the frame loop because the check needs the server lock,
/// which the frame loop does not take (see `scene`). See
/// `iface::layer::watchdog`
pub fn lock_watchdog(now_ms: u32) {
    let mut s = SERVER.lock();
    iface::layer::watchdog(&mut s, now_ms);
}
pub use iface::wm::{console_held, wm_chord, wm_held};

use state::{Grid, Lease, CONSOLES, DESKTOP, GLOBALS, SERVER, WM};

/// Records the screen the compositor will present on. Called once the display
/// is up; until then the output global reports a zero-sized screen
pub fn set_output(width: i32, height: i32, refresh_mhz: i32) {
    let mut s = SERVER.lock();
    s.width = width;
    s.height = height;
    s.refresh_mhz = refresh_mhz;
    // A layer surface is measured against the screen's edges, so a different
    // screen gives each one a different size, and it is told before the windows
    // are arranged, because what it reserves is part of what they are arranged
    // into
    let layers: Vec<usize> = (0..s.surfaces.len())
        .filter(|&i| s.surfaces[i].layer.is_some())
        .collect();
    for si in layers {
        iface::layer::configure_layer(&mut s, si);
    }
    // A different screen is a different work area, and every window needs
    // arranging for it
    layout::arrange(&mut s);
    iface::desktop::send_work_area(&mut s);
}

/// Where a window is and how big, as the compositor has it
///
/// For a test and for the shell. Returns the goal rather than the position: the
/// goal is the arrangement's answer, and the position is where the window has
/// got to so far
pub fn window_box(client: u32, surface: u32) -> Option<(i32, i32, i32, i32)> {
    let s = SERVER.lock();
    let si = s.surface_idx((client, surface))?;
    let g = s.surfaces[si].goal;
    Some((g.x, g.y, g.w, g.h))
}

/// Tells a window a new size, from outside the server
///
/// For the pointer's resize grab, which runs on the compositor thread with
/// nothing held. Everything inside the server calls `surface::configure`
/// directly.
///
/// Silent when the window has gone: a drag whose window closed under it is not
/// an error
pub fn resize_window(client: u32, surface: u32, w: i32, h: i32) {
    if w <= 0 || h <= 0 {
        return;
    }
    let mut s = SERVER.lock();
    let Some(si) = s.surface_idx((client, surface)) else { return };
    surface::configure(&mut s, si, w, h);
}

/// The flags a `window` event would carry for this window
pub fn window_flags(client: u32, surface: u32) -> Option<u32> {
    let s = SERVER.lock();
    let si = s.surface_idx((client, surface))?;
    Some(s.surfaces[si].wm_flags(s.active == Some((client, surface))))
}

/// This window's decoration: the desktop's numbers with its own laid over them
pub fn window_effects(client: u32, surface: u32) -> Option<crate::mayland::render::Effects> {
    let s = SERVER.lock();
    let si = s.surface_idx((client, surface))?;
    Some(s.surfaces[si].fx.over(crate::mayland::render::effects()))
}

/// Which window has the keyboard, for a layout that puts a new window beside
/// the one being looked at
///
/// Takes the server: every caller is inside an arrangement, inside the lock,
/// and a second `state::lock()` there spins for ever. Nothing in `layout` calls
/// a `SERVER.lock()` function while holding a `&mut Server`
pub(in crate::mayland) fn focused_window(s: &Server) -> Option<(u32, u32)> {
    s.focus
}

/// The display layer says an output changed (`kms::events`): takes its state
/// again and tells every client holding an output or the output manager.
///
/// The snapshot is taken before the server's lock, since `kms::state` reads the
/// driver's tables, and the lock is held only to swap it in and queue the
/// events
pub fn outputs_changed() {
    let snap = iface::outputs::snapshot();
    let mut s = SERVER.lock();
    s.outputs = snap;
    iface::outputs::announce(&mut s);
}

/// What became of an output configuration, from the kernel task that applied
/// it (`iface::outputs`)
pub fn output_config_answer(client: u32, object: u32, answer: iface::outputs::Answer) {
    let mut s = SERVER.lock();
    iface::outputs::answer(&mut s, client, object, answer);
}

pub fn output_size() -> Option<(i32, i32)> {
    let s = SERVER.lock();
    if s.width > 0 && s.height > 0 { Some((s.width, s.height)) } else { None }
}

// Connection lifecycle

/// Open a connection. The returned id names it for submit/drain/disconnect
pub fn connect(pid: u64) -> u32 {
    let mut s = SERVER.lock();
    let id = s.next_client;
    s.next_client = s.next_client.wrapping_add(1).max(1);
    s.clients.push(Client::new(id, pid));
    crate::serial_println!("[mayland] client {} connected (pid {})", id, pid);
    id
}

pub fn disconnect(client: u32) {
    let mut s = SERVER.lock();
    let idx = match s.client_idx(client) { Some(i) => i, None => return };

    // Whatever it was listening to stops being sent to it
    DESKTOP.lock().retain(|l| l.client != client);

    // Whatever it was holding the pointer with (a window it was dragging, a
    // menu it had open) goes with it. Nothing is sent back: the client asking
    // is the one that died
    let _ = crate::mayland::input::grab::forget_client(client);

    // What it put on the clipboard. A selection is a promise by a running
    // program: past its exit it would be a copy nobody can be asked about, in
    // memory nothing else reclaims
    let offer_pool = selection::forget_client(&mut s, client);

    // If it was the one driving, the interface is free again, along with its
    // chords. A dead window manager still holding the Super key would leave the
    // keyboard dead, and the machine has no other way to be driven
    if matches!(*WM.lock(), Some(l) if l.client == client) {
        *WM.lock() = None;
        crate::mayland::input::clear_chords();
    }

    // The console's, which is how the kernel's own console window comes back
    // when the terminal exits or crashes. Only the lease is dropped here;
    // putting the window back needs this lock released, so it happens on the
    // next frame in `console_pump`
    //
    // Whichever terminal this client was drawing, it is not drawing it now
    {
        let mut leases = CONSOLES.lock();
        for slot in leases.iter_mut() {
            if matches!(slot, Some(l) if l.client == client) { *slot = None; }
        }
    }

    // The keyboard lock, if it had it: the login screen crashing. The
    // compositor must not believe a screen is up in front of a machine nobody
    // has logged into. Reported loudly, because mikuD's restart of the greeter
    // puts the screen back and a silent gap here is a root console on the
    // display
    if matches!(s.lock, Some((c, _)) if c == client) {
        s.lock = None;
        crate::mayland::input::set_locked(false);
        crate::serial_println!(
            "[mayland] the client holding the keyboard disconnected              ({} the seat) - nothing is drawing the screen",
            if crate::process::session::seat_has_session(crate::process::session::SEAT0)
                { "a session owns" } else { "nobody owns" }
        );
    }

    // Everything the connection owned goes with it. Surfaces and buffers are
    // reachable only through its object map, so nothing else can hold them
    if matches!(s.focus, Some((c, _)) if c == client) {
        s.focus = None;
    }
    // Said to the desktop as the window closing, a few lines down
    if matches!(s.active, Some((c, _)) if c == client) {
        s.active = None;
    }
    // The pointer too. Nobody to send a `leave` to, but a stale id would send
    // the next button to a dead connection, or to whoever inherits the client
    // id later
    if matches!(s.pointer_focus, Some((c, _)) if c == client) {
        s.pointer_focus = None;
    }
    if matches!(s.cursor, Some(ref c) if c.client == client) {
        s.cursor = None;
    }
    // Its windows are going without passing through `unmake_window`, so the
    // window manager is told here. The list must not keep windows of an exited
    // process
    let gone: Vec<u32> = s.surfaces.iter()
        .filter(|x| x.client == client && x.role == Role::Toplevel)
        .map(|x| x.id).collect();
    for sid in gone {
        iface::desktop::event(&mut s, protocol::desktop::CLOSED,
                              &[Arg::Uint(client), Arg::Uint(sid)]);
    }
    // Before the surfaces and the pools go: a window's last frame has to be
    // copied while its memory still has an owner. A ghosted surface loses its
    // client id, so the sweep below leaves it
    let mine: Vec<usize> = (0..s.surfaces.len())
        .filter(|&i| s.surfaces[i].client == client
                     && s.surfaces[i].role == Role::Toplevel)
        .collect();
    for si in mine {
        crate::mayland::shell::ghost::ghost_window(&mut s, si);
    }
    s.surfaces.retain(|x| x.client != client);
    s.buffers.retain(|x| x.client != client);
    // Somebody else may still have a window worth focusing
    if s.focus.is_none() {
        let next = focus::next_focusable(&s, None);
        focus::set_focus(&mut s, next);
    }
    let mut handles: Vec<u32> = s.pools.iter().filter(|p| p.client == client)
        .map(|p| p.handle).collect();
    handles.extend(s.clients[idx].keymap);
    handles.extend(s.clients[idx].grid.map(|g| g.handle));
    handles.extend(core::mem::take(&mut s.clients[idx].orphan_pools));
    // The selection pool this client was last handed. It is not in `s.pools`:
    // the compositor made it, so no object of the client's points at it and the
    // filter above cannot find it
    handles.extend(offer_pool);
    s.pools.retain(|p| p.client != client);
    s.clients.remove(idx);
    // Its windows went with it, so everything left is arranged differently, and
    // if one was a panel the edge it kept comes back, which has to happen first
    iface::layer::reserve_edges(&mut s);
    layout::arrange(&mut s);
    // The cursor is over whatever was behind what went. The pointer focus was
    // only cleared above, so until the mouse moved the next click would go to
    // nobody
    focus::repoint(&mut s);
    drop(s);

    for h in handles {
        let _ = shm::destroy(h);
    }
    // Its windows closing is news to the window manager, which is asleep
    wake_readers();
    crate::serial_println!("[mayland] client {} disconnected", client);
}

pub fn is_alive(client: u32) -> bool {
    let s = SERVER.lock();
    s.alive(client)
}

/// Why a connection died, if it did
///
/// For the selftest: a case about a refusal passes just as well when the
/// connection died of something else. This tells a client killed for filling
/// its event queue from one killed for running out of object ids
pub fn death(client: u32) -> Option<&'static str> {
    let s = SERVER.lock();
    s.client_idx(client).and_then(|i| s.clients[i].death)
}

/// Feed bytes from the transport and dispatch every whole message in them
pub fn submit(client: u32, bytes: &[u8]) {
    let mut s = SERVER.lock();
    let idx = match s.client_idx(client) { Some(i) => i, None => return };
    if !s.clients[idx].alive {
        return;
    }
    s.clients[idx].inbuf.extend_from_slice(bytes);
    dispatch::dispatch_pending(&mut s, idx);
    // A request often queues events for somebody else (a window manager asking
    // for the list, a client taking focus off another), and that somebody is
    // asleep. The client's own reply needs no wake: it is this thread, and it
    // reads them when this returns
    drop(s);
    wake_readers();
}

/// Take the events queued for a client
pub fn drain(client: u32) -> Vec<u8> {
    let mut s = SERVER.lock();
    match s.client_idx(client) {
        Some(i) => s.clients[i].out.take(),
        None => Vec::new(),
    }
}

pub fn pending_bytes(client: u32) -> usize {
    let s = SERVER.lock();
    s.client_idx(client).map(|i| s.clients[i].out.len()).unwrap_or(0)
}

/// Wakes every client with events waiting for it
///
/// A ring-3 client waits for its next event inside `may_recv`
/// (`kernel/net/socket.rs`), which sleeps. Called at the end of a composed
/// frame, where callbacks, buffer releases, input and console damage have just
/// been queued, and after `submit`, for the events one client's request queues
/// for another.
///
/// No wait queue to register on: the connection table knows which process owns
/// each connection. `wakeup` on a running task does nothing.
///
/// The pids are collected under the lock and woken after it is dropped: the
/// first thing a woken reader does is take this lock
pub fn wake_readers() {
    let pending: Vec<(u64, u32)> = {
        let s = SERVER.lock();
        s.clients
            .iter()
            // pid 0 is an in-kernel client (the console, the wallpaper, the
            // demo), called by the compositor thread itself
            //
            // A connection the server has ended is woken too, until its program
            // lets go of it: what it reads now is the end of it
            .filter(|c| c.pid != 0 && (!c.out.is_empty() || !c.alive))
            .map(|c| (c.pid, c.id))
            .collect()
    };
    for (pid, id) in pending {
        crate::sched::wakeup(pid);
        // A client in `poll` on its compositor socket is not asleep on its own
        // pid, because it watches other descriptors too and may be another of
        // its threads. It is on its connection's queue
        crate::sched::wait::wake_key(crate::sched::wait::key::mayland(id));
    }
}

// Introspection for the shell

pub struct ClientInfo {
    pub id: u32,
    pub pid: u64,
    pub objects: usize,
    pub queued: usize,
    pub alive: bool,
    pub death: Option<&'static str>,
}

pub fn clients() -> Vec<ClientInfo> {
    let s = SERVER.lock();
    s.clients.iter().map(|c| ClientInfo {
        id: c.id,
        pid: c.pid,
        objects: c.objects.len(),
        queued: c.out.len(),
        alive: c.alive,
        death: c.death,
    }).collect()
}

pub struct SurfaceInfo {
    pub client: u32,
    pub id: u32,
    pub commits: u64,
    pub has_buffer: bool,
    pub pending_frames: usize,
    pub role: &'static str,
    pub title: alloc::string::String,
    /// Has the keyboard
    pub focused: bool,
    /// Is the active window, drawn with the active border
    pub active: bool,
    /// Nothing is drawn over this one
    pub on_top: bool,
}

/// Surfaces bottom of the stack first, so the shell prints them in the order
/// they are drawn. Anything without a window role sorts before the windows
pub fn surfaces() -> Vec<SurfaceInfo> {
    let s = SERVER.lock();
    let top = s.stack().last().map(|&i| (s.surfaces[i].client, s.surfaces[i].id));
    let mut order: Vec<usize> = (0..s.surfaces.len()).collect();
    order.sort_by_key(|&i| (s.surfaces[i].role == Role::Toplevel, s.surfaces[i].z));
    order.into_iter().map(|i| {
        let x = &s.surfaces[i];
        SurfaceInfo {
            client: x.client,
            id: x.id,
            commits: x.commits,
            has_buffer: x.current.buffer.is_some(),
            pending_frames: x.frames.len(),
            role: match x.role {
                Role::None => "none",
                Role::Xdg => "xdg_surface",
                Role::Toplevel => "toplevel",
                Role::Cursor => "cursor",
                Role::Popup => "popup",
                // Which band, since a layer surface has no title to go by
                Role::Layer => match x.layer.map(|l| l.band) {
                    Some(protocol::layer::BACKGROUND) => "layer/background",
                    Some(protocol::layer::BOTTOM) => "layer/bottom",
                    Some(protocol::layer::TOP) => "layer/top",
                    Some(protocol::layer::OVERLAY) => "layer/overlay",
                    _ => "layer",
                },
            },
            title: x.title.clone(),
            focused: s.focus == Some((x.client, x.id)),
            active: s.active == Some((x.client, x.id)),
            on_top: top == Some((x.client, x.id)),
        }
    }).collect()
}

pub fn globals() -> &'static [Global] { GLOBALS }

/// A fence a surface on screen is waiting on: whose, which, and the commit
/// that carried it
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Fence {
    pub key: (u32, u32),
    pub fence: u64,
    commit: u64,
}

/// GPU fences the surfaces on screen are waiting on, taken and cleared.
///
/// A client that drew with `gpu_submit` can commit without stalling on its own
/// work, and the wait lands here. The compositor knows how much of the frame is
/// left
pub fn take_fences() -> alloc::vec::Vec<Fence> {
    let mut s = SERVER.lock();
    let mut out = alloc::vec::Vec::new();
    for surf in s.surfaces.iter_mut() {
        if surf.current.fence != 0 {
            out.push(Fence { key: (surf.client, surf.id), fence: surf.current.fence, commit: surf.commits });
            surf.current.fence = 0;
        }
    }
    out
}

/// Damage rectangles a late surface keeps before they become one
const KEPT_DAMAGE: usize = 32;

/// Fences that had not passed when the frame went ahead without them: what the
/// frame read of those surfaces was read before the card had finished it. Their
/// damage is not retired (the next frame reads it again) and the fence is
/// waited on again then, unless a commit since has its own. After
/// `presentable_for_frame`, which counted what this frame took.
///
/// Otherwise a late fence keeps a frame of half-drawn pixels: the shell below
/// the windows is updated only where damage says (`render/base.rs`), and a
/// background drawn into new memory (zeroed, so see-through) would show the
/// compositor's wallpaper until the client next damaged the same place. It ends
/// when the fence passes or its channel dies, which `wait_fence` counts as
/// passed
pub fn still_pending(late: &[Fence]) {
    let mut s = SERVER.lock();
    for f in late {
        let Some(si) = s.surface_idx(f.key) else { continue };
        let surf = &mut s.surfaces[si];
        surf.damage_read = Some(0);
        if surf.commits == f.commit && surf.current.fence == 0 {
            surf.current.fence = f.fence;
        }
        if surf.damage.len() > KEPT_DAMAGE {
            let all = surf.damage.iter().fold(None, |a: Option<(i32, i32, i32, i32)>, &(x, y, w, h)| Some(match a {
                None => (x, y, x + w, y + h),
                Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x + w), y1.max(y + h)),
            }));
            if let Some((x0, y0, x1, y1)) = all {
                surf.damage.clear();
                surf.damage.push((x0, y0, x1 - x0, y1 - y0));
            }
        }
    }
}
