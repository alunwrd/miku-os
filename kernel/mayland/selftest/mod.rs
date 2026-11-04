// Mayland selftest
//
// The two halves of the protocol talking to each other with nothing mocked: the
// client encoder produces bytes, the server consumes exactly those bytes, and
// the client decoder reads back what the server sent. It runs on the CPU and
// touches no hardware.
//
// This file is the harness and the running order; the cases are filed by
// subject:
//
//      access    what a client that is not the desktop may have
//      encoding  the wire, a split message, and the registry handshake
//      buffers   pools, buffers, and when a buffer goes back
//      windows   roles, and what a surface may do while it has one
//      tiling    where the layout puts things
//      wm        may_wm: the lease, the window list, the params
//      conf      the config grammar, and the letters after `bind`
//      console   may_console: the grid, its shape, its damage
//      layer     may_layer: bands, and the edges a panel keeps
//      pointer   the pointer, the cursor image and the keymap
//      keyboard  the seat's layouts: the switch, the group, the bar
//      anim      curves, and the things that ride on them
//      gfx       drawing: one glyph, from both halves
//      below     the shell below the windows, what the pass may write over
//      clipboard may_clipboard: what one program hands to another
//      compose   blending
//      outputs   may_output_manager and the monitor lines
//      popup     popups: where they go, what they hold, what lets go
//      toolkit   the widget kit and the curves
//
// The shared scaffolding stays here: a `Runner`, a client with a pool, a window
// already open.
extern crate alloc;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::mayland::client::{ClientConn, Event};
// The requests themselves, shared with the userspace client half
use crate::mayland::requests::Requests;
use crate::mayland::render;
use crate::mayland::protocol::{self, Interface};
use crate::mayland::input;
use crate::mayland::server;
use crate::mayland::shell::layout;
use crate::mayland::shell::workspace;
use crate::mayland::server::shm;
use crate::mayland::outq::Writer;
use crate::mayland::wire::{self, Arg, Reader};

mod access;
mod anim;
mod below;
mod buffers;
mod clipboard;
mod compose;
mod conf;
mod console;
mod encoding;
mod gfx;
mod keyboard;
mod layer;
mod outputs;
mod pointer;
mod popup;
mod tiling;
mod toolkit;
mod windows;
mod wm;

// The Case/Report/Runner scaffolding is shared with every other selftest in
// the tree; only `needs` below is about mayland
pub use crate::kcore::selftest::{Case, Report, Runner};

impl Runner {
    /// A case that needs a leased channel nobody else may be holding.
    ///
    /// `may_wm` and `may_console` are one client at a time, and on a machine
    /// running its desktop that client is the window manager and the terminal.
    /// Evicting them would leave the machine with no keys and no console, so
    /// the case is recorded as skipped and the run stays green.
    ///
    /// `may_desktop` is not on this list: any number of clients may hold it.
    /// What makes these cases unsafe on a live machine is a client driving it,
    /// and that is `may_wm`
    fn needs(&mut self, name: &'static str, iface: Interface) -> bool {
        let held = match iface {
            Interface::Wm => server::wm_held(),
            Interface::Console => server::console_held(),
            _ => false,
        };
        if held {
            self.skip(name, format!("skipped: {} is held by something already running",
                                    iface.name()));
        }
        held
    }
}

/// Round-trip one message through the server and decode the reply
fn exchange(conn: &mut ClientConn, client: u32) -> Vec<Event> {
    let bytes = conn.take();
    server::submit(client, &bytes);
    let reply = server::drain(client);
    let (events, _) = conn.decode(&reply);
    events
}

/// What is on screen that belongs to these clients, bottom to top
///
/// A case owns the windows it opened and nothing else. With the desktop up
/// there is a wallpaper and a console on screen too. Surface ids are per
/// connection, so matching on the id alone is not enough: two clients can both
/// have a surface 7
fn mine(clients: &[u32]) -> Vec<server::Presentable> {
    server::presentable().into_iter()
        .filter(|p| clients.contains(&p.client))
        .collect()
}

fn first_error(events: &[Event]) -> Option<(u32, String)> {
    events.iter().find_map(|e| match e {
        Event::Error { object, message, .. } => Some((*object, message.clone())),
        _ => None,
    })
}

/// Opens one window on a fresh client and returns everything needed to drive it
fn one_window() -> Option<(u32, ClientConn, u32, u32, u32)> {
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let (cl, mut conn, comp, xdg, handle, buf) =
        client_with_pool((stride * h) as usize, w, h, stride)?;
    let surface = conn.create_surface(comp);
    let xs = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xs);
    let events = exchange(&mut conn, cl);
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs, serial);
    conn.attach(surface, buf, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    Some((cl, conn, surface, handle, xs))
}

