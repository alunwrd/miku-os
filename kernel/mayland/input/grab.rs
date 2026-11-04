// What the pointer is doing instead of pointing
//
// Two things can take the pointer away from the client under it, asking the
// same question (who owns the pointer now), so they are behind one lock and in
// one enum. A third, a shell's focus grab, keeps the pointer to a list of
// surfaces and can be held alongside a menu, so it has a lock of its own
// (`FOCUS`). A window being carried and an open menu are mutually exclusive.
//
// The lock rule:
//
// Nothing here may call anything that takes `SERVER`. `begin_move` is reached
// from `iface::wm`, inside the server lock, and `spin::Mutex` does not recurse.
// Every function here locks, copies out what it needs, and unlocks before its
// caller goes near the server. The order is `SERVER` then `HELD`, and `drain`
// in `pointer.rs` is the one caller taking `HELD` with no server lock.
//
// The keyboard's grab is not here:
//
// `input::GRAB` in `mod.rs`, the compositor holding the keys while a client is
// focused, is an `AtomicBool` there: it is read inside the keyboard interrupt,
// on a core that may hold this lock.

extern crate alloc;
use alloc::vec::Vec;
use spin::Mutex;

/// A window being carried or stretched by the pointer
///
/// While this is held the client hears nothing: it is being moved, and motion
/// events would have it redrawing a cursor no longer over it.
///
/// Copy: `drain` takes it out from under the lock, works on the copy, and puts
/// it back, since holding the lock across `layout::move_window` would break the
/// rule above
#[derive(Clone, Copy)]
pub(in crate::mayland) struct Drag {
    pub client: u32,
    pub surface: u32,
    /// Zero for a move; a mask of `protocol::wm::edge` values for a resize
    pub edges: u32,
    /// The size being pulled towards, carried across frames because a configure
    /// is an absolute size and the mouse reports a delta. Kept here: the window
    /// takes its own time to arrive at a size, and reading it back would make
    /// the drag chase what the client has drawn
    pub w: i32,
    pub h: i32,
}

/// Who owns the pointer, when it is not the surface under it
pub(in crate::mayland) enum Grab {
    /// A window is being carried or stretched
    Drag(Drag),
    /// A menu is open. The chain is parent-first, so the last entry is the
    /// popup a new grab is taken on and the first is the one whose dismissal
    /// closes everything.
    ///
    /// A chain, as a menu opens a submenu: closing the outer one closes the
    /// inner, and a click inside any of them is a click inside the menu
    Popup(Vec<(u32, u32)>),
}

static HELD: Mutex<Option<Grab>> = Mutex::new(None);

/// The deepest a menu may nest
///
/// Not a protocol limit: a limit on the work the stacking pass and the
/// dismissal walk do per frame, and on how far a client can make the compositor
/// recurse
pub(in crate::mayland) const MAX_POPUP_DEPTH: usize = 8;

// The drag

/// Which window is being dragged, if any
pub fn dragging() -> Option<(u32, u32)> {
    match &*HELD.lock() {
        Some(Grab::Drag(d)) => Some((d.client, d.surface)),
        _ => None,
    }
}

/// The drag as it stands, copied out so the caller may take the server lock
pub(in crate::mayland) fn drag() -> Option<Drag> {
    match &*HELD.lock() {
        Some(Grab::Drag(d)) => Some(*d),
        _ => None,
    }
}

/// Put back a drag whose size the last frame changed
///
/// Only if a drag is still held: the button may have come up while the caller
/// worked on its copy, and a resize reinstating itself would never let go
pub(in crate::mayland) fn update_drag(d: Drag) {
    let mut h = HELD.lock();
    if matches!(*h, Some(Grab::Drag(_))) {
        *h = Some(Grab::Drag(d));
    }
}

