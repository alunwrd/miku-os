// What a client that is not the desktop may have
//
// Every case is about a refusal, and each also checks that the same client can
// still do what it should, so a filter that refuses everything fails. Where the
// reason can be asked, it is: `server::death` says why a connection ended.
//
// How a client here is made unprivileged
//
// `server::connect` takes the pid of the process behind the connection, and the
// filter asks that pid for its credentials. A pid with no process answers
// `None` (`sched::cred_checked`), and `None` is refused. So a connection opened
// for a pid that never existed is an unprivileged client, with no process to
// spawn and drop from root.

use super::*;

/// A pid the process table will never hold
const NO_SUCH_PID: u64 = 0xFFFF_FFFF;

/// The three interfaces that are not for everybody, by name
///
/// `may_console` is not one of them: `server::access::tier` decides that at
/// `bind`, where the terminal is named. Duplicated from `tier()`;
/// `test_bind_of_a_hidden_global_is_fatal` below binds every global the
/// registry offers, not just these.
const PRIVILEGED: &[&str] = &["may_desktop", "may_wm", "may_layer"];

/// The six that are
const PUBLIC: &[&str] =
    &["may_compositor", "may_shm", "may_seat", "may_output", "may_xdg", "may_console"];

/// What the registry tells a client
fn advertised(pid: u64) -> (u32, ClientConn, Vec<(u32, alloc::string::String)>) {
    let cl = server::connect(pid);
    let mut conn = ClientConn::new();
    let _ = conn.get_registry();
    let names = exchange(&mut conn, cl).iter().filter_map(|e| match e {
        Event::Global { name, interface, .. } => Some((*name, interface.clone())),
        _ => None,
    }).collect();
    (cl, conn, names)
}

/// The number each global goes by, from a client allowed to see them
///
/// The registry advertises by number and the numbers are the server's own, so
/// the case asks as the compositor, which is allowed everything, then uses them
/// as an unprivileged client
fn numbers() -> Vec<(u32, alloc::string::String)> {
    let (cl, _conn, seen) = advertised(0);
    server::disconnect(cl);
    seen
}

/// A global a client may not have is one it is never told about
pub(super) fn test_registry_hides_the_privileged(t: &mut Runner) {
    const NAME: &str = "access: an unprivileged client is not told the privileged globals exist";
    let (cl, _conn, seen) = advertised(NO_SUCH_PID);
    let has = |w: &str| seen.iter().any(|(_, g)| g == w);
    let hidden = PRIVILEGED.iter().all(|w| !has(w));
    let kept = PUBLIC.iter().all(|w| has(w));
    server::disconnect(cl);

    // The same question as the kernel's own clients, which are the compositor
    // and are allowed everything. A filter hiding the globals from everybody
    // would pass the first test alone
    let all = numbers();
    let root_sees_all = PRIVILEGED.iter()
        .all(|w| all.iter().any(|(_, g)| g == w));

    t.check(NAME, hidden && kept && root_sees_all,
            format!("privileged hidden {}, public still offered {}, \
                     the compositor's own clients still see them {} (saw {:?})",
                    hidden, kept, root_sees_all,
                    seen.iter().map(|(_, g)| g).collect::<Vec<_>>()));
}

/// Guessing the number of a hidden global does not get it
///
/// The refusal has to be fatal and first. Fatal, because a bind that did
/// nothing would leave the client holding an id it takes for a window manager.
/// First, because `on_registry` checks the interface name against the global
/// after checking visibility. Each hidden global is bound the way its rightful
/// owner would bind it
pub(super) fn test_bind_of_a_hidden_global_is_fatal(t: &mut Runner) {
    const NAME: &str = "access: binding a hidden global by its number kills the connection";
    let mut refused = 0usize;
    let mut wrong_reason = Vec::new();

    for (number, iname) in numbers() {
        if !PRIVILEGED.contains(&iname.as_str()) {
            continue;
        }
        let Some(iface) = protocol::Interface::from_name(&iname) else { continue };
        let cl = server::connect(NO_SUCH_PID);
        let mut conn = ClientConn::new();
        let registry = conn.get_registry();
        let _ = exchange(&mut conn, cl);
        let _id = conn.bind(registry, number, iface, iface.version());
        let events = exchange(&mut conn, cl);

        // Dead, and dead of this. The wording is the one a nonexistent name
        // gets, so the refusal does not reveal that the global exists
        let why = first_error(&events).map(|(_, m)| m);
        let dead = !server::is_alive(cl);
        if dead && matches!(why.as_deref(), Some(m) if m.contains("no global under that name")) {
            refused += 1;
        } else {
            wrong_reason.push((iname.clone(), dead, why));
        }
        server::disconnect(cl);
    }

    t.check(NAME, refused == PRIVILEGED.len() && wrong_reason.is_empty(),
            format!("{} of {} hidden globals refused as though they did not exist; \
                     anything else: {:?}",
                    refused, PRIVILEGED.len(), wrong_reason));
}

