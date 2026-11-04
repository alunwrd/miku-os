// What the server is made of
//
// Every type behind an object id, and the four statics that are not behind one.
// Nothing in this file decides anything: a `Surface` knows where it is, what it
// was told to be and what it is showing, and the modules beside this one tell
// it.
//
// `WM` and `CONSOLE` are leases: who may speak on a channel, which outlives any
// one request and belongs to no client's object map. `CURRENT_WS` is the one
// fact about the desktop `stack()` needs. Everything else a client can see
// hangs off the one `Server` behind the one lock.

use alloc::vec::Vec;
use spin::Mutex;

use crate::mayland::objects::ObjectMap;
use crate::mayland::outq::Writer;
use crate::mayland::protocol::{self, Interface, DISPLAY_ID};
use crate::mayland::wire::{Arg};
use crate::mayland::shell::layout::{self, Rect};

use super::{scene};

/// Registry globals. IDs only require server-lifetime stability.
#[derive(Clone, Copy)]
pub struct Global {
    pub name: u32,
    pub iface: Interface,
}

pub(in crate::mayland::server) const GLOBALS: &[Global] = &[
    Global { name: 1, iface: Interface::Compositor },
    Global { name: 2, iface: Interface::Shm },
    Global { name: 3, iface: Interface::Seat },
    Global { name: 4, iface: Interface::Output },
    Global { name: 5, iface: Interface::Xdg },
    Global { name: 6, iface: Interface::Wm },
    Global { name: 7, iface: Interface::Console },
    Global { name: 8, iface: Interface::Layer },
    Global { name: 9, iface: Interface::Desktop },
    Global { name: 10, iface: Interface::Clipboard },
    Global { name: 11, iface: Interface::OutputManager },
];

/// Everyone holding `may_desktop`: the client, the object id, and the version.
///
/// A list, because a panel showing the workspaces needs the same events the
/// window manager acts on. The four verbs of this interface are a user asking;
/// the desktop's policy is behind `WM`.
///
/// The version is here rather than in the object map because every window event
/// asks how much of the window this client may be told, and it can differ
/// between holders
pub(in crate::mayland::server) static DESKTOP: Mutex<Vec<Lease>> = Mutex::new(Vec::new());

/// The one client that decides the desktop's policy.
///
/// Holding this interface is commanding: the registry hands it to one client,
/// so there is no race with a panel connecting earlier and no list of opcodes
/// to police.
///
/// A window manager binds `may_desktop` as well for the facts; this is what it
/// has that a panel has not
pub(in crate::mayland::server) static WM: Mutex<Option<Lease>> = Mutex::new(None);

/// The client drawing a terminal, on the same terms: one at a time, released
/// when the holder goes away. One lease per terminal, indexed by
/// `TtyId::index`.
///
/// A window manager decides where windows go; this decides what the text
/// console looks like, and a machine can have one without the other.
///
/// Two programs drawing one grid would rub out each other's caret. Slot 0 is
/// the system console
pub(in crate::mayland::server) static CONSOLES:
    Mutex<[Option<Lease>; crate::io::tty::MAX_TTY]> =
    Mutex::new([None; crate::io::tty::MAX_TTY]);

#[derive(Clone, Copy)]
pub(in crate::mayland::server) struct Lease {
    pub(in crate::mayland) client: u32,
    pub(in crate::mayland::server) object: u32,
    pub(in crate::mayland::server) version: u32,
}

/// Who holds a one-at-a-time interface
///
/// A connection killed by a protocol error is dead at once but stays in the
/// client table until the transport notices, which for a process that never
/// closes its socket is never. Read straight from the cell, `may_wm` and
/// `may_console` could each be taken for the boot by one bind that failed on
/// purpose.
///
/// So nothing reads `WM` or `CONSOLE` directly: `alive` is asked here, and a
/// dead client's lease reads as free
pub(in crate::mayland::server) fn holder(s: &Server, cell: &Mutex<Option<Lease>>)
    -> Option<Lease>
{
    let held = *cell.lock();
    held.filter(|l| s.alive(l.client))
}

/// Who is drawing one terminal, if anybody still alive is
///
/// The same rule as `holder`, per slot
pub(in crate::mayland::server) fn console_holder(s: &Server, slot: usize)
    -> Option<Lease>
{
    if slot >= crate::io::tty::MAX_TTY { return None; }
    CONSOLES.lock()[slot].filter(|l| s.alive(l.client))
}

/// Every terminal somebody is drawing, as (slot, lease)
pub(in crate::mayland::server) fn console_leases(s: &Server) -> Vec<(usize, Lease)> {
    CONSOLES.lock().iter().enumerate()
        .filter_map(|(i, l)| l.filter(|l| s.alive(l.client)).map(|l| (i, l)))
        .collect()
}

/// The same question for a shared interface: everyone holding it and still
/// alive, in the order they bound it
pub(in crate::mayland::server) fn holders(s: &Server, cell: &Mutex<Vec<Lease>>)
    -> Vec<Lease>
{
    cell.lock().iter().copied().filter(|l| s.alive(l.client)).collect()
}

/// The workspace being looked at. One, until a window manager says otherwise
pub(in crate::mayland) static CURRENT_WS: Mutex<u32> = Mutex::new(1);

pub fn workspace() -> u32 { *CURRENT_WS.lock() }