/// Carry a window with the pointer until the button comes up
///
/// Called from `may_wm`, inside the server lock, so nothing here may take it:
/// everything the grab needs is passed in
pub(in crate::mayland) fn begin_move(client: u32, surface: u32) {
    *HELD.lock() = Some(Grab::Drag(Drag { client, surface, edges: 0, w: 0, h: 0 }));
    crate::serial_println!("[mayland] client {} surface {} carried by the pointer",
                           client, surface);
}

/// The same, pulling one corner. `w`/`h` are the size it starts from
pub(in crate::mayland) fn begin_resize(client: u32, surface: u32, edges: u32,
                                       w: i32, h: i32) {
    *HELD.lock() = Some(Grab::Drag(Drag { client, surface, edges, w, h }));
    crate::serial_println!(
        "[mayland] client {} surface {} resized by the pointer, edges {:#x}, from {}x{}",
        client, surface, edges, w, h);
}

// The popup chain

/// Take the pointer and the keyboard for an open menu
///
/// Refused during a window drag: the pointer is spoken for
pub(in crate::mayland) fn popup_open(key: (u32, u32)) -> Result<(), &'static str> {
    let mut h = HELD.lock();
    match &mut *h {
        Some(Grab::Drag(_)) =>
            Err("may_popup: the pointer is carrying a window"),
        Some(Grab::Popup(chain)) => {
            if chain.len() >= MAX_POPUP_DEPTH {
                return Err("may_popup: menus are nested deeper than this compositor goes");
            }
            if chain.contains(&key) {
                return Err("may_popup: this popup already holds the grab");
            }
            chain.push(key);
            Ok(())
        }
        None => {
            let mut chain = Vec::new();
            chain.push(key);
            *h = Some(Grab::Popup(chain));
            Ok(())
        }
    }
}

/// Whether this surface is somewhere in the open menu
pub(in crate::mayland) fn popup_holds(key: (u32, u32)) -> bool {
    match &*HELD.lock() {
        Some(Grab::Popup(chain)) => chain.contains(&key),
        _ => false,
    }
}

/// The client the open menu belongs to, if any
///
/// Every popup in a chain is one client's (xdg-shell cannot make one client's
/// popup a child of another's), so the first entry answers for all. Decides
/// whether a pointer event may leave the menu
pub(in crate::mayland) fn popup_client() -> Option<u32> {
    match &*HELD.lock() {
        Some(Grab::Popup(chain)) => chain.first().map(|&(c, _)| c),
        _ => None,
    }
}

/// The whole chain, innermost last, copied out so the caller may send events
pub(in crate::mayland) fn popup_chain() -> Vec<(u32, u32)> {
    match &*HELD.lock() {
        Some(Grab::Popup(chain)) => chain.clone(),
        _ => Vec::new(),
    }
}

/// Close the menu back to, but not including, `key`
///
/// Returns what was closed, innermost first, for one `popup_done` per entry in
/// the order a client expects. `None` for `key` closes the whole chain
pub(in crate::mayland) fn popup_close_to(key: Option<(u32, u32)>) -> Vec<(u32, u32)> {
    let mut h = HELD.lock();
    let Some(Grab::Popup(chain)) = &mut *h else { return Vec::new() };
    let keep = match key {
        Some(k) => match chain.iter().position(|&e| e == k) {
            Some(i) => i + 1,
            // Not in the chain at all: everything goes
            None => 0,
        },
        None => 0,
    };
    let closed: Vec<(u32, u32)> = chain.drain(keep..).rev().collect();
    if chain.is_empty() {
        *h = None;
    }
    closed
}

// The focus grab

/// A client's focus grab in force (`may_focus_grab`): its object, and the
/// surfaces of its own the pointer may reach while it holds
///
/// Beside `HELD`: a menu opens from inside a shell's overview, and the two are
/// held at once. The menu's chain decides first, and when shut the grab is
/// still there
pub(in crate::mayland) struct FocusGrab {
    pub client: u32,
    pub object: u32,
    pub surfaces: Vec<u32>,
}

static FOCUS: Mutex<Option<FocusGrab>> = Mutex::new(None);

