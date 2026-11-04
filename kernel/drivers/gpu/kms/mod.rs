// The display layer: what a monitor, a head and a change to them are to the
// rest of the system, whoever drives the screen
//
// `dev/kms-architecture.md` is the design. The model is DRM's (connectors and
// heads as objects, a request checked as a whole and applied as one), the
// specifics are NVKMS's (the request's shape, `nvSetDispModeEvo`'s commit
// order). The seam is a trait, as `scanout.rs` is for the frame path, since
// there is one compositor.
//
// A request is checked in two halves: the layer's rules (`check::plan`, no
// hardware) and the driver's (link, IMP, heads), every head at once. It is
// committed in `nvSetDispModeEvo`'s order (`commit`): frames held, variable
// refresh down, the driver's one update, variable refresh up. What a head is
// doing lives here (`heads`), written by the driver when its hardware took a
// change; `state()` lays it over what the driver reports.
//
// The lock
//
// One, sleeping, held across check and commit: two requests do not interleave,
// and a GSP-RM reload (`exclusive`) does not run under one. Order: KMS -> RM ->
// DISP_MEM. The frame path takes none of them.

pub mod check;
pub mod commit;
pub mod events;
pub mod heads;
pub mod names;
pub mod request;

use alloc::string::String;
use alloc::vec::Vec;

use crate::kcore::sched_lock::SchedMutex;

pub use super::edid::Timing;
/// The colour pipeline's state: ramp, range, dither, depth, balance. Lives with
/// the NVIDIA modeset; nothing in it is NVIDIA's (`dev/colour-pipeline.md`)
pub use super::nvidia::gsp_common::modeset::{Curve, Depth, Dither, HeadColour, Range};
pub use check::Change;
pub use request::{HeadRequest, Request};

/// What kind of socket an output is, named as DRM names connectors, so a Linux
/// `monitor =` line means the same output
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Connector {
    DisplayPort,
    HdmiA,
    DviD,
    DviI,
    Vga,
    Lvds,
    Dsi,
    /// A virtual machine's display (bochs-drm's connector type)
    Virtual,
    /// What simpledrm calls the firmware's framebuffer, and what DRM calls a
    /// connector it has no type for
    Unknown,
}

impl Connector {
    /// `drm_connector_enum_list` (`drm_connector.c`)
    pub fn drm_name(self) -> &'static str {
        match self {
            Connector::DisplayPort => "DP",
            Connector::HdmiA => "HDMI-A",
            Connector::DviD => "DVI-D",
            Connector::DviI => "DVI-I",
            Connector::Vga => "VGA",
            Connector::Lvds => "LVDS",
            Connector::Dsi => "DSI",
            Connector::Virtual => "Virtual",
            Connector::Unknown => "Unknown",
        }
    }
}

/// What is plugged into an output, as its EDID describes it
pub struct Monitor {
    pub name: String,
    /// The manufacturer, the EDID's three-letter PNP id (`VIE`)
    pub make: String,
    /// Best first, the order every driver here already keeps
    pub modes: Vec<Timing>,
    /// The monitor's own preferred timing, where it is in `modes`: what
    /// Hyprland's `preferred` asks for, which need not be the best
    pub preferred: Option<usize>,
    /// The vertical refresh range, in Hz
    pub refresh_range: Option<(u32, u32)>,
    /// Bits per component it accepts; 0 when its EDID does not say
    pub max_bpc: u8,
}

