// Mayland interfaces, written by hand
//
// Interface names, opcode numbers, and argument orders follow
// refsrc/wayland/wayland/protocol/wayland.xml, with may_* for wl_*.
//
// Two divergences, both about file descriptors:
//
//   may_shm.create_pool takes a u32 shared-memory handle where Wayland takes
//   a file descriptor, and may_keyboard.keymap does the same. MikuOS has no
//   SCM_RIGHTS, and the compositor is the kernel, so it looks the handle up
//   in a table
//
// Versions are what is implemented here.

#![allow(dead_code)]

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Interface {
    Display,
    Registry,
    Callback,
    Compositor,
    Shm,
    ShmPool,
    Buffer,
    Surface,
    Region,
    Seat,
    Pointer,
    Keyboard,
    Output,
    /// The window role, three objects as in xdg-shell: a surface is pixels, an
    /// xdg_surface makes them a window, and a toplevel makes it a window on its
    /// own
    Xdg,
    XdgSurface,
    Toplevel,
    /// A recipe for where a child surface goes. An object as in xdg-shell, so a
    /// menu's items reuse it and `reposition` reruns it against a screen edge
    /// the client cannot see
    Positioner,
    /// A menu, tooltip, or dropdown: a surface placed against another, drawn
    /// outside the opening window
    Popup,
    /// What is on the screen and what the user asked for. Several clients may
    /// hold it, as a panel needs the same facts as the window manager.
    /// Privileged, since a window list holds every program's title; the
    /// registry hides it from others
    Desktop,
    /// The desktop's policy: layout, gaps, numbers, curves, border, key chords.
    /// One client holds it, and holding it is commanding. The facts it needs
    /// are `may_desktop`, bound beside this
    Wm,
    /// The kernel's text console, given to one client to draw. The kernel keeps
    /// the character grid (pen, wrapping, reflow, history) and the client the
    /// pixels, so this carries a shared-memory handle on the cell grid and
    /// which rows changed. Separate from `may_wm`, since it is a different
    /// client with a different job
    Console,
    /// The role for a surface that is not a window: a panel, wallpaper, or lock
    /// screen. It names the edge it belongs to and how much it keeps, which a
    /// tiling layout routes around, and the band it lives in. Shaped after
    /// wlr-layer-shell, trimmed to four requests
    Layer,
    LayerSurface,
    /// What one program hands another: copy and paste. Not shaped after
    /// `wl_data_device`, which moves content down a file descriptor; this
    /// transport has none. Only a client just typed at may set the selection,
    /// and only the focused one may read it
    Clipboard,
    /// Outputs as the display layer has them, every connector and mode, and how
    /// to change them. wlr-output-management's meaning in two interfaces: a
    /// connector's name is stable, so a head is a name and a mode an index into
    /// its list. Bound only by the holder of `may_wm`, since mode and refresh
    /// are policy (`dev/kms-architecture.md`, K3)
    OutputManager,
    /// One change to the outputs, built and then tested or applied once
    OutputConfiguration,
    /// Hyprland's focus grab (`hyprland_focus_grab_v1`), from `may_layer`
    /// version 2. While active, the pointer reaches only the listed surfaces,
    /// and a press elsewhere ends the grab with `cleared` and goes no further.
    /// A shell's overview closes this way without the click landing behind it
    FocusGrab,
}

impl Interface {
    pub fn name(self) -> &'static str {
        match self {
            Interface::Display    => "may_display",
            Interface::Registry   => "may_registry",
            Interface::Callback   => "may_callback",
            Interface::Compositor => "may_compositor",
            Interface::Shm        => "may_shm",
            Interface::ShmPool    => "may_shm_pool",
            Interface::Buffer     => "may_buffer",
            Interface::Surface    => "may_surface",
            Interface::Region     => "may_region",
            Interface::Seat       => "may_seat",
            Interface::Pointer    => "may_pointer",
            Interface::Keyboard   => "may_keyboard",
            Interface::Output     => "may_output",
            Interface::Xdg        => "may_xdg",
            Interface::XdgSurface => "may_xdg_surface",
            Interface::Toplevel   => "may_toplevel",
            Interface::Positioner => "may_positioner",
            Interface::Popup      => "may_popup",
            Interface::Desktop    => "may_desktop",
            Interface::Wm         => "may_wm",
            Interface::Console    => "may_console",
            Interface::Clipboard    => "may_clipboard",
            Interface::Layer        => "may_layer",
            Interface::LayerSurface => "may_layer_surface",
            Interface::OutputManager       => "may_output_manager",
            Interface::OutputConfiguration => "may_output_configuration",
            Interface::FocusGrab => "may_focus_grab",
        }
    }

    /// The highest version this implementation speaks
    pub fn version(self) -> u32 {
        match self {
            // Version 2 adds `app_id` to the window event and the work area
            // event; a client binding version 1 gets the shorter event.
            //
            // Version 3 adds `signal` and `signalled`: one word carried between
            // two clients, opaque to the compositor. A listener at 2 does not
            // get the event.
            //
            // Version 4 adds `keyboard_layout`, which a bar shows.
            //
            // Version 5 adds `capture` and `captured`: a window's pixels copied
            // into the asker's pool, for an overview's cells
            Interface::Desktop => 5,
            // Version 2 is `wl_output` version 4's: `name` and `description`. A
            // client bound at 1 gets what it did before
            Interface::Output => 2,
            // Version 2 adds `get_focus_grab`
            Interface::Layer => 2,
            // Everything else is at 1
            _ => 1,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        const ALL: &[Interface] = &[
            Interface::Display, Interface::Registry, Interface::Callback,
            Interface::Compositor, Interface::Shm, Interface::ShmPool,
            Interface::Buffer, Interface::Surface, Interface::Region,
            Interface::Seat, Interface::Pointer, Interface::Keyboard,
            Interface::Output, Interface::Xdg, Interface::XdgSurface,
            Interface::Toplevel, Interface::Positioner, Interface::Popup,
            Interface::Desktop, Interface::Wm, Interface::Console,
            Interface::Layer, Interface::LayerSurface, Interface::Clipboard,
            Interface::OutputManager, Interface::OutputConfiguration,
            Interface::FocusGrab,
        ];
        ALL.iter().copied().find(|i| i.name() == name)
    }
}

/// Object id of the display, live at the start of each connection
pub const DISPLAY_ID: u32 = 1;

// may_display

pub mod display {
    // requests
    pub const SYNC:         u16 = 0; // new_id callback
    pub const GET_REGISTRY: u16 = 1; // new_id registry