/// Put a grab in force. The one it replaces, another object's, is returned so
/// the caller can tell its client it is over
pub(in crate::mayland) fn focus_take(g: FocusGrab) -> Option<(u32, u32)> {
    let mut f = FOCUS.lock();
    let old = f.take().filter(|o| (o.client, o.object) != (g.client, g.object)).map(|o| (o.client, o.object));
    *f = Some(g);
    old
}

/// Let go of this object's grab, if it is the one in force
pub(in crate::mayland) fn focus_release(client: u32, object: u32) {
    let mut f = FOCUS.lock();
    if f.as_ref().is_some_and(|g| (g.client, g.object) == (client, object)) {
        *f = None;
    }
}

/// End the grab in force, for a press beside it: who to tell
pub(in crate::mayland) fn focus_clear() -> Option<(u32, u32)> {
    FOCUS.lock().take().map(|g| (g.client, g.object))
}

/// Whether a grab is in force, and whose
pub(in crate::mayland) fn focus_client() -> Option<u32> {
    FOCUS.lock().as_ref().map(|g| g.client)
}

/// Whether the grab in force lists this surface
pub(in crate::mayland) fn focus_holds(key: (u32, u32)) -> bool {
    FOCUS.lock().as_ref().is_some_and(|g| g.client == key.0 && g.surfaces.contains(&key.1))
}

/// A surface is gone: off the list, and a list left empty lets go
pub(in crate::mayland) fn focus_forget_surface(key: (u32, u32)) {
    let mut f = FOCUS.lock();
    if let Some(g) = f.as_mut().filter(|g| g.client == key.0) {
        g.surfaces.retain(|&s| s != key.1);
        if g.surfaces.is_empty() {
            *f = None;
        }
    }
}

// Letting go

/// Let go of a drag, whatever was held. On the button coming up, and on the
/// window going away: a grab on a closed window would apply the next drag to
/// whatever inherited its id
///
/// A popup grab is not ended by a button: a menu stays open until dismissed or
/// destroyed, so this only clears a drag
pub(in crate::mayland) fn end_drag() -> bool {
    let mut h = HELD.lock();
    if matches!(*h, Some(Grab::Drag(_))) {
        *h = None;
        return true;
    }
    false
}

/// Everything this client held goes with it
///
/// Returns the popups that were open, innermost first, so the caller can tell a
/// surviving client its menu is gone. When the client itself died the list is
/// dropped
pub(in crate::mayland) fn forget_client(client: u32) -> Vec<(u32, u32)> {
    {
        let mut f = FOCUS.lock();
        if f.as_ref().is_some_and(|g| g.client == client) {
            *f = None;
        }
    }
    let mut h = HELD.lock();
    match &*h {
        Some(Grab::Drag(d)) if d.client == client => { *h = None; Vec::new() }
        Some(Grab::Popup(chain)) if chain.first().is_some_and(|&(c, _)| c == client) => {
            let closed = chain.iter().rev().copied().collect();
            *h = None;
            closed
        }
        _ => Vec::new(),
    }
}

/// One surface is gone: drop a drag on it, and close the menu from it inwards
pub(in crate::mayland) fn forget_surface(key: (u32, u32)) -> Vec<(u32, u32)> {
    let mut h = HELD.lock();
    match &mut *h {
        Some(Grab::Drag(d)) if (d.client, d.surface) == key => { *h = None; Vec::new() }
        Some(Grab::Popup(chain)) => {
            let Some(i) = chain.iter().position(|&e| e == key) else { return Vec::new() };
            let closed: Vec<(u32, u32)> = chain.drain(i..).rev().collect();
            if chain.is_empty() {
                *h = None;
            }
            closed
        }
        _ => Vec::new(),
    }
}

/// Drop everything. The compositor stopping, and nothing else
pub(in crate::mayland) fn clear() {
    *HELD.lock() = None;
    *FOCUS.lock() = None;
}
