// may_clipboard: the three requests
//
// The decisions live in `server::selection`; here the wire is read and a
// refusal turned into a message.

extern crate alloc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::mayland::protocol;
use crate::mayland::wire::{Arg, Reader};

use crate::mayland::server::selection::{self, Which};
use crate::mayland::server::state::*;

pub(in crate::mayland) fn on_clipboard(s: &mut Server, idx: usize, sender: u32,
                                       opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    let cid = s.clients[idx].id;
    match opcode {
        protocol::clipboard::SET => {
            let serial = r.uint().map_err(|_| "clipboard set: truncated")?;
            let which = r.uint().map_err(|_| "clipboard set: truncated")?;
            let mimes = r.string().map_err(|_| "clipboard set: bad type list")?
                .unwrap_or("");
            let handle = r.uint().map_err(|_| "clipboard set: truncated")?;
            let offset = r.uint().map_err(|_| "clipboard set: truncated")?;
            let len = r.uint().map_err(|_| "clipboard set: truncated")?;

            // A selection is a consequence of somebody pressing something. See
            // `is_input_serial`: this catches a confused client, not a lying
            // one
            if !s.is_input_serial(serial) {
                return Err("may_clipboard: a copy that answers no key and no button");
            }
            let which = Which::from(which)
                .ok_or("may_clipboard: a selection this compositor does not have")?;
            // One string, split on spaces, rather than a real array: a type
            // name has no spaces, and the wire's array type carries bytes, not
            // a list of strings
            let mimes: Vec<String> = mimes.split(' ')
                .filter(|m| !m.is_empty())
                .map(|m| m.to_string())
                .collect();
            selection::set(s, which, cid, mimes, handle, offset as usize, len as usize)
        }
        protocol::clipboard::GET => {
            // Not checked. Reading the selection is gated on holding the
            // keyboard, which is stronger than a serial; the argument is
            // carried for clients written against a compositor that does check
            // it
            let _serial = r.uint().map_err(|_| "clipboard get: truncated")?;
            let which = r.uint().map_err(|_| "clipboard get: truncated")?;
            let mime = r.string().map_err(|_| "clipboard get: bad type")?
                .unwrap_or("").to_string();
            let which = Which::from(which)
                .ok_or("may_clipboard: a selection this compositor does not have")?;
            let (handle, size) = selection::offer_to(s, which, cid, &mime)?;
            // Answered on the object that asked: two windows of one program can
            // both have the interface bound
            let Some(i) = s.client_idx(cid) else { return Ok(()) };
            s.clients[i].queue(sender, protocol::clipboard::DATA, &[
                Arg::Uint(which as u32), Arg::Uint(handle), Arg::Uint(size as u32),
                Arg::Str(Some(&mime)),
            ]);
            Ok(())
        }
        protocol::clipboard::CLEAR => {
            let which = r.uint().map_err(|_| "clipboard clear: truncated")?;
            let which = Which::from(which)
                .ok_or("may_clipboard: a selection this compositor does not have")?;
            selection::clear(s, which, cid)
        }
        _ => Err("may_clipboard: unknown request"),
    }
}
