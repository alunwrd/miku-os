// may_output_manager, may_output_configuration and the monitor lines: K3
//
// The window manager's half of the display layer (`dev/kms-architecture.md`,
// K3): Hyprland's `monitor =` lines read and turned into a mode on a real
// panel's list, the manager given only to the holder of `may_wm`, and a
// configuration's three answers (cancelled against an old serial, failed in
// check's words, succeeded), which arrive from a kernel task after the request.

use super::*;

/// `monitor =` lines, read as Hyprland reads them, and the mode each one means
/// on this machine's panel
pub(super) fn test_monitor_lines(t: &mut Runner) {
    use libmay::monitor::{self, By, Mode};
    let m = |width, height, refresh_mhz| Mode { width, height, refresh_mhz };
    // The panel's 1080p ladder, then a 720p mode
    let panel = [
        m(1920, 1080, 240_000), m(1920, 1080, 200_000), m(1920, 1080, 180_000),
        m(1920, 1080, 165_000), m(1920, 1080, 144_000), m(1920, 1080, 119_880),
        m(1920, 1080, 100_000), m(1920, 1080, 60_000), m(1280, 720, 59_930),
    ];
    let pick = |line: &str| monitor::parse(line).ok()
        .and_then(|r| monitor::pick(r.want, &panel, Some(0)));
    let picks = [
        pick("DP-3, 1920x1080@240, 0x0, 1"),
        pick("DP-3, 1920x1080@200, 0x0, 1"),
        pick("DP-3, 1920x1080, auto, 1"),
        pick("DP-3, highrr, auto, 1"),
        pick("DP-3, 1280x720@60, auto, 1"),
        pick("DP-3, preferred, auto, auto"),
        pick(", 1920x1080@119.88, auto, 1"),
    ];
    t.check(
        "monitor: @240 is mode 0, @200 mode 1, no @ means 60 Hz, highrr the fastest, 720p the 720p, \
         preferred the monitor's, 119.88 its own",
        picks == [Some(0), Some(1), Some(7), Some(0), Some(8), Some(0), Some(5)],
        format!("{:?}", picks),
    );

    let full = monitor::parse("DP-3, 1920x1080@240, 0x0, 1, vrr, 1, bitdepth, 10");
    let desc = monitor::parse("desc:VIE HTMS270F240, highres, auto, 1");
    let any = monitor::parse(", preferred, auto, 1");
    let off = monitor::parse("HDMI-A-1, disable");
    t.check(
        "monitor: vrr and bitdepth are read; desc: matches the description's start; \
         an empty name is every output; disable is a rule",
        full.as_ref().is_ok_and(|r| r.vrr == Some(1) && r.bitdepth == Some(10) && r.position == Some((0, 0)))
            && desc.as_ref().is_ok_and(|r| r.by == By::Description
                && r.matches("DP-3", "VIE HTMS270F240") && !r.matches("DP-3", "SAM TV"))
            && any.as_ref().is_ok_and(|r| r.by == By::Any && r.matches("Virtual-1", ""))
            && off.as_ref().is_ok_and(|r| r.disabled && r.matches("hdmi-a-1", "")),
        String::new(),
    );

    let refused = [
        "DP-3, 1920x1080, 0x0, 2",
        "DP-3, transform, 1",
        "DP-3, 1920x1080, 0x0, 1, mirror, DP-1",
        "DP-3, 1920x1080, 0x0, 1, vrr, 2",
        "DP-3, 1920x1080, 0x0, 1, bitdepth, 12",
        "DP-3, 1920x1080, 0x0, 1, cm, hdr",
        "DP-3, wide, auto, 1",
    ];
    let taken: Vec<&str> = refused.iter().copied().filter(|l| monitor::parse(l).is_ok()).collect();
    t.check(
        "monitor: scale 2, transform, mirror, vrr 2, bitdepth 12, cm and a word that is no mode are refused",
        taken.is_empty(),
        format!("taken: {:?}", taken),
    );
}

/// A client with `may_wm` and the output manager, and the events of the bind:
/// the whole state, ending in `done`
fn manager_client() -> Option<(u32, ClientConn, u32, Vec<Event>)> {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |want: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (Some(wn), Some(mn)) = (name_of("may_wm"), name_of("may_output_manager")) else {
        server::disconnect(cl);
        return None;
    };
    conn.bind(registry, wn, Interface::Wm, 1);
    let mgr = conn.bind(registry, mn, Interface::OutputManager, 1);
    let events = exchange(&mut conn, cl);
    if first_error(&events).is_some() {
        server::disconnect(cl);
        return None;
    }
    Some((cl, conn, mgr, events))
}

/// Waits for a configuration's answer, sent by a kernel task after the request
/// has been taken. Two seconds, a tick at a time
fn answer(conn: &mut ClientConn, cl: u32, config: u32) -> Option<Event> {
    for _ in 0..500 {
        let found = exchange(conn, cl).into_iter().find(|e| matches!(e,
            Event::OutConfigSucceeded { config: c }
            | Event::OutConfigFailed { config: c, .. }
            | Event::OutConfigCancelled { config: c } if *c == config));
        if found.is_some() {
            return found;
        }
        crate::sched::sleep(1);
    }
    None
}