/// Pixel formats advertised on may_shm. Both are 32 bits per pixel in the
/// framebuffer's own order, so no pixel is converted on the way to the screen
pub(in crate::mayland::server) const FORMATS: &[u32] = &[
    protocol::shm::FORMAT_XRGB8888,
    protocol::shm::FORMAT_ARGB8888,
];

/// One step of a region: a rectangle added to it or taken out of it
///
/// A region is its steps in the order they were said, and a point is in it as
/// the last step whose rectangle holds the point says: in after an add, out
/// after a subtract, out when none holds it. This gives the same set as
/// Wayland's union and difference, without building it
#[derive(Clone, Copy, PartialEq, Debug)]
pub(in crate::mayland) struct RegionStep {
    pub(in crate::mayland) add: bool,
    pub(in crate::mayland) x: i32,
    pub(in crate::mayland) y: i32,
    pub(in crate::mayland) w: i32,
    pub(in crate::mayland) h: i32,
}

/// How many steps one region may have, and how many regions a client may hold.
/// A region is a mask (a panel's few rectangles), and both are small because
/// the pointer tests every step of the surface under it
pub(in crate::mayland) const MAX_REGION_STEPS: usize = 32;
pub(in crate::mayland) const MAX_REGIONS: usize = 64;

/// Whether `(x, y)`, in surface coordinates, is in the region `steps`
pub(in crate::mayland) fn region_holds(steps: &[RegionStep], x: i32, y: i32) -> bool {
    steps.iter().fold(false, |inside, r| {
        if x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h { r.add } else { inside }
    })
}

#[derive(Clone, Copy, Default)]
pub(in crate::mayland::server) struct SurfaceState {
    pub(in crate::mayland::server) buffer: Option<u32>,
    pub(in crate::mayland::server) scale: i32,
    /// A GPU fence these pixels are waiting on, or zero.
    ///
    /// Only a client that drew with `gpu_submit` sets one, so it need not stall
    /// on its own work before committing: the compositor waits instead, with a
    /// budget, knowing how much of the frame is left
    pub(in crate::mayland::server) fence: u64,
}

/// What a surface has been told it is. A surface with no role is pixels nobody
/// has claimed, and is not shown: scene::presentable() filters on this
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Role {
    None,
    /// get_xdg_surface has been called: it is a window, but not yet what kind
    Xdg,
    /// get_toplevel: a window in its own right, and something that can be
    /// focused
    Toplevel,
    /// Handed to may_pointer.set_cursor: the surface is the pointer's image,
    /// not a window. A role rather than a flag, which keeps it out of `stack()`
    /// and so out of composition and hit testing
    Cursor,
    /// A menu, a tooltip, a dropdown: on screen and clickable, positioned
    /// against another surface rather than by the layout.
    ///
    /// A role rather than a flag on a toplevel, as for `Layer`: it is never
    /// tiled, never in the focus cycle, and drawn directly above the surface it
    /// hangs off
    Popup,
    /// A panel, the wallpaper, a lock screen: on screen and clickable, but not
    /// a window.
    ///
    /// A role rather than more flags on a toplevel, because it decides two
    /// things a toplevel has no answer for: which band it is drawn in, and how
    /// much of an edge it keeps off the layout
    Layer,
}

/// What a layer surface has asked for
///
/// `exclusive` is the only one the layout reads: negative means reserve nothing
/// (a lock screen or a notification), on screen and in front, with the tiling
/// unaffected
#[derive(Clone, Copy)]
pub(in crate::mayland) struct LayerState {
    pub(in crate::mayland::server) band: u32,
    pub(in crate::mayland::server) anchor: u32,
    pub(in crate::mayland::server) want_w: i32,
    pub(in crate::mayland::server) want_h: i32,
    pub(in crate::mayland::server) exclusive: i32,
    pub(in crate::mayland::server) keyboard: u32,
    /// What it says it is (`layer_surface.set_namespace`), for the layer
    /// rules: empty until it says
    pub(in crate::mayland) namespace: libmay::conf::Text,
}

