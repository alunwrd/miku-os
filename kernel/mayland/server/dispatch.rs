// Where a message goes
//
// Bytes in, one call out. The only place in the server that knows the interface
// table as a whole: everything under `iface` is reached from the one `match`
// below.
//
// An `Err` from a handler kills the connection: it becomes a
// `may_display.error` and the client is marked dead, because it means the
// client sent something the protocol does not allow. Requests that legitimately
// do not apply return `Ok(())` and do nothing, so several handlers below appear
// to ignore things.

use alloc::vec::Vec;

use crate::mayland::objects::ObjectError;
use crate::mayland::protocol::{self, Interface, DISPLAY_ID};
use crate::mayland::wire::{self, Reader};

use super::{iface};
use super::state::*;

pub(in crate::mayland) fn dispatch_pending(s: &mut Server, idx: usize) {
    let client = s.clients[idx].id;
    let mut cost = BatchCost::default();
    loop {
        if !s.clients[idx].alive {
            s.clients[idx].inbuf.clear();
            break;
        }
        let header = match wire::peek_header(&s.clients[idx].inbuf) {
            Ok(Some(h)) => h,
            Ok(None) => break, // wait for more bytes
            Err(_) => {
                s.clients[idx].fail(DISPLAY_ID, protocol::display::ERR_INVALID_METHOD,
                                    "malformed message header");
                break;
            }
        };

        // Copy the body out before dispatching: the handlers take &mut Server,
        // and the inbuf lives inside it
        let body: Vec<u8> = s.clients[idx].inbuf[wire::HEADER_LEN..header.size].to_vec();
        s.clients[idx].inbuf.drain(..header.size);

        let iface = s.clients[idx].objects.iface_of(header.sender);
        let step = crate::kcore::time::Stopwatch::start();
        dispatch_one(s, idx, header.sender, header.opcode, &body);
        cost.add(iface, header.opcode, step.elapsed_us());
    }
    cost.say(client);
}

/// What one client's batch of requests cost, under the server lock
///
/// The lock reports a hold that was too long (`held.rs`), but only where it was
/// taken, which for a client's requests is always `submit`. This says which
/// requests.
#[derive(Default)]
struct BatchCost {
    total_us: u64,
    count: u32,
    /// The slowest single request: interface, opcode, us
    slowest: Option<(Option<Interface>, u16, u64)>,
    /// Time per kind of request, for a batch of many cheap ones
    kinds: Vec<(Option<Interface>, u16, u32, u64)>,
}

impl BatchCost {
    fn add(&mut self, iface: Option<Interface>, opcode: u16, us: u64) {
        self.total_us += us;
        self.count += 1;
        if self.slowest.is_none_or(|(_, _, w)| us > w) {
            self.slowest = Some((iface, opcode, us));
        }
        if let Some(k) = self.kinds.iter_mut().find(|k| k.0 == iface && k.1 == opcode) {
            k.2 += 1;
            k.3 += us;
        } else if self.kinds.len() < 32 {
            self.kinds.push((iface, opcode, 1, us));
        }
    }

    fn say(&self, client: u32) {
        if self.total_us < super::held::SLOW_HOLD_US {
            return;
        }
        let name = |i: Option<Interface>| i.map_or("(gone)", |i| i.name());
        let (si, so, sus) = self.slowest.unwrap_or((None, 0, 0));
        let most = self.kinds.iter().max_by_key(|k| k.3).copied();
        let (mi, mo, mn, mus) = most.unwrap_or((None, 0, 0, 0));
        crate::kdebug!(
            "[mayland] client {}: {} requests took {} us under the server lock; slowest {} \
             opcode {} ({} us); most time in {} opcode {} ({} of them, {} us)",
            client, self.count, self.total_us, name(si), so, sus, name(mi), mo, mn, mus);
    }
}

