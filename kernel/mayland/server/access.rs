// Which globals a client may have
//
// Wayland gates a too-powerful protocol with a filter on the registry, not a
// check inside the request handler: `wl_display_set_global_filter`
// (`wayland-server.c:1447`), consulted in two places, the loop that advertises
// the globals (`wayland-server.c:1223`) and `registry_bind` (`:1072`). A global
// the filter hides is never advertised, so a client is not told the interface
// exists, and binding it by a guessed name is a fatal protocol error.
//
// This is that, for the same two places in `iface::display`. A check per
// request is a check somebody forgets to add to request eighteen, and what is
// protected is the interface, not one of its verbs.
//
// The tiers
//
//      public        may_compositor, may_shm, may_seat, may_output, may_xdg
//                    Everything a program needs to put a window on the screen
//      privileged    may_desktop, may_layer
//                    may_desktop is every program's window title and geometry;
//                    upstream files the same thing (`ext-foreign-toplevel-list`)
//                    among the privileged protocols. may_layer can put a
//                    surface over everything and take the keyboard, which is
//                    why wlroots gates layer-shell too
//      control       may_wm
//                    The desktop's policy. One holder, enforced at bind
//      per-terminal  may_console
//                    Public as a global, because a terminal a program made for
//                    itself is its own; which terminal it may draw is decided
//                    at `bind`, and the machine's console among them needs
//                    CAP_MAY_PRIV there. One holder per terminal
//
// What the check decides
//
// The question is whether this client may command the desktop. Answering with
// "is this client root" would force maywm to be root, so the program that draws
// window borders could read /etc/shadow. CAP_MAY_PRIV is that authority alone,
// and it follows the seat: the greeter holds it while nobody is logged in, and
// the kernel moves it to the session's leader on login
// (kernel/process/session.rs). The leader, `maysession`, hands it on to the
// programs written down in /etc/mayland/session.d and to nothing else, so a
// browser started from the launcher cannot put a surface over everything and
// read the keyboard.
//
// `may_desktop` is read-only and `may_wm` carries the policy, so a panel needs
// only the first.

use crate::mayland::protocol::Interface;

use super::state::*;

/// How much a client has to be to hold an interface
#[derive(Clone, Copy, PartialEq, Eq)]
pub(in crate::mayland::server) enum Tier {
    /// Anything that connected
    Public,
    /// Holds CAP_MAY_PRIV: see the note at the top of the file
    Privileged,
}

/// What an interface costs to hold
///
/// Only globals reach this: a surface, a buffer or a toplevel is reachable only
/// through the global that makes it
pub(in crate::mayland::server) fn tier(iface: Interface) -> Tier {
    match iface {
        Interface::Desktop | Interface::Layer | Interface::Wm
            => Tier::Privileged,
        // Only the holder of `may_wm` may bind it, decided at bind in
        // `iface::display`, as for `may_wm`'s one holder
        Interface::OutputManager => Tier::Privileged,
        // `may_console` is public, and what it costs is decided per terminal
        // rather than per client.
        //
        // Letting anybody draw the machine's own console is letting anybody
        // read the shell, but a terminal a program made for itself is its own:
        // the protocol says "an ordinary user's terminal needs no capability
        // beyond having made its own". So the decision is at `bind`, where the
        // terminal is named.
        //
        // Gating the global instead would give every terminal emulator
        // `CAP_MAY_PRIV`, the capability that lets a program cover the screen
        // and take the keyboard
        Interface::Console => Tier::Public,
        _ => Tier::Public,
    }
}

/// Whether this client may be told that this global exists
///
/// The whole access decision, called from the registry's two places and nowhere
/// else. A client is a pid, and a pid is an identity.
pub(in crate::mayland::server) fn visible(c: &Client, iface: Interface) -> bool {
    if tier(iface) == Tier::Public {
        return true;
    }
    privileged(c.pid)
}

/// Whether a connection's process may hold a privileged interface
///
/// Two special cases:
///
///   - pid 0 is the kernel. The console, the wallpaper and the demo are
///     clients of this protocol and run on the compositor thread with no
///     process behind them. They are allowed what the compositor is allowed
///   - a pid with no process is refused. `sched::cred_of` answers root for
///     a pid it cannot find, which is safe for a caller asking about itself
///     and wrong here: a connection whose process has exited would be
///     promoted. So this asks `cred_checked`, which says `None`
pub(in crate::mayland::server) fn privileged(pid: u64) -> bool {
    if pid == 0 {
        return true;
    }
    match crate::sched::cred_checked(pid) {
        Some(c) => c.has(crate::process::cred::CAP_MAY_PRIV),
        None    => false,
    }
}