pub struct Surface {
    pub(in crate::mayland) client: u32,
    pub(in crate::mayland) id: u32,
    pub(in crate::mayland) role: Role,
    /// Object ids of the role objects, 0 when there is none
    pub(in crate::mayland::server) xdg_id: u32,
    pub(in crate::mayland::server) toplevel_id: u32,
    pub(in crate::mayland::server) title: alloc::string::String,
    /// What the program calls itself, as opposed to what it is showing
    ///
    /// A window rule matches on this: a title changes with the open document
    /// and an app id does not
    pub(in crate::mayland::server) app_id: alloc::string::String,
    /// Where this surface sits in the stack: higher is nearer the viewer. Only
    /// the order matters, so raising a window is one assignment
    pub(in crate::mayland) z: u64,
    /// This surface is the backdrop and stays at the bottom
    ///
    /// Set by `lower_window` and `set_backdrop`; while set, nothing raises it.
    /// Focus still reaches it (the console has to be typeable), but focus is
    /// not stacking. Without this a click on the desktop would raise the
    /// console over every window
    pub(in crate::mayland) backdrop: bool,
    /// The surface this one hangs off, if any
    ///
    /// Not part of `Role::Popup`: a subsurface, a tooltip and a drag icon are
    /// the same relationship with a different role. Everything that has to know
    /// a surface follows another (the stacking pass, moving a window, tearing
    /// one down) asks this
    pub(in crate::mayland) parent: Option<(u32, u32)>,
    /// Where this surface sits relative to its parent's origin
    ///
    /// `pos` stays the position on the screen: composition and the hit test
    /// need not know they are looking at a child. This keeps the two in step
    /// when the parent moves
    pub(in crate::mayland) rel: (i32, i32),
    /// For `Role::Popup`: the object id of the popup, 0 when there is none
    pub(in crate::mayland::server) popup_id: u32,
    /// For `Role::Layer`: which band, which edges, how much of them, and
    /// whether it wants the keyboard. Behind one `Option` rather than four
    /// fields no other role reads
    pub(in crate::mayland) layer: Option<LayerState>,
    /// This surface is never given the keyboard
    ///
    /// The wallpaper is a window only so composition has something to reveal
    /// where a window was. Without this it is picked up as the topmost window
    /// left when the last real one closes, and every key after that goes into
    /// it, which looks like a hang. Not the same as `backdrop`, which the
    /// console also is and which has to stay typeable
    pub(in crate::mayland::server) no_focus: bool,
    /// Which workspace this surface belongs to, and whether it is on all of
    /// them
    ///
    /// Sticky is for the console, which drives the machine, and the wallpaper,
    /// which is the screen. Everything else follows the switch
    pub(in crate::mayland) workspace: u32,
    pub(in crate::mayland) sticky: bool,
    /// The size last proposed to this window, repeated whenever a configure is
    /// sent for another reason, most often becoming active, so a focus change
    /// does not resize it
    pub(in crate::mayland) last_w: i32,
    pub(in crate::mayland) last_h: i32,
    /// The last configure sent, and the last the client acknowledged. A
    /// toplevel that commits a buffer having acknowledged nothing is showing
    /// pixels drawn for a size never agreed on
    pub(in crate::mayland::server) configure_sent: u32,
    pub(in crate::mayland::server) configure_acked: u32,
    pub(in crate::mayland::server) pending: SurfaceState,
    pub(in crate::mayland::server) current: SurfaceState,
    /// Where the pointer reaches it, from its last commit: the whole surface
    /// when `None`, else the region it set. And what the next commit makes
    /// it, when the client has set one since (`Some(None)` is the whole
    /// surface again)
    pub(in crate::mayland::server) input: Option<Vec<RegionStep>>,
    pub(in crate::mayland::server) pending_input: Option<Option<Vec<RegionStep>>>,
    /// Damage accumulated since the last commit, in surface coordinates
    pub(in crate::mayland::server) pending_damage: Vec<(i32, i32, i32, i32)>,
    pub(in crate::mayland::server) damage: Vec<(i32, i32, i32, i32)>,
    /// How much of `damage` the frame being composed has read
    ///
    /// Counted by `presentable_for_frame` and retired, exactly that much, by
    /// `frame_composed`. The server is not locked between the two, and on the
    /// card that is when clients commit: the frame callbacks go out at the top
    /// of the frame, so a client woken by one draws and commits while `present`
    /// runs. Clearing the whole list would lose that commit's rectangles, and
    /// the window's copy in video memory would keep the old pixels.
    ///
    /// `None` is a surface the frame did not draw: nobody will read its damage,
    /// and it is drawn whole when it is drawn again
    pub(in crate::mayland::server) damage_read: Option<usize>,
    /// Callback ids requested since the last commit, and the armed ones that
    /// fire at the next frame
    pub(in crate::mayland::server) pending_frames: Vec<u32>,
    pub(in crate::mayland::server) frames: Vec<u32>,
    /// Buffers a later commit took the place of before any frame drew them
    ///
    /// A client that committed twice between two frames would otherwise never
    /// get the first buffer back, and a client alternating two of them would
    /// stop. Released with `current` at the next `frame_done`: a frame after
    /// the commit, never during it
    pub(in crate::mayland::server) retired: Vec<u32>,
    /// Where this window is on the screen, in pixels from the top left
    ///
    /// The compositor's, not the client's: a client says how big it wants to be
    /// and draws what it is told, and where that lands is a layout decision.
    ///
    /// Where the window is. Where it is going is `goal`, and `layout::apply`
    /// alone closes the distance between them
    pub(in crate::mayland) pos: (i32, i32),
    /// Where the layout says this window belongs, and whether the size in it is
    /// the layout's answer or the client's own
    ///
    /// Separate from `pos` so there is somewhere to interpolate from and to.
    /// Composition reads `pos`, a layout reads `goal`
    pub(in crate::mayland) goal: Rect,
    pub(in crate::mayland) goal_sized: bool,
    /// Where it sits on its own workspace, kept while that workspace is not the
    /// one looked at: a switch slides its windows off the screen, which moves
    /// `pos` and `goal` with them. What a shell is told of a window not on
    /// screen (`desktop::send_window`): Hyprland's `at`, the window's place and
    /// not where an animation has it. Empty until the window first leaves the
    /// screen
    pub(in crate::mayland) home: Rect,
    /// Where and how big the shells were last told it is (`x, y, w, h` of
    /// a `may_desktop.window` event), so a layout that changes it says so
    /// (`desktop::tell_moved`). `None` until a shell is told of it at all
    pub(in crate::mayland) told: Option<(i32, i32, i32, i32)>,
    /// Never yet arranged: this window has just opened, and the first
    /// arrangement is the one an opening animation starts from
    pub(in crate::mayland) fresh: bool,
    /// The window is gone and this is its afterimage
    ///
    /// A close animation cannot read the client's buffer: the client went away
    /// and its shared memory with it. So the last frame is copied into memory
    /// the compositor owns, and for the length of the curve this is a surface
    /// with no connection behind it, drawing from its own pool, freed when the
    /// curve ends. A frame sooner is a use-after-free, a frame later is a leak
    pub(in crate::mayland) ghost: Option<Ghost>,
    /// Whether `pos` has been decided yet. A window is placed once, at the
    /// commit that first gives it a size; after that it moves only when asked
    pub(in crate::mayland) placed: bool,
    /// Out of the tiling, keeping the size it drew itself
    ///
    /// The window manager's decision, from a rule or a key. A dialog a tiling
    /// layout would cut in half is the reason; it is per-window state a layout
    /// has to read
    pub(in crate::mayland) floating: bool,
    /// The whole work area, over everything, and no decoration
    ///
    /// Remembered because leaving fullscreen puts the window back into whatever
    /// arrangement the desktop has by then
    pub(in crate::mayland) fullscreen: bool,
    /// On every workspace because somebody asked, as `sticky` is on every
    /// workspace because the compositor cannot do without it
    pub(in crate::mayland) pinned: bool,
    /// What this window asked for that the desktop's defaults do not say
    pub(in crate::mayland) fx: crate::mayland::render::Fx,
    /// The curve this surface is on, if any
    ///
    /// Beside `pos` and `goal`: a layout says where a window belongs and
    /// composition reads where it is. This adds where it started and the clock
    /// that says how far it has got
    pub(in crate::mayland) anim: crate::mayland::shell::anim::Anim,
    /// True between commit and the frame that consumes the buffer
    pub(in crate::mayland::server) unconsumed: bool,
    pub(in crate::mayland::server) commits: u64,
}

