// Every request a client can make
//
// One file compiled into both sides by `#[path]`, as with `wire.rs` and
// `protocol.rs`. The two halves keep what differs: where bytes go, how ids are
// allocated, and how events are decoded.
//
// A connection implements `Sink`, "put these bytes somewhere" and "give me an
// id", and every request follows from the blanket implementation of `Requests`
// below.

#![allow(dead_code)]

use super::protocol::{self, Interface, DISPLAY_ID};
use super::wire::{self, Arg};

/// What a connection provides for the requests below. Buffering, flushing, the
/// id table, and over-long messages are the connection's own
pub trait Sink {
    /// Encode one message into whatever this connection sends through
    fn put(&mut self, sender: u32, opcode: u16, args: &[Arg]);

    /// A fresh object id, remembered with its interface so its events can be
    /// decoded
    fn alloc_id(&mut self, iface: Interface) -> u32;

    /// Which version of an interface was bound, for a decoder that needs it.
    /// `may_wm`'s `window` event is shaped by the version
    fn note_version(&mut self, _iface: Interface, _version: u32) {}
}

/// Every request, for anything that is a `Sink`
pub trait Requests: Sink {
    // may_display, may_registry

    fn get_registry(&mut self) -> u32 {
        let id = self.alloc_id(Interface::Registry);
        self.put(DISPLAY_ID, protocol::display::GET_REGISTRY, &[Arg::NewId(id)]);
        id
    }

    fn sync(&mut self) -> u32 {
        let id = self.alloc_id(Interface::Callback);
        self.put(DISPLAY_ID, protocol::display::SYNC, &[Arg::NewId(id)]);
        id
    }

    fn bind(&mut self, registry: u32, name: u32, iface: Interface, version: u32) -> u32 {
        let id = self.alloc_id(iface);
        self.note_version(iface, version);
        self.put(registry, protocol::registry::BIND, &[
            Arg::Uint(name), Arg::Str(Some(iface.name())), Arg::Uint(version), Arg::NewId(id),
        ]);
        id
    }

    // may_compositor, may_shm, may_surface

    fn create_surface(&mut self, compositor: u32) -> u32 {
        let id = self.alloc_id(Interface::Surface);
        self.put(compositor, protocol::compositor::CREATE_SURFACE, &[Arg::NewId(id)]);
        id
    }

    fn create_pool(&mut self, shm: u32, handle: u32, size: i32) -> u32 {
        let id = self.alloc_id(Interface::ShmPool);
        self.put(shm, protocol::shm::CREATE_POOL, &[
            Arg::NewId(id), Arg::Uint(handle), Arg::Int(size),
        ]);
        id
    }

    fn create_buffer(&mut self, pool: u32, offset: i32, width: i32, height: i32,
                     stride: i32, format: u32) -> u32
    {
        let id = self.alloc_id(Interface::Buffer);
        self.put(pool, protocol::shm_pool::CREATE_BUFFER, &[
            Arg::NewId(id), Arg::Int(offset), Arg::Int(width), Arg::Int(height),
            Arg::Int(stride), Arg::Uint(format),
        ]);
        id
    }

    /// Gives the pool object back. Buffers cut from it keep working, since the
    /// memory belongs to the shared-memory handle
    fn destroy_pool(&mut self, pool: u32) {
        self.put(pool, protocol::shm_pool::DESTROY, &[]);
    }

    /// Tells the server the pool covers more of the handle. Only grows, and
    /// only within the handle
    fn resize_pool(&mut self, pool: u32, size: i32) {
        self.put(pool, protocol::shm_pool::RESIZE, &[Arg::Int(size)]);
    }

    /// Gives a buffer back. Allowed while attached, leaving the surface with no
    /// content until the next attach, and while on screen, since the pixels
    /// belong to the handle. Buffers never returned fill the id table
    fn destroy_buffer(&mut self, buffer: u32) {
        self.put(buffer, protocol::buffer::DESTROY, &[]);
    }

