// Which monitors this card has, and what each is doing
//
// One table, one entry per output the board reports, holding what was found out
// about it (what it physically is, whether something is plugged in, what that
// can do) and what was done with it (which head and SOR it was given, what is
// being scanned out, whether it owns the cursor plane).
//
// Two rules
//
// The lock is never held across an RM call. Asking GSP-RM about a monitor is a
// round trip through the message queue, and the code wanting the table makes
// those calls. So an `Output` is built as a local, then handed over; a change
// is a short mutation of computed fields. `with_all` is for printing.
//
// It is not read from interrupt context. `blackwell/interrupts.rs` reads its
// head from an atomic (`VBLANK_HEAD`) written once at bring-up: a spin lock a
// thread holds and an interrupt wants stops the machine.
//
// `blackwell/window.rs` re-exports the types below, for
// `blackwell::boot::live_scanout` and friends in `scanout.rs`, `mayland` and
// the shell.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use super::disp::{ConnectorData, DpcdCaps, OrInfo};
use super::modeset::Mode;

/// The scanout surface on a monitor now.
///
/// Set when the window arms and the hardware confirms it, cleared when the
/// display is torn down. Everything putting pixels on the panel goes through
/// here
#[derive(Clone, Copy, Debug)]
pub struct LiveScanout {
    /// Physical video-memory address of the surface
    pub vram: u64,
    /// Bytes per scanline, a multiple of `PITCH_GRANULARITY`
    pub pitch: u32,
    pub width: u32,
    pub height: u32,
    pub head: u32,
    pub window: u32,
}

/// The cursor plane armed on a head now, if any. Position is hardware state on
/// the PIO channel, not read back here
#[derive(Clone, Copy, Debug)]
pub struct LiveCursor {
    /// Physical video-memory address of the cursor image
    pub vram: u64,
    /// Width and height in pixels: the plane is square
    pub size: u32,
    /// The head whose plane this is, which is also the PIO channel instance
    pub head: u32,
}

/// One output resource on the board, and whatever is plugged into it.
///
/// `display_id` is a single bit, not an index: how RM names a connector across
/// the `NV0073` family, the set arriving as one mask from `GET_SUPPORTED`.
/// Control calls use the field as-is
pub struct Output {
    /// The RM display id: exactly one bit set
    pub display_id: u32,
    /// What the connector physically is: SOR/DAC/PIOR and its protocol
    pub or: OrInfo,
    /// Which socket on the bracket it is, and of what kind. `None` when RM
    /// would not say; the display layer then leaves it unnamed
    /// (`nvidia/kms.rs`)
    pub connector: Option<ConnectorData>,
    /// What the sink says over AUX. `None` on anything not DisplayPort, and on
    /// a DP link whose DPCD read failed
    pub dpcd: Option<DpcdCaps>,
    /// The raw EDID, kept with its decode: it cannot be read again once the
    /// monitor is gone
    pub edid: Vec<u8>,
    /// The monitor's own name, from the EDID's 0xFC descriptor
    pub name: String,
    /// Timings this monitor offers, best first
    pub modes: Vec<Mode>,
    /// What it says it can do over HDMI, from its CEA extension. All zero on a
    /// DisplayPort panel and on a monitor with no extension, so the driver asks
    /// the board as well before turning HDMI on
    pub hdmi: crate::drivers::gpu::edid::HdmiCaps,
    /// The refresh range the EDID gives, lowest and highest in Hz: what
    /// variable refresh may move between (`modeset::vrr::edid_range`)
    pub vrr_range: Option<(u32, u32)>,
    /// Whether `GET_CONNECT_STATE` last said something was plugged in
    pub connected: bool,
    /// The head driving it, once it has been lit
    pub head: Option<u32>,
    /// The serial output resource RM assigned, once it has been lit
    pub sor: Option<u32>,
    /// The head's output LUTs, in video memory: three, so a new curve goes into
    /// one the head is not reading (`modeset::OlutRing`).
    ///
    /// A mode change re-pushes the head's whole method block, and
    /// `head_set_mode` takes the LUT address with it: `None` disables the LUT,
    /// and a head so left composites in a colour space nothing downstream
    /// expects, a black screen (the FP16 note in `dev/gb206-display.md`)
    pub olut: Option<super::modeset::OlutRing>,
    /// What is being scanned out on it
    pub scanout: Option<LiveScanout>,
    /// Its head's cursor plane, when that head has one armed
    pub cursor: Option<LiveCursor>,
    // What the head was asked to be (mode, colour, sleep, variable refresh) is
    // in the display layer (`drivers::gpu::kms::heads`). This table keeps what
    // is hardware
}