impl Surface {
    /// Whether this surface is something the screen shows at all
    ///
    /// Three roles are: a window, a layer surface, and a popup. A cursor is
    /// drawn by the pointer, and a surface with no role yet is pixels nobody
    /// has claimed
    pub(in crate::mayland) fn on_screen(&self) -> bool {
        matches!(self.role, Role::Toplevel | Role::Layer | Role::Popup)
    }

    /// Whether it has committed a buffer, so there is something to show
    pub(in crate::mayland) fn has_buffer(&self) -> bool {
        self.current.buffer.is_some()
    }

    /// Which band this surface is drawn in, low to high
    ///
    /// Windows sit between `BOTTOM` and `TOP`: a panel is in front of every
    /// window without being raised, and the wallpaper behind every window
    /// without being lowered. Doubled so windows have a band of their own
    /// between two of the protocol's, and integer order still does the sorting.
    ///
    /// A child never reaches here: `stack` places it directly above its parent,
    /// so its band is its parent's by construction, which keeps a menu from
    /// being drawn under its window
    pub(in crate::mayland) fn band(&self) -> u32 {
        match self.layer {
            Some(l) => l.band * 2,
            // A window: above `BOTTOM`, below `TOP`. A backdrop goes behind the
            // others by its own z within this band
            None => protocol::layer::BOTTOM * 2 + 1,
        }
    }

    /// Whether this surface is part of the shell below the windows: the
    /// backdrop, or a layer in BACKGROUND or BOTTOM. The compositor keeps those
    /// as one image on the GPU path, as Hyprland draws them before the windows
    /// and blurs them once (`dev/mayland-below-windows.md`)
    pub(in crate::mayland) fn below_windows(&self) -> bool {
        self.backdrop || self.band() < protocol::layer::BOTTOM * 2 + 1
    }

    /// Whether this surface belongs on the workspace being looked at
    ///
    /// The one place this is answered
    pub(in crate::mayland) fn on_workspace(&self, ws: u32) -> bool {
        self.sticky || self.pinned || self.workspace == ws
    }

    /// The `may_wm` flags this surface would be reported with
    ///
    /// `window_flags` answers the shell and `send_window` the window manager,
    /// both from here
    pub(in crate::mayland) fn wm_flags(&self, focused: bool) -> u32 {
        let mut f = 0u32;
        if focused { f |= protocol::wm::FLAG_FOCUSED; }
        if self.sticky { f |= protocol::wm::FLAG_STICKY; }
        if self.no_focus { f |= protocol::wm::FLAG_NO_FOCUS; }
        if self.backdrop { f |= protocol::wm::FLAG_BACKDROP; }
        if self.floating { f |= protocol::wm::FLAG_FLOATING; }
        if self.fullscreen { f |= protocol::wm::FLAG_FULLSCREEN; }
        if self.pinned { f |= protocol::wm::FLAG_PINNED; }
        f
    }

    /// Whether the compositor may hand this surface the keyboard
    ///
    /// A window may unless it said otherwise; a layer surface may only if it
    /// asked, so a panel does not take the keyboard from the window being typed
    /// at
    pub(in crate::mayland) fn focusable(&self) -> bool {
        if self.no_focus {
            return false;
        }
        match self.layer {
            Some(l) => l.keyboard != protocol::layer_surface::KEYBOARD_NONE,
            None => self.role == Role::Toplevel,
        }
    }
}

