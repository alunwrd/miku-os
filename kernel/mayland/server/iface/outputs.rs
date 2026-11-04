// may_output_manager, may_output_configuration: the outputs, and changing them
//
// K3 of `dev/kms-architecture.md`. The display layer (`drivers::gpu::kms`) has
// every connector, what is on it and what its head is doing; this hands that to
// the one client allowed to change it, the holder of `may_wm`, because which
// mode and which refresh are policy, and carries its changes back as requests
// to the layer.
//
// wlr-output-management's meaning, flattened: a head is a connector's name and
// a mode is an index into that head's list, so neither is an object the server
// makes (`protocol::output_manager`).
//
// The state
//
// A snapshot (`Outputs`), taken outside the server's lock (`kms::state` reads
// the driver's tables) and swapped in whenever the layer says something changed
// (`server::outputs_changed`). Every manager is sent all of it again: heads,
// modes and `done` with the layer's serial.
//
// Applying
//
// Not here. The server's lock is a spin lock, and `kms::apply` sleeps on RPC
// and stops the compositor while a raster changes. So `test` and `apply` hand
// the configuration to a kernel task, and its answer arrives later as
// `succeeded`, `failed` or `cancelled` (`server::output_config_answer`).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::gpu::kms;
use crate::mayland::protocol::{output_configuration as oc, output_manager as om, Interface};
use crate::mayland::wire::{Arg, Reader};

use crate::mayland::server::dispatch;
use crate::mayland::server::state::*;

/// One connector, as the display layer had it when it last said so
#[derive(Clone)]
pub struct Head {
    pub name: String,
    pub description: String,
    pub make: String,
    pub model: String,
    pub flags: u32,
    pub current: i32,
    pub preferred: i32,
    pub adaptive_sync: u32,
    pub max_bpc: u32,
    pub bpc: u32,
    /// Width, height, refresh in millihertz
    pub modes: Vec<(i32, i32, i32)>,
}

/// Every connector, and the layer's serial for that state
#[derive(Clone)]
pub struct Outputs {
    pub serial: u32,
    pub heads: Vec<Head>,
}

impl Outputs {
    pub const fn empty() -> Self {
        Outputs { serial: 0, heads: Vec::new() }
    }

    /// The head the compositor draws on
    pub fn primary(&self) -> Option<&Head> {
        self.heads.iter().find(|h| h.flags & om::PRIMARY != 0)
            .or_else(|| self.heads.iter().find(|h| h.flags & om::ENABLED != 0))
    }
}

/// The display layer's state, as this protocol carries it. Outside the server's
/// lock. The serial is read first: a change landing between the two reads makes
/// the snapshot newer than its serial, and a configuration made against it is
/// cancelled
pub fn snapshot() -> Outputs {
    let serial = kms::events::serial();
    let st = kms::state();
    let heads = st.outputs.iter().map(|o| {
        let m = o.monitor.as_ref();
        let h = o.head.as_ref();
        let mut flags = 0;
        if o.connected { flags |= om::CONNECTED; }
        if h.is_some() { flags |= om::ENABLED; }
        if o.primary { flags |= om::PRIMARY; }
        Head {
            name: o.name.clone(),
            description: match m {
                Some(m) => format!("{} {}", m.make, m.name).trim().into(),
                None => String::new(),
            },
            make: m.map_or_else(String::new, |m| m.make.clone()),
            model: m.map_or_else(String::new, |m| m.name.clone()),
            flags,
            current: h.and_then(|h| h.mode).map_or(-1, |i| i as i32),
            preferred: m.and_then(|m| m.preferred).map_or(-1, |i| i as i32),
            adaptive_sync: match h.map(|h| h.vrr) {
                Some(kms::Vrr::On { .. }) => om::VRR_ON,
                Some(kms::Vrr::Off) => om::VRR_OFF,
                _ => om::VRR_UNSUPPORTED,
            },
            max_bpc: m.map_or(0, |m| m.max_bpc as u32),
            bpc: h.and_then(|h| h.colour).map_or(0, |c| c.depth.bits()),
            modes: m.map_or_else(Vec::new, |m| m.modes.iter()
                .map(|t| (t.h_display as i32, t.v_display as i32, t.refresh_centihz() as i32 * 10))
                .collect()),
        }
    }).collect();
    Outputs { serial, heads }
}

/// Everything, to one manager: every head, its modes, then `done`
pub(in crate::mayland::server) fn send_state(s: &mut Server, idx: usize, manager: u32) {
    let Server { outputs, clients, .. } = s;
    let c = &mut clients[idx];
    for h in &outputs.heads {
        c.queue(manager, om::HEAD, &[
            Arg::Str(Some(&h.name)),
            Arg::Str(Some(&h.description)),
            Arg::Uint(h.flags),
            Arg::Int(h.current),
            Arg::Int(h.preferred),
            Arg::Uint(h.adaptive_sync),
            Arg::Uint(h.max_bpc),
            Arg::Uint(h.bpc),
        ]);
        for (i, &(w, ht, r)) in h.modes.iter().enumerate() {
            c.queue(manager, om::MODE, &[
                Arg::Str(Some(&h.name)), Arg::Uint(i as u32), Arg::Int(w), Arg::Int(ht), Arg::Int(r),
            ]);
        }
    }
    c.queue(manager, om::DONE, &[Arg::Uint(outputs.serial)]);
}

/// The state changed: every manager hears all of it, every `may_output` its
/// mode again
pub(in crate::mayland::server) fn announce(s: &mut Server) {
    for idx in 0..s.clients.len() {
        if !s.clients[idx].alive {
            continue;
        }
        for id in s.clients[idx].objects.ids_of(Interface::OutputManager) {
            send_state(s, idx, id);
        }
        for id in s.clients[idx].objects.ids_of(Interface::Output) {
            super::display::send_output_mode(s, idx, id);
        }
    }
}