impl Output {
    /// A freshly enumerated output, before anything has been driven
    pub fn new(display_id: u32, or: OrInfo) -> Self {
        Self {
            display_id,
            or,
            connector: None,
            dpcd: None,
            edid: Vec::new(),
            name: String::new(),
            modes: Vec::new(),
            hdmi: Default::default(),
            vrr_range: None,
            connected: false,
            head: None,
            sor: None,
            olut: None,
            scanout: None,
            cursor: None,
        }
    }

    /// True when this output is putting a picture on a monitor.
    ///
    /// A sleeping output has a surface and a window and scans them, but no head
    /// owns its SOR. Asleep is head state, so the display layer is asked
    pub fn lit(&self) -> bool {
        self.scanout.is_some()
            && !crate::drivers::gpu::kms::heads::get(self.display_id).is_some_and(|r| r.asleep)
    }

    /// The timing its head rasters, from the display layer's record
    pub fn mode(&self) -> Option<Mode> {
        crate::drivers::gpu::kms::heads::get(self.display_id).map(|r| Mode::from_timing(&r.mode))
    }

    /// How good a monitor this is, for picking the one to put the console on:
    /// most pixels, then fastest, the ranking `drivers::gpu::edid` uses between
    /// two timings
    pub fn rank(&self) -> (u64, u32) {
        match self.modes.first() {
            Some(m) => (m.h_display as u64 * m.v_display as u64, m.refresh_centihz()),
            None => (0, 0),
        }
    }
}

/// Every output the board reports, connected or not. A `Vec`: `GET_SUPPORTED`
/// returns a mask whose set bits are not contiguous and whose count is the
/// board's
static OUTPUTS: Mutex<Vec<Output>> = Mutex::new(Vec::new());

/// The display id of the output the console and the compositor live on.
///
/// Zero means none, safe since a display id always has one bit set. Beside the
/// table, not a flag inside it, so "which one is primary" has one answer
static PRIMARY: Mutex<u32> = Mutex::new(0);

/// Forget every output, for a display teardown or a reset
pub fn reset() {
    OUTPUTS.lock().clear();
    *PRIMARY.lock() = 0;
    // The heads' records go with the table they describe
    crate::drivers::gpu::kms::heads::clear();
}

/// Add an output just enumerated, replacing any earlier entry with the same
/// display id: a monitor re-plugged is the same connector with a different sink
pub fn add(out: Output) {
    let mut t = OUTPUTS.lock();
    match t.iter().position(|o| o.display_id == out.display_id) {
        Some(i) => t[i] = out,
        None => t.push(out),
    }
}

/// How many outputs the board reported
pub fn count() -> usize {
    OUTPUTS.lock().len()
}

/// Look at the whole table, for printing and deciding, never for anything
/// making an RM call; see the note at the top
pub fn with_all<R>(f: impl FnOnce(&[Output]) -> R) -> R {
    f(&OUTPUTS.lock())
}

/// Change one output in place. The closure must not make an RM call
pub fn with_mut<R>(display_id: u32, f: impl FnOnce(&mut Output) -> R) -> Option<R> {
    let mut t = OUTPUTS.lock();
    t.iter_mut().find(|o| o.display_id == display_id).map(f)
}

/// Read one field out of one output
pub fn with<R>(display_id: u32, f: impl FnOnce(&Output) -> R) -> Option<R> {
    let t = OUTPUTS.lock();
    t.iter().find(|o| o.display_id == display_id).map(f)
}

/// Which output the console and the compositor are on, if any
pub fn primary_id() -> Option<u32> {
    let id = *PRIMARY.lock();
    (id != 0).then_some(id)
}

/// Name the primary output. Called once the modeset lighting it has committed,
/// not when it was chosen: a monitor picked and then refused is not where the
/// console goes
pub fn set_primary(display_id: u32) {
    *PRIMARY.lock() = display_id;
}

/// Read something off the primary output
pub fn with_primary<R>(f: impl FnOnce(&Output) -> R) -> Option<R> {
    let id = primary_id()?;
    with(id, f)
}