/// A window's last frame, in memory that outlives the window
#[derive(Clone, Copy)]
pub(in crate::mayland) struct Ghost {
    pub(in crate::mayland) handle: u32,
    pub(in crate::mayland) phys: u64,
    pub(in crate::mayland) width: i32,
    pub(in crate::mayland) height: i32,
    pub(in crate::mayland) stride: usize,
    pub(in crate::mayland) format: u32,
    /// The window it is the afterimage of, `(client, surface)`. The renderer
    /// hands that window's copy in video memory over to the ghost
    /// (`render::texture`), so a close does not begin by uploading the
    /// window it already has
    pub(in crate::mayland) from: (u32, u32),
}

pub struct BufferObj {
    pub(in crate::mayland) client: u32,
    pub(in crate::mayland) id: u32,
    pub(in crate::mayland::server) pool: u32,
    pub(in crate::mayland::server) offset: usize,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub format: u32,
    /// Set while a surface holds this buffer as current
    ///
    /// That is, until a later commit takes its place, not until the frame that
    /// drew it. See the rule at the top of `surface.rs`
    pub(in crate::mayland::server) busy: bool,
}

pub(in crate::mayland::server) struct PoolObj {
    pub(in crate::mayland) client: u32,
    pub(in crate::mayland) id: u32,
    pub(in crate::mayland::server) handle: u32,
    pub(in crate::mayland::server) size: usize,
}

pub struct Client {
    pub(in crate::mayland) id: u32,
    pub(in crate::mayland) pid: u64,
    pub(in crate::mayland::server) objects: ObjectMap,
    /// Bytes received but not yet forming a whole message
    pub(in crate::mayland::server) inbuf: Vec<u8>,
    /// Events waiting for the transport
    pub(in crate::mayland::server) out: Writer,
    pub(in crate::mayland::server) alive: bool,
    /// Why the connection died, for the shell to print
    pub(in crate::mayland::server) death: Option<&'static str>,
    /// The pool this client's keymap was written into, made when it binds a
    /// keyboard and destroyed with the connection
    pub(in crate::mayland::server) keymap: Option<u32>,
    /// The pool the console's character grid is copied into, for the one client
    /// holding `may_console`. Here rather than beside the lease so it is freed
    /// with everything else the connection owned
    pub(in crate::mayland::server) grid: Option<Grid>,
    /// The grid shape this client last asked for and has not been given yet.
    /// Recorded rather than acted on, because re-flowing the console takes a
    /// lock the request path must not take (see the `may_console` section)
    pub(in crate::mayland::server) want_grid: Option<(usize, usize)>,
    /// How far back this client has asked to scroll its terminal.
    ///
    /// Recorded as `want_grid` is: the request arrives inside the server's
    /// lock, and moving the window touches the console's
    pub(in crate::mayland::server) want_view: Option<usize>,
    /// The terminal this client bound, if it holds `may_console` at all
    pub(in crate::mayland::server) console_tty: Option<crate::io::tty::TtyId>,
    /// Shared-memory handles whose `may_shm_pool` object the client destroyed
    ///
    /// The memory outlives the object (buffers made from it keep working, as
    /// Wayland says), but with the pool object gone the disconnect sweep cannot
    /// see it. Userspace is covered by the pid sweep at process exit; in-kernel
    /// clients run as pid 0, which nothing sweeps
    pub(in crate::mayland::server) orphan_pools: Vec<u32>,
    /// The regions this client has built, by object id: their steps, in
    /// order. Beside `objects` as the positioners are
    pub(in crate::mayland::server) regions: Vec<(u32, Vec<RegionStep>)>,
    /// The focus grabs this client has made, by object id: the surfaces each
    /// will list at its next commit
    pub(in crate::mayland::server) focus_grabs: Vec<(u32, Vec<u32>)>,
    /// The positioners this client has built, by object id
    ///
    /// Beside `objects`: an object map holds what interface an id is, and a
    /// positioner is seven numbers on top of that. A client keeps one per menu
    /// it can open and reuses it, so the list is short and a scan is enough
    pub(in crate::mayland::server) positioners:
        Vec<(u32, crate::mayland::shell::popup::Positioner)>,
    /// The selection pool this client was last handed, and what is in it
    ///
    /// One at a time: a client asking again gets the old pool freed before the
    /// new one is made, so a program pasting in a loop does not use up its
    /// share of `shm`
    pub(in crate::mayland::server) offer_pool: Option<u32>,
}

/// The shared-memory copy of the console's character grid, and what was last
/// said about it
#[derive(Clone, Copy)]
pub(in crate::mayland::server) struct Grid {
    pub(in crate::mayland::server) handle: u32,
    pub(in crate::mayland::server) size: usize,
    pub(in crate::mayland::server) cols: usize,
    pub(in crate::mayland::server) rows: usize,
    /// Where the caret was in the last `damage` event. Kept because the caret
    /// moves on its own (an arrow key edits nothing) and the client has to rub
    /// it out of where it was
    pub(in crate::mayland::server) cursor: (usize, usize),
}