pub(in crate::mayland) fn dispatch_one(s: &mut Server, idx: usize, sender: u32, opcode: u16, body: &[u8]) {
    let iface = match s.clients[idx].objects.iface_of(sender) {
        Some(i) => i,
        None => {
            s.clients[idx].fail(sender, protocol::display::ERR_INVALID_OBJECT,
                                "request against an id that is not a live object");
            return;
        }
    };

    let mut r = Reader::new(body);
    let result = match iface {
        Interface::Display    => iface::display::on_display(s, idx, sender, opcode, &mut r),
        Interface::Registry   => iface::display::on_registry(s, idx, sender, opcode, &mut r),
        Interface::Compositor => iface::compositor::on_compositor(s, idx, sender, opcode, &mut r),
        Interface::Shm        => iface::buffers::on_shm(s, idx, sender, opcode, &mut r),
        Interface::ShmPool    => iface::buffers::on_shm_pool(s, idx, sender, opcode, &mut r),
        Interface::Buffer     => iface::buffers::on_buffer(s, idx, sender, opcode, &mut r),
        Interface::Surface    => iface::compositor::on_surface(s, idx, sender, opcode, &mut r),
        Interface::Seat       => iface::seat::on_seat(s, idx, sender, opcode, &mut r),
        Interface::Region     => iface::compositor::on_region(s, idx, sender, opcode, &mut r),
        Interface::Output     => iface::display::on_output(s, idx, sender, opcode, &mut r),
        Interface::Xdg        => iface::xdg::on_xdg(s, idx, sender, opcode, &mut r),
        Interface::XdgSurface => iface::xdg::on_xdg_surface(s, idx, sender, opcode, &mut r),
        Interface::Toplevel   => iface::xdg::on_toplevel(s, idx, sender, opcode, &mut r),
        Interface::Positioner => iface::xdg::on_positioner(s, idx, sender, opcode, &mut r),
        Interface::Popup      => iface::xdg::on_popup(s, idx, sender, opcode, &mut r),
        Interface::Desktop    => iface::desktop::on_desktop(s, idx, sender, opcode, &mut r),
        Interface::Wm         => iface::wm::on_wm(s, idx, sender, opcode, &mut r),
        Interface::Console    => iface::console::on_console(s, idx, sender, opcode, &mut r),
        Interface::Clipboard  => iface::clipboard::on_clipboard(s, idx, sender, opcode, &mut r),
        Interface::Layer      => iface::layer::on_layer(s, idx, sender, opcode, &mut r),
        Interface::LayerSurface => iface::layer::on_layer_surface(s, idx, sender, opcode, &mut r),
        Interface::OutputManager => iface::outputs::on_manager(s, idx, sender, opcode, &mut r),
        Interface::OutputConfiguration => iface::outputs::on_configuration(s, idx, sender, opcode, &mut r),
        Interface::FocusGrab => iface::layer::on_focus_grab(s, idx, sender, opcode, &mut r),
        // Callbacks and input objects are server-driven; a request to one is
        // either 'release' or a mistake
        Interface::Callback   => Err("may_callback has no requests"),
        Interface::Pointer | Interface::Keyboard => iface::seat::on_input_object(s, idx, sender, opcode, &mut r),
    };

    if let Err(why) = result {
        s.clients[idx].fail(sender, protocol::display::ERR_INVALID_METHOD, why);
        return;
    }

    // Every argument should be consumed by now: a handler that decodes its
    // whole signature and then finds `wire::Reader::is_empty` false has a
    // malformed message.
    //
    // Reported rather than fatal, because some handlers answer `Ok(())` without
    // reading their arguments (`toplevel::SHOW_WINDOW_MENU` and
    // `SET_MINIMIZED`, documented as "lose the feature, not the connection").
    // Making this fatal means auditing those first
    if !r.is_empty() {
        crate::serial_println!(
            "[mayland] client {} sent {} bytes too many to {:?} opcode {} - \
             either the handler does not decode its whole signature or the \
             client does not encode it",
            s.clients[idx].id, r.remaining(), iface, opcode,
        );
    }
}

/// Bind a new_id argument, turning the object-map errors into protocol errors
pub(in crate::mayland) fn bind_new(s: &mut Server, idx: usize, id: u32, iface: Interface, version: u32)
    -> Result<(), &'static str>
{
    match s.clients[idx].objects.insert_client(id, iface, version) {
        Ok(()) => Ok(()),
        Err(ObjectError::BadRange) => Err("new id outside the client's id range"),
        Err(ObjectError::InUse)    => Err("new id is already a live object"),
        Err(ObjectError::Full)     => Err("too many live objects"),
        Err(ObjectError::NotFound) => Err("object vanished"),
    }
}

/// Remove an id from the object map and tell the client it is free
pub(in crate::mayland) fn destroy_object(s: &mut Server, idx: usize, id: u32) {
    let c = &mut s.clients[idx];
    let _ = c.objects.remove(id);
    c.delete_id(id);
}