    // events
    pub const ERROR:     u16 = 0; // object_id, uint code, string message
    pub const DELETE_ID: u16 = 1; // uint id

    // error codes carried by the error event
    pub const ERR_INVALID_OBJECT: u32 = 0;
    pub const ERR_INVALID_METHOD: u32 = 1;
    pub const ERR_NO_MEMORY:      u32 = 2;
    pub const ERR_IMPLEMENTATION: u32 = 3;
}

// may_registry

pub mod registry {
    // requests
    //   bind(uint name, string interface, uint version, new_id id)
    //
    // Interface and version are in the message because the created argument has
    // no fixed type, as in Wayland
    pub const BIND: u16 = 0;

    // events
    pub const GLOBAL:        u16 = 0; // uint name, string interface, uint version
    pub const GLOBAL_REMOVE: u16 = 1; // uint name
}

// may_callback

pub mod callback {
    // events
    pub const DONE: u16 = 0; // uint callback_data

    // The server destroys a callback as it fires; the client must not reuse the
    // id
}

// may_compositor

pub mod compositor {
    // requests
    pub const CREATE_SURFACE: u16 = 0; // new_id surface
    pub const CREATE_REGION:  u16 = 1; // new_id region
}

// may_shm / may_shm_pool / may_buffer

pub mod shm {
    // requests
    //   create_pool(new_id pool, uint handle, int size)
    // Wayland's second argument is a file descriptor; here it is a handle from
    // mayland::shm, from the shared-memory syscall
    pub const CREATE_POOL: u16 = 0;

    // events
    pub const FORMAT: u16 = 0; // uint format

    // error codes
    pub const ERR_INVALID_FORMAT: u32 = 0;
    pub const ERR_INVALID_STRIDE: u32 = 1;
    pub const ERR_INVALID_HANDLE: u32 = 2;

    // pixel formats, same numbering as wl_shm.format
    pub const FORMAT_ARGB8888: u32 = 0;
    pub const FORMAT_XRGB8888: u32 = 1;
}

pub mod shm_pool {
    // requests
    //   create_buffer(new_id buffer, int offset, int width, int height,
    //                 int stride, uint format)
    pub const CREATE_BUFFER: u16 = 0;
    pub const DESTROY:       u16 = 1;
    pub const RESIZE:        u16 = 2; // int size
}

pub mod buffer {
    // requests
    pub const DESTROY: u16 = 0;

    // events
    //   release: the compositor is done reading this buffer and the client
    //   may draw into it. Lifetime rules are in server.rs
    pub const RELEASE: u16 = 0;
}

// may_surface

pub mod surface {
    // requests
    pub const DESTROY:           u16 = 0;
    pub const ATTACH:            u16 = 1; // object buffer, int x, int y
    pub const DAMAGE:            u16 = 2; // int x, y, width, height
    pub const FRAME:             u16 = 3; // new_id callback
    pub const SET_OPAQUE_REGION: u16 = 4; // object region
    pub const SET_INPUT_REGION:  u16 = 5; // object region
    pub const COMMIT:               u16 = 6;
    pub const SET_BUFFER_TRANSFORM: u16 = 7; // int transform
    pub const SET_BUFFER_SCALE:     u16 = 8; // int scale
    pub const DAMAGE_BUFFER:        u16 = 9; // int x, y, width, height
    pub const OFFSET:               u16 = 10; // int x, int y
    /// uint fence_lo, uint fence_hi: a GPU fence this commit's pixels wait on.
    ///
    /// For a client that drew with `gpu_submit` and does not want to stall
    /// before committing. The compositor absorbs the wait with a budget. Zero
    /// or not sent when the pixels are ready, as for any CPU-drawn client
    pub const SET_FENCE:            u16 = 11;

    // events
    pub const ENTER: u16 = 0; // object output
    pub const LEAVE: u16 = 1; // object output

    // error codes
    pub const ERR_INVALID_SCALE:     u32 = 0;
    pub const ERR_INVALID_TRANSFORM: u32 = 1;
    pub const ERR_INVALID_SIZE:      u32 = 2;
    pub const ERR_INVALID_OFFSET:    u32 = 3;

    /// The only buffer transform accepted. Rotation and flipping need a pass
    /// the copy engine cannot do
    pub const TRANSFORM_NORMAL: i32 = 0;
}

// may_region

pub mod region {
    // requests
    pub const DESTROY:  u16 = 0;
    pub const ADD:      u16 = 1; // int x, y, width, height
    pub const SUBTRACT: u16 = 2; // int x, y, width, height
}

// may_seat / may_pointer / may_keyboard

pub mod seat {
    // requests
    pub const GET_POINTER:  u16 = 0; // new_id pointer
    pub const GET_KEYBOARD: u16 = 1; // new_id keyboard
    pub const RELEASE:      u16 = 2;

    // events
    pub const CAPABILITIES: u16 = 0; // uint capabilities
    pub const NAME:         u16 = 1; // string name

    pub const CAP_POINTER:  u32 = 1;
    pub const CAP_KEYBOARD: u32 = 2;
}

pub mod pointer {
    // requests
    pub const SET_CURSOR: u16 = 0; // uint serial, object surface, int x, int y
    pub const RELEASE:    u16 = 1;

    // events
    pub const ENTER:  u16 = 0; // uint serial, object surface, fixed x, fixed y
    pub const LEAVE:  u16 = 1; // uint serial, object surface
    pub const MOTION: u16 = 2; // uint time, fixed x, fixed y
    pub const BUTTON: u16 = 3; // uint serial, uint time, uint button, uint state
    pub const AXIS:   u16 = 4; // uint time, uint axis, fixed value

    pub const BUTTON_RELEASED: u32 = 0;
    pub const BUTTON_PRESSED:  u32 = 1;
}

pub mod keyboard {
    // requests
    pub const RELEASE: u16 = 0;

    // events
    //   keymap(uint format, uint handle, uint size)
    // A shared-memory handle where Wayland passes a file descriptor, as with
    // shm
    pub const KEYMAP:      u16 = 0;
    pub const ENTER:       u16 = 1; // uint serial, object surface, array keys
    pub const LEAVE:       u16 = 2; // uint serial, object surface
    pub const KEY:         u16 = 3; // uint serial, uint time, uint key, uint state
    pub const MODIFIERS:   u16 = 4; // uint serial, depressed, latched, locked, group
                                    // group is the layout in use, an index into
                                    // the keymap's groups
    pub const REPEAT_INFO: u16 = 5; // int rate, int delay