impl Client {
    pub(in crate::mayland::server) fn new(id: u32, pid: u64) -> Self {
        let mut objects = ObjectMap::new();
        // The display is object 1 and exists before the client says anything
        let _ = objects.insert_client(DISPLAY_ID, Interface::Display, 1);
        Client {
            id,
            pid,
            objects,
            inbuf: Vec::new(),
            out: Writer::new(),
            alive: true,
            death: None,
            keymap: None,
            grid: None,
            want_grid: None,
            want_view: None,
            console_tty: None,
            orphan_pools: Vec::new(),
            regions: Vec::new(),
            focus_grabs: Vec::new(),
            positioners: Vec::new(),
            offer_pool: None,
        }
    }

    /// Queues one event, and kills the connection if it will not fit
    ///
    /// Every event the server sends goes through here rather than
    /// `out.message`. When a client has stopped reading and its queue has
    /// reached `MAX_QUEUED`, the connection ends: dropping the event would
    /// leave a client that missed a `leave` or a `release` in a wrong state for
    /// ever. Wayland disconnects here too.
    ///
    /// Not checked at the call sites: an event is a courtesy to a client that
    /// may already be dead
    pub(in crate::mayland::server) fn queue(&mut self, object: u32, opcode: u16,
                                            args: &[Arg]) {
        if self.out.message(object, opcode, args).is_err() && self.alive {
            self.alive = false;
            self.death = Some("the client stopped reading its events");
            crate::serial_println!(
                "[mayland] client {} killed: {} bytes queued and unread",
                self.id, self.out.len()
            );
        }
    }

    /// Queue a protocol error against 'object' and kill the connection
    pub(in crate::mayland::server) fn fail(&mut self, object: u32, code: u32, message: &'static str) {
        let _ = self.out.message(DISPLAY_ID, protocol::display::ERROR, &[
            Arg::Object(object),
            Arg::Uint(code),
            Arg::Str(Some(message)),
        ]);
        self.alive = false;
        self.death = Some(message);
        crate::serial_println!(
            "[mayland] client {} killed: object {} - {}", self.id, object, message
        );
    }

    /// Tells the client an id is free again. Wayland requires this before the
    /// client may reuse the id, so a request in flight against the old object
    /// cannot be mistaken for one against the new
    pub(in crate::mayland::server) fn delete_id(&mut self, id: u32) {
        let _ = self.out.message(DISPLAY_ID, protocol::display::DELETE_ID, &[Arg::Uint(id)]);
    }
}

/// What a client asked its cursor to be. `surface == 0` means it asked for no
/// cursor at all
pub(in crate::mayland::server) struct CursorSurface {
    pub(in crate::mayland) client: u32,
    pub(in crate::mayland::server) surface: u32,
    pub(in crate::mayland::server) hot_x: i32,
    pub(in crate::mayland::server) hot_y: i32,
}

/// The cursor as the compositor needs it: where its pixels are and where in
/// them the point is
pub struct CursorImage {
    pub phys: u64,
    pub width: i32,
    pub height: i32,
    pub stride: usize,
    pub hot_x: i32,
    pub hot_y: i32,
    /// ARGB8888 or XRGB8888. A cursor is the one surface where the difference
    /// matters: everything else is composed opaquely
    pub format: u32,
    /// The pool and byte offset the pixels live at, for reading them with the
    /// CPU rather than the copy engine
    pub pool: u32,
    pub offset: usize,
}