    fn attach(&mut self, surface: u32, buffer: u32, x: i32, y: i32) {
        self.put(surface, protocol::surface::ATTACH, &[
            Arg::Object(buffer), Arg::Int(x), Arg::Int(y),
        ]);
    }

    fn damage(&mut self, surface: u32, x: i32, y: i32, w: i32, h: i32) {
        self.put(surface, protocol::surface::DAMAGE, &[
            Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h),
        ]);
    }

    fn frame(&mut self, surface: u32) -> u32 {
        let id = self.alloc_id(Interface::Callback);
        self.put(surface, protocol::surface::FRAME, &[Arg::NewId(id)]);
        id
    }

    fn commit(&mut self, surface: u32) {
        self.put(surface, protocol::surface::COMMIT, &[]);
    }

    /// A region, with rectangles added and removed in order, for
    /// `set_input_region`
    fn create_region(&mut self, compositor: u32) -> u32 {
        let id = self.alloc_id(Interface::Region);
        self.put(compositor, protocol::compositor::CREATE_REGION, &[Arg::NewId(id)]);
        id
    }

    fn region_add(&mut self, region: u32, x: i32, y: i32, w: i32, h: i32) {
        self.put(region, protocol::region::ADD, &[Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]);
    }

    fn region_subtract(&mut self, region: u32, x: i32, y: i32, w: i32, h: i32) {
        self.put(region, protocol::region::SUBTRACT, &[Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]);
    }

    fn region_destroy(&mut self, region: u32) {
        self.put(region, protocol::region::DESTROY, &[]);
    }

    /// A focus grab (may_layer version 2): surfaces listed with
    /// `focus_grab_add`, in force from `focus_grab_commit`
    fn get_focus_grab(&mut self, layer: u32) -> u32 {
        let id = self.alloc_id(Interface::FocusGrab);
        self.put(layer, protocol::layer::GET_FOCUS_GRAB, &[Arg::NewId(id)]);
        id
    }

    fn focus_grab_add(&mut self, grab: u32, surface: u32) {
        self.put(grab, protocol::focus_grab::ADD_SURFACE, &[Arg::Object(surface)]);
    }

    fn focus_grab_remove(&mut self, grab: u32, surface: u32) {
        self.put(grab, protocol::focus_grab::REMOVE_SURFACE, &[Arg::Object(surface)]);
    }

    fn focus_grab_commit(&mut self, grab: u32) {
        self.put(grab, protocol::focus_grab::COMMIT, &[]);
    }

    fn focus_grab_destroy(&mut self, grab: u32) {
        self.put(grab, protocol::focus_grab::DESTROY, &[]);
    }

    /// Where the pointer reaches the surface from its next commit: the region
    /// as it is now, or the whole surface for region 0. Elsewhere the pointer
    /// is over what is under it
    fn set_input_region(&mut self, surface: u32, region: u32) {
        self.put(surface, protocol::surface::SET_INPUT_REGION, &[Arg::Object(region)]);
    }

    /// Says this commit's pixels are still being written by the GPU. For a
    /// client that drew with `gpu_submit`: it commits and the compositor
    /// absorbs the wait. Sent before `commit`, and applies to that commit only
    fn set_fence(&mut self, surface: u32, fence: u64) {
        self.put(surface, protocol::surface::SET_FENCE, &[
            Arg::Uint(fence as u32), Arg::Uint((fence >> 32) as u32),
        ]);
    }

    fn destroy_surface(&mut self, surface: u32) {
        self.put(surface, protocol::surface::DESTROY, &[]);
    }

    // may_xdg: a surface that is a window

    fn get_xdg_surface(&mut self, xdg: u32, surface: u32) -> u32 {
        let id = self.alloc_id(Interface::XdgSurface);
        self.put(xdg, protocol::xdg::GET_XDG_SURFACE, &[
            Arg::NewId(id), Arg::Object(surface),
        ]);
        id
    }

    fn get_toplevel(&mut self, xdg_surface: u32) -> u32 {
        let id = self.alloc_id(Interface::Toplevel);
        self.put(xdg_surface, protocol::xdg_surface::GET_TOPLEVEL, &[Arg::NewId(id)]);
        id
    }

    fn set_title(&mut self, toplevel: u32, title: &str) {
        self.put(toplevel, protocol::toplevel::SET_TITLE, &[Arg::Str(Some(title))]);
    }

    /// What kind of program this window belongs to, which a window rule
    /// matches. A title changes with what the program is doing; this does not
    fn set_app_id(&mut self, toplevel: u32, app_id: &str) {
        self.put(toplevel, protocol::toplevel::SET_APP_ID, &[Arg::Str(Some(app_id))]);
    }

    fn ack_configure(&mut self, xdg_surface: u32, serial: u32) {
        self.put(xdg_surface, protocol::xdg_surface::ACK_CONFIGURE, &[Arg::Uint(serial)]);
    }

    /// The client's own teardown, innermost object first
    fn destroy_toplevel(&mut self, toplevel: u32) {
        self.put(toplevel, protocol::toplevel::DESTROY, &[]);
    }

    fn destroy_xdg_surface(&mut self, xdg_surface: u32) {
        self.put(xdg_surface, protocol::xdg_surface::DESTROY, &[]);
    }

    // may_positioner / may_popup: a surface placed against another. The recipe
    // is an object: a menu opens the same way each time, and `reposition`
    // reruns it against a screen edge the client cannot see

    fn create_positioner(&mut self, xdg: u32) -> u32 {
        let id = self.alloc_id(Interface::Positioner);
        self.put(xdg, protocol::xdg::CREATE_POSITIONER, &[Arg::NewId(id)]);
        id
    }

    fn positioner_set_size(&mut self, positioner: u32, w: i32, h: i32) {
        self.put(positioner, protocol::positioner::SET_SIZE,
                 &[Arg::Int(w), Arg::Int(h)]);
    }

    /// What it hangs off, in the parent's coordinates: a menu button, or for a
    /// context menu, the one-pixel box the cursor was in
    fn positioner_set_anchor_rect(&mut self, positioner: u32,
                                  x: i32, y: i32, w: i32, h: i32) {
        self.put(positioner, protocol::positioner::SET_ANCHOR_RECT,
                 &[Arg::Int(x), Arg::Int(y), Arg::Int(w), Arg::Int(h)]);
    }

    fn positioner_set_anchor(&mut self, positioner: u32, anchor: u32) {
        self.put(positioner, protocol::positioner::SET_ANCHOR, &[Arg::Uint(anchor)]);
    }

    fn positioner_set_gravity(&mut self, positioner: u32, gravity: u32) {
        self.put(positioner, protocol::positioner::SET_GRAVITY, &[Arg::Uint(gravity)]);
    }

    /// What to do when the menu does not fit: flip, slide in from the edge, or
    /// nothing
    fn positioner_set_constraint(&mut self, positioner: u32, adjust: u32) {
        self.put(positioner, protocol::positioner::SET_CONSTRAINT_ADJUSTMENT,
                 &[Arg::Uint(adjust)]);
    }

    fn positioner_set_offset(&mut self, positioner: u32, x: i32, y: i32) {
        self.put(positioner, protocol::positioner::SET_OFFSET,
                 &[Arg::Int(x), Arg::Int(y)]);
    }

    fn destroy_positioner(&mut self, positioner: u32) {
        self.put(positioner, protocol::positioner::DESTROY, &[]);
    }

    fn get_popup(&mut self, xdg_surface: u32, parent: u32, positioner: u32) -> u32 {
        let id = self.alloc_id(Interface::Popup);
        self.put(xdg_surface, protocol::xdg_surface::GET_POPUP, &[
            Arg::NewId(id), Arg::Object(parent), Arg::Object(positioner),
        ]);
        id
    }

    /// Takes the pointer and keyboard for this menu. `serial` came with the
    /// opening key or button; the compositor refuses a grab answering nothing
    fn popup_grab(&mut self, popup: u32, seat: u32, serial: u32) {
        self.put(popup, protocol::popup::GRAB, &[Arg::Object(seat), Arg::Uint(serial)]);
    }

    fn popup_reposition(&mut self, popup: u32, positioner: u32, token: u32) {
        self.put(popup, protocol::popup::REPOSITION,
                 &[Arg::Object(positioner), Arg::Uint(token)]);
    }

    fn destroy_popup(&mut self, popup: u32) {
        self.put(popup, protocol::popup::DESTROY, &[]);
    }

    // may_clipboard: what one program hands another

    /// Puts `len` bytes of `handle` into a selection. The types are one string
    /// of space-separated names, best first. The compositor copies the bytes,
    /// so the pool is reusable on return
    fn clipboard_set(&mut self, clip: u32, serial: u32, which: u32, mimes: &str,
                     handle: u32, offset: u32, len: u32) {
        self.put(clip, protocol::clipboard::SET, &[
            Arg::Uint(serial), Arg::Uint(which), Arg::Str(Some(mimes)),
            Arg::Uint(handle), Arg::Uint(offset), Arg::Uint(len),
        ]);
    }

    /// Asks for the selection. Answered with `ClipboardData`, carrying a pool
    /// handle to adopt with `Pool::adopt` and free after reading
    fn clipboard_get(&mut self, clip: u32, serial: u32, which: u32, mime: &str) {
        self.put(clip, protocol::clipboard::GET, &[
            Arg::Uint(serial), Arg::Uint(which), Arg::Str(Some(mime)),
        ]);
    }

    /// Puts text into the console's input as if typed: the terminal's paste.
    /// The kernel routes keys past the terminal to the shell, so it has no
    /// input of its own
    fn console_paste(&mut self, console: u32, text: &str) {
        self.put(console, protocol::console::PASTE, &[Arg::Str(Some(text))]);
    }

    fn clipboard_clear(&mut self, clip: u32, which: u32) {
        self.put(clip, protocol::clipboard::CLEAR, &[Arg::Uint(which)]);
    }

    // may_seat

    fn get_keyboard(&mut self, seat: u32) -> u32 {
        let id = self.alloc_id(Interface::Keyboard);
        self.put(seat, protocol::seat::GET_KEYBOARD, &[Arg::NewId(id)]);
        id
    }

    fn get_pointer(&mut self, seat: u32) -> u32 {
        let id = self.alloc_id(Interface::Pointer);
        self.put(seat, protocol::seat::GET_POINTER, &[Arg::NewId(id)]);
        id
    }

    /// Asks for this surface to be the cursor, with its point at (hot_x,
    /// hot_y). A surface id of 0 asks for no cursor
    fn set_cursor(&mut self, pointer: u32, surface: u32, hot_x: i32, hot_y: i32) {
        self.put(pointer, protocol::pointer::SET_CURSOR, &[
            Arg::Uint(0), Arg::Object(surface), Arg::Int(hot_x), Arg::Int(hot_y),
        ]);
    }

    // may_desktop, for anything allowed to see the desktop. The facts, and the
    // four verbs of a user asking. Sent by a panel, or the window manager,
    // which binds this beside `may_wm`

    fn desk_get_windows(&mut self, desk: u32, serial: u32) {
        self.put(desk, protocol::desktop::GET_WINDOWS, &[Arg::Uint(serial)]);
    }

    fn desk_focus(&mut self, desk: u32, client: u32, surface: u32) {
        self.put(desk, protocol::desktop::FOCUS, &[Arg::Uint(client), Arg::Uint(surface)]);
    }

    fn desk_close(&mut self, desk: u32, client: u32, surface: u32) {
        self.put(desk, protocol::desktop::CLOSE, &[Arg::Uint(client), Arg::Uint(surface)]);
    }

    fn desk_workspace(&mut self, desk: u32, index: u32) {
        self.put(desk, protocol::desktop::WORKSPACE, &[Arg::Uint(index)]);
    }

    /// Says a word to every listener on `may_desktop`, unread by the
    /// compositor. Its meaning is between the sender, such as a key binding in
    /// the window manager, and the shell with a layer of that name
    fn desk_signal(&mut self, desk: u32, name: &str) {
        self.put(desk, protocol::desktop::SIGNAL, &[Arg::Str(Some(name))]);
    }

    fn desk_move_to_workspace(&mut self, desk: u32, client: u32, surface: u32, index: u32) {
        self.put(desk, protocol::desktop::MOVE_TO_WORKSPACE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Uint(index),
        ]);
    }

    /// Copies a window's pixels into `pool` (this connection's object) at
    /// `offset`; `CAPTURED` with `serial` gives the result. Version 5
    fn desk_capture(&mut self, desk: u32, client: u32, surface: u32, pool: u32, offset: u32, serial: u32) {
        self.put(desk, protocol::desktop::CAPTURE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Uint(pool), Arg::Uint(offset), Arg::Uint(serial),
        ]);
    }

    // may_wm, the desktop's policy, for the one program that holds it

    fn wm_move(&mut self, wm: u32, client: u32, surface: u32, x: i32, y: i32) {
        self.put(wm, protocol::wm::MOVE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Int(x), Arg::Int(y),
        ]);
    }

    fn wm_resize(&mut self, wm: u32, client: u32, surface: u32, w: i32, h: i32) {
        self.put(wm, protocol::wm::RESIZE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Int(w), Arg::Int(h),
        ]);
    }

    fn wm_set_layout(&mut self, wm: u32, layout: u32) {
        self.put(wm, protocol::wm::SET_LAYOUT, &[Arg::Uint(layout)]);
    }

    fn wm_set_gaps(&mut self, wm: u32, outer: i32, inner: i32) {
        self.put(wm, protocol::wm::SET_GAPS, &[Arg::Int(outer), Arg::Int(inner)]);
    }

    fn wm_bind_chord(&mut self, wm: u32, id: u32, mods: u32, keycode: u32, flags: u32) {
        self.put(wm, protocol::wm::BIND_CHORD, &[
            Arg::Uint(id), Arg::Uint(mods), Arg::Uint(keycode), Arg::Uint(flags),
        ]);
    }

    /// Returns every desktop-wide number to the compositor's defaults. Sent
    /// before a config is applied, so what the file no longer says stops
    /// applying
    fn wm_reset(&mut self, wm: u32) {
        self.put(wm, protocol::wm::RESET, &[]);
    }

    /// Choose the picture behind everything. An empty path means none
    // may_output_manager / may_output_configuration

    /// A configuration made against the state `serial` names
    fn outputs_configure(&mut self, manager: u32, serial: u32) -> u32 {
        let id = self.alloc_id(Interface::OutputConfiguration);
        self.put(manager, protocol::output_manager::CREATE_CONFIGURATION,
                 &[Arg::NewId(id), Arg::Uint(serial)]);
        id
    }

    fn outputs_stop(&mut self, manager: u32) {
        self.put(manager, protocol::output_manager::STOP, &[]);
    }

    /// One head as it should be. `mode` and `adaptive_sync` take
    /// `output_configuration::KEEP`; `bpc` 0 keeps the depth
    fn output_set(&mut self, config: u32, head: &str, mode: i32, adaptive_sync: i32, bpc: u32) {
        self.put(config, protocol::output_configuration::SET, &[
            Arg::Str(Some(head)), Arg::Int(mode), Arg::Int(adaptive_sync), Arg::Uint(bpc),
        ]);
    }

    fn output_disable(&mut self, config: u32, head: &str) {
        self.put(config, protocol::output_configuration::DISABLE_HEAD, &[Arg::Str(Some(head))]);
    }

    fn output_apply(&mut self, config: u32) {
        self.put(config, protocol::output_configuration::APPLY, &[]);
    }

    fn output_test(&mut self, config: u32) {
        self.put(config, protocol::output_configuration::TEST, &[]);
    }

    fn output_config_destroy(&mut self, config: u32) {
        self.put(config, protocol::output_configuration::DESTROY, &[]);
    }

    fn wm_set_wallpaper(&mut self, wm: u32, path: &str) {
        self.put(wm, protocol::wm::SET_WALLPAPER, &[Arg::Str(Some(path))]);
    }

    /// A rule for every layer whose namespace matches `pattern`
    fn wm_set_layer_rule(&mut self, wm: u32, rule: u32, pattern: &str) {
        self.put(wm, protocol::wm::SET_LAYER_RULE, &[Arg::Uint(rule), Arg::Str(Some(pattern))]);
    }

    /// Puts the pointer at a place on the screen
    fn wm_warp_pointer(&mut self, wm: u32, x: i32, y: i32) {
        self.put(wm, protocol::wm::WARP_POINTER, &[Arg::Int(x), Arg::Int(y)]);
    }

    fn wm_unbind_chord(&mut self, wm: u32, id: u32) {
        self.put(wm, protocol::wm::UNBIND_CHORD, &[Arg::Uint(id)]);
    }

    /// One of the desktop's numbers in the param's unit: fixed point on the
    /// wire, a real number here
    fn wm_set_param(&mut self, wm: u32, param: u32, value: f32) {
        self.put(wm, protocol::wm::SET_PARAM, &[
            Arg::Uint(param), Arg::Fixed(wire::fixed_from_f32(value)),
        ]);
    }

    /// The same number for one window. `set` false returns the window to the
    /// desktop's value
    fn wm_set_window_param(&mut self, wm: u32, client: u32, surface: u32,
                           param: u32, value: f32, set: bool) {
        self.put(wm, protocol::wm::SET_WINDOW_PARAM, &[
            Arg::Uint(client), Arg::Uint(surface),
            Arg::Uint(param), Arg::Fixed(wire::fixed_from_f32(value)), Arg::Uint(set as u32),
        ]);
    }

    /// The booleans: floating, fullscreen, pinned. `mask` is which, and `flags`
    /// what they become
    fn wm_set_window_state(&mut self, wm: u32, client: u32, surface: u32,
                           mask: u32, flags: u32) {
        self.put(wm, protocol::wm::SET_WINDOW_STATE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Uint(mask), Arg::Uint(flags),
        ]);
    }

    fn wm_swap(&mut self, wm: u32, a_client: u32, a_surface: u32,
               b_client: u32, b_surface: u32) {
        self.put(wm, protocol::wm::SWAP, &[
            Arg::Uint(a_client), Arg::Uint(a_surface),
            Arg::Uint(b_client), Arg::Uint(b_surface),
        ]);
    }

    /// Names a curve. `id` is the config's numbering, in file order
    fn wm_set_curve(&mut self, wm: u32, id: u32, x1: f32, y1: f32, x2: f32, y2: f32) {
        self.put(wm, protocol::wm::SET_CURVE, &[
            Arg::Uint(id),
            Arg::Fixed(wire::fixed_from_f32(x1)), Arg::Fixed(wire::fixed_from_f32(y1)),
            Arg::Fixed(wire::fixed_from_f32(x2)), Arg::Fixed(wire::fixed_from_f32(y2)),
        ]);
    }

    /// What one class animates with. `speed` is in hundreds of milliseconds, as
    /// such configs write it
    fn wm_set_animation(&mut self, wm: u32, class: u32, enabled: bool,
                        speed: f32, curve: u32, style: u32, arg: f32) {
        self.put(wm, protocol::wm::SET_ANIMATION, &[
            Arg::Uint(class), Arg::Uint(enabled as u32),
            Arg::Fixed(wire::fixed_from_f32(speed)),
            Arg::Uint(curve), Arg::Uint(style),
            Arg::Fixed(wire::fixed_from_f32(arg)),
        ]);
    }

    /// How wide the ring around a window is, and its two colours
    fn wm_set_border(&mut self, wm: u32, size: i32, active: u32, inactive: u32) {
        self.put(wm, protocol::wm::SET_BORDER, &[
            Arg::Uint(size.max(0) as u32), Arg::Uint(active), Arg::Uint(inactive),
        ]);
    }

    /// Carries a window with the pointer until the button comes up. `(0, 0)`
    /// means the window under the pointer, for a mouse binding
    fn wm_begin_move(&mut self, wm: u32, client: u32, surface: u32) {
        self.put(wm, protocol::wm::BEGIN_MOVE, &[
            Arg::Uint(client), Arg::Uint(surface),
        ]);
    }

    /// The same, resizing. `edges` of zero picks from the pointer's position in
    /// the window
    fn wm_begin_resize(&mut self, wm: u32, client: u32, surface: u32, edges: u32) {
        self.put(wm, protocol::wm::BEGIN_RESIZE, &[
            Arg::Uint(client), Arg::Uint(surface), Arg::Uint(edges),
        ]);
    }

    // may_console, for the one program drawing the console

    /// Says which terminal this client draws. Once, first
    fn console_bind(&mut self, console: u32, tty: u32) {
        self.put(console, protocol::console::BIND, &[Arg::Uint(tty)]);
    }

    fn console_set_size(&mut self, console: u32, cols: u32, rows: u32) {
        self.put(console, protocol::console::SET_SIZE, &[
            Arg::Uint(cols), Arg::Uint(rows),
        ]);
    }

    /// Scroll the window back through the history. `0` is the live screen
    fn console_set_view(&mut self, console: u32, rows_back: u32) {
        self.put(console, protocol::console::SET_VIEW, &[Arg::Uint(rows_back)]);
    }

    /// Hand the console back. The kernel's own console window returns
    fn console_detach(&mut self, console: u32) {
        self.put(console, protocol::console::DETACH, &[]);
    }

    // may_layer, for what is not a window

    /// Gives a surface the layer role in one of `protocol::layer`'s bands, in
    /// place of `get_xdg_surface`: a surface has one role
    fn get_layer_surface(&mut self, layer: u32, surface: u32, band: u32) -> u32 {
        let id = self.alloc_id(Interface::LayerSurface);
        self.put(layer, protocol::layer::GET_LAYER_SURFACE, &[
            Arg::NewId(id), Arg::Object(surface), Arg::Uint(band),
        ]);
        id
    }

    /// Which edges this surface is stuck to. Two opposite edges means stretch
    fn layer_set_anchor(&mut self, ls: u32, edges: u32) {
        self.put(ls, protocol::layer_surface::SET_ANCHOR, &[Arg::Uint(edges)]);
    }

    /// Desired size. Zero along an axis anchored at both ends
    fn layer_set_size(&mut self, ls: u32, width: i32, height: i32) {
        self.put(ls, protocol::layer_surface::SET_SIZE, &[
            Arg::Int(width), Arg::Int(height),
        ]);
    }

    /// How many pixels of its edge it keeps. -1 reserves nothing
    fn layer_set_exclusive(&mut self, ls: u32, amount: i32) {
        self.put(ls, protocol::layer_surface::SET_EXCLUSIVE, &[Arg::Int(amount)]);
    }

    /// Whether it wants the keyboard: none, on demand, or all of it
    fn layer_set_keyboard(&mut self, ls: u32, mode: u32) {
        self.put(ls, protocol::layer_surface::SET_KEYBOARD, &[Arg::Uint(mode)]);
    }

    /// What the layer is, for the window manager's layer rules
    fn layer_set_namespace(&mut self, ls: u32, namespace: &str) {
        self.put(ls, protocol::layer_surface::SET_NAMESPACE, &[Arg::Str(Some(namespace))]);
    }

    fn layer_ack_configure(&mut self, ls: u32, serial: u32) {
        self.put(ls, protocol::layer_surface::ACK_CONFIGURE, &[Arg::Uint(serial)]);
    }

    fn destroy_layer_surface(&mut self, ls: u32) {
        self.put(ls, protocol::layer_surface::DESTROY, &[]);
    }

    /// Sends an opcode no interface defines, to check the server refuses it.
    /// Only for tests
    fn raw(&mut self, object: u32, opcode: u16, args: &[Arg]) {
        self.put(object, opcode, args);
    }
}

impl<T: Sink> Requests for T {}