    pub const KEYMAP_NO_KEYMAP: u32 = 0;
    pub const KEYMAP_XKB_V1:    u32 = 1;
    /// One-group table, not sent. The number stays taken: a client knowing only
    /// this is told an unknown format and types US
    pub const KEYMAP_MAY_V1:    u32 = 2;
    /// The compositor's table, a group per layout the seat switches between;
    /// see keymap.rs. The format field would allow xkb later. The group a key
    /// was typed in is the last field of `MODIFIERS`
    pub const KEYMAP_MAY_V2:    u32 = 3;

    pub const KEY_RELEASED: u32 = 0;
    pub const KEY_PRESSED:  u32 = 1;
}

// may_output

pub mod output {
    // requests
    pub const RELEASE: u16 = 0;

    // events
    //   geometry(int x, int y, int phys_width, int phys_height,
    //            int subpixel, string make, string model, int transform)
    pub const GEOMETRY: u16 = 0;
    pub const MODE:     u16 = 1; // uint flags, int width, int height, int refresh
    pub const DONE:     u16 = 2;
    pub const SCALE:    u16 = 3; // int factor
    // version 2 (`wl_output` version 4)
    pub const NAME:        u16 = 4; // string name: `DP-3`, never changes
    pub const DESCRIPTION: u16 = 5; // string description: make and model

    pub const MODE_CURRENT:   u32 = 1;
    pub const MODE_PREFERRED: u32 = 2;
}

// may_output_manager / may_output_configuration: changing the outputs
//
// wlr-output-management-unstable-v1's meaning, flattened: a head is its name
// and a mode an index into its list. Opcodes are wlr's where they correspond.
//
// The manager sends every head and its modes, then `done` with a serial, again
// on any change. A configuration is made against that serial; one against an
// older serial is `cancelled`.

pub mod output_manager {
    // requests
    pub const CREATE_CONFIGURATION: u16 = 0; // new_id, uint serial
    pub const STOP:                 u16 = 1;

    // events
    //   head(string name, string description, uint flags, int current,
    //        int preferred, uint adaptive_sync, uint max_bpc, uint bpc)
    pub const HEAD:     u16 = 0;
    pub const DONE:     u16 = 1; // uint serial
    pub const FINISHED: u16 = 2;
    //   mode(string head, uint index, int width, int height, int refresh_mhz)
    pub const MODE:     u16 = 3;

    // `head` flags
    pub const CONNECTED: u32 = 1;
    /// A head drives it
    pub const ENABLED:   u32 = 2;
    /// The compositor draws on it
    pub const PRIMARY:   u32 = 4;

    // `adaptive_sync`
    pub const VRR_UNSUPPORTED: u32 = 0;
    pub const VRR_OFF:         u32 = 1;
    pub const VRR_ON:          u32 = 2;
}

pub mod output_configuration {
    // requests
    //   set(string head, int mode, int adaptive_sync, uint bpc): `KEEP` for
    //   unchanged, bpc 0 for the same
    pub const SET:          u16 = 0;
    pub const DISABLE_HEAD: u16 = 1; // string head
    pub const APPLY:        u16 = 2;
    pub const TEST:         u16 = 3;
    pub const DESTROY:      u16 = 4;

    // events
    pub const SUCCEEDED: u16 = 0;
    pub const FAILED:    u16 = 1; // string reason: check's words
    pub const CANCELLED: u16 = 2;

    pub const KEEP: i32 = -1;
}

// may_xdg / may_xdg_surface / may_toplevel: the window role
//
// Opcodes follow xdg-shell.xml.
//
// The compositor proposes a size; the client acknowledges by serial and commits
// a buffer drawn for it. The ack says which resize a given buffer answers

pub mod xdg {
    // requests
    pub const DESTROY:           u16 = 0;
    pub const CREATE_POSITIONER: u16 = 1;
    pub const GET_XDG_SURFACE:   u16 = 2; // new_id, object surface
    pub const PONG:              u16 = 3; // uint serial

    // events
    pub const PING: u16 = 0; // uint serial

    // error codes
    pub const ERR_ROLE:                u32 = 0;
    pub const ERR_DEFUNCT_SURFACES:    u32 = 1;
    pub const ERR_NOT_THE_TOPMOST_POPUP: u32 = 2;
    pub const ERR_INVALID_POPUP_PARENT:  u32 = 3;
    pub const ERR_INVALID_SURFACE_STATE: u32 = 4;
}

pub mod xdg_surface {
    // requests
    pub const DESTROY:             u16 = 0;
    pub const GET_TOPLEVEL:        u16 = 1; // new_id
    pub const GET_POPUP:           u16 = 2;
    pub const SET_WINDOW_GEOMETRY: u16 = 3; // int x, y, width, height
    pub const ACK_CONFIGURE:       u16 = 4; // uint serial

    // events
    pub const CONFIGURE: u16 = 0; // uint serial

    // error codes
    pub const ERR_NOT_CONSTRUCTED:     u32 = 1;
    pub const ERR_ALREADY_CONSTRUCTED: u32 = 2;
    pub const ERR_UNCONFIGURED_BUFFER: u32 = 3;
}

// may_positioner / may_popup: a surface placed against another
//
// xdg-shell's positioner: which corner of the button it hangs off (`anchor`),
// which of its own corners meets that point (`gravity`), and what to do off
// screen (`constraint`).
//
// Order: anchor point on the anchor rectangle, then gravity, then the offset,
// then the constraint.

pub mod positioner {
    // requests
    pub const DESTROY:                   u16 = 0;
    /// int width, int height: the popup's size
    pub const SET_SIZE:                  u16 = 1;
    /// int x, y, width, height: what it hangs off, in the parent's coordinates
    pub const SET_ANCHOR_RECT:           u16 = 2;
    /// uint: which point of the anchor rectangle
    pub const SET_ANCHOR:                u16 = 3;
    /// uint: which corner of the popup meets it
    pub const SET_GRAVITY:               u16 = 4;
    /// uint: a mask of `adjust`, the action at a screen edge
    pub const SET_CONSTRAINT_ADJUSTMENT: u16 = 5;
    /// int x, int y: nudged by this much afterwards
    pub const SET_OFFSET:                u16 = 6;
    // Accepted and not acted on. A reactive positioner needs a client told
    // mid-drag; parent size and configure are for a client constraining itself.
    // Numbered as in xdg-shell, so a toolkit sending them is ignored and not
    // disconnected
    pub const SET_REACTIVE:              u16 = 7;
    pub const SET_PARENT_SIZE:           u16 = 8;
    pub const SET_PARENT_CONFIGURE:      u16 = 9;

