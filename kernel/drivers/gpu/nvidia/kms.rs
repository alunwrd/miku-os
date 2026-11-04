// The display layer's NVIDIA driver (`drivers::gpu::kms`)
//
// K2 of `dev/kms-architecture.md`. The layer checks a request against its rules
// and orders the commit (`kms::commit`); this is the driver's half:
// - `state` reads the output table (`gsp_common::output`): what is plugged in,
//   which head drives it. What the head was asked to be is the layer's record
//   (`kms::heads`), which `kms::state` lays over this;
// - `check` asks what the hardware decides: the link at the new depth, IMP's
//   opinion beside it, variable refresh against the new mode, free heads;
// - `apply` lights new heads (`control::light`, a modeset of its own), then
//   sends every other change in one update (`blackwell/apply.rs`);
// - `set_vrr` is `nvDisableVrr` / `nvEnableVrr`, around the update.
//
// A display id is not a connector (a DP++ port is two of them), so the ids are
// grouped by the socket RM says they are on (`disp::connector_data`) and named
// as nvidia-drm names them (`kms::names`): the panel on this machine is `DP-3`,
// as on Linux.

use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::gpu::kms::{
    names, Change, Connector, HeadColour, HeadState, Kms, Monitor, Output, State, Vrr,
};

use super::blackwell::apply::{self, HeadPlan};
use super::blackwell::boot;
use super::gsp_common::disp;
use super::gsp_common::modeset::vrr as vrr_rules;
use super::gsp_common::modeset::{Depth, Mode};
use super::gsp_common::output;

pub struct Nvidia;

pub static NVIDIA: Nvidia = Nvidia;

/// RM's connector type as Linux names it: `AllocConnector`'s table
/// (`nvkms-rm.c`) into NVKMS's type, then
/// `nvkms_connector_type_to_drm_connector_type` (`nvidia-drm-utils.c:80`).
/// `None` for the types NVKMS makes no connector for
pub(super) fn connector_kind(rm_type: u32) -> Option<Connector> {
    Some(match rm_type {
        // DP_EXT, DP_INT, DP_MINI_EXT, DP_SERIALIZER, LFH_DP_1, LFH_DP_2
        0x46 | 0x47 | 0x48 | 0x49 | 0x64 | 0x65 => Connector::DisplayPort,
        // USB_C and ADC: NVKMS keeps them, DRM has no type for them
        0x71 | 0x32 => Connector::Unknown,
        0x00 => Connector::Vga,
        // DVI_I_TV_SVIDEO, DVI_I_TV_COMPOSITE, DVI_I, LFH_DVI_I_1, LFH_DVI_I_2
        0x20 | 0x21 | 0x30 | 0x38 | 0x39 => Connector::DviI,
        0x31 => Connector::DviD,
        // SPWG, OEM
        0x40 | 0x41 => Connector::Lvds,
        // HDMI_A, HDMI_C_MINI
        0x61 | 0x63 => Connector::HdmiA,
        0x72 => Connector::Dsi,
        // VIRTUAL_WFD, STEREO_3PIN_DIN, UNKNOWN, and the TV types NVKMS's
        // table does not have
        _ => return None,
    })
}

/// The types a DisplayPort output may have. NVKMS drops a DP-protocol output
/// of any other kind (`AllocConnector`, after the table)
pub(super) fn dp_compatible(rm_type: u32) -> bool {
    matches!(rm_type, 0x46 | 0x47 | 0x48 | 0x49 | 0x64 | 0x65 | 0x71)
}

/// Every display id that is a face of a connector Linux would name, ascending
fn faces() -> Vec<names::Face> {
    let mut f: Vec<names::Face> = output::with_all(|outs| {
        outs.iter()
            .filter_map(|o| {
                // RM would not say which socket: a connector of its own, of the
                // kind its protocol implies. Not Linux's name, but the output
                // can still be named (the boot log has the warning)
                let Some(c) = o.connector else {
                    let kind = if o.or.is_dp() {
                        Connector::DisplayPort
                    } else if o.or.is_tmds() {
                        Connector::HdmiA
                    } else {
                        return None;
                    };
                    return Some(names::Face {
                        key: o.display_id,
                        physical: 0x8000_0000 | o.display_id,
                        connector: kind,
                    });
                };
                if c.count == 0 {
                    return None;
                }
                let t = c.types[0];
                let kind = connector_kind(t)?;
                if o.or.is_dp() && !dp_compatible(t) {
                    return None;
                }
                Some(names::Face {
                    key: o.display_id,
                    physical: c.physical_index,
                    connector: kind,
                })
            })
            .collect()
    });
    f.sort_by_key(|x| x.key);
    f
}