pub fn run() -> Report {
    let mut t = Runner::new();

    // Every test below places its windows itself, which a tiling layout would
    // undo when the next one opened, so layout is switched off for the run and
    // restored at the end
    let layout_was = layout::kind();
    layout::set_kind(layout::Kind::Float);

    // Every window already open steps out of the tiling for the duration, so a
    // case measuring its rectangle measures the layout's arithmetic and not the
    // console, which would halve the work area
    let parked = workspace::park_tiled();
    if !parked.is_empty() {
        crate::serial_println!(
            "[mayland] selftest: {} window(s) already open, parked for the run",
            parked.len());
    }

    access::test_registry_hides_the_privileged(&mut t);
    access::test_bind_of_a_hidden_global_is_fatal(&mut t);
    access::test_layer_is_out_of_reach(&mut t);
    access::test_a_failed_bind_does_not_take_the_interface(&mut t);
    access::test_pool_share(&mut t);
    access::test_unread_events_end_the_connection(&mut t);
    encoding::test_wire(&mut t);
    encoding::test_partial(&mut t);
    encoding::test_handshake(&mut t);
    buffers::test_buffer_lifecycle(&mut t);
    windows::test_no_role(&mut t);
    windows::test_unconfigured_buffer(&mut t);
    windows::test_two_windows(&mut t);
    windows::test_lower(&mut t);
    tiling::test_placement(&mut t);
    tiling::test_tiling(&mut t);
    tiling::test_layout_follows_output(&mut t);
    tiling::test_tiles_stable_under_focus(&mut t);
    buffers::test_adopted_pool(&mut t);
    buffers::test_damage(&mut t);
    buffers::test_commit_fence(&mut t);
    buffers::test_video_pool(&mut t);
    pointer::test_pointer(&mut t);
    pointer::test_set_cursor(&mut t);
    pointer::test_follow_mouse(&mut t);
    pointer::test_keymap(&mut t);
    keyboard::test_layout_switch(&mut t);
    keyboard::test_layout_terminal(&mut t);
    keyboard::test_layout_group_on_the_wire(&mut t);
    keyboard::test_layout_desktop_and_config(&mut t);
    gfx::test_text(&mut t);
    encoding::test_bad_opcode(&mut t);
    buffers::test_buffer_past_pool(&mut t);
    buffers::test_foreign_pool(&mut t);
    windows::test_backdrop_excluded_from_layout(&mut t);
    wm::test_wm_is_exclusive(&mut t);
    outputs::test_monitor_lines(&mut t);
    outputs::test_output_manager(&mut t);
    outputs::test_output_name(&mut t);
    outputs::test_output_configuration(&mut t);
    wm::test_wm_lists_what_is_on_screen(&mut t);
    wm::test_wm_lists_places_across_a_switch(&mut t);
    wm::test_desktop_captures_a_window(&mut t);
    wm::test_desktop_hears_what_the_layout_moved(&mut t);
    wm::test_wm_moves_and_closes(&mut t);
    wm::test_wm_chords(&mut t);
    conf::test_conf_line_numbers(&mut t);
    conf::test_conf_variables(&mut t);
    conf::test_conf_line_too_long(&mut t);
    conf::test_conf_source_depth(&mut t);
    conf::test_conf_bind_key(&mut t);
    conf::test_conf_layers(&mut t);
    conf::test_conf_too_many_files(&mut t);
    conf::test_qoi_decode(&mut t);
    anim::test_workspace_anim_halves(&mut t);
    compose::test_compose_split(&mut t);
    compose::test_over(&mut t);
    compose::test_edge_against_nothing(&mut t);
    compose::test_colour_alpha(&mut t);
    compose::test_default_is_opaque(&mut t);
    compose::test_paths_agree_when_opaque(&mut t);
    compose::test_outline(&mut t);
    compose::test_outlined_window(&mut t);
    compose::test_shape_params(&mut t);
    below::test_members(&mut t);
    below::test_owed(&mut t);
    below::test_folded(&mut t);
    below::test_margins(&mut t);
    below::test_refused_is_dressed(&mut t);
    below::test_stacked(&mut t);
    toolkit::test_ease_curves(&mut t);
    toolkit::test_ui_field(&mut t);
    toolkit::test_ui_field_full(&mut t);
    conf::test_input_scaling(&mut t);
    conf::test_chord_repeat(&mut t);
    conf::test_chord_release(&mut t);
    conf::test_chord_lone_modifier(&mut t);
    conf::test_chord_passthrough(&mut t);
    wm::test_workspaces(&mut t);
    wm::test_workspace_focus(&mut t);
    wm::test_wm_params(&mut t);
    wm::test_wm_window_state(&mut t);
    wm::test_wm_work_area(&mut t);
    wm::test_wm_version_1_window(&mut t);
    wm::test_wm_swap(&mut t);
    wm::test_pointer_grab(&mut t);
    tiling::test_dwindle_tiles(&mut t);
    tiling::test_dwindle_collapses(&mut t);
    anim::test_curve_shape(&mut t);
    anim::test_curve_ease_in(&mut t);
    anim::test_anim_table(&mut t);
    anim::test_tick_lands(&mut t);
    anim::test_move_starts_a_curve(&mut t);
    anim::test_ghost_lifetime(&mut t);
    anim::test_layer_open(&mut t);
    windows::test_stack_keeps_a_leaver(&mut t);
    layer::test_layer_reserves(&mut t);
    layer::test_layer_no_exclusive(&mut t);
    layer::test_layer_edges_add_up(&mut t);
    layer::test_layer_keeps_clear(&mut t);
    layer::test_layer_input_region(&mut t);
    layer::test_focus_grab(&mut t);
    layer::test_layer_on_demand_takes_the_keyboard_as_it_appears(&mut t);
    layer::test_layer_bands(&mut t);
    layer::test_layer_click_keeps_the_keyboard(&mut t);
    layer::test_layer_keyboard_leaves_the_window_active(&mut t);
    layer::test_layer_arriving_under_a_still_pointer(&mut t);
    layer::test_layer_pushes_windows(&mut t);
    layer::test_lock_pins_the_focus(&mut t);
    layer::test_lock_needs_all_three(&mut t);
    console::test_console_grid(&mut t);
    console::test_console_is_exclusive(&mut t);
    console::test_console_set_size(&mut t);
    console::test_console_damage(&mut t);
    popup::test_popup_placement(&mut t);
    popup::test_popup_constraints(&mut t);
    popup::test_popup_is_a_child(&mut t);
    popup::test_popup_grab(&mut t);
    popup::test_popup_teardown(&mut t);
    popup::test_popup_grab_cannot_be_stolen(&mut t);
    popup::test_popup_refuses_a_chain_that_does_not_end(&mut t);
    clipboard::test_clipboard_round_trip(&mut t);
    clipboard::test_clipboard_boundaries(&mut t);
    anim::test_border_ring(&mut t);
    // Last of the cases touching desktop-wide numbers: it leaves every one at
    // its default, which is what the run started from
    wm::test_wm_reset(&mut t);

    // The console's own cases. They live in `io::console` because what they
    // check is private to it, and run here so there is one command
    for (name, ok, detail) in crate::io::console::selftest() {
        t.check(name, ok, detail);
    }

    workspace::unpark_tiled(&parked);
    layout::set_kind(layout_was);
    Report { cases: t.cases }
}