    /// Which point of the anchor rectangle the popup hangs off
    pub mod anchor {
        pub const NONE:         u32 = 0;
        pub const TOP:          u32 = 1;
        pub const BOTTOM:       u32 = 2;
        pub const LEFT:         u32 = 3;
        pub const RIGHT:        u32 = 4;
        pub const TOP_LEFT:     u32 = 5;
        pub const BOTTOM_LEFT:  u32 = 6;
        pub const TOP_RIGHT:    u32 = 7;
        pub const BOTTOM_RIGHT: u32 = 8;
        pub const LAST:         u32 = 8;
    }

    /// Which way the popup extends from that point. The same nine names,
    /// mirrored: `gravity::BOTTOM_RIGHT` puts its top left at the anchor point
    pub mod gravity {
        pub const NONE:         u32 = 0;
        pub const TOP:          u32 = 1;
        pub const BOTTOM:       u32 = 2;
        pub const LEFT:         u32 = 3;
        pub const RIGHT:        u32 = 4;
        pub const TOP_LEFT:     u32 = 5;
        pub const BOTTOM_LEFT:  u32 = 6;
        pub const TOP_RIGHT:    u32 = 7;
        pub const BOTTOM_RIGHT: u32 = 8;
        pub const LAST:         u32 = 8;
    }

    /// What may be done when the popup does not fit
    pub mod adjust {
        pub const NONE:     u32 = 0;
        pub const SLIDE_X:  u32 = 1;
        pub const SLIDE_Y:  u32 = 2;
        pub const FLIP_X:   u32 = 4;
        pub const FLIP_Y:   u32 = 8;
        /// Accepted in the mask and not done: resizing would wait on a client
        /// redraw. A popup that does not fit slides or flips
        pub const RESIZE_X: u32 = 16;
        pub const RESIZE_Y: u32 = 32;
        pub const ALL:      u32 = 63;
    }

    // error codes
    pub const ERR_INVALID_INPUT: u32 = 0;
}

pub mod popup {
    // requests
    pub const DESTROY:    u16 = 0;
    /// object seat, uint serial: take the pointer and the keyboard
    pub const GRAB:       u16 = 1;
    /// object positioner, uint token: run a new recipe
    pub const REPOSITION: u16 = 2;

    // events
    /// int x, int y, int width, int height: where it went, relative to the
    /// parent. Sent before its xdg_surface configure, as xdg-shell specifies
    pub const CONFIGURE:   u16 = 0;
    /// The menu is dismissed. The client destroys the popup; this is sent once
    pub const POPUP_DONE:  u16 = 1;
    /// uint token: the reposition with this token has been applied
    pub const REPOSITIONED: u16 = 2;

    // error codes
    /// A grab on a popup that is not the innermost one open
    pub const ERR_INVALID_GRAB: u32 = 0;
}

// may_wm
//
// The window manager's channel: one client may bind it, seeing and moving every
// window. First come, first served, released on disconnect, with no capability
// behind it.
//
// Geometry is not here. The compositor decides a tiled window's size and place;
// this chooses the layout and its numbers and moves individual windows, as with
// Hyprland. The desktop works with nothing bound, and a window reaches the
// screen without a round trip to ring 3.
//
// Windows are named by (client, surface), the protocol's name, also printed by
// `mayland status`.

/// may_desktop: what is on the screen, and what the user asked for
///
/// Desktop facts and the four verbs for key presses. Several clients hold it at
/// once. Privileged, since a window list has every title and geometry, as
/// `ext-foreign-toplevel-list` is upstream; see `server::access`
pub mod desktop {
    // requests
    /// uint serial: answered by one `WINDOW` per window, then `WINDOWS_DONE`
    pub const GET_WINDOWS:       u16 = 0;
    /// uint client, uint surface
    pub const FOCUS:             u16 = 1;
    /// uint client, uint surface: asks the window to close, never forces it
    pub const CLOSE:             u16 = 2;
    /// uint index
    pub const WORKSPACE:         u16 = 3;
    /// uint client, uint surface, uint index
    pub const MOVE_TO_WORKSPACE: u16 = 4;
    /// string name: says a word to every listener. The compositor does not
    /// interpret it: a key binding lives in the window manager, and what it
    /// opens is a layer in another program. Version 3 and above
    pub const SIGNAL:            u16 = 5;
    /// uint client, uint surface, uint pool, uint offset, uint serial: copies
    /// the window's last committed pixels into the asker's pool at `offset`,
    /// rows `width * 4` bytes apart; answered by `CAPTURED`.
    ///
    /// What end4's overview shows in a workspace cell, as Hyprland's toplevel
    /// export gives quickshell. The copy is unscaled; the shell draws it into
    /// its own cell. Counted against the asker's pool, and only for a holder of
    /// this interface. Version 5 and above
    pub const CAPTURE:           u16 = 6;

    // events
    /// uint client, uint surface, string title, string app_id, int x, int y,
    /// int w, int h, uint flags, uint workspace
    ///
    /// `app_id` is what a window rule matches, since a title changes with the
    /// document. A client bound at version 1 gets the shorter event, see
    /// `version()`
    pub const WINDOW:            u16 = 0;
    /// uint serial: the requested list ends here
    pub const WINDOWS_DONE:      u16 = 1;
    /// uint client, uint surface
    pub const OPENED:            u16 = 2;
    /// uint client, uint surface
    pub const CLOSED:            u16 = 3;
    /// uint client, uint surface: 0,0 when nothing has the keyboard
    pub const FOCUSED:           u16 = 4;
    /// uint index
    pub const WORKSPACE_CHANGED: u16 = 5;
    /// int x, int y, int w, int h, int screen_w, int screen_h
    ///
    /// Rectangle a layout may use: the screen minus the outer gap and reserved
    /// edges. Sent at bind and on change. Version 2 and above
    pub const WORK_AREA:         u16 = 6;
    /// string name: what was said with `SIGNAL`. Sent to every listener
    /// including the sender, so a shell can talk to itself across two
    /// connections. Version 3 and above
    pub const SIGNALLED:         u16 = 7;
    /// uint group, uint groups, string code, string name: the keyboard layout
    /// in use, as `1, 2, "ru", "Russian"`.
    ///
    /// For a bar's indicator. Sent at bind and on a change of layout or list.
    /// Its own event, since the compositor must not speak in the shell's words.
    /// Version 4 and above
    pub const KEYBOARD_LAYOUT:   u16 = 8;
    /// uint serial, uint copied, int w, int h, uint format: what `CAPTURE` did,
    /// to the asker alone.
    ///
    /// `copied` 1: the window's `w` by `h` pixels are in the pool, in `format`
    /// (`shm::FORMAT_*`). 0 with a size: they did not fit from the offset, and
    /// a bigger pool would. 0 with no size: nothing to copy, whether no such
    /// window, nothing committed, or pixels in video memory, which this does
    /// not read. Version 5 and above
    pub const CAPTURED:          u16 = 9;