/// A layer surface is a keyboard grab, so it is not for everybody
///
/// `set_keyboard(exclusive)` takes the focus when sent, which is why wlroots
/// gates layer-shell. An unprivileged client cannot have the interface that
/// makes the object the request goes to; the case checks that by trying
pub(super) fn test_layer_is_out_of_reach(t: &mut Runner) {
    const NAME: &str = "access: an unprivileged client cannot take the keyboard with a layer";
    let Some((number, _)) = numbers().into_iter().find(|(_, g)| g == "may_layer") else {
        t.check(NAME, false, "may_layer is not a global at all".to_string());
        return;
    };

    // Asked for by number, by an unprivileged client, naming it correctly
    let cl = server::connect(NO_SUCH_PID);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let _ = exchange(&mut conn, cl);
    let layer = conn.bind(registry, number, Interface::Layer, 1);
    let refused = first_error(&exchange(&mut conn, cl)).is_some() && !server::is_alive(cl);

    // The request the gate exists for, sent anyway. The object was never bound
    // and the connection is already dead, so the keyboard is checked afterwards
    // to catch a compositor that dispatched it regardless
    let focus_before = server::focus_key();
    conn.raw(layer, protocol::layer::GET_LAYER_SURFACE,
             &[Arg::Uint(0), Arg::Uint(0), Arg::Uint(protocol::layer::OVERLAY)]);
    let _ = exchange(&mut conn, cl);
    let focus_stayed = server::focus_key() == focus_before;
    server::disconnect(cl);

    // The other half: a client allowed the interface gets it. Without this the
    // case passes when may_layer is simply broken
    let cl0 = server::connect(0);
    let mut c0 = ClientConn::new();
    let registry0 = c0.get_registry();
    let _ = exchange(&mut c0, cl0);
    let _l0 = c0.bind(registry0, number, Interface::Layer, 1);
    let allowed = first_error(&exchange(&mut c0, cl0)).is_none() && server::is_alive(cl0);
    server::disconnect(cl0);

    t.check(NAME, refused && focus_stayed && allowed,
            format!("the bind was refused {}, the keyboard did not move {}, \
                     the compositor's own client may still have it {}",
                    refused, focus_stayed, allowed));
}

/// One owner may not have the whole pool table
pub(super) fn test_pool_share(t: &mut Runner) {
    const NAME: &str = "access: one process may not take every shared-memory pool";
    // A pid of its own, so the count is this case's and not the machine's
    const OWNER: u64 = 0xFFFF_FFFE;
    let mut held = Vec::new();
    let mut refusal = None;
    for _ in 0..shm::MAX_POOLS_PER_OWNER + 2 {
        match shm::create(OWNER, 4096) {
            Ok(h) => held.push(h),
            Err(e) => { refusal = Some(e); break; }
        }
    }
    let got = held.len();
    for h in &held {
        let _ = shm::destroy(*h);
    }
    // The table is usable again afterwards. A cap that counted destroyed pools
    // would refuse for the rest of the boot
    let after = shm::create(OWNER, 4096);
    let recovered = after.is_ok();
    if let Ok(h) = after {
        let _ = shm::destroy(h);
    }

    // The refusal has to be this one. With the desktop up the table already
    // holds the wallpaper, the console grid and keymaps, and `Full` would only
    // report those pools
    let by_share = refusal == Some(shm::ShmError::OwnerFull);
    t.check(NAME, got == shm::MAX_POOLS_PER_OWNER && by_share && recovered,
            format!("held {} of a limit of {}, refused with {:?}, table usable after {}",
                    got, shm::MAX_POOLS_PER_OWNER, refusal, recovered));
}

