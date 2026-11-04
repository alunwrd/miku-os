// What one program hands to another
//
// Copy and paste, and later the middle-button selection and drag-and-drop, so
// this is a store of selections and not a clipboard. `Which::Primary` is the
// same machinery with a different slot.
//
// Why not wl_data_device
//
// Wayland moves the content through a file descriptor: the source client is
// handed the write end of a pipe. Four interfaces negotiate that fd
// (`data_device_manager`, `data_device`, `data_source`, `data_offer`), and this
// transport has no fds, only a message queue and shared-memory pools. What is
// kept is the rule: only a client just typed at may set the selection, and only
// the focused one may read it.
//
// Why the content is copied twice
//
// In: the source names a range of a pool it owns and the compositor copies it
// into the kernel heap. Out: the compositor makes a pool owned by the reader
// and copies it in again. `shm::map_into` refuses a pool to any process but its
// owner and refuses a second address space, so one pool cannot be shared
// between two clients. `may_console` hands a client the character grid the same
// way.

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::mayland::protocol::{self, Interface};
use crate::mayland::wire::Arg;

use super::shm;
use super::state::*;

/// Which selection. The clipboard is what a copy goes into; the primary is
/// what a drag over text goes into on a machine with a middle button
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::mayland) enum Which {
    Clipboard = 0,
    Primary = 1,
}

impl Which {
    pub(in crate::mayland) fn from(n: u32) -> Option<Which> {
        match n {
            0 => Some(Which::Clipboard),
            1 => Some(Which::Primary),
            _ => None,
        }
    }
}

/// The most one selection may hold
///
/// A hard ceiling on what a client can make the kernel keep. Over it the
/// request is refused with a reason, since a paste that is silently half the
/// copy is worse than none
pub(in crate::mayland) const MAX_BYTES: usize = 64 * 1024;

/// How many types one selection may be offered in
const MAX_MIMES: usize = 8;
/// The longest a type name may be. `application/vnd.…` gets nowhere near it
const MAX_MIME_LEN: usize = 128;

pub(in crate::mayland) struct Selection {
    /// The client that put this here, so its disconnect can take the selection
    /// with it and only it may clear it
    pub owner: u32,
    /// In the source's order of preference, best first
    pub mimes: Vec<String>,
    pub bytes: Vec<u8>,
}

/// Puts something in a selection
///
/// The bytes are read from the source's own pool, which `shm::with_bytes`
/// bounds-checks: the range is the client's word, and `checked_add` plus the
/// pool's length makes it safe
pub(in crate::mayland) fn set(s: &mut Server, which: Which, owner: u32,
                              mimes: Vec<String>, handle: u32, offset: usize,
                              len: usize) -> Result<(), &'static str>
{
    if mimes.is_empty() {
        return Err("may_clipboard: a selection with no type is a selection nobody can ask for");
    }
    if mimes.len() > MAX_MIMES {
        return Err("may_clipboard: more types than this compositor keeps");
    }
    if mimes.iter().any(|m| m.is_empty() || m.len() > MAX_MIME_LEN) {
        return Err("may_clipboard: a type name that is empty or absurd");
    }
    if len > MAX_BYTES {
        return Err("may_clipboard: a selection larger than this compositor keeps");
    }
    // The pool must be the setter's own, as `claim` checks for every
    // pool-taking request in this server
    let Some(pid) = s.clients.iter().find(|c| c.id == owner).map(|c| c.pid) else {
        return Err("may_clipboard: no such client");
    };
    shm::claim(handle, pid).map_err(|_| "may_clipboard: not this client's pool")?;
    let bytes = shm::with_bytes(handle, offset, len, |b| b.to_vec())
        .map_err(|_| "may_clipboard: that range is not inside that pool")?;

    let taken = which as usize;
    let previous = s.selections[taken].as_ref().map(|sel| sel.owner);
    s.selections[taken] = Some(Selection { owner, mimes, bytes });

    // Whoever had it is told it is gone, and everybody is told what is there
    // now. Two events: a client holding a copy can drop it, and a client
    // waiting to paste can enable its menu item
    if let Some(old) = previous.filter(|&o| o != owner) {
        to_clipboards(s, Some(old), protocol::clipboard::LOST,
                      &[Arg::Uint(which as u32)]);
    }
    announce(s, which);
    crate::serial_println!("[mayland] client {} put {} bytes on selection {}",
                           owner, len, which as u32);
    Ok(())
}

/// Says what is in a selection, to one client or to all of them
///
/// A new client is told at bind, so a program starting after the copy knows
/// there is something to paste
pub(in crate::mayland) fn announce_to(s: &mut Server, which: Which, to: Option<u32>) {
    let taken = which as usize;
    let Some(first) = s.selections[taken].as_ref().and_then(|sel| sel.mimes.first())
    else { return };
    let mime = first.clone();
    to_clipboards(s, to, protocol::clipboard::OFFER,
                  &[Arg::Uint(which as u32), Arg::Str(Some(&mime))]);
}

fn announce(s: &mut Server, which: Which) {
    announce_to(s, which, None);
}