/// One line per connector for the boot log: `DP-3 = 0x00002000, 0x00001000`
pub fn connector_lines() -> Vec<String> {
    names::name(&faces())
        .iter()
        .map(|n| {
            let mut s = alloc::format!("{} =", n.name);
            for (i, k) in n.keys.iter().enumerate() {
                s.push_str(&alloc::format!(
                    "{} {:#010x}",
                    if i == 0 { "" } else { "," },
                    k
                ));
            }
            s
        })
        .collect()
}

/// The layer's name for a display id, if Linux would have one
pub fn name_of(display_id: u32) -> Option<String> {
    names::name(&faces())
        .into_iter()
        .find(|n| n.keys.contains(&display_id))
        .map(|n| n.name)
}

fn monitor(o: &output::Output) -> Option<Monitor> {
    if !o.connected {
        return None;
    }
    let edid = crate::drivers::gpu::edid::parse(&o.edid);
    let modes: Vec<_> = o.modes.iter().map(Mode::to_timing).collect();
    Some(Monitor {
        name: o.name.clone(),
        make: edid
            .as_ref()
            .map_or_else(String::new, |e| String::from(e.manufacturer_str())),
        preferred: edid
            .as_ref()
            .and_then(|e| e.preferred())
            .and_then(|p| modes.iter().position(|t| *t == p)),
        modes,
        refresh_range: o.vrr_range,
        max_bpc: edid.map_or(0, |e| e.bpc),
    })
}

/// The head as the hardware knows it. Mode, sleep, colour and `Vrr::On` are the
/// layer's record, laid over this by `kms::state`; here is what only the driver
/// knows: which head, the surface's size, and whether variable refresh could
/// run
fn head(o: &output::Output) -> Option<HeadState> {
    let head = o.head?;
    let mode = o.mode();
    let size = match (o.scanout, mode) {
        (Some(l), _) => (l.width, l.height),
        (None, Some(m)) => (m.h_display, m.v_display),
        (None, None) => (0, 0),
    };
    let vrr = match vrr_rules::allowed(o.or.is_dp(), o.dpcd.as_ref(), o.vrr_range, mode.as_ref()) {
        Ok(_) => Vrr::Off,
        Err(why) => Vrr::Unsupported(why),
    };
    Some(HeadState {
        head,
        size,
        refresh_centihz: mode.map_or(0, |m| m.refresh_centihz()),
        mode: None,
        asleep: false,
        colour: Some(HeadColour::DEFAULT),
        vrr,
    })
}

/// What the hardware decides about one change, asked without touching it
fn check_one(o: &output::Output, c: &Change) -> Result<(), String> {
    let depth = c.colour.map_or(Depth::Bpc8, |x| x.depth);
    let possible = |m: &Mode| boot::mode_possible(&o.or, o.dpcd.as_ref(), &o.hdmi, m, depth.bits());
    // The timing it would run: the one asked for, the one running, or, for a
    // head being lit, the best the link can carry, as `light` picks
    let mode = match (c.mode, o.mode()) {
        (Some(i), _) => *o.modes.get(i).ok_or_else(|| {
            alloc::format!("{} has no mode {}, 'display' numbers them", c.output, i)
        })?,
        (None, Some(m)) => m,
        (None, None) => *o
            .modes
            .iter()
            .find(|m| possible(m))
            .ok_or_else(|| String::from("it offers nothing this link can carry"))?,
    };
    if (c.light || c.raster || c.depth_changes()) && !possible(&mode) {
        let r = mode.refresh_centihz();
        return Err(alloc::format!(
            "this link cannot carry {}x{} @ {}.{:02} Hz at {} bits a component",
            mode.h_display,
            mode.v_display,
            r / 100,
            r % 100,
            depth.bits()
        ));
    }
    if c.vrr_on_after {
        vrr_rules::allowed(o.or.is_dp(), o.dpcd.as_ref(), o.vrr_range, Some(&mode))
            .map_err(|why| alloc::format!("no variable refresh: {}", why))?;
    }
    if !c.light && (o.head.is_none() || o.sor.is_none()) {
        return Err(String::from("nothing drives it"));
    }
    Ok(())
}