/// A bind that fails leaves the interface free
///
/// The one-at-a-time interfaces are leases. `bind_new` refuses an id the client
/// already has live (id 1 is always `may_display`) and the refusal kills the
/// connection, which stays in the table until the process closes its socket. A
/// lease recorded before the object existed would stay held by that dead
/// client, and `may_wm` and `may_console` would be taken for the rest of the
/// boot by four words on the wire.
///
/// Two things prevent it, and the case needs both: the lease is taken after the
/// bind succeeds, and a lease belonging to a dead client does not count as
/// held. So the connection is left open afterwards
pub(super) fn test_a_failed_bind_does_not_take_the_interface(t: &mut Runner) {
    const NAME: &str = "access: a bind that fails does not take the interface with it";
    if t.needs(NAME, Interface::Wm) { return; }
    if t.needs(NAME, Interface::Console) { return; }
    let all = numbers();
    let name_of = |want: &str| all.iter().find(|(_, g)| g == want).map(|(n, _)| *n);
    let (Some(wn), Some(kn)) = (name_of("may_wm"), name_of("may_console")) else {
        t.check(NAME, false, "may_wm or may_console is not a global".to_string());
        return;
    };

    // The compositor's own client, because this is not about the filter: the
    // client is allowed the interface and blocks everybody else by failing to
    // take it
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let _ = exchange(&mut conn, cl);
    for (number, iface) in [(wn, Interface::Wm), (kn, Interface::Console)] {
        conn.raw(registry, protocol::registry::BIND, &[
            Arg::Uint(number), Arg::Str(Some(iface.name())), Arg::Uint(1),
            // may_display's id, which this connection has had since it opened
            Arg::NewId(protocol::DISPLAY_ID),
        ]);
    }
    let died = first_error(&exchange(&mut conn, cl)).is_some() && !server::is_alive(cl);
    // It stays connected, so nothing has swept it
    let free_while_it_sits_there = !server::wm_held() && !server::console_held();

    // Somebody else may still have them
    let taken = {
        let cl2 = server::connect(0);
        let mut c2 = ClientConn::new();
        let registry2 = c2.get_registry();
        let _ = exchange(&mut c2, cl2);
        let _wm = c2.bind(registry2, wn, Interface::Wm, 1);
        let console = c2.bind(registry2, kn, Interface::Console, 1);
        // Binding the global does not claim a terminal: which one is what
        // `may_console.bind` says, and holding one is what `console_held`
        // answers about
        c2.console_bind(console, 0);
        let ok = first_error(&exchange(&mut c2, cl2)).is_none()
                 && server::wm_held() && server::console_held();
        server::disconnect(cl2);
        ok
    };
    server::disconnect(cl);

    t.check(NAME, died && free_while_it_sits_there && taken,
            format!("the bad bind was fatal {}, neither interface was held by the \
                     dead connection {}, and a live client could still take both {}",
                    died, free_while_it_sits_there, taken));
}

/// A client that stops reading is disconnected
///
/// `sync` is the generator: twelve bytes in, twenty-four queued back (a `done`
/// and the `delete_id` that frees its own id). The recycled id matters, since a
/// client asking for a thousand registries would hit `MAX_OBJECTS` first and
/// die of that with the queue never full
pub(super) fn test_unread_events_end_the_connection(t: &mut Runner) {
    const NAME: &str = "access: a client that never reads its events is disconnected";
    let cl = server::connect(NO_SUCH_PID);
    let mut conn = ClientConn::new();

    // Past the ceiling: at 24 bytes queued each, a quarter of a megabyte is
    // about eleven thousand
    let mut submitted = 0usize;
    for _ in 0..20_000 {
        conn.sync();
        submitted += 1;
        // In batches, so the request buffer stays small while the queue that
        // is never drained grows. Nothing is ever read back
        if submitted % 500 == 0 {
            let bytes = conn.take();
            server::submit(cl, &bytes);
            if !server::is_alive(cl) {
                break;
            }
        }
    }

    // Never drained. The queue is capped, and the connection is dead of the
    // queue and not of anything else
    let queued = server::pending_bytes(cl);
    let dead = !server::is_alive(cl);
    let capped = queued <= crate::mayland::outq::MAX_QUEUED;
    let why = server::death(cl);
    let by_queue = matches!(why, Some(m) if m.contains("stopped reading"));
    server::disconnect(cl);

    t.check(NAME, dead && capped && by_queue,
            format!("killed {} after {} requests, because {:?}, queue held {} bytes \
                     against a ceiling of {}",
                    dead, submitted, why, queued, crate::mayland::outq::MAX_QUEUED));
}