pub struct Server {
    pub(in crate::mayland::server) clients: Vec<Client>,
    pub(in crate::mayland) surfaces: Vec<Surface>,
    pub(in crate::mayland::server) buffers: Vec<BufferObj>,
    pub(in crate::mayland::server) pools: Vec<PoolObj>,
    pub(in crate::mayland::server) next_client: u32,
    /// Surface ids for afterimages, counted from a range no client reaches
    ///
    /// A ghost is a surface of its own with client 0, and the compositor
    /// remembers what it drew last frame under `(client, surface)`. Two windows
    /// of different clients can share a surface id, so their ghosts would be
    /// one key. Counted from `1 << 31`: a client's ids go up from 1 and never
    /// get near it
    pub(in crate::mayland::server) next_ghost: u32,
    /// Handed out to raise a window to the top of the stack
    pub(in crate::mayland::server) next_z: u64,
    /// Bumped for every event that carries one. Serials order input against
    /// the requests a client makes in response to it
    pub(in crate::mayland::server) serial: u32,
    /// The last few serials that went out on an input event
    ///
    /// Two requests are only allowed as a consequence of somebody pressing
    /// something: taking the pointer for a menu, and putting something in the
    /// selection.
    ///
    /// `next_serial` is a counter, so a client could guess a live serial. This
    /// catches a confused client (answering an event already answered, or one
    /// it never received), not a lying one. The boundary against that is
    /// `access`
    pub(in crate::mayland::server) input_serials: [u32; 32],
    pub(in crate::mayland::server) input_serial_at: usize,
    /// Which surface has the keyboard, as (client, surface id): a toplevel,
    /// or a layer surface that asked for it
    pub(in crate::mayland) focus: Option<(u32, u32)>,
    /// The window that is the active one: drawn with the active border, and the
    /// one the desktop is told is focused
    ///
    /// The last toplevel the keyboard went to. A layer surface taking the
    /// keyboard (the launcher) does not change it, as in Hyprland:
    /// `rawSurfaceFocus` moves the keyboard and leaves `m_focusWindow`, so the
    /// border stays lit, the bar keeps the title, and a bind acting on the
    /// active window still means the window
    pub(in crate::mayland) active: Option<(u32, u32)>,
    /// The surface that has the keyboard and will not give it up
    ///
    /// Set when a client holding CAP_MAY_PRIV asks for KEYBOARD_EXCLUSIVE on an
    /// overlay layer surface while no session owns the seat: the greeter,
    /// before anybody has logged in.
    ///
    /// `KEYBOARD_EXCLUSIVE` alone only calls `set_focus` once and nothing holds
    /// it there, which a launcher does not mind. Every path that moves the
    /// keyboard consults this: see `focus::locked_away`, and
    /// `input::intercept`, which stops dispatching chords and stops letting
    /// Ctrl+Alt+Esc hand the keyboard back to a root shell
    pub(in crate::mayland) lock: Option<(u32, u32)>,
    /// Which surface the pointer is over, as (client, surface id). Separate
    /// from the keyboard focus, as Wayland has two: a menu highlights under the
    /// cursor while typing goes elsewhere
    pub(in crate::mayland::server) pointer_focus: Option<(u32, u32)>,
    /// The cursor image the client under the pointer asked for, if it asked.
    /// Dropped whenever the pointer crosses into another surface
    pub(in crate::mayland::server) cursor: Option<CursorSurface>,
    /// The clipboard and the primary selection, in that order
    ///
    /// An array indexed by `selection::Which` so every path is written once.
    /// The middle-button selection is the same machinery with a different index
    pub(in crate::mayland::server) selections:
        [Option<super::selection::Selection>; 2],
    /// Screen geometry the output global reports
    pub width: i32,
    pub height: i32,
    pub refresh_mhz: i32,
    /// Every connector as the display layer last said, for `may_output`'s
    /// name and `may_output_manager` (`iface::outputs`)
    pub(in crate::mayland::server) outputs: super::iface::outputs::Outputs,
    /// Output configurations being built or waiting for their answer
    pub(in crate::mayland::server) output_configs: Vec<super::iface::outputs::Pending>,
}

impl Server {
    pub(in crate::mayland::server) const fn new() -> Self {
        Server {
            clients: Vec::new(),
            surfaces: Vec::new(),
            buffers: Vec::new(),
            pools: Vec::new(),
            next_client: 1,
            next_ghost: 1 << 31,
            // Not 1: lowering a window counts downwards from wherever the
            // surfaces below it started, and it has to have somewhere to go
            next_z: 1 << 32,
            serial: 1,
            input_serials: [0; 32],
            input_serial_at: 0,
            focus: None,
            active: None,
            lock: None,
            pointer_focus: None,
            selections: [None, None],
            cursor: None,
            width: 0,
            height: 0,
            refresh_mhz: 60_000,
            outputs: super::iface::outputs::Outputs::empty(),
            output_configs: Vec::new(),
        }
    }

    pub(in crate::mayland) fn next_serial(&mut self) -> u32 {
        let s = self.serial;
        self.serial = self.serial.wrapping_add(1);
        s
    }

    /// A serial that went out on a key or a button
    ///
    /// Taken from `next_serial` and remembered here: a client quotes back the
    /// number it was given, so there is one numbering
    pub(in crate::mayland) fn input_serial(&mut self) -> u32 {
        let s = self.next_serial();
        let at = self.input_serial_at;
        self.input_serials[at] = s;
        self.input_serial_at = (at + 1) % self.input_serials.len();
        s
    }

    /// Whether this serial is one recently handed out with an input event
    ///
    /// Zero is never one: it fills the ring before anything has been pressed,
    /// and a client sending `0` would otherwise match
    pub(in crate::mayland) fn is_input_serial(&self, serial: u32) -> bool {
        serial != 0 && self.input_serials.contains(&serial)
    }

    pub(in crate::mayland) fn client_idx(&self, id: u32) -> Option<usize> {
        self.clients.iter().position(|c| c.id == id)
    }

    /// Whether this connection is still worth sending anything to
    ///
    /// A client killed by a protocol error is dead here and gone from the table
    /// only later, when the transport notices. Asking whether somebody holds
    /// something means this
    pub(in crate::mayland::server) fn alive(&self, id: u32) -> bool {
        self.client_idx(id).map(|i| self.clients[i].alive).unwrap_or(false)
    }

    pub(in crate::mayland) fn surface_idx(&self, key: (u32, u32)) -> Option<usize> {
        self.surfaces.iter().position(|x| x.client == key.0 && x.id == key.1)
    }

    /// Where a surface's current pixels are, for something that has to copy
    /// them
    ///
    /// Asked by the shell when a window is closing and its last frame has to be
    /// kept. Flattened rather than a `&BufferObj`, since buffer ownership is
    /// the server's
    pub(in crate::mayland) fn window_pixels(&self, si: usize)
        -> Option<(i32, i32, usize, u32, u32, usize)>
    {
        let surf = &self.surfaces[si];
        let bid = surf.current.buffer?;
        let b = self.buffers.iter().find(|b| b.client == surf.client && b.id == bid)?;
        Some((b.width, b.height, b.stride, b.format, b.pool, b.offset))
    }

