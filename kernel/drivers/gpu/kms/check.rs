// The layer's half of a check: what a request changes, and whether it may
//
// `plan` is a pure function from a request and the state to one `Change` per
// head that changes, with no hardware or locks, so `nvidia selftest` can run it
// on a made-up state. The rules here need no driver: which mode, which depth
// the monitor takes, what a running compositor allows. The driver's own `check`
// then sees every change at once (`ValidateProposedModeSetHwState`'s place in
// `nvSetDispModeEvo`).
//
// Only what a request changes is judged: a monitor running a mode is never
// refused sleep over that mode.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use super::{Depth, HeadColour, HeadState, Output, Request, State, Vrr};
use super::request::HeadRequest;

/// One head's part of a checked request: where it is, where it goes, and
/// which of the commit's steps it takes
#[derive(Clone)]
pub struct Change {
    pub output: String,
    /// The driver's key for the output
    pub key: u32,
    /// What the head does now; `None` when nothing drives the output
    pub now: Option<HeadState>,
    /// The mode asked for, into the monitor's list. `None` keeps the running
    /// one or, on an output being lit, leaves the pick to the driver (the best
    /// its link carries)
    pub mode: Option<usize>,
    /// The colour the head ends with; `None` on a display with no colour
    /// pipeline
    pub colour: Option<HeadColour>,
    pub asleep: bool,
    pub vrr: bool,
    /// A head is given to an output nothing drives
    pub light: bool,
    /// A different timing on a head that is already lit
    pub raster: bool,
    /// A different colour
    pub recolour: bool,
    /// `Some(true)` puts the monitor to sleep, `Some(false)` wakes it
    pub sleep: Option<bool>,
    /// Variable refresh comes down before anything else changes
    /// (`nvDisableVrr` before the apply)...
    pub vrr_off_before: bool,
    /// ...and goes up after it (`nvEnableVrr`)
    pub vrr_on_after: bool,
    /// The frame path has to stop while this lands: a new raster under a
    /// running compositor
    pub hold_frames: bool,
}

impl Change {
    /// The depth changes: more bits on the same link
    pub fn depth_changes(&self) -> bool {
        let now = self.now.and_then(|h| h.colour).map(|c| c.depth);
        self.colour.map(|c| c.depth) != now
    }
}

/// Every head the request changes, or why it may not. `compositor` is passed
/// in, so a test can say either
pub fn plan(req: &Request, state: &State, compositor: bool) -> Result<Vec<Change>, String> {
    let mut out = Vec::new();
    for (i, r) in req.heads.iter().enumerate() {
        if req.heads[..i].iter().any(|x| x.output.eq_ignore_ascii_case(&r.output)) {
            return Err(format!("{} is asked for twice in one request", r.output));
        }
        let o = state
            .output(&r.output)
            .ok_or_else(|| format!("there is no output called {}", r.output))?;
        if let Some(c) = one(o, r, compositor)? {
            out.push(c);
        }
    }
    Ok(out)
}

fn one(o: &Output, r: &HeadRequest, compositor: bool) -> Result<Option<Change>, String> {
    let now = o.head;
    let lit = now.is_some();
    if !r.lit {
        return if lit {
            Err(String::from(
                "nothing can let go of a head yet - 'display off' puts the monitor to sleep"))
        } else {
            Ok(None)
        };
    }
    let light = !lit;
    if light && r.asleep {
        return Err(String::from("an output is lit awake - light it, then put it to sleep"));
    }

    let modes = o.monitor.as_ref().map_or(&[][..], |m| &m.modes[..]);
    if let Some(i) = r.mode {
        if i >= modes.len() {
            return Err(format!("{} has no mode {} - 'display' numbers them", o.name, i));
        }
    }
    let running = now.and_then(|h| h.mode);
    let raster = lit && r.mode.is_some() && r.mode != running;

    let colour_now = now.and_then(|h| h.colour);
    let colour = r.colour.or(colour_now);
    // A head is lit with the default colour, so asking for that is no change
    let recolour = match (lit, r.colour) {
        (_, None) => false,
        (true, Some(c)) => Some(c) != colour_now,
        (false, Some(c)) => c != HeadColour::DEFAULT,
    };

    let asleep_now = now.is_some_and(|h| h.asleep);
    let sleep = (lit && r.asleep != asleep_now).then_some(r.asleep);

    let vrr_now = matches!(now.map(|h| h.vrr), Some(Vrr::On { .. }));
    // Down before a new raster, before sleep, and when asked to; up after
    // whenever it is wanted and is not already up and staying up
    let vrr_off_before = vrr_now && (!r.vrr || raster || r.asleep);
    let vrr_on_after = r.vrr && !r.asleep && (!vrr_now || vrr_off_before);
    // Asked for, with nothing to vary: reported, not dropped as "no change"
    // below
    if r.vrr && r.asleep && !vrr_now {
        return Err(String::from("a sleeping monitor has no refresh to vary"));
    }

    if !(light || raster || recolour || sleep.is_some() || vrr_off_before || vrr_on_after) {
        return Ok(None);
    }

    // A monitor has to be there for anything that sends it a picture. Sleep
    // does not need one: the hot-plug pass puts an output whose monitor left to
    // sleep through here
    if !o.connected && (light || raster || sleep == Some(false) || vrr_on_after) {
        return Err(String::from("nothing is plugged into it"));
    }

    // Ten bits only where the EDID says ten: "did not say" is refused, not
    // taken as eight. Judged when the depth or the raster changes
    let depth_now = colour_now.map(|c| c.depth);
    let depth = colour.map(|c| c.depth);
    if depth == Some(Depth::Bpc10) && (depth != depth_now || raster || light) {
        let bpc = o.monitor.as_ref().map_or(0, |m| m.max_bpc);
        if bpc == 0 {
            return Err(String::from("this monitor's EDID does not say what depth it takes"));
        }
        if bpc < 10 {
            return Err(format!("this monitor takes {} bits a component, not 10", bpc));
        }
    }

    // What the driver already said about the running mode. A new mode is the
    // driver's to judge (`vrr::allowed` against it)
    if vrr_on_after && !raster {
        if let Some(HeadState { vrr: Vrr::Unsupported(why), .. }) = now {
            return Err(format!("no variable refresh: {}", why));
        }
    }

    // A new raster under the compositor holds its frames while it lands: it
    // stops, the head changes, and it starts again on the screen as it now is
    // (K3). Lighting stays refused: it pushes a second head's modeset through
    // the core channel the compositor's head flips on, and nothing composes a
    // second head yet
    let mut hold_frames = false;
    if compositor {
        if light {
            return Err(String::from("the compositor is running - stop it first (mayland stop)"));
        }
        hold_frames = raster;
    }

    Ok(Some(Change {
        output: o.name.clone(),
        key: o.key,
        now,
        mode: r.mode,
        colour,
        asleep: r.asleep,
        vrr: r.vrr,
        light,
        raster,
        recolour,
        sleep,
        vrr_off_before,
        vrr_on_after,
        hold_frames,
    }))
}