/// Sets up a client with a pool, returning (client, conn, shm obj, compositor,
/// xdg, pool handle, buffer id)
fn client_with_pool(bytes: usize, w: i32, h: i32, stride: i32)
    -> Option<(u32, ClientConn, u32, u32, u32, u32)>
{
    let handle = shm::create(0, bytes).ok()?;
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |want: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (sn, cn, xn) = match (name_of("may_shm"), name_of("may_compositor"),
                              name_of("may_xdg")) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => { server::disconnect(cl); let _ = shm::destroy(handle); return None; }
    };
    let shm_obj = conn.bind(registry, sn, Interface::Shm, 1);
    let comp = conn.bind(registry, cn, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xn, Interface::Xdg, 1);
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    let buffer = conn.create_buffer(pool, 0, w, h, stride, protocol::shm::FORMAT_XRGB8888);
    Some((cl, conn, comp, xdg, handle, buffer))
}

/// Maps a window and shows one frame of it. Returns the surface id and whether
/// the compositor said it was the active window
fn open_window(conn: &mut ClientConn, cl: u32, comp: u32, xdg: u32, title: &str,
               buffer: u32, w: i32, h: i32) -> (u32, bool)
{
    let surface = conn.create_surface(comp);
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    let toplevel = conn.get_toplevel(xdg_surface);
    conn.set_title(toplevel, title);
    let events = exchange(conn, cl);

    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    let active = events.iter().any(|e| matches!(
        e, Event::ToplevelConfigure { activated: true, .. }));

    conn.ack_configure(xdg_surface, serial);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(conn, cl);
    // The compositor places a new window itself by a policy meant to change.
    // The tests below are about stacking, focus or input, so the windows are
    // pinned at the origin and the policy is tested on its own
    layout::set_window_pos(cl, surface, 0, 0);
    (surface, active)
}

fn active_states(conn: &ClientConn, cl: u32) -> Vec<bool> {
    let reply = server::drain(cl);
    let (events, _) = conn.decode(&reply);
    events.iter().filter_map(|e| match e {
        Event::ToplevelConfigure { activated, .. } => Some(*activated),
        _ => None,
    }).collect()
}

/// Everything the server has queued for a client, decoded
fn drain_events(conn: &mut ClientConn, cl: u32) -> Vec<Event> {
    let reply = server::drain(cl);
    let (events, _) = conn.decode(&reply);
    events
}