/// Change something on the primary output
pub fn with_primary_mut<R>(f: impl FnOnce(&mut Output) -> R) -> Option<R> {
    let id = primary_id()?;
    with_mut(id, f)
}

/// Every head with a picture on it, with the width it scans.
///
/// A modeset for a second monitor needs this before touching the core channel:
/// one channel serves every head and an UPDATE arms all at once. See
/// `disp::core_claim_tiles`
pub fn lit_heads() -> Vec<(u32, u32)> {
    let t = OUTPUTS.lock();
    t.iter()
        .filter_map(|o| Some((o.head?, o.scanout?.width)))
        .collect()
}

/// A head nobody has taken, from the mask the board reports.
///
/// `head_mask` is the board's, so a head absent from it does not exist. `None`
/// means every head is spoken for: four monitors on a four-head board
pub fn free_head(head_mask: u32) -> Option<u32> {
    let t = OUTPUTS.lock();
    (0..32u32)
        .filter(|h| head_mask & (1 << h) != 0)
        .find(|h| !t.iter().any(|o| o.head == Some(*h)))
}

// Cables arriving and leaving

/// `NV2080_NOTIFIERS_HOTPLUG` and `NV2080_NOTIFIERS_DP_IRQ`, the two RM
/// notifiers a display driver subscribes to.
///
/// Hotplug is a cable; DP_IRQ is the sink raising `IRQ_VECTOR` in its DPCD,
/// which on a plain SST link wants the link retrained
pub const NOTIFIER_HOTPLUG: u32 = 1;
pub const NOTIFIER_DP_IRQ: u32 = 7;

/// What arrived since the last look. Two masks: a monitor swapped on one
/// connector is an unplug and a plug in one notification.
///
/// A queue, not a call. The notification is decoded inside the RPC receive
/// path, and what is done about it (a connect state, an EDID, a modeset) is
/// another RM call, which would re-enter the transport mid-drain. The same rule
/// as `mayland::input::drain` running on the compositor thread
static HOTPLUG: Mutex<(u32, u32)> = Mutex::new((0, 0));

/// Record a hotplug notification. Safe from inside the RPC drain: one short
/// lock and no call
pub fn note_hotplug(plugged: u32, unplugged: u32) {
    let mut h = HOTPLUG.lock();
    h.0 |= plugged;
    h.1 |= unplugged;
}

/// Take what has arrived, leaving the queue empty. `(plugged, unplugged)`, both
/// zero when nothing happened
pub fn take_hotplug() -> (u32, u32) {
    let mut h = HOTPLUG.lock();
    core::mem::replace(&mut *h, (0, 0))
}

/// Sinks that raised their DP interrupt (a "short pulse") since the last look.
/// Queued like hot-plug: what is done about it is AUX reads and a retrain, RM
/// calls
static DP_IRQ: Mutex<u32> = Mutex::new(0);

pub fn note_dp_irq(display_id: u32) {
    *DP_IRQ.lock() |= display_id;
}

pub fn take_dp_irq() -> u32 {
    core::mem::replace(&mut *DP_IRQ.lock(), 0)
}

// The primary's live state

/// What is on the monitor the console is on, if anything is
pub fn live_scanout() -> Option<LiveScanout> {
    with_primary(|o| o.scanout).flatten()
}

/// The armed cursor plane on that same monitor, or None when the compositor
/// has to draw its own
pub fn live_cursor() -> Option<LiveCursor> {
    with_primary(|o| o.cursor).flatten()
}

/// The head fetches a different surface now.
///
/// A page flip changes what is on the monitor and nothing else: same head,
/// window, geometry. Without this `live_scanout` names the surface the
/// compositor started with, a buffer nothing scans and, after `disable_flip`,
/// somebody else's
pub fn set_live_surface(vram: u64) {
    with_primary_mut(|o| {
        if let Some(l) = o.scanout.as_mut() {
            l.vram = vram;
        }
    });
}

/// Nothing of ours is on the primary monitor, for a display teardown or a
/// reset. Drawing into the old address would write video memory at random
pub fn clear_live_scanout() {
    with_primary_mut(|o| o.scanout = None);
}

/// Forget the cursor plane, paired with clearing the scanout
pub fn clear_live_cursor() {
    with_primary_mut(|o| o.cursor = None);
}