impl Kms for Nvidia {
    fn name(&self) -> &'static str {
        "the NVIDIA card"
    }

    fn state(&self) -> State {
        let named = names::name(&faces());
        let outputs = output::with_all(|outs| {
            named
                .iter()
                .filter_map(|n| {
                    let faces: Vec<&output::Output> = n
                        .keys
                        .iter()
                        .filter_map(|k| outs.iter().find(|o| o.display_id == *k))
                        .collect();
                    // The face in use: the one a head drives, else the one a
                    // monitor answers on, else the one it was named after
                    let o = faces
                        .iter()
                        .find(|o| o.head.is_some())
                        .or_else(|| faces.iter().find(|o| o.connected))
                        .or_else(|| faces.first())?;
                    Some(Output {
                        name: n.name.clone(),
                        connector: n.connector,
                        key: o.display_id,
                        connected: o.connected,
                        primary: output::primary_id() == Some(o.display_id),
                        monitor: monitor(o),
                        head: head(o),
                    })
                })
                .collect()
        });
        State { outputs }
    }

    fn check(&self, changes: &[Change]) -> Result<(), String> {
        // Heads for every output being lit. The class's four when RM is not
        // asked; `propose` asks it, and refuses with the board's own mask
        let lights = changes.iter().filter(|c| c.light).count();
        if lights > 0 {
            let used = output::with_all(|outs| outs.iter().filter(|o| o.head.is_some()).count());
            let free = (disp::MAX_HEADS as usize).saturating_sub(used);
            if lights > free {
                return Err(alloc::format!(
                    "{} output(s) to light and {} free head(s)",
                    lights,
                    free
                ));
            }
        }
        // Two new rasters in one update need their tiles decided together,
        // which is the second head's work (`blackwell/apply.rs`)
        if changes.iter().filter(|c| c.raster).count() > 1 {
            return Err(String::from(
                "one new mode per request, two at once come with a second monitor to check them on",
            ));
        }
        for c in changes {
            output::with(c.key, |o| check_one(o, c))
                .unwrap_or_else(|| Err(String::from("the output went away")))?;
        }
        // IMP's opinion of every new raster, logged beside the link's: the link
        // decides, IMP is the second opinion (`display-nvkms.md`, decision 2)
        for c in changes.iter().filter(|c| c.raster) {
            if let Some(m) = c
                .mode
                .and_then(|i| output::with(c.key, |o| o.modes.get(i).copied()).flatten())
            {
                boot::imp_opinion_of(c.key, &m);
            }
        }
        Ok(())
    }

    fn set_vrr(&self, c: &Change, on: bool) -> Result<u32, String> {
        boot::set_vrr(c.key, on).map_err(String::from)
    }

    fn apply(&self, changes: &[Change]) -> Result<Vec<String>, String> {
        let mut done = Vec::new();
        // New heads first, each its own modeset: the head's channels are
        // allocated as it is lit, and its window attached after
        for c in changes.iter().filter(|c| c.light) {
            boot::light(c.key, c.mode).map_err(|e| with_done(String::from(e), &done))?;
            done.push(alloc::format!(
                "{}: lit - it has a head, a window and a surface of its own",
                c.output
            ));
        }
        // Everything else in one update
        let plans: Vec<HeadPlan> = changes
            .iter()
            .filter_map(|c| {
                let mode = c
                    .raster
                    .then(|| {
                        c.mode.and_then(|i| {
                            output::with(c.key, |o| o.modes.get(i).copied())
                                .flatten()
                                .map(|m| (i, m))
                        })
                    })
                    .flatten();
                let colour = if c.recolour { c.colour } else { None };
                (mode.is_some() || colour.is_some() || c.sleep.is_some()).then(|| HeadPlan {
                    output: c.output.clone(),
                    display_id: c.key,
                    mode,
                    colour,
                    sleep: c.sleep,
                })
            })
            .collect();
        if !plans.is_empty() {
            done.extend(apply::commit(&plans).map_err(|e| with_done(e, &done))?);
        }
        Ok(done)
    }
}

/// A failure, and what had already happened before it
fn with_done(e: String, done: &[String]) -> String {
    if done.is_empty() {
        e
    } else {
        alloc::format!("{} (already done: {})", e, done.join("; "))
    }
}