    // flags on a WINDOW event
    pub const FLAG_FOCUSED:  u32 = 1;
    /// On every workspace, not movable by the window manager: the console and
    /// wallpaper
    pub const FLAG_STICKY:   u32 = 2;
    /// Outside the layout, at the back
    pub const FLAG_BACKDROP: u32 = 4;
    /// Out of the tiling and keeping its own size
    pub const FLAG_FLOATING:   u32 = 8;
    /// The whole work area, over everything
    pub const FLAG_FULLSCREEN: u32 = 16;
    /// On every workspace by request, unlike `STICKY`, which the compositor
    /// sets for the console and wallpaper
    pub const FLAG_PINNED:     u32 = 32;
    /// Never given the keyboard by the compositor's focus cycle: a splash
    /// screen, notification, or toolbar. Hyprland's `windowrule = nofocus, ...`
    pub const FLAG_NO_FOCUS:   u32 = 64;

    /// Workspace 0 means "where it already is" and is never a target. Ten, as
    /// on Hyprland: Ctrl+1 to Ctrl+0
    pub const WORKSPACES: u32 = 10;
}

/// may_wm: the desktop's policy
///
/// The layout, gaps, numbers, curves, border, and the key chords the shell
/// wants first. One client at a time. Facts such as windows and keyboard focus
/// are in `may_desktop`, which a panel may bind without being given the policy
pub mod wm {
    // Window flags travel on a `may_desktop.window` event, and the state mask
    // below has the same bits
    #[allow(unused_imports)]
    pub use super::desktop::{FLAG_BACKDROP, FLAG_FLOATING, FLAG_FOCUSED,
                             FLAG_FULLSCREEN, FLAG_NO_FOCUS, FLAG_PINNED,
                             FLAG_STICKY};

    // requests
    /// uint client, uint surface, int x, int y
    pub const MOVE:              u16 = 0;
    /// uint client, uint surface, int width, int height
    pub const RESIZE:            u16 = 1;
    /// uint layout
    pub const SET_LAYOUT:        u16 = 2;
    /// int outer, int inner
    pub const SET_GAPS:          u16 = 3;
    /// uint id, uint mods, uint keycode, uint flags
    ///
    /// `flags` is the `chord` module below: the letters after `bind` in a
    /// config line
    pub const BIND_CHORD:        u16 = 4;
    /// uint id
    pub const UNBIND_CHORD:      u16 = 5;
    /// uint param, fixed value: one desktop-wide number. A channel, not a
    /// request per setting. Unknown params are ignored, so a config for a newer
    /// compositor still boots
    pub const SET_PARAM:         u16 = 6;
    /// uint client, uint surface, uint param, fixed value, uint set
    ///
    /// The same number for one window, as Hyprland's window rules:
    /// `opacity 0.8`, `rounding 0`, `noblur`. `set` of 0 returns to the
    /// desktop's value
    pub const SET_WINDOW_PARAM:  u16 = 7;
    /// uint client, uint surface, uint mask, uint flags
    ///
    /// A window's booleans: floating, fullscreen, pinned. `mask` says which
    /// this message is about, so one request sets and clears
    pub const SET_WINDOW_STATE:  u16 = 8;
    /// uint client, uint surface, uint client2, uint surface2: trade places
    pub const SWAP:              u16 = 9;
    /// uint id, fixed x1, fixed y1, fixed x2, fixed y2: names a curve. A
    /// request, since a bezier is four numbers under a name
    pub const SET_CURVE:         u16 = 10;
    /// uint class, uint enabled, fixed speed, uint curve, uint style, fixed
    /// arg: what one class animates with. Same shape as the config line:
    /// `animation = windows, 1, 4, easeOut, popin 80%`
    pub const SET_ANIMATION:     u16 = 11;
    /// uint size, uint active, uint inactive: the ring around a window.
    ///
    /// A request, since the param channel carries 24.8 fixed point and a color
    /// is 24 integer bits. The two colors travel together as one decision.
    ///
    /// `0xAARRGGBB`: the border blends over the window and what is behind it
    /// (`dev/mayland-window-shape.md`). An alpha of 0 means none given, and is
    /// solid
    pub const SET_BORDER:        u16 = 12;
    /// uint client, uint surface: carry this window with the pointer.
    ///
    /// It follows the cursor until the starting button comes up, and the client
    /// is told nothing. `0, 0` means the window under the pointer, as for a
    /// mouse binding. Config: `bindm = ALT, mouse:272, movewindow`
    pub const BEGIN_MOVE:        u16 = 13;
    /// uint client, uint surface, uint edges: resize it with the pointer.
    /// `edges` is a mask of the `edge` values below. Zero picks them from the
    /// pointer's quadrant of the window
    pub const BEGIN_RESIZE:      u16 = 14;
    /// No arguments: returns every desktop-wide number to its default.
    ///
    /// `set_param` and its neighbors are a channel of changes, so re-reading a
    /// config would otherwise keep what the file removed. The defaults belong
    /// to the compositor.
    ///
    /// Chords are not touched, being addressed by id and re-bound by their
    /// sender, nor are per-window numbers
    pub const RESET:             u16 = 16;
    /// string path: the picture behind everything, or "" for none.
    ///
    /// Drawn by the compositor, named in a config only the window manager
    /// reads, so the path crosses on this one-client interface. A path instead
    /// of pixels: the compositor has `libmay::qoi` and owns the destination
    /// buffer.
    ///
    /// An empty string restores the gradient, and `RESET` sends it
    pub const SET_WALLPAPER:     u16 = 17;
    /// int x, int y: puts the pointer somewhere.
    ///
    /// `movecursor` in a config, and the corner dispatcher built on it. On this
    /// interface since moving the cursor is desktop policy. The crossing is
    /// computed as for a mouse, so a warp onto a window enters it
    pub const WARP_POINTER:      u16 = 15;