/// The manager is the window manager's alone, and tells everything there is
pub(super) fn test_output_manager(t: &mut Runner) {
    const NAME: &str = "outputs: only may_wm's holder gets the manager; it is told every head, \
                        its modes, and done with the layer's serial";
    if t.needs(NAME, Interface::Wm) { return; }

    // Without `may_wm` the bind is refused, fatally, as every refused bind is
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let mn = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_output_manager" => Some(*name),
        _ => None,
    });
    let refused = match mn {
        Some(mn) => {
            conn.bind(registry, mn, Interface::OutputManager, 1);
            first_error(&exchange(&mut conn, cl)).is_some()
        }
        None => false,
    };
    server::disconnect(cl);

    let Some((cl, _conn, _mgr, events)) = manager_client() else {
        t.check(NAME, false, "the window manager could not bind the manager".to_string());
        return;
    };
    let heads: Vec<&String> = events.iter().filter_map(|e| match e {
        Event::OutHead { name, .. } => Some(name),
        _ => None,
    }).collect();
    let modes = events.iter().filter(|e| matches!(e, Event::OutMode { .. })).count();
    let serial = events.iter().find_map(|e| match e {
        Event::OutDone { serial } => Some(*serial),
        _ => None,
    });
    let st = crate::drivers::gpu::kms::state();
    let want_modes: usize = st.outputs.iter().map(|o| o.monitor.as_ref().map_or(0, |m| m.modes.len())).sum();
    t.check(
        NAME,
        refused
            && heads.len() == st.outputs.len()
            && st.outputs.iter().all(|o| heads.iter().any(|h| **h == o.name))
            && modes == want_modes
            && serial == Some(crate::drivers::gpu::kms::events::serial()),
        format!("refused {} heads {:?} modes {}/{} serial {:?}", refused, heads, modes, want_modes, serial),
    );
    server::disconnect(cl);
}

/// `may_output` version 2 names the output it is; version 1 gets the events it
/// always did
pub(super) fn test_output_name(t: &mut Runner) {
    const NAME: &str = "outputs: may_output 2 says the connector's name, version 1 is not told one";
    let st = crate::drivers::gpu::kms::state();
    let Some(primary) = st.outputs.iter().find(|o| o.primary).map(|o| o.name.clone()) else {
        t.skip(NAME, "no output is the compositor's".to_string());
        return;
    };
    let named = |version: u32| -> Option<Vec<Event>> {
        let cl = server::connect(0);
        let mut conn = ClientConn::new();
        let registry = conn.get_registry();
        let events = exchange(&mut conn, cl);
        let on = events.iter().find_map(|e| match e {
            Event::Global { name, interface, .. } if interface == "may_output" => Some(*name),
            _ => None,
        })?;
        conn.bind(registry, on, Interface::Output, version);
        let events = exchange(&mut conn, cl);
        server::disconnect(cl);
        Some(events)
    };
    let name_in = |evs: &Option<Vec<Event>>| evs.as_ref().and_then(|e| e.iter().find_map(|e| match e {
        Event::OutputName { name } => Some(name.clone()),
        _ => None,
    }));
    let (v2, v1) = (named(2), named(1));
    let done = |evs: &Option<Vec<Event>>| evs.as_ref().is_some_and(|e| e.iter().any(|e| matches!(e, Event::OutputDone)));
    t.check(
        NAME,
        name_in(&v2).as_deref() == Some(primary.as_str()) && name_in(&v1).is_none() && done(&v1) && done(&v2),
        format!("v2 {:?} v1 {:?}", name_in(&v2), name_in(&v1)),
    );
}

/// A configuration's three answers, each from the kernel task that ran it
pub(super) fn test_output_configuration(t: &mut Runner) {
    const NAME: &str = "outputs: an old serial is cancelled, letting go of a head and an output that \
                        is not there fail in check's words, the running mode tested succeeds";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, mgr, events)) = manager_client() else {
        t.check(NAME, false, "the window manager could not bind the manager".to_string());
        return;
    };
    let serial = events.iter().find_map(|e| match e {
        Event::OutDone { serial } => Some(*serial),
        _ => None,
    }).unwrap_or(0);
    // The output the compositor draws on, and the mode it runs
    let head = events.iter().find_map(|e| match e {
        Event::OutHead { name, flags, current, .. }
            if flags & protocol::output_manager::ENABLED != 0 => Some((name.clone(), *current)),
        _ => None,
    });
    let Some((name, current)) = head else {
        t.skip(NAME, "no output is lit".to_string());
        server::disconnect(cl);
        return;
    };
    use protocol::output_configuration::KEEP;

    let stale = conn.outputs_configure(mgr, serial.wrapping_sub(1));
    conn.output_set(stale, &name, KEEP, KEEP, 0);
    conn.output_test(stale);
    let cancelled = matches!(answer(&mut conn, cl, stale), Some(Event::OutConfigCancelled { .. }));

    let release = conn.outputs_configure(mgr, serial);
    conn.output_disable(release, &name);
    conn.output_test(release);
    let released = answer(&mut conn, cl, release);
    let release_failed = matches!(&released, Some(Event::OutConfigFailed { reason, .. }) if reason.contains("let go"));

    let nowhere = conn.outputs_configure(mgr, serial);
    conn.output_set(nowhere, "Nowhere-9", KEEP, KEEP, 0);
    conn.output_test(nowhere);
    let missing = answer(&mut conn, cl, nowhere);
    let missing_failed = matches!(&missing, Some(Event::OutConfigFailed { reason, .. }) if reason.contains("no output called"));

    let same = conn.outputs_configure(mgr, serial);
    conn.output_set(same, &name, current, KEEP, 0);
    conn.output_test(same);
    let same_ok = matches!(answer(&mut conn, cl, same), Some(Event::OutConfigSucceeded { .. }));

    t.check(
        NAME,
        cancelled && release_failed && missing_failed && same_ok,
        format!("cancelled {} release {:?} missing {:?} same {}", cancelled, released, missing, same_ok),
    );
    server::disconnect(cl);
}
