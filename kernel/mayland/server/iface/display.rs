// may_display, may_registry, may_output: the handshake
//
// The three requests a client makes before it knows anything. `sync` is an
// ordering: everything sent before it has been processed by the time its
// callback fires. `get_registry` answers with the globals, and `bind` turns one
// of those names into an object.
//
// Two globals are not for everybody, and `bind` decides that before the id is
// bound, so a refused client has the id it asked for still free.

use alloc::string::String;
use alloc::vec::Vec;

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Arg, Reader};

use crate::mayland::server::{access, dispatch};
use crate::mayland::server::state::*;
use super::desktop;

pub(in crate::mayland) fn on_display(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::display::SYNC => {
            let id = r.new_id().map_err(|_| "sync: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Callback, 1)?;
            // Nothing is queued behind this one, so it is already done:
            // everything the client sent earlier has been processed by the time
            // this fires
            let serial = s.next_serial();
            let c = &mut s.clients[idx];
            c.queue(id, protocol::callback::DONE, &[Arg::Uint(serial)]);
            let _ = c.objects.remove(id);
            c.delete_id(id);
            Ok(())
        }
        protocol::display::GET_REGISTRY => {
            let id = r.new_id().map_err(|_| "get_registry: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Registry, 1)?;
            // Filtered here and not only at bind: a global a client may not
            // have is one it is never told about, as in Wayland's
            // `display_get_registry` (`wayland-server.c:1223`)
            let globals: Vec<(u32, &'static str, u32)> = GLOBALS.iter()
                .filter(|g| access::visible(&s.clients[idx], g.iface))
                .map(|g| (g.name, g.iface.name(), g.iface.version()))
                .collect();
            let c = &mut s.clients[idx];
            for (name, iname, ver) in globals {
                c.queue(id, protocol::registry::GLOBAL, &[
                    Arg::Uint(name),
                    Arg::Str(Some(iname)),
                    Arg::Uint(ver),
                ]);
            }
            Ok(())
        }
        _ => Err("may_display: unknown request"),
    }
}

pub(in crate::mayland) fn on_registry(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    if opcode != protocol::registry::BIND {
        return Err("may_registry: unknown request");
    }
    let name = r.uint().map_err(|_| "bind: truncated")?;
    let iname = r.string().map_err(|_| "bind: bad interface string")?
        .ok_or("bind: null interface")?;
    let version = r.uint().map_err(|_| "bind: truncated")?;
    let id = r.new_id().map_err(|_| "bind: truncated")?;

    let global = GLOBALS.iter().find(|g| g.name == name)
        .ok_or("bind: no global under that name")?;
    // Same answer and wording as for a name that does not exist, so the refusal
    // does not reveal the global. Fatal, as upstream: `registry_bind` posts
    // `invalid_object`
    if !access::visible(&s.clients[idx], global.iface) {
        return Err("bind: no global under that name");
    }
    if global.iface.name() != iname {
        return Err("bind: interface does not match the global");
    }
    if version == 0 || version > global.iface.version() {
        return Err("bind: unsupported version");
    }
    let iface = global.iface;
    let cid = s.clients[idx].id;

    // Refused before anything is written down. `holder` decides, so a lease
    // left by a dead connection does not count
    if iface == Interface::Wm && matches!(holder(s, &WM), Some(l) if l.client != cid) {
        return Err("may_wm: another client is driving the desktop");
    }
    // Which mode and which refresh are policy, and policy is `may_wm`'s: the
    // outputs are configured by the client holding it and nobody else
    // (`dev/kms-architecture.md`, K3). Bound after `may_wm`, the order a window
    // manager binds them in
    if iface == Interface::OutputManager && !matches!(holder(s, &WM), Some(l) if l.client == cid) {
        return Err("may_output_manager: only the window manager configures outputs");
    }
    // The console has one holder per terminal, so there is nothing to refuse
    // here: which terminal is what `may_console.bind` says, and that is where
    // two programs reaching for one grid are told apart

    // The object exists before anything is leased to it. `bind_new` fails on an
    // id the client already has live (id 1 is always `may_display`) and the
    // failure kills the connection. Recorded the other way round, `may_wm`
    // would be leased to an object never created, held by a client disconnected
    // only when its process closes the socket
    dispatch::bind_new(s, idx, id, iface, version)?;

    match iface {
        // Everyone bound to may_desktop hears the same events. Binding twice
        // replaces the earlier object: one connection, one channel
        Interface::Desktop => {
            let mut held = DESKTOP.lock();
            held.retain(|l| l.client != cid);
            held.push(Lease { client: cid, object: id, version });
        }
        // may_wm has one holder, and holding it is commanding. There is no seat
        // to claim afterwards
        Interface::Wm => *WM.lock() = Some(Lease { client: cid, object: id, version }),
        // `may_console` claims nothing here. Which terminal this client draws
        // is what `may_console.bind` says, and until then it holds the
        // interface and no terminal (see `iface::console::on_console`)
        Interface::Console => {}
        _ => {}
    }

    // Some globals owe the client state the moment it binds them
    match iface {
        Interface::Shm => {
            let c = &mut s.clients[idx];
            for f in FORMATS {
                c.queue(id, protocol::shm::FORMAT, &[Arg::Uint(*f)]);
            }
        }
        Interface::Seat => {
            let caps = protocol::seat::CAP_KEYBOARD | protocol::seat::CAP_POINTER;
            let c = &mut s.clients[idx];
            c.queue(id, protocol::seat::CAPABILITIES, &[Arg::Uint(caps)]);
            c.queue(id, protocol::seat::NAME, &[Arg::Str(Some("seat0"))]);
        }
        Interface::Output => send_output(s, idx, id, version),
        // Everything at once, as it will be again after every change
        Interface::OutputManager => super::outputs::send_state(s, idx, id),
        // The rectangle it is about to lay windows out in, owed at bind as the
        // output's mode is: a client having to ask for the first one would be
        // deciding against nothing
        //
        // And the keyboard layout in use: a bar would otherwise show nothing
        // until the first switch
        Interface::Desktop => {
            desktop::send_work_area(s);
            desktop::send_keyboard_layout(s);
        }
        // A program starting after the copy has to know there is something to
        // paste: told at bind, so a paste menu item is not wrongly disabled
        Interface::Clipboard => {
            use crate::mayland::server::selection::{announce_to, Which};
            announce_to(s, Which::Clipboard, Some(cid));
            announce_to(s, Which::Primary, Some(cid));
        }
        _ => {}
    }
    Ok(())
}

/// The output the compositor draws on, as a client binding it is owed it.
///
/// Make and model from the monitor's EDID and, from version 2 (`wl_output`
/// version 4), the connector's name and a description. One global, the screen
/// the compositor composes: an output nothing composes onto is not one a window
/// can be put on (`dev/kms-architecture.md`, K3)
fn send_output(s: &mut Server, idx: usize, id: u32, version: u32) {
    let (make, model, name, description) = match s.outputs.primary() {
        Some(h) => (h.make.clone(), h.model.clone(), h.name.clone(), h.description.clone()),
        None => ("MikuOS".into(), "mayland".into(), String::new(), String::new()),
    };
    let c = &mut s.clients[idx];
    c.queue(id, protocol::output::GEOMETRY, &[
        Arg::Int(0), Arg::Int(0),      // position in the global space
        Arg::Int(0), Arg::Int(0),      // physical size, mm: unknown
        Arg::Int(0),                   // subpixel: unknown
        Arg::Str(Some(&make)),
        Arg::Str(Some(&model)),
        Arg::Int(0),                   // transform: normal
    ]);
    c.queue(id, protocol::output::SCALE, &[Arg::Int(1)]);
    if version >= 2 && !name.is_empty() {
        c.queue(id, protocol::output::NAME, &[Arg::Str(Some(&name))]);
        c.queue(id, protocol::output::DESCRIPTION, &[Arg::Str(Some(&description))]);
    }
    send_output_mode(s, idx, id);
}

/// The mode the screen is in, and `done`. Again on every change: Wayland sends
/// the current mode, not the list - the list is `may_output_manager`'s
pub(in crate::mayland::server) fn send_output_mode(s: &mut Server, idx: usize, id: u32) {
    let (w, h, refresh) = (s.width, s.height, s.refresh_mhz);
    let preferred = s.outputs.primary().is_some_and(|p| p.current >= 0 && p.current == p.preferred);
    let mut flags = protocol::output::MODE_CURRENT;
    if preferred {
        flags |= protocol::output::MODE_PREFERRED;
    }
    let c = &mut s.clients[idx];
    c.queue(id, protocol::output::MODE, &[
        Arg::Uint(flags), Arg::Int(w), Arg::Int(h), Arg::Int(refresh),
    ]);
    c.queue(id, protocol::output::DONE, &[]);
}

pub(in crate::mayland) fn on_output(s: &mut Server, idx: usize, sender: u32, opcode: u16, _r: &mut Reader)
    -> Result<(), &'static str>
{
    if opcode != protocol::output::RELEASE {
        return Err("may_output: unknown request");
    }
    dispatch::destroy_object(s, idx, sender);
    Ok(())
}