    /// Which edges a resize pulls, in the shape of xdg-shell's
    /// `xdg_toplevel.resize_edge`
    pub mod edge {
        pub const TOP:    u32 = 1;
        pub const BOTTOM: u32 = 2;
        pub const LEFT:   u32 = 4;
        pub const RIGHT:  u32 = 8;
        pub const ALL:    u32 = TOP | BOTTOM | LEFT | RIGHT;
    }

    // events
    /// uint id: a chord this client bound was pressed. Here because a chord
    /// goes to whoever asked, and only this interface's holder may ask
    pub const CHORD:             u16 = 0;

    // layouts, for SET_LAYOUT
    pub const LAYOUT_FLOAT:   u32 = 0;
    pub const LAYOUT_MASTER:  u32 = 1;
    /// Every window splits the one it opened on, along that window's longer
    /// side. Hyprland's default
    pub const LAYOUT_DWINDLE: u32 = 2;

    /// Booleans `set_window_state` carries, in its mask and flags. Same bits as
    /// the `FLAG_` values of a `may_desktop.window` event
    pub mod state {
        pub const FLOATING:   u32 = super::FLAG_FLOATING;
        pub const FULLSCREEN: u32 = super::FLAG_FULLSCREEN;
        pub const PINNED:     u32 = super::FLAG_PINNED;
        pub const NO_FOCUS:   u32 = super::FLAG_NO_FOCUS;
        /// Everything a window manager may set
        pub const ALL: u32 = FLOATING | FULLSCREEN | PINNED | NO_FOCUS;
    }

    /// Numbers `set_param` and `set_window_param` carry. Every value is 24.8
    /// fixed point in the param's natural unit: pixels for a gap, a fraction of
    /// one for an opacity. Grouped so an addition does not renumber later
    /// groups
    pub mod param {
        // The layout
        pub const GAPS_OUT:      u32 = 0;
        pub const GAPS_IN:       u32 = 1;
        /// How a dwindle split divides its box: 1.0 is even, range 0.1 to 1.9,
        /// as in Hyprland
        pub const SPLIT_RATIO:   u32 = 2;
        /// How much of the work area the master window takes, 0 to 1
        pub const MASTER_FACTOR: u32 = 3;

        // Decoration, often per window
        pub const EFFECTS:     u32 = 16;
        pub const ALPHA:       u32 = 17;
        pub const ROUNDING:    u32 = 18;
        pub const SHADOW:      u32 = 19;
        pub const SHADOW_SIZE: u32 = 20;
        pub const BLUR_SIZE:   u32 = 21;
        /// `rounding_power`: 2 is a circle, higher is squarer, and the radius
        /// is `ROUNDING · power / 2` as in Hyprland (`shape::Shape`)
        pub const ROUNDING_POWER: u32 = 22;
        /// `inactive_opacity`: applied to every window but the active one.
        /// `ALPHA` is the active one's, `active_opacity`
        pub const INACTIVE_ALPHA: u32 = 23;
        /// Darkening of every window but the active one, 0 to 1: `dim_strength`
        /// while `dim_inactive` is on, else 0
        pub const DIM:            u32 = 24;

        // Animations: the global default a `set_animation` for a class
        // overrides. Curves and tuples go through `SET_CURVE` and
        // `SET_ANIMATION`
        pub const ANIM_SPEED: u32 = 32;
        pub const ANIM_CURVE: u32 = 33;
        /// Whether anything animates: `animations { enabled = ... }`
        pub const ANIM_ENABLED: u32 = 34;

        // Input, Hyprland's `input {}` block. A config naming an unknown value
        // is told so
        /// Key repeats per second, and the wait before the first, in
        /// milliseconds. Both go to the keyboard controller, which does the
        /// repeating, and to `repeat_info`
        pub const REPEAT_RATE:    u32 = 48;
        pub const REPEAT_DELAY:   u32 = 49;
        /// Whether the keyboard follows the pointer into a window. 0 is off, 1
        /// is Hyprland's default
        pub const FOLLOW_MOUSE:   u32 = 50;
        /// A multiplier on pointer movement. Hyprland writes -1..1 with 0
        /// meaning device speed; read here as a factor of `1 + value`, clamped,
        /// since there is no acceleration curve
        pub const SENSITIVITY:    u32 = 51;
        /// Scrolling the other way, as a touchpad does
        pub const NATURAL_SCROLL: u32 = 52;
        /// `kb_layout`: up to four indices into `keymap::LAYOUTS`, four bits
        /// each, plus one so zero is no layout: `us,ru` is 0x21. Exact in an
        /// f32, which carries 24 bits. The window manager reads names; the
        /// compositor gets numbers
        pub const KB_LAYOUTS:     u32 = 53;
        /// `kb_options`: which chord moves between layouts. 0 is none, 1 is
        /// `grp:alt_shift_toggle`
        pub const KB_SWITCH:      u32 = 54;
    }

    /// Most chords bound at once. The window manager spends the budget, and the
    /// keyboard interrupt scans a table of this size: 64 relaxed loads per
    /// keystroke
    pub const MAX_CHORDS: usize = 64;

    pub mod chord {
        /// `e`: keep firing while held. The default is once per press, since a
        /// chord matches the make code and typematic repeat sends make codes
        pub const REPEAT:      u32 = 1 << 0;
        /// `r`: fire when the key comes up. On a modifier key
        /// (`bindr = SUPER, Super_L`), only if nothing else went down while
        /// held
        pub const RELEASE:     u32 = 1 << 1;
        /// `n`: the key also goes to whoever would have had it
        pub const PASSTHROUGH: u32 = 1 << 2;
        /// `l`: works while locked. Accepted and inert, as there is no lock
        /// screen
        pub const LOCKED:      u32 = 1 << 3;
        /// `i`: match the named modifiers and ignore others held. The ordinary
        /// match is exact, so Super+2 does not fire on Ctrl+Super+2
        pub const IGNORE_MODS: u32 = 1 << 4;
    }

    // The modifier mask in BIND_CHORD is `keymap`'s (`MOD_SHIFT` and the rest),
    // and the keycode is from the table sent to every client

    /// Workspace 0 means "where it already is" and is never a target
    #[allow(unused_imports)]
    pub use super::desktop::WORKSPACES;

