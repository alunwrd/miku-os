// may_seat and the input objects
//
// One seat, one keyboard, one pointer, and two things the compositor owes a
// client binding them: the keymap, which it decides and the client reads from
// shared memory, and the cursor, which the client decides and the compositor
// draws.
//
// The keymap pool is per connection, because a pool is mapped by its owner and
// the owner is a process. It is eight kilobytes, room for four layouts, and is
// made only when something binds a keyboard.

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::{Arg, Reader};

use crate::mayland::server::{dispatch, shm};
use crate::mayland::server::state::*;

pub(in crate::mayland) fn on_seat(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    match opcode {
        protocol::seat::GET_POINTER => {
            let id = r.new_id().map_err(|_| "get_pointer: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Pointer, 1)?;
            Ok(())
        }
        protocol::seat::GET_KEYBOARD => {
            let id = r.new_id().map_err(|_| "get_keyboard: truncated")?;
            dispatch::bind_new(s, idx, id, Interface::Keyboard, 1)?;
            // A keyboard owes its client a keymap immediately: the layout the
            // compositor decides, in a pool the client maps, the same exchange
            // Wayland makes with an fd
            let (format, handle, size) = match keymap_pool(s, idx) {
                Some((h, n)) => (protocol::keyboard::KEYMAP_MAY_V2, h, n as u32),
                // Out of pools or out of memory: said in the format field
                None => (protocol::keyboard::KEYMAP_NO_KEYMAP, 0, 0),
            };
            let c = &mut s.clients[idx];
            c.queue(id, protocol::keyboard::KEYMAP, &[
                Arg::Uint(format),
                Arg::Uint(handle),
                Arg::Uint(size),
            ]);
            let (rate, delay) = crate::mayland::input::conf::repeat_info();
            c.queue(id, protocol::keyboard::REPEAT_INFO, &[
                Arg::Int(rate),  // keys per second
                Arg::Int(delay), // milliseconds before repeat starts
            ]);
            Ok(())
        }
        protocol::seat::RELEASE => {
            dispatch::destroy_object(s, idx, sender);
            Ok(())
        }
        _ => Err("may_seat: unknown request"),
    }
}

pub(in crate::mayland) fn on_input_object(s: &mut Server, idx: usize, sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    // release is opcode 0 on may_keyboard and opcode 1 on may_pointer, which is
    // Wayland's numbering
    let iface = s.clients[idx].objects.iface_of(sender);
    let release = match iface {
        Some(Interface::Keyboard) => protocol::keyboard::RELEASE,
        Some(Interface::Pointer)  => protocol::pointer::RELEASE,
        _ => return Err("not an input object"),
    };
    if iface == Some(Interface::Pointer) && opcode == protocol::pointer::SET_CURSOR {
        return set_cursor(s, idx, r);
    }
    if opcode == release {
        dispatch::destroy_object(s, idx, sender);
        Ok(())
    } else {
        Err("input object: unknown request")
    }
}

/// Tells every keyboard the repeat numbers again
///
/// `repeat_info` is pushed, so a keyboard that bound before the window manager
/// read its config was told the old numbers. Called from `set_param` when
/// `input { repeat_rate }` or `repeat_delay` changes, on an ordinary boot once,
/// before anything has bound
pub(in crate::mayland) fn send_repeat_info(s: &mut Server) {
    let (rate, delay) = crate::mayland::input::conf::repeat_info();
    for idx in 0..s.clients.len() {
        // The ids first and the sending after: `ids_of` borrows the client's
        // object table and `queue` wants the client itself
        let kbds = s.clients[idx].objects.ids_of(Interface::Keyboard);
        for id in kbds {
            s.clients[idx].queue(id, protocol::keyboard::REPEAT_INFO, &[
                Arg::Int(rate), Arg::Int(delay),
            ]);
        }
    }
}

/// The keymap pool this client reads its layouts from, made on demand
///
/// One per connection, as at the top of this file. Always `keymap::SIZE_MAX`,
/// so a config changing the list of layouts can rewrite it in place (see
/// `rewrite_keymaps`)
pub(in crate::mayland) fn keymap_pool(s: &mut Server, idx: usize) -> Option<(u32, usize)> {
    use crate::mayland::keymap::SIZE_MAX;
    if let Some(h) = s.clients[idx].keymap {
        return shm::size(h).ok().map(|n| (h, n));
    }
    let pid = s.clients[idx].pid;
    let handle = shm::create(pid, SIZE_MAX).ok()?;
    if !write_keymap(handle) {
        let _ = shm::destroy(handle);
        return None;
    }
    s.clients[idx].keymap = Some(handle);
    Some((handle, SIZE_MAX))
}

/// Serialise the seat's layouts into a keymap pool
fn write_keymap(handle: u32) -> bool {
    use crate::mayland::keymap::{self, MAX_GROUPS, SIZE_MAX};
    let mut list = [&keymap::LAYOUTS[0]; MAX_GROUPS];
    let n = crate::io::input::layout::layouts(&mut list);
    let ok = shm::with_bytes_mut(handle, 0, SIZE_MAX, |b| keymap::serialise(b, &list[..n]).is_some());
    matches!(ok, Ok(true))
}

/// Rewrites every client's keymap with the seat's layouts as they are now
///
/// In place and without a new `keymap` event: a client reads the header on
/// every keystroke, and the same handle sent again would be a second mapping of
/// one pool. Called from `set_param` when `kb_layout` changes, on an ordinary
/// boot once, before anything has bound a keyboard
pub(in crate::mayland) fn rewrite_keymaps(s: &mut Server) {
    for c in &s.clients {
        if let Some(h) = c.keymap {
            if !write_keymap(h) {
                crate::serial_println!("[mayland] keymap: could not rewrite client {}'s", c.id);
            }
        }
    }
}

/// may_pointer.set_cursor: the client under the pointer says what the pointer
/// should look like over it
///
/// Only the client the pointer is over may set it, or any client could change
/// the cursor over anyone's window. The surface takes the cursor role, which
/// keeps it out of the stack, composition and hit testing.
///
/// A null surface is how a client asks for no cursor over it
pub(in crate::mayland) fn set_cursor(s: &mut Server, idx: usize, r: &mut Reader) -> Result<(), &'static str> {
    let _serial = r.uint().map_err(|_| "set_cursor: truncated")?;
    let sid = r.object().map_err(|_| "set_cursor: truncated")?;
    let hot_x = r.int().map_err(|_| "set_cursor: truncated")?;
    let hot_y = r.int().map_err(|_| "set_cursor: truncated")?;

    let client = s.clients[idx].id;
    // Not an error: a client can lag behind the pointer leaving it, and a stale
    // set_cursor is a race it cannot avoid
    if s.pointer_focus.map(|(c, _)| c) != Some(client) {
        return Ok(());
    }

    if sid == 0 {
        s.cursor = Some(CursorSurface { client, surface: 0, hot_x: 0, hot_y: 0 });
        return Ok(());
    }

    let Some(si) = s.surface_idx((client, sid)) else {
        return Err("set_cursor: no such surface");
    };
    match s.surfaces[si].role {
        Role::None => s.surfaces[si].role = Role::Cursor,
        Role::Cursor => {}
        // Wayland's wl_pointer.set_cursor is a role assignment, and a surface
        // that is already a window cannot also be a cursor
        _ => return Err("set_cursor: surface already has a role"),
    }
    s.cursor = Some(CursorSurface { client, surface: sid, hot_x, hot_y });
    Ok(())
}