/// Hands a client the contents, in a pool of its own
///
/// The pool is made in the reader's name: `shm::map_into` checks the owner and
/// refuses a second address space, so one pool everybody maps cannot be built.
/// One outstanding pool per client, the previous one freed here, so a program
/// pasting in a loop does not use up its share of pools in `shm`
pub(in crate::mayland) fn offer_to(s: &mut Server, which: Which, client: u32,
                                   mime: &str) -> Result<(u32, usize), &'static str>
{
    // Only the client being typed at. A background program that could read the
    // clipboard could read a password from a password manager; Wayland draws
    // the same boundary. It is not a strong one: a program that can raise a
    // window can take the focus
    if !matches!(s.focus, Some((c, _)) if c == client) {
        return Err("may_clipboard: only the window being typed at may read the selection");
    }
    let taken = which as usize;
    let sel = s.selections[taken].as_ref()
        .ok_or("may_clipboard: there is nothing in that selection")?;
    if !sel.mimes.iter().any(|m| m == mime) {
        return Err("may_clipboard: the selection is not offered in that type");
    }
    let bytes = sel.bytes.clone();
    if bytes.is_empty() {
        return Err("may_clipboard: that selection is empty");
    }

    let Some(idx) = s.client_idx(client) else {
        return Err("may_clipboard: no such client")
    };
    let pid = s.clients[idx].pid;
    // The old one first. A client asking repeatedly would otherwise run out of
    // pools and be unable to make a buffer
    if let Some(old) = s.clients[idx].offer_pool.take() {
        let _ = shm::destroy_owned(old, pid);
    }
    let handle = shm::create(pid, bytes.len())
        .map_err(|_| "may_clipboard: no memory for the selection")?;
    if shm::with_bytes_mut(handle, 0, bytes.len(), |b| b.copy_from_slice(&bytes)).is_err() {
        let _ = shm::destroy_owned(handle, pid);
        return Err("may_clipboard: the selection would not fit its own pool");
    }
    s.clients[idx].offer_pool = Some(handle);
    crate::serial_println!("[mayland] client {} took {} bytes off selection {}",
                           client, bytes.len(), which as u32);
    Ok((handle, bytes.len()))
}

/// Empties a selection, at the request of the client that filled it
///
/// Only the owner: otherwise any program could deny the clipboard to every
/// other
pub(in crate::mayland) fn clear(s: &mut Server, which: Which, by: u32)
    -> Result<(), &'static str>
{
    let taken = which as usize;
    match s.selections[taken].as_ref() {
        None => Ok(()),
        Some(sel) if sel.owner != by =>
            Err("may_clipboard: only the client that set a selection may clear it"),
        Some(_) => {
            s.selections[taken] = None;
            to_clipboards(s, None, protocol::clipboard::LOST, &[Arg::Uint(which as u32)]);
            Ok(())
        }
    }
}

/// A client has gone and takes what it put there with it
///
/// A selection is a promise by a running program, and the bytes would otherwise
/// never be reclaimed short of another copy.
///
/// Returns the pool it was last handed, for the caller to free once the server
/// lock is released. The compositor made that pool, so it is in no client's
/// object list and the disconnect sweep cannot see it
pub(in crate::mayland) fn forget_client(s: &mut Server, client: u32) -> Option<u32> {
    for which in [Which::Clipboard, Which::Primary] {
        let taken = which as usize;
        if matches!(s.selections[taken].as_ref(), Some(sel) if sel.owner == client) {
            s.selections[taken] = None;
            to_clipboards(s, None, protocol::clipboard::LOST, &[Arg::Uint(which as u32)]);
        }
    }
    // Handed back rather than dropped: the sweep in `disconnect` frees the
    // pools a client made objects for, and this one has none. It goes on the
    // same list and is freed with the rest once the lock is released, the one
    // place in this server where a pool is destroyed
    s.client_idx(client).and_then(|idx| s.clients[idx].offer_pool.take())
}

/// What is in a selection, for the shell and the selftest
///
/// A read with no side effects and no permission check: both callers are the
/// machine asking about itself. `offer_to` is the one a client reaches, and it
/// decides who may
pub(in crate::mayland) fn peek(s: &Server, which: Which) -> Option<(usize, usize)> {
    s.selections[which as usize].as_ref().map(|sel| (sel.mimes.len(), sel.bytes.len()))
}

/// Sends one message to every `may_clipboard` a client has bound
///
/// `to` of `None` means everybody holding the interface. Same shape as
/// `focus::to_pointers`: a client may have bound the global more than once, and
/// both objects are live
fn to_clipboards(s: &mut Server, to: Option<u32>, opcode: u16, args: &[Arg]) {
    for idx in 0..s.clients.len() {
        if !s.clients[idx].alive {
            continue;
        }
        if matches!(to, Some(c) if c != s.clients[idx].id) {
            continue;
        }
        let ids = s.clients[idx].objects.ids_of(Interface::Clipboard);
        for id in ids {
            s.clients[idx].queue(id, opcode, args);
        }
    }
}