    /// What `set_animation` covers, with Hyprland's names. In and out are
    /// separate, since configs give them different durations
    pub mod anim_class {
        pub const WINDOWS:      u32 = 0;
        pub const WINDOWS_IN:   u32 = 1;
        pub const WINDOWS_OUT:  u32 = 2;
        pub const LAYERS:       u32 = 3;
        pub const LAYERS_IN:    u32 = 4;
        pub const LAYERS_OUT:   u32 = 5;
        pub const FADE:         u32 = 6;
        pub const WORKSPACES:   u32 = 7;
        /// The two halves of a workspace switch. Both inherit `workspaces` when
        /// unset, as in Hyprland, so one line sets both
        pub const WORKSPACES_IN:  u32 = 8;
        pub const WORKSPACES_OUT: u32 = 9;
        /// A surface's alpha on its own clock, Hyprland's `fade` tree. A
        /// window's opacity rises on `fadeIn` while its size moves on
        /// `windowsIn`, a layer's on `fadeLayersIn` while it moves on
        /// `layersIn`. Each falls back to its parent: `fadeLayersIn` to
        /// `fadeLayers`, that to `fade` (`dev/mayland-layers-above.md`, section
        /// 4)
        pub const FADE_IN:         u32 = 10;
        pub const FADE_OUT:        u32 = 11;
        pub const FADE_LAYERS:     u32 = 12;
        pub const FADE_LAYERS_IN:  u32 = 13;
        pub const FADE_LAYERS_OUT: u32 = 14;
        pub const COUNT:        u32 = 15;
    }

    /// uint rule, string namespace: a rule for every layer surface matching:
    /// `layerrule = noanim, quickshell:overview`. `.*` is any namespace,
    /// anything else a substring, as for window rules. `reset` forgets all of
    /// them
    pub const SET_LAYER_RULE:    u16 = 18;
    /// What a layer rule does
    pub mod layer_rule {
        /// Hyprland's `noanim`: the layer appears at once, with no fade or
        /// movement
        pub const NOANIM: u32 = 0;
    }

    /// What an animation does. `FADE` and `POPIN` need the compositor's shader
    /// pass; `SLIDE` is a position and works wherever the desktop is composed
    pub mod anim_style {
        pub const NONE:  u32 = 0;
        pub const SLIDE: u32 = 1;
        pub const FADE:  u32 = 2;
        /// Grows from `arg` of the final size: `popin 80%` is arg 0.8
        pub const POPIN: u32 = 3;
    }
}

pub mod toplevel {
    // requests
    pub const DESTROY:          u16 = 0;
    pub const SET_PARENT:       u16 = 1;
    pub const SET_TITLE:        u16 = 2;  // string
    pub const SET_APP_ID:       u16 = 3;  // string
    pub const SHOW_WINDOW_MENU: u16 = 4;
    /// uint serial: carry me with the pointer.
    ///
    /// Sent on a press on the strip a client draws as a title bar. The window
    /// follows the pointer until the button comes up. `serial` is the press;
    /// unchecked, with one seat and one pointer
    pub const MOVE:             u16 = 5;
    /// uint serial, uint edges: stretch me with the pointer. `edges` is a
    /// `wm::edge` mask; zero picks from the pointer's position in the window
    pub const RESIZE:           u16 = 6;
    pub const SET_MAX_SIZE:     u16 = 7;  // int width, height
    pub const SET_MIN_SIZE:     u16 = 8;  // int width, height
    pub const SET_MAXIMIZED:    u16 = 9;
    pub const UNSET_MAXIMIZED:  u16 = 10;
    pub const SET_FULLSCREEN:   u16 = 11;
    pub const UNSET_FULLSCREEN: u16 = 12;
    pub const SET_MINIMIZED:    u16 = 13;

    // events
    pub const CONFIGURE: u16 = 0; // int width, int height, array states
    pub const CLOSE:     u16 = 1; // the compositor would like this window gone

    // states carried by configure, as u32 words in the array
    pub const STATE_MAXIMIZED:  u32 = 1;
    pub const STATE_FULLSCREEN: u32 = 2;
    pub const STATE_RESIZING:   u32 = 3;
    pub const STATE_ACTIVATED:  u32 = 4;
}

// may_console
//
// The seam between the kernel's text console and the program drawing it.
//
// What the text is stays in the kernel: the pen, wrapping, scrolling, reflow.
// How it looks belongs to the client: the font, cell size, background, caret,
// margin.
//
// The grid travels as a shared-memory handle, as `may_keyboard.keymap` does.
// Messages say where it is and which rows moved.

pub mod console {
    // requests
    /// uint cols, uint rows: the grid the client has room for. The kernel
    /// reflows to it; a size with no room for a character is refused
    pub const SET_SIZE: u16 = 0;
    /// Gives the console back without closing the connection. The kernel's own
    /// console window returns
    pub const DETACH:   u16 = 1;
    /// string text: put this into the console's input as if typed. For a paste,
    /// since the terminal receives no keys. Safe because `may_console` is one
    /// privileged client at a time.
    ///
    /// Recorded and replayed a frame later: reaching the keyboard's decoder
    /// takes locks the request path must not take, as with `set_size`
    pub const PASTE:    u16 = 2;
    /// uint tty: the terminal this client draws. Sent once, first. A second
    /// bind is a protocol error.
    ///
    /// Binding rule: hold the master end of that pty, obtained by creating it,
    /// or it is the system console and you hold the seat. So no program can
    /// bind a terminal it did not create
    pub const BIND:     u16 = 3;
    /// uint rows_back: scrolls the window back through what left the screen.
    /// `0` is the live screen.
    ///
    /// A request, since the pool is allocated once at the grid's ceiling with
    /// no room for extra rows. The compositor clamps to the history, and the
    /// following `DAMAGE` says what the window holds.
    ///
    /// Typing returns it to 0; output does not, so a busy log can be read back
    pub const SET_VIEW: u16 = 4;

    // events
    /// uint handle, uint size, uint cols, uint rows
    ///
    /// The shared-memory handle holding the cell grid, and the shape in use.
    ///
    /// `size` is the capacity, the largest possible grid; `cols`/`rows` is what
    /// is in it, row-major at the front. The handle never changes for the
    /// connection, since a client reads the grid holding nothing. Re-sent when
    /// the shape changes
    pub const GRID:   u16 = 0;
    /// uint row0, uint row1, uint cursor_col, uint cursor_row
    ///
    /// Rows `row0..row1` changed since the last one. At most once per frame, so
    /// the range is a union. The cursor rides along since the client must erase
    /// it
    pub const DAMAGE: u16 = 1;
    /// The terminal this client was drawing has gone: its master closed or it
    /// hung up. The pool stays mapped and stops changing
    pub const GONE:   u16 = 2;

