// Mayland: the MikuOS display protocol, compositor and desktop
//
// Wayland's model, MikuOS's plumbing. Clients own their pixels and hand them to
// a compositor as buffers; the compositor decides what ends up on screen and
// when. Anything true of Wayland's wire format is true here, down to the byte
// layout of a message.
//
// Wayland passes file descriptors over a Unix socket because a compositor is a
// process that has to be handed kernel objects. Ours is the kernel: shared
// memory is named by a handle in the message stream, and the compositor reads
// it directly; see server/shm.rs.
//
// The layers:
//
//      proto    the bytes. A message, an interface table, an id space.
//               Nothing here knows what a window is
//      gfx      drawing. A canvas, glyphs, widgets, shared with every client
//               that draws, so it is beside the protocol and not inside the
//               compositor
//      server   the display server. Connections, objects, surface and buffer
//               lifetime, and one file per interface under `iface`. It sends
//               and receives; it decides nothing
//      shell    the desktop. Where windows go, which workspace is being looked
//               at, what animates and for how long, what is behind everything.
//               The half a Hyprland config is written against, policy over
//               the server's state
//      render   the compositor. Damage, the copy engine, effects, the cursor:
//               pixels, and the only layer that touches the card
//      input    keys and the mouse, from the interrupt to the focused surface
//      selftest the two halves talking to each other
//
// Not a strict stack. `proto` and `gfx` are underneath everything and depend on
// nothing above them. `server` owns the state, and `shell` and `render` are
// written against it and keep almost none of their own. `server` calls up into
// `shell`: a request implying a decision (a window opening, a workspace
// changing, an edge being reserved) is answered by asking the desktop, which
// arranges the server's own surfaces. That call goes through a handful of named
// functions.
//
// The rule between them: inside a layer, `super::sibling`; across layers, the
// full `crate::mayland::layer::module`. A `super::super::` in this tree means a
// file is in the wrong directory.
//
// Shared with userspace:
//
// `proto` and `gfx` are compiled into both halves from one copy in `libmay`,
// re-exported by each of those two modules, and included by `mod` in each
// ring-3 client's main.rs. Both are re-exported at this level as well:
// `mayland::protocol::wm::FLAG_FOCUSED`

#![allow(dead_code)]

pub mod proto;
pub mod gfx;
pub mod server;
pub mod shell;
pub mod render;
pub mod input;
pub mod selftest;

pub use proto::{client, keymap, objects, outq, protocol, requests, wire};
pub use gfx::{font, paint, text, ui};

/// The compositor, as a thing mikuD starts
///
/// A graphical boot brings compositing up before there is a greeter to draw.
///
/// A kernel thread: the server, the renderer and the input path are in ring 0,
/// and `render::start` spawns a thread. This one starts it, then watches: if
/// the screen goes away, mikuD is told by this returning to the waiting state.
pub fn compositor_thread() -> ! {
    // Something may have brought it up already (`run=mayland_start` on the boot
    // line, or an operator at the console); then this thread has nothing to
    // start and everything to watch
    if render::running() {
        crate::serial_println!("[mayland] compositing was already up - watching it");
    } else {
        match render::start() {
            Ok(()) => crate::serial_println!("[mayland] compositing, for the graphical target"),
            Err(e) => {
                // Not a panic and not a restart loop: a machine with no screen
                // boots to a console, and mikuD's fallback notices that the
                // greeter never came up
                crate::serial_println!("[mayland] cannot compose ({}) - no graphical session", e);
                loop { crate::sched::sleep(60_000); }
            }
        }
    }

    // Awake rarely, for two questions. A compositor that fell over is a desktop
    // with nobody presenting it, and restarting it here keeps the machine in
    // the graphical target
    loop {
        // 250 ticks is a second: the timer runs at 250 Hz. Often enough to
        // notice a stopped desktop, rare enough for an idle machine
        crate::sched::sleep(250);

        // The frame loop cannot stop itself (`render::stop` waits for the frame
        // in flight, on that thread waiting for itself), so a watchdog that
        // decided the screen goes back to the console leaves the request here
        if render::take_stop_request() {
            crate::serial_println!(
                "[mayland] giving the screen back to the console - nothing was drawing it");
            render::stop();
            continue;
        }

        if !render::running() {
            // Someone typed `mayland stop`, or a watchdog gave the screen back.
            // A requested stop holds until someone asks for the opposite:
            // restarting would remove the only way out of a desktop gone wrong
            if render::stopped_by_request() {
                continue;
            }
            crate::serial_println!("[mayland] compositing stopped - starting it again");
            if let Err(e) = render::start() {
                crate::serial_println!("[mayland] cannot compose ({})", e);
                loop { crate::sched::sleep(60_000); }
            }
        }
    }
}

/// Publish the screen geometry to the output global
///
/// Called after the console has a framebuffer. Until then may_output reports a
/// zero-sized screen
pub fn init() {
    // The outputs as the display layer has them, now and on change:
    // `may_output`'s name and mode, and everything `may_output_manager` says
    crate::drivers::gpu::kms::events::listen(server::outputs_changed);
    server::outputs_changed();
    if let Some((w, h)) = crate::io::console::fb_size() {
        server::set_output(w as i32, h as i32, 60_000);
        crate::serial_println!(
            "[mayland] protocol ready - output {}x{}, {} globals",
            w, h, server::globals().len()
        );
    } else {
        crate::serial_println!("[mayland] protocol ready - no framebuffer yet");
    }

    // Here and not at `render::start`: a screen of pixels is eight megabytes in
    // one contiguous piece, and by the time a desktop is asked for the GSP
    // firmware has been loaded and the frame allocator has no such piece
    shell::wallpaper::reserve();
}