/// Variable refresh on a head
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vrr {
    /// Not on this monitor, in this mode, or on this display at all, and why
    Unsupported(&'static str),
    Off,
    /// A frame per flip, repeated after `timeout_us` if none comes
    On { timeout_us: u32 },
}

/// A head driving an output
#[derive(Clone, Copy)]
pub struct HeadState {
    pub head: u32,
    /// The raster's active area
    pub size: (u32, u32),
    pub refresh_centihz: u32,
    /// Where the running timing is in the monitor's list; `None` when it is
    /// not one of them (a firmware mode)
    pub mode: Option<usize>,
    /// No picture reaches the monitor, which has been told to sleep
    pub asleep: bool,
    /// `None` on a display with no colour pipeline to set
    pub colour: Option<HeadColour>,
    pub vrr: Vrr,
}

/// A connector, what is on it and what drives it
pub struct Output {
    /// `DP-3`, `HDMI-A-1`, `Virtual-1`: Linux's name for the same socket
    pub name: String,
    pub connector: Connector,
    /// The driver's own handle for it, opaque here: a display id on NVIDIA
    pub key: u32,
    pub connected: bool,
    /// The output the compositor and the console are on
    pub primary: bool,
    pub monitor: Option<Monitor>,
    pub head: Option<HeadState>,
}

/// Every output, as the driver sees it now
pub struct State {
    pub outputs: Vec<Output>,
}

impl State {
    /// An output by name, as typed: `dp-3` is `DP-3`
    pub fn output(&self, name: &str) -> Option<&Output> {
        self.outputs.iter().find(|o| o.name.eq_ignore_ascii_case(name))
    }

    /// An output by the driver's key
    pub fn by_key(&self, key: u32) -> Option<&Output> {
        self.outputs.iter().find(|o| o.key == key)
    }

    /// The first output a head drives, what a command means when it names none
    pub fn first_lit(&self) -> Option<&Output> {
        self.outputs.iter().find(|o| o.head.is_some())
    }
}

/// What a display driver implements. The layer orders the steps
/// (`commit`); the driver does them and records each head as its hardware
/// takes the change (`heads::set`, `heads::update`)
pub trait Kms: Sync {
    /// For `display` and the log: which of these is running the screen
    fn name(&self) -> &'static str;

    /// Outputs, monitors, and which head drives each, as the driver sees them.
    /// The heads' mode, colour, sleep and variable refresh come from the
    /// layer's records (`heads`), laid over this by `kms::state`
    fn state(&self) -> State;

    /// The driver's half of a check, for every change at once. Changes nothing
    /// and may be asked any number of times
    fn check(&self, changes: &[Change]) -> Result<(), String>;

    /// Variable refresh on or off on one head, outside the update
    /// (`nvDisableVrr`, `nvEnableVrr`). The timeout in microseconds when on
    fn set_vrr(&self, c: &Change, on: bool) -> Result<u32, String>;

    /// Every change `check` passed: new heads lit first, then one update for
    /// the rest. A line for each thing that happened
    fn apply(&self, changes: &[Change]) -> Result<Vec<String>, String>;
}

/// Held across a check and its commit, and across a GSP-RM reload
static LOCK: SchedMutex<()> = SchedMutex::named("kms", ());

/// Whoever drives the screen on this machine.
///
/// Asked every time: a card whose GSP-RM was unloaded and loaded again answers
/// with a different table behind it. A card answers even with its display down:
/// the firmware's framebuffer is off the screen once the card has been driven
pub fn current() -> &'static dyn Kms {
    if crate::drivers::gpu::nvidia::device().is_some() {
        return &crate::drivers::gpu::nvidia::kms::NVIDIA;
    }
    super::display::kms()
}

/// Every output, as the driver sees it now, with each head as it was last
/// made to be
pub fn state() -> State {
    with_records(current().state())
}

/// The layer's records over the driver's answer. A head with no record keeps
/// what the driver said; bochs and the firmware framebuffer have none
fn with_records(mut st: State) -> State {
    for o in st.outputs.iter_mut() {
        let (Some(h), Some(r)) = (o.head.as_mut(), heads::get(o.key)) else {
            continue;
        };
        h.refresh_centihz = r.mode.refresh_centihz();
        h.mode = o.monitor.as_ref().and_then(|m| m.modes.iter().position(|t| *t == r.mode));
        h.asleep = r.asleep;
        h.colour = r.colour;
        if let Some(timeout_us) = r.vrr {
            h.vrr = Vrr::On { timeout_us };
        }
    }
    st
}

/// Check a request, and apply it unless it only asked to be checked
pub fn apply(req: &Request) -> Result<Vec<String>, String> {
    let (result, committed) = {
        let _held = LOCK.lock();
        let k = current();
        let changes = check::plan(req, &with_records(k.state()), crate::mayland::render::running())?;
        k.check(&changes)?;
        if req.test_only {
            if changes.is_empty() {
                return Ok(alloc::vec![String::from("nothing would change")]);
            }
            return Ok(changes
                .iter()
                .map(|c| alloc::format!("{}: it would be taken - nothing was changed", c.output))
                .collect());
        }
        // Said whether it worked or not: a failure halfway has changed
        // something too, and the listener reads the state
        (commit::run(k, &changes), !changes.is_empty())
    };
    if committed {
        events::changed();
    }
    result
}

/// Run `f` with no request in flight and none starting: a GSP-RM unload, load
/// or reset, which takes the whole display down and brings it back. Nothing
/// inside `f` may call `apply`: the lock is not re-entrant
pub fn exclusive<R>(f: impl FnOnce() -> R) -> R {
    let r = {
        let _held = LOCK.lock();
        f()
    };
    // Every output may be different now, or gone
    events::changed();
    r
}