    /// One cell, sixteen bytes so a cell index is a shift.
    ///
    /// Layout: `ch` as a little-endian `u32`, then foreground `r,g,b`,
    /// background `r,g,b`, `attrs` as a little-endian `u16`, and five bytes of
    /// padding.
    ///
    /// `attrs` bit 8 says whether the background was set, so a cell can follow
    /// the window it is drawn in
    pub const CELL_BYTES: usize = 16;

    /// `attrs` bits, in SGR's order
    pub const ATTR_BOLD:      u16 = 1 << 0;
    pub const ATTR_DIM:       u16 = 1 << 1;
    pub const ATTR_ITALIC:    u16 = 1 << 2;
    pub const ATTR_UNDERLINE: u16 = 1 << 3;
    pub const ATTR_BLINK:     u16 = 1 << 4;
    pub const ATTR_REVERSE:   u16 = 1 << 5;
    pub const ATTR_HIDDEN:    u16 = 1 << 6;
    pub const ATTR_STRIKE:    u16 = 1 << 7;
    /// Whether the background bytes mean anything
    pub const ATTR_BG_SET:    u16 = 1 << 8;
}

// may_layer / may_layer_surface: the role for what is not a window
//
// A panel, wallpaper, or lock screen. Shaped after wlr-layer-shell, trimmed to
// which band and how much of an edge.
//
// `background` is behind every window, `overlay` in front of every window
// including a fullscreen one, and neither is in the focus cycle unless it asks.
//
// A surface anchored to an edge with a positive exclusive zone takes that many
// pixels off the work area, which tiling, maximize, and the window manager
// already read.

pub mod layer {
    // requests on may_layer
    /// new_id layer_surface, object surface, uint layer
    pub const GET_LAYER_SURFACE: u16 = 0;
    pub const DESTROY:           u16 = 1;
    /// new_id focus_grab: version 2
    pub const GET_FOCUS_GRAB:    u16 = 2;

    // the bands, bottom to top
    pub const BACKGROUND: u32 = 0;
    pub const BOTTOM:     u32 = 1;
    pub const TOP:        u32 = 2;
    pub const OVERLAY:    u32 = 3;
    pub const BANDS:      u32 = 4;
}

/// may_focus_grab: a list of the client's own surfaces, in force from `commit`.
/// A non-empty list holds the grab, one client at a time, so a newer grab ends
/// an older one with `cleared`. An empty list releases it silently
pub mod focus_grab {
    // requests
    /// object surface
    pub const ADD_SURFACE:    u16 = 0;
    /// object surface
    pub const REMOVE_SURFACE: u16 = 1;
    pub const COMMIT:         u16 = 2;
    pub const DESTROY:        u16 = 3;

    // events
    /// The grab is over: a press beside its surfaces, or another grab
    pub const CLEARED: u16 = 0;

    /// How many surfaces one grab may list
    pub const MAX_SURFACES: usize = 16;
}

pub mod layer_surface {
    // requests
    /// uint edges: a mask of the ANCHOR_ values below. Anchoring to two
    /// opposite edges stretches between them
    pub const SET_ANCHOR:    u16 = 0;
    /// int width, int height: 0 along an axis anchored at both edges
    pub const SET_SIZE:      u16 = 1;
    /// int amount: pixels of its edge this surface keeps. -1 reserves nothing,
    /// for a lock screen or notification; 0 and above come off the work area
    pub const SET_EXCLUSIVE: u16 = 2;
    /// uint mode: one of the KEYBOARD_ values below
    pub const SET_KEYBOARD:  u16 = 3;
    pub const DESTROY:       u16 = 4;
    /// uint serial: the xdg-shell handshake. A layer surface has no xdg_surface
    /// to ack through
    pub const ACK_CONFIGURE: u16 = 5;
    /// string namespace: what the layer is, `quickshell:overview`, as a layer
    /// rule matches (`wm::SET_LAYER_RULE`). wlr-layer-shell takes it with
    /// `get_layer_surface`; here it is a separate request before the first
    /// commit. At most 64 bytes are kept
    pub const SET_NAMESPACE: u16 = 6;

    // events
    /// uint serial, int width, int height
    pub const CONFIGURE: u16 = 0;
    /// The compositor took the surface away: the output went or the session
    /// ended. The client's destroy is still owed
    pub const CLOSED:    u16 = 1;

    // anchor edges
    pub const ANCHOR_TOP:    u32 = 1;
    pub const ANCHOR_BOTTOM: u32 = 2;
    pub const ANCHOR_LEFT:   u32 = 4;
    pub const ANCHOR_RIGHT:  u32 = 8;
    pub const ANCHOR_ALL:    u32 = 15;

    /// None, for a panel: drawn and clickable, the keyboard stays with the
    /// focused window
    pub const KEYBOARD_NONE:      u32 = 0;
    /// Takes the keyboard when clicked, and gives it back when something else
    /// is
    pub const KEYBOARD_ON_DEMAND: u32 = 1;
    /// Takes and holds the keyboard: a launcher or lock screen
    pub const KEYBOARD_EXCLUSIVE: u32 = 2;

    // error codes
    pub const ERR_ROLE:        u32 = 0;
    pub const ERR_INVALID_SIZE: u32 = 1;
    pub const ERR_INVALID_ANCHOR: u32 = 2;
    pub const ERR_INVALID_LAYER:  u32 = 3;
}

// may_clipboard: what one program hands another
//
// One interface instead of Wayland's four, and a slot index instead of a second
// interface for the middle-button selection: `which` separates clipboard from
// primary selection.
//
// Content travels in a shared-memory pool both ways. Going in, the client names
// a range of a pool it owns. Coming out, the compositor makes a pool in the
// reader's name, since `shm` does not map one pool into two address spaces, and
// the reader adopts it as with the console grid.

pub mod clipboard {
    // requests
    /// uint serial, uint which, array mimes, uint handle, uint offset, uint len
    ///
    /// `serial` is one the compositor sent with a key or button, so a program
    /// cannot take the clipboard at an arbitrary moment
    pub const SET:   u16 = 0;
    /// uint serial, uint which, string mime
    pub const GET:   u16 = 1;
    /// uint which: only the client that set it may
    pub const CLEAR: u16 = 2;

    // events
    /// uint which, string mime: there is something to paste, of this type
    pub const OFFER: u16 = 0;
    /// uint which, uint handle, uint size, string mime: the answer to GET. Map
    /// the handle, read `size` bytes, and free it
    pub const DATA:  u16 = 1;
    /// uint which: its content is gone
    pub const LOST:  u16 = 2;

    /// The clipboard proper, and the middle-button selection
    pub const CLIPBOARD: u32 = 0;
    pub const PRIMARY:   u32 = 1;
}