/// A configuration being built: what it was made against, and every head it
/// names
pub(in crate::mayland::server) struct Pending {
    pub client: u32,
    pub object: u32,
    pub serial: u32,
    pub sets: Vec<Set>,
    /// Tested or applied: a configuration is used once, as in wlr
    pub used: bool,
}

#[derive(Clone)]
pub(in crate::mayland::server) struct Set {
    pub head: String,
    pub disable: bool,
    pub mode: i32,
    pub adaptive_sync: i32,
    pub bpc: u32,
}

pub(in crate::mayland) fn on_manager(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        om::CREATE_CONFIGURATION => {
            let id = r.new_id().map_err(|_| "create_configuration: truncated")?;
            let serial = r.uint().map_err(|_| "create_configuration: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::OutputConfiguration, 1)?;
            // Left over by clients that have gone: their answers would go
            // nowhere
            let alive: Vec<u32> = s.clients.iter().filter(|c| c.alive).map(|c| c.id).collect();
            s.output_configs.retain(|p| alive.contains(&p.client));
            let cid = s.clients[idx].id;
            s.output_configs.push(Pending { client: cid, object: id, serial, sets: Vec::new(), used: false });
            Ok(())
        }
        om::STOP => {
            s.clients[idx].queue(sender, om::FINISHED, &[]);
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_output_manager: unknown request"),
    }
}

pub(in crate::mayland) fn on_configuration(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    let Some(pi) = s.output_configs.iter().position(|p| p.client == cid && p.object == sender) else {
        return Err("may_output_configuration: no such configuration");
    };
    match opcode {
        oc::SET | oc::DISABLE_HEAD => {
            let head: String = r.string().map_err(|_| "set: bad head name")?.ok_or("set: null head")?.into();
            let set = if opcode == oc::SET {
                Set {
                    head,
                    disable: false,
                    mode: r.int().map_err(|_| "set: truncated")?,
                    adaptive_sync: r.int().map_err(|_| "set: truncated")?,
                    bpc: r.uint().map_err(|_| "set: truncated")?,
                }
            } else {
                Set { head, disable: true, mode: oc::KEEP, adaptive_sync: oc::KEEP, bpc: 0 }
            };
            let p = &mut s.output_configs[pi];
            if p.used {
                return Err("may_output_configuration: already used");
            }
            // wlr's `already_configured_head`
            if p.sets.iter().any(|x| x.head.eq_ignore_ascii_case(&set.head)) {
                return Err("may_output_configuration: that head is already in this configuration");
            }
            p.sets.push(set);
            Ok(())
        }
        oc::APPLY | oc::TEST => {
            let p = &mut s.output_configs[pi];
            if p.used {
                return Err("may_output_configuration: already used");
            }
            p.used = true;
            let (serial, sets) = (p.serial, core::mem::take(&mut p.sets));
            let test = opcode == oc::TEST;
            crate::sched::submit_task(move || {
                let answer = run(serial, &sets, test);
                crate::mayland::server::output_config_answer(cid, sender, answer);
            });
            Ok(())
        }
        oc::DESTROY => {
            s.output_configs.remove(pi);
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_output_configuration: unknown request"),
    }
}

/// What became of a configuration
pub enum Answer {
    Succeeded,
    Failed(String),
    Cancelled,
}

/// A configuration, as a request to the display layer. On a kernel task:
/// this sleeps on RPC, and stops and starts the compositor around a new raster
fn run(serial: u32, sets: &[Set], test: bool) -> Answer {
    if serial != kms::events::serial() {
        return Answer::Cancelled;
    }
    match request(sets, test) {
        Ok(()) => Answer::Succeeded,
        Err(e) => Answer::Failed(e),
    }
}

fn request(sets: &[Set], test: bool) -> Result<(), String> {
    let st = kms::state();
    let mut heads = Vec::new();
    for set in sets {
        let o = st.output(&set.head)
            .ok_or_else(|| format!("there is no output called {}", set.head))?;
        if set.disable {
            if o.head.is_some() {
                return Err(format!("{}: nothing can let go of a head yet", o.name));
            }
            continue;
        }
        let mut r = kms::HeadRequest::current(o);
        r.lit = true;
        if set.mode >= 0 {
            r.mode = Some(set.mode as usize);
        }
        if set.adaptive_sync >= 0 {
            r.vrr = set.adaptive_sync == 1;
        }
        match set.bpc {
            0 => {}
            8 | 10 => {
                let mut c = r.colour.unwrap_or(kms::HeadColour::DEFAULT);
                c.depth = if set.bpc == 10 { kms::Depth::Bpc10 } else { kms::Depth::Bpc8 };
                r.colour = Some(c);
            }
            other => return Err(format!("{}: {} bits a component is not a depth here", o.name, other)),
        }
        heads.push(r);
    }
    kms::apply(&kms::Request { heads, test_only: test }).map(|_| ())
}

/// Send what became of a configuration, if its client and its object are still
/// there to hear it
pub(in crate::mayland::server) fn answer(s: &mut Server, client: u32, object: u32, a: Answer) {
    let Some(idx) = s.client_idx(client) else { return };
    let c = &mut s.clients[idx];
    if c.objects.iface_of(object) != Some(Interface::OutputConfiguration) {
        return;
    }
    match a {
        Answer::Succeeded => c.queue(object, oc::SUCCEEDED, &[]),
        Answer::Failed(why) => c.queue(object, oc::FAILED, &[Arg::Str(Some(&why))]),
        Answer::Cancelled => c.queue(object, oc::CANCELLED, &[]),
    }
}