    /// Pushes a window's afterimage as a surface of its own
    ///
    /// The shell decides whether there is one and what curve it rides; this
    /// makes it, because a `Surface` is the server's to build. A surface rather
    /// than a flag on the dying one because the client is often still there and
    /// owns its own (see `shell::ghost`).
    ///
    /// What composition reads is copied across: where it is, how big, its
    /// workspace, its decoration. What a client owns (buffers, callbacks, role
    /// objects) stays with the surface that has a client
    pub(in crate::mayland) fn push_ghost(&mut self, si: usize, ghost: Ghost,
                                         anim: crate::mayland::shell::anim::Anim) {
        let id = self.next_ghost;
        self.next_ghost = self.next_ghost.wrapping_add(1).max(1 << 31);
        let src = &self.surfaces[si];
        let afterimage = Surface {
            client: 0,
            id,
            // A toplevel, because that is what composition draws; never focused
            // or tiled, because nobody is behind it to type at, and the windows
            // left should close over its space at once
            role: Role::Toplevel,
            xdg_id: 0,
            toplevel_id: 0,
            title: src.title.clone(),
            app_id: alloc::string::String::new(),
            z: src.z,
            backdrop: src.backdrop,
            // An afterimage hangs off nothing: its parent may be what closed
            parent: None,
            rel: (0, 0),
            popup_id: 0,
            layer: None,
            no_focus: true,
            workspace: src.workspace,
            sticky: src.sticky,
            last_w: src.last_w,
            last_h: src.last_h,
            configure_sent: 0,
            configure_acked: 0,
            pending: SurfaceState { buffer: None, scale: 1, fence: 0 },
            current: SurfaceState { buffer: None, scale: 1, fence: 0 },
            input: None,
            pending_input: None,
            pending_damage: Vec::new(),
            // What the window committed and no frame has drawn yet. The ghost's
            // pixels include it, and the copy it inherits does not
            damage: src.damage.clone(),
            // How much of it the frame now composing has read under the
            // window's own name: that much has gone into the copy the ghost
            // inherits
            damage_read: src.damage_read,
            pending_frames: Vec::new(),
            frames: Vec::new(),
            retired: Vec::new(),
            pos: src.pos,
            goal: src.goal,
            home: src.home,
            // An afterimage is no window, and no shell is told of it
            told: None,
            goal_sized: src.goal_sized,
            fresh: false,
            ghost: Some(ghost),
            placed: true,
            floating: true,
            fullscreen: false,
            pinned: src.pinned,
            fx: src.fx,
            anim,
            unconsumed: false,
            commits: 0,
        };
        self.surfaces.push(afterimage);
    }

    /// Toplevels bottom to top
    pub(in crate::mayland) fn stack(&self) -> Vec<usize> {
        // The workspace filter belongs here only. Composition, the pointer's
        // hit test and the focus cycle all go through this, so a window on
        // another workspace is not drawn, clickable or reachable by Tab, from
        // one condition
        let ws = *CURRENT_WS.lock();
        let visible: Vec<usize> = (0..self.surfaces.len())
            .filter(|&i| self.surfaces[i].on_screen()
                         && (self.surfaces[i].on_workspace(ws)
                             // And whatever is still leaving: a workspace slide
                             // is windows going out one side while others come
                             // in the other
                             || self.surfaces[i].anim.running))
            .collect();

        // Only the surfaces standing on their own are sorted. Band first, then
        // z within the band: a layer surface is in front of or behind every
        // window by what it is, not by when it was raised
        let mut roots: Vec<usize> = visible.iter().copied()
            .filter(|&i| self.surfaces[i].parent.is_none())
            .collect();
        roots.sort_by_key(|&i| (self.surfaces[i].band(), self.surfaces[i].z));

        // Then every child goes directly above the surface it hangs off,
        // wherever that landed. `z = parent.z + 1` would hold only until the
        // next `raise`
        let mut order = Vec::with_capacity(visible.len());
        for r in roots {
            self.push_subtree(r, &visible, &mut order, 0);
        }
        order
    }

    /// One surface and everything hanging off it, parent first
    ///
    /// Depth-bounded: a surface made its own ancestor would otherwise keep this
    /// function and the server lock going for ever. `get_popup` refuses a cycle
    /// before it is built; this keeps the drawing side safe regardless
    fn push_subtree(&self, root: usize, visible: &[usize], out: &mut Vec<usize>,
                    depth: usize) {
        out.push(root);
        if depth >= crate::mayland::input::grab::MAX_POPUP_DEPTH {
            return;
        }
        let key = (self.surfaces[root].client, self.surfaces[root].id);
        let mut kids: Vec<usize> = visible.iter().copied()
            .filter(|&i| self.surfaces[i].parent == Some(key))
            .collect();
        kids.sort_by_key(|&i| self.surfaces[i].z);
        for k in kids {
            self.push_subtree(k, visible, out, depth + 1);
        }
    }
}

pub(in crate::mayland::server) static SERVER: super::held::Held<Server> =
    super::held::Held::new(Server::new());

/// The one server, for the parts of the compositor in another file
///
/// `layout` is the only caller: the arrangement of windows is the server's
/// state, and the alternative is handing it a copy of everything it arranges
#[track_caller]
pub(in crate::mayland) fn lock() -> super::held::HeldGuard<'static, Server> {
    SERVER.lock()
}
