// may_desktop and may_wm: the facts, and the policy
//
// The two interfaces a shell speaks. `may_desktop` is what is on the screen and
// what the user asked for, and any number of clients may hold it; `may_wm` is
// the desktop's policy, and exactly one client may. A window manager binds both
// and a panel only the first.
//
// Also what crosses them: the window list, the moves and closes, the params,
// the chords, the workspaces, and the version rule: a client that bound version
// 1 of `may_desktop` is told a version 1 window.

use super::*;

/// Binds both halves of a window manager, or fails the case and returns
///
/// A client, the registry, and the two globals a shell speaks. Cases about a
/// panel bind `may_desktop` alone
pub(super) fn wm_client() -> Option<(u32, ClientConn, u32, u32, u32, u32)> {
    wm_client_at(protocol::Interface::Desktop.version())
}

/// The same, binding a `may_desktop` version of our choosing, for the case that
/// checks an old shell is still spoken to in its own version
pub(super) fn wm_client_at(version: u32) -> Option<(u32, ClientConn, u32, u32, u32, u32)> {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |want: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (Some(dn), Some(wn), Some(cn), Some(xn)) =
        (name_of("may_desktop"), name_of("may_wm"),
         name_of("may_compositor"), name_of("may_xdg"))
    else { server::disconnect(cl); return None };
    let desk = conn.bind(registry, dn, Interface::Desktop, version);
    let wm = conn.bind(registry, wn, Interface::Wm, 1);
    let comp = conn.bind(registry, cn, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xn, Interface::Xdg, 1);
    if first_error(&exchange(&mut conn, cl)).is_some() {
        server::disconnect(cl);
        return None;
    }
    Some((cl, conn, desk, wm, comp, xdg))
}

/// A panel binds the facts; one client binds the policy, and only one
///
/// Holding `may_wm` is commanding it, so a panel can draw a workspace pill
/// without having `bind_chord` and `set_layout`, and a panel that connects
/// earlier cannot take the channel
pub(super) fn test_wm_is_exclusive(t: &mut Runner) {
    const NAME: &str = "wm: one client holds the policy, any number hold the facts";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl_a, mut ca, _desk_a, wm_a, _, _)) = wm_client() else {
        t.check(NAME, false, "the window manager could not bind".to_string());
        return;
    };
    // Holding it is commanding. There is no seat to claim afterwards, and the
    // first request means the same as the second
    ca.wm_set_layout(wm_a, protocol::wm::LAYOUT_MASTER);
    let drives = first_error(&exchange(&mut ca, cl_a)).is_none();

    // A second window manager is refused where it asks, not where it acts.
    // `wm_client` binds may_wm as part of its handshake, so this failing is the
    // refusal, and it is fatal, as a protocol error is
    let second = wm_client();
    let refused_second = second.is_none();
    if let Some((cl, _, _, _, _, _)) = second {
        server::disconnect(cl);
    }

    // A panel binds the facts alone and is given them
    let Some((cl_b, mut cb, desk_b)) = panel_client() else {
        server::disconnect(cl_a);
        t.check(NAME, false, "the panel could not bind may_desktop".to_string());
        return;
    };

    // What a user asks for it may send, because a panel has to be clickable:
    // pressing a workspace on the bar switches to it
    let was_ws = server::workspace();
    let want = if was_ws == 1 { 2 } else { 1 };
    cb.desk_workspace(desk_b, want);
    let asked = first_error(&exchange(&mut cb, cl_b)).is_none()
                && server::workspace() == want;
    workspace::set_workspace(was_ws);

    // Events reach it without its having asked for anything
    let win = one_window();
    let heard = exchange(&mut cb, cl_b).iter()
        .any(|e| matches!(e, Event::DeskOpened { .. }));

    // The policy is not reachable from here. The panel holds a may_desktop
    // object only, so a policy opcode sent to the only id it has is an opcode
    // that interface does not have, and the connection dies for it
    cb.raw(desk_b, protocol::wm::SET_BORDER,
           &[Arg::Uint(4), Arg::Uint(0xFF0000), Arg::Uint(0x00FF00)]);
    let no_policy = first_error(&exchange(&mut cb, cl_b)).is_some();

    // The interface is free again when its holder goes
    server::disconnect(cl_a);
    let freed = !server::wm_held();

    if let Some((cl_w, _, _, hw, _)) = win {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
    }
    server::disconnect(cl_b);

    t.check(NAME, drives && refused_second && asked && heard && no_policy && freed,
            format!("the holder drove the desktop {}, a second was refused {}, \
                     a panel switched a workspace {}, heard a window open {}, \
                     has no policy object at all {}, freed when the holder went {}",
                    drives, refused_second, asked, heard, no_policy, freed));
}

/// A panel: `may_desktop` and nothing else
///
/// What the shell binds. Returns the connection and the desktop object
fn panel_client() -> Option<(u32, ClientConn, u32)> {
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let Some(name) = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_desktop" => Some(*name),
        _ => None,
    }) else {
        // The connection is already open here, so `?` would leave it behind,
        // and a leaked connection is a client the next case finds on screen
        server::disconnect(cl);
        return None;
    };
    let desk = conn.bind(registry, name, Interface::Desktop,
                         protocol::Interface::Desktop.version());
    if first_error(&exchange(&mut conn, cl)).is_some() {
        server::disconnect(cl);
        return None;
    }
    Some((cl, conn, desk))
}

/// What the window manager is told is what is on the screen
pub(super) fn test_wm_lists_what_is_on_screen(t: &mut Runner) {
    const NAME: &str = "wm: the window list is the screen, box for box";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };

    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;
    let mut clients = Vec::new();
    for _ in 0..2 {
        match client_with_pool(bytes, w, h, stride) {
            Some(c) => clients.push(c),
            None => break,
        }
    }
    let cleanup = |clients: Vec<(u32, ClientConn, u32, u32, u32, u32)>| {
        for (cl, _, _, _, hnd, _) in clients {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
    };
    if clients.len() < 2 {
        cleanup(clients);
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    let mut ids = Vec::new();
    for (cl, conn, comp, xdg, _, buf) in clients.iter_mut() {
        let surface = conn.create_surface(*comp);
        let xs = conn.get_xdg_surface(*xdg, surface);
        conn.get_toplevel(xs);
        let events = exchange(conn, *cl);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        conn.ack_configure(xs, serial);
        conn.attach(surface, *buf, 0, 0);
        conn.damage(surface, 0, 0, w, h);
        conn.commit(surface);
        let _ = exchange(conn, *cl);
        ids.push((*cl, surface));
    }

    // Opening a window is told, not only asked about
    let opened = exchange(&mut wm_conn, cl_wm).iter()
        .filter(|e| matches!(e, Event::DeskOpened { .. })).count();

    wm_conn.desk_get_windows(desk, 77);
    let events = exchange(&mut wm_conn, cl_wm);
    let listed: Vec<(u32, u32, i32, i32)> = events.iter().filter_map(|e| match e {
        Event::DeskWindow { client, surface, x, y, .. } => Some((*client, *surface, *x, *y)),
        _ => None,
    }).collect();
    let done = events.iter().any(|e| matches!(e, Event::DeskWindowsDone { serial: 77 }));

    // Every window the compositor would present is in the list, at the same
    // place. The window manager aims at the boxes it was told about here, so
    // the two must agree
    let on_screen = server::presentable();
    let agrees = on_screen.iter().all(|p| listed.iter()
        .any(|(c, s, x, y)| *c == p.client && *s == p.surface
                            && (*x, *y) == (p.offset.0, p.offset.1)));

    t.check(NAME, done && agrees && opened >= 2 && listed.len() >= on_screen.len(),
            format!("{} listed against {} on screen, agree {}, {} open events, \
                     the list ended {}",
                    listed.len(), on_screen.len(), agrees, opened, done));

    cleanup(clients);
    server::disconnect(cl_wm);
    layout::set_kind(was);
}


/// A window is listed where it sits on its workspace, not where a switch has
/// slid it
///
/// A switch slides the windows it leaves off the screen and brings the others
/// in from off it. Hyprland's `at` is the window's place, so this is too, and
/// end4's overview draws each terminal in its own workspace
pub(super) fn test_wm_lists_places_across_a_switch(t: &mut Runner) {
    const NAME: &str = "wm: a window is listed where it sits, not where a switch slid it";
    use crate::mayland::shell::anim;
    use protocol::wm::anim_class as class;
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl_wm, mut wm_conn, desk, _wm, _, _)) = wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        server::disconnect(cl_wm);
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };
    let saved = (anim::enabled(), anim::own_spec(class::WORKSPACES_OUT), anim::own_spec(class::WORKSPACES_IN));
    let slide = anim::Spec { enabled: true, ms: 100, curve: 0, style: anim::style::SLIDE, arg: 1.0 };
    anim::set_spec(class::WORKSPACES_OUT, slide);
    anim::set_spec(class::WORKSPACES_IN, slide);

    let mut place = || -> Option<(i32, i32)> {
        wm_conn.desk_get_windows(desk, 9);
        exchange(&mut wm_conn, cl_wm).iter().find_map(|e| match e {
            Event::DeskWindow { client, surface, x, y, .. } if (*client, *surface) == (cl_w, sw) => Some((*x, *y)),
            _ => None,
        })
    };
    let was_ws = server::workspace();
    let before = place();
    workspace::set_workspace(if was_ws == 1 { 2 } else { 1 });
    // Every curve to its end: the window is off the screen now
    layout::tick(u64::MAX / 4);
    let away = place();
    workspace::set_workspace(was_ws);
    // On its way back in, from off the screen
    let arriving = place();
    layout::tick(u64::MAX / 4);
    let back = place();

    anim::set_spec(class::WORKSPACES_OUT, saved.1);
    anim::set_spec(class::WORKSPACES_IN, saved.2);
    anim::set_enabled(saved.0);
    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
    server::disconnect(cl_wm);

    t.check(NAME, before.is_some() && away == before && arriving == before && back == before,
            format!("listed at {:?}, away {:?}, arriving {:?}, back {:?}", before, away, arriving, back));
}

/// A window the layout makes smaller is told of, unasked
///
/// A shell knows where a window is from the list it asked for, and the layout
/// moves windows unasked: a second one opening takes half of the first's room.
/// end4's overview needs this to draw each window in its cell
pub(super) fn test_desktop_hears_what_the_layout_moved(t: &mut Runner) {
    const NAME: &str = "desktop: a window the layout resizes is told of without asking";
    if t.needs(NAME, Interface::Wm) { return; }
    let was_layout = layout::kind();
    layout::set_kind(layout::Kind::Master);
    let Some((cl_wm, mut wm_conn, desk, _wm, _, _)) = wm_client() else {
        layout::set_kind(was_layout);
        t.check(NAME, false, "may_desktop could not be bound".to_string());
        return;
    };
    let Some((cl_a, _ca, sa, ha, _)) = one_window() else {
        server::disconnect(cl_wm);
        layout::set_kind(was_layout);
        t.check(NAME, false, "the first window could not be set up".to_string());
        return;
    };
    let width_of = |events: &[Event]| events.iter().rev().find_map(|e| match e {
        Event::DeskWindow { client, surface, width, .. } if (*client, *surface) == (cl_a, sa) => Some(*width),
        _ => None,
    });
    wm_conn.desk_get_windows(desk, 7);
    let alone = width_of(&exchange(&mut wm_conn, cl_wm));
    let second = one_window();
    // Nothing asked: what the first window's listener hears by itself
    let after = width_of(&exchange(&mut wm_conn, cl_wm));

    if let Some((cl_b, _cb, _sb, hb, _)) = second {
        server::disconnect(cl_b);
        let _ = shm::destroy(hb);
    }
    server::disconnect(cl_a);
    let _ = shm::destroy(ha);
    server::disconnect(cl_wm);
    layout::set_kind(was_layout);

    let ok = matches!((alone, after), (Some(a), Some(b)) if b < a);
    t.check(NAME, ok, format!("alone {alone:?} wide, told {after:?} once the second opened"));
}

/// A window's pixels are copied whole into the asker's own pool
///
/// What end4's overview shows in a workspace's cell. Copied as committed (rows
/// of the window's width, wherever its own rows were), at the offset asked, and
/// only when they fit: the answer names the size, so a too-small pool can be
/// grown and asked again, and a window that is not there has no size
pub(super) fn test_desktop_captures_a_window(t: &mut Runner) {
    const NAME: &str = "desktop: a window's pixels are copied into the asker's pool";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl_wm, mut wm_conn, desk, _wm, _, _)) = wm_client() else {
        t.check(NAME, false, "may_desktop could not be bound".to_string());
        return;
    };
    let Some((cl_w, _cw, sw, hw, _)) = one_window() else {
        server::disconnect(cl_wm);
        t.check(NAME, false, "the window could not be set up".to_string());
        return;
    };
    // one_window's: 64 by 32, rows 256 bytes apart. Each pixel says where it is
    let (w, h) = (64usize, 32usize);
    let _ = shm::with_bytes_mut(hw, 0, w * h * 4, |b| {
        for y in 0..h {
            for x in 0..w {
                let px = 0xFF00_005Au32 | (x as u32) << 16 | (y as u32) << 8;
                b[(y * w + x) * 4..][..4].copy_from_slice(&px.to_le_bytes());
            }
        }
    });
    // The asker's pool: room for the window 16 bytes in, and not 17
    let size = 16 + w * h * 4;
    let mine = shm::create(0, size).ok();
    let registry = wm_conn.get_registry();
    let shm_name = exchange(&mut wm_conn, cl_wm).iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_shm" => Some(*name),
        _ => None,
    });
    let (Some(mine), Some(shm_name)) = (mine, shm_name) else {
        server::disconnect(cl_w);
        let _ = shm::destroy(hw);
        server::disconnect(cl_wm);
        t.check(NAME, false, "no pool for the asker".to_string());
        return;
    };
    let shm_obj = wm_conn.bind(registry, shm_name, Interface::Shm, 1);
    let pool = wm_conn.create_pool(shm_obj, mine, size as i32);

    let mut ask = |client: u32, surface: u32, offset: u32, serial: u32| {
        wm_conn.desk_capture(desk, client, surface, pool, offset, serial);
        exchange(&mut wm_conn, cl_wm).iter().find_map(|e| match e {
            Event::DeskCaptured { serial: s, copied, width, height, .. } if *s == serial =>
                Some((*copied, *width, *height)),
            _ => None,
        })
    };
    let too_far = ask(cl_w, sw, 17, 1);
    let fits = ask(cl_w, sw, 16, 2);
    let nobody = ask(cl_w, sw + 1000, 0, 3);
    // One pool at a time: the table's lock is not taken twice
    let got = shm::with_bytes(mine, 16, w * h * 4, |b| b.to_vec());
    let want = shm::with_bytes(hw, 0, w * h * 4, |b| b.to_vec());
    let same = match (got, want) {
        (Ok(g), Ok(wn)) => Ok(g == wn),
        (Err(e), _) | (_, Err(e)) => Err(e),
    };

    server::disconnect(cl_w);
    let _ = shm::destroy(hw);
    server::disconnect(cl_wm);
    let _ = shm::destroy(mine);

    let ok = too_far == Some((false, 64, 32)) && fits == Some((true, 64, 32)) && nobody == Some((false, 0, 0))
        && same == Ok(true);
    t.check(NAME, ok, format!("17 in {too_far:?}, 16 in {fits:?}, no window {nobody:?}, pixels the same {same:?}"));
}

/// One number set for the desktop, and the same number set for one window
///
/// The param channel keeps each new knob from being a new request, so the case
/// checks that a number reaches two different owners through one opcode
pub(super) fn test_wm_params(t: &mut Runner) {
    const NAME: &str = "wm: one channel sets the desktop's numbers and one window's";
    if t.needs(NAME, Interface::Wm) { return; }
    let gaps_was = layout::gaps();
    let fx_was = render::effects();

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let Some((cl, _conn, surface, handle, _)) = one_window() else {
        server::disconnect(cl_wm);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };

    use protocol::wm::param;
    // The layout owns this one
    wm_conn.wm_set_param(wm, param::GAPS_OUT, 21.0);
    // And the compositor owns this one, through the same request
    wm_conn.wm_set_param(wm, param::ALPHA, 0.5);
    // This one is about a single window, which is what a window rule is
    wm_conn.wm_set_window_param(wm, cl, surface, param::ALPHA, 0.25, true);
    // A param neither of them has is ignored rather than fatal: a config
    // written for a newer compositor still has to put a desktop on the screen
    wm_conn.wm_set_param(wm, 9999, 1.0);
    let err = first_error(&exchange(&mut wm_conn, cl_wm));

    let gaps = layout::gaps();
    let desktop = render::effects();
    let mine = server::window_effects(cl, surface);
    let ok = err.is_none()
        && gaps.0 == 21
        && (desktop.alpha - 0.5).abs() < 0.01
        && mine.map(|e| (e.alpha - 0.25).abs() < 0.01).unwrap_or(false);

    // And unset gives the window back to the desktop
    wm_conn.wm_set_window_param(wm, cl, surface, param::ALPHA, 0.0, false);
    let _ = exchange(&mut wm_conn, cl_wm);
    let back = server::window_effects(cl, surface)
        .map(|e| (e.alpha - 0.5).abs() < 0.01).unwrap_or(false);

    t.check(NAME, ok && back,
            format!("gaps {:?}, desktop alpha {:.2}, this window {:?}, \
                     inherits again after unset {}, unknown param {}",
                    gaps, desktop.alpha, mine.map(|e| e.alpha), back,
                    match &err { Some((_, why)) => why.clone(),
                                 None => "ignored".to_string() }));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
    server::disconnect(cl_wm);
    layout::set_gaps(gaps_was.0, gaps_was.1);
    render::set_param(param::ALPHA, fx_was.alpha);
}

/// Floating leaves the tiling; fullscreen fills the work area and gives it back
pub(super) fn test_wm_window_state(t: &mut Runner) {
    const NAME: &str = "wm: floating leaves the tiling, fullscreen fills the work area";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let Some((cl, _conn, surface, handle, _)) = one_window() else {
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    let _ = exchange(&mut wm_conn, cl_wm);

    // Tiled: the one window on the workspace fills the work area
    let area = layout::work_area_now();
    let tiled = server::window_box(cl, surface);
    let tiled_ok = tiled == Some((area.x, area.y, area.w, area.h));

    use protocol::wm::state;
    wm_conn.wm_set_window_state(wm, cl, surface, state::FLOATING, state::FLOATING);
    let _ = exchange(&mut wm_conn, cl_wm);
    // Out of the layout: it keeps the size it had rather than being told a
    // new one, and nothing else on the workspace was told anything either
    let floating_flagged = server::window_flags(cl, surface)
        .map(|f| f & protocol::wm::FLAG_FLOATING != 0)
        .unwrap_or(false);

    wm_conn.wm_set_window_state(wm, cl, surface, state::FULLSCREEN, state::FULLSCREEN);
    let _ = exchange(&mut wm_conn, cl_wm);
    let full = layout::work_area_now_floating();
    let fs = server::window_box(cl, surface);
    let fs_ok = fs == Some((full.x, full.y, full.w, full.h));

    wm_conn.wm_set_window_state(wm, cl, surface, state::FULLSCREEN, 0);
    let _ = exchange(&mut wm_conn, cl_wm);
    let after = server::window_flags(cl, surface)
        .map(|f| f & protocol::wm::FLAG_FULLSCREEN == 0)
        .unwrap_or(false);

    t.check(NAME, tiled_ok && floating_flagged && fs_ok && after,
            format!("tiled {:?} against work area {:?}, floating {}, \
                     fullscreen {:?} against {:?}, left it {}",
                    tiled, (area.x, area.y, area.w, area.h), floating_flagged,
                    fs, (full.x, full.y, full.w, full.h), after));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
    server::disconnect(cl_wm);
    layout::set_kind(was);
}

/// The work area arrives unasked, and follows the gap that shapes it
///
/// It is a fact about the screen, so it crosses `may_desktop`: a panel has to
/// know the rectangle it reserves out of, and holds nothing else. The gap that
/// moves it is policy, so it crosses `may_wm`. A number set on one interface is
/// heard on the other
pub(super) fn test_wm_work_area(t: &mut Runner) {
    const NAME: &str = "wm: the work area is sent on bind and follows the gaps";
    if t.needs(NAME, Interface::Wm) { return; }
    let gaps_was = layout::gaps();

    // Bound here rather than through `wm_client`: the events the bind itself
    // owes are under test, and the helper has already read them
    let cl_wm = server::connect(0);
    let mut wm_conn = ClientConn::new();
    let registry = wm_conn.get_registry();
    let globals = exchange(&mut wm_conn, cl_wm);
    let name_of = |want: &str| globals.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == want => Some(*name),
        _ => None,
    });
    let (Some(dn), Some(wn)) = (name_of("may_desktop"), name_of("may_wm")) else {
        server::disconnect(cl_wm);
        t.check(NAME, false, "may_desktop or may_wm is not advertised".to_string());
        return;
    };
    // The desktop first: the work area the bind owes goes to it, and a may_wm
    // bound first would have nothing to send it to
    let _desk = wm_conn.bind(registry, dn, Interface::Desktop,
                             Interface::Desktop.version());
    let on_bind = exchange(&mut wm_conn, cl_wm).iter().find_map(|e| match e {
        Event::DeskWorkArea { x, y, width, height, .. } => Some((*x, *y, *width, *height)),
        _ => None,
    });
    let wm = wm_conn.bind(registry, wn, Interface::Wm, Interface::Wm.version());
    let _ = exchange(&mut wm_conn, cl_wm);

    use protocol::wm::param;
    wm_conn.wm_set_param(wm, param::GAPS_OUT, 40.0);
    let after = exchange(&mut wm_conn, cl_wm).iter().rev().find_map(|e| match e {
        Event::DeskWorkArea { x, y, width, height, .. } => Some((*x, *y, *width, *height)),
        _ => None,
    });

    let (sw, sh) = server::output_size().unwrap_or((0, 0));
    let expected = (40, 40, sw - 80, sh - 80);
    let ok = on_bind.is_some() && after == Some(expected);

    t.check(NAME, ok,
            format!("on bind {:?}, after a 40 px outer gap {:?}, expected {:?}",
                    on_bind, after, expected));

    server::disconnect(cl_wm);
    layout::set_gaps(gaps_was.0, gaps_was.1);
}

/// A window manager built for version 1 is still spoken to in its version
pub(super) fn test_wm_version_1_window(t: &mut Runner) {
    const NAME: &str = "wm: a version 1 window manager is told a version 1 window";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Float);

    // Version 1 first, and the message measured on the wire rather than
    // decoded: the decoder in this tree only knows version 2
    let len_at = |version: u32| -> Option<usize> {
        let (cl_wm, mut wm_conn, desk, _wm, _, _) = wm_client_at(version)?;
        let (cl, _conn, _surface, handle, _) = match one_window() {
            Some(x) => x,
            None => { server::disconnect(cl_wm); return None }
        };
        let _ = server::drain(cl_wm);
        wm_conn.desk_get_windows(desk, 1);
        server::submit(cl_wm, &wm_conn.take());
        let bytes = server::drain(cl_wm);
        let mut at = 0usize;
        let mut found = None;
        while let Ok(Some(h)) = wire::peek_header(&bytes[at..]) {
            // On the desktop object, which carries the facts. `desktop::WINDOW`
            // and `wm::MOVE` are both opcode 0, so the wrong object would not
            // be a compile error
            if h.opcode == protocol::desktop::WINDOW && h.sender == desk {
                found = Some(h.size);
                break;
            }
            at += h.size;
        }
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        server::disconnect(cl_wm);
        found
    };

    let v1 = len_at(1);
    let v2 = len_at(2);
    // One string longer. An empty string is eight bytes on the wire, not
    // four: the length word counts the terminator, and the terminator is
    // padded to a word of its own
    let ok = matches!((v1, v2), (Some(a), Some(b)) if b == a + 8);

    t.check(NAME, ok,
            format!("version 1 window event {:?} bytes, version 2 {:?}", v1, v2));
    layout::set_kind(was);
}

/// Two windows trade places, and the layout follows the trade
pub(super) fn test_wm_swap(t: &mut Runner) {
    const NAME: &str = "wm: two windows trade places in the layout";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let (a, b) = (one_window(), one_window());
    let (Some((cl_a, _ca, sa, ha, _)), Some((cl_b, _cb, sb, hb, _))) = (a, b) else {
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    let _ = exchange(&mut wm_conn, cl_wm);

    let before = (server::window_box(cl_a, sa), server::window_box(cl_b, sb));
    wm_conn.wm_swap(wm, cl_a, sa, cl_b, sb);
    let err = first_error(&exchange(&mut wm_conn, cl_wm));
    let after = (server::window_box(cl_a, sa), server::window_box(cl_b, sb));

    // The master's box is now the other window's, and the two are still the
    // same two boxes: a swap that lost a rectangle would be a resize
    let ok = err.is_none() && before.0 == after.1 && before.1 == after.0
             && before.0 != before.1;

    t.check(NAME, ok, format!("before {:?}, after {:?}", before, after));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
    server::disconnect(cl_wm);
    layout::set_kind(was);
}

/// The window manager moves the focus and asks a window to close
pub(super) fn test_wm_moves_and_closes(t: &mut Runner) {
    const NAME: &str = "wm: focus, move, and a close the client is asked for";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Float);

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let Some((cl, mut conn, comp, xdg, handle, buf)) =
        client_with_pool((stride * h) as usize, w, h, stride)
    else {
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };

    let surface = conn.create_surface(comp);
    let xs = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xs);
    let events = exchange(&mut conn, cl);
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs, serial);
    conn.attach(surface, buf, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    let _ = exchange(&mut wm_conn, cl_wm);

    // Moved to a place nothing would have put it
    wm_conn.wm_move(wm, cl, surface, 300, 200);
    let _ = exchange(&mut wm_conn, cl_wm);
    let moved = server::presentable().iter()
        .find(|p| p.client == cl && p.surface == surface)
        .map(|p| p.offset) == Some((300, 200));

    // Focused, and the window manager hears about it.
    //
    // A second window first, because a new window takes the keyboard: asking
    // for the focus already there is not a change, and there is no event for a
    // state that did not move
    let second = conn.create_surface(comp);
    let xs2 = conn.get_xdg_surface(xdg, second);
    conn.get_toplevel(xs2);
    let events = exchange(&mut conn, cl);
    let serial2 = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs2, serial2);
    conn.attach(second, buf, 0, 0);
    conn.damage(second, 0, 0, w, h);
    conn.commit(second);
    let _ = exchange(&mut conn, cl);
    let _ = exchange(&mut wm_conn, cl_wm);

    wm_conn.desk_focus(desk, cl, surface);
    let told = exchange(&mut wm_conn, cl_wm).iter().any(|e| matches!(
        e, Event::DeskFocused { client, surface: s } if *client == cl && *s == surface));
    let focused = server::focus_key() == Some((cl, surface));

    // Closing is a request to the client, not a window taken away behind its
    // back: the window is still there afterwards, and the client has been asked
    wm_conn.desk_close(desk, cl, surface);
    let _ = exchange(&mut wm_conn, cl_wm);
    let asked = exchange(&mut conn, cl).iter()
        .any(|e| matches!(e, Event::ToplevelClose));
    let still_there = server::presentable().iter()
        .any(|p| p.client == cl && p.surface == surface);

    t.check(NAME, moved && told && focused && asked && still_there,
            format!("moved {}, focus told {} and taken {}, close asked for {}, \
                     window still there until the client agrees {}",
                    moved, told, focused, asked, still_there));

    server::disconnect(cl);
    server::disconnect(cl_wm);
    let _ = shm::destroy(handle);
    layout::set_kind(was);
}

/// `may_wm.reset` puts the desktop back to what it starts as
///
/// A config file is re-read and a line has been deleted. Every request carrying
/// a number is a change, not a description, so without this the desktop would
/// be in the state of the old config plus the new one.
///
/// Five places keep their own defaults, so five things are checked: the
/// layout's gaps, the paint effects, the ring around a window, the animation
/// table, and the numbers about input
pub(super) fn test_wm_reset(t: &mut Runner) {
    use crate::mayland::protocol::wm::param;
    const NAME: &str = "wm: reset puts every desktop-wide number back to its default";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl, mut conn, _desk, wm, _, _)) = wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };

    conn.wm_set_param(wm, param::GAPS_IN, 40.0);
    conn.wm_set_param(wm, param::ROUNDING, 33.0);
    conn.wm_set_param(wm, param::FOLLOW_MOUSE, 1.0);
    conn.wm_set_border(wm, 5, 0x112233, 0x445566);
    conn.wm_set_curve(wm, 0, 0.9, 0.1, 0.9, 0.1);
    conn.wm_set_animation(wm, crate::mayland::protocol::wm::anim_class::WINDOWS,
                          true, 4.0, 0, crate::mayland::protocol::wm::anim_style::FADE, 1.0);
    let _ = exchange(&mut conn, cl);

    let changed = layout::gaps().1 == 40
        && crate::mayland::render::effects::effects().corner == 33.0
        && crate::mayland::render::effects::border().size == 5
        && crate::mayland::shell::anim::enabled()
        && crate::mayland::input::conf::follow_mouse();

    conn.wm_reset(wm);
    let _ = exchange(&mut conn, cl);

    let gaps = layout::gaps();
    let fx = crate::mayland::render::effects::effects();
    let border = crate::mayland::render::effects::border();
    let back = gaps == (16, 12)
        && fx.corner == crate::mayland::render::effects::Effects::DEFAULT.corner
        && border.size == 0
        && !crate::mayland::shell::anim::enabled()
        && !crate::mayland::input::conf::follow_mouse();

    server::disconnect(cl);
    t.check(NAME, changed && back,
            format!("all five took the change {}; after reset gaps {:?}, \
                     rounding {}, border {}, animations on {}, follow_mouse {}",
                    changed, gaps, fx.corner, border.size,
                    crate::mayland::shell::anim::enabled(),
                    crate::mayland::input::conf::follow_mouse()));
}

/// A bound chord arrives as one event, and the compositor's own are not the
/// window manager's to take
pub(super) fn test_wm_chords(t: &mut Runner) {
    const NAME: &str = "wm: a bound chord is delivered once, unbound keys are not";
    if t.needs(NAME, Interface::Wm) { return; }
    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };

    // KEY_2 under Super, which is what a workspace chord looks like
    const KEY_2: u32 = 3;
    const KEY_3: u32 = 4;
    wm_conn.wm_bind_chord(wm, 5, input::MOD_LOGO, KEY_2, 0);
    let _ = exchange(&mut wm_conn, cl_wm);
    let bound = input::chords_bound() >= 1;

    // Fired the way the keyboard fires it, through the same queue
    server::wm_chord(5);
    let events = exchange(&mut wm_conn, cl_wm);
    let fired: Vec<u32> = events.iter().filter_map(|e| match e {
        Event::WmChord { id } => Some(*id), _ => None,
    }).collect();

    // And gone when it is given back
    wm_conn.wm_unbind_chord(wm, 5);
    let _ = exchange(&mut wm_conn, cl_wm);
    let cleared = input::chords_bound();

    // An id nobody bound is not a chord
    wm_conn.wm_bind_chord(wm, 6, input::MOD_LOGO, KEY_3, 0);
    let _ = exchange(&mut wm_conn, cl_wm);
    // The lease going away takes the chords with it, or a dead window manager
    // leaves the Super key swallowed
    server::disconnect(cl_wm);
    let after_death = input::chords_bound();

    t.check(NAME, bound && fired == [5] && cleared == 0 && after_death == 0,
            format!("bound {}, delivered {:?}, left after unbind {}, \
                     left after the window manager died {}",
                    bound, fired, cleared, after_death));
}

/// A workspace is which windows are on the screen, and the console is on all
pub(super) fn test_workspaces(t: &mut Runner) {
    const NAME: &str = "wm: a workspace hides windows, and never the sticky ones";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    let was_ws = server::workspace();
    layout::set_kind(layout::Kind::Float);

    let Some((cl_wm, mut wm_conn, desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;
    let mut clients = Vec::new();
    for _ in 0..2 {
        match client_with_pool(bytes, w, h, stride) {
            Some(c) => clients.push(c),
            None => break,
        }
    }
    if clients.len() < 2 {
        for (cl, _, _, _, hnd, _) in clients {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    let mut ids = Vec::new();
    for (cl, conn, comp, xdg, _, buf) in clients.iter_mut() {
        let surface = conn.create_surface(*comp);
        let xs = conn.get_xdg_surface(*xdg, surface);
        conn.get_toplevel(xs);
        let events = exchange(conn, *cl);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        conn.ack_configure(xs, serial);
        conn.attach(surface, *buf, 0, 0);
        conn.damage(surface, 0, 0, w, h);
        conn.commit(surface);
        let _ = exchange(conn, *cl);
        ids.push((*cl, surface));
    }
    let here = |k: (u32, u32)| server::presentable().iter()
        .any(|p| p.client == k.0 && p.surface == k.1);

    // One of them is made sticky, the way the console is
    workspace::set_sticky(ids[0].0, ids[0].1, true);

    let both_before = here(ids[0]) && here(ids[1]);
    let ws = server::workspace();
    let other = if ws == 1 { 2 } else { 1 };

    wm_conn.desk_workspace(desk, other);
    let _ = exchange(&mut wm_conn, cl_wm);
    let switched = server::workspace() == other;
    // The sticky one is still there; the other one is not
    let after = (here(ids[0]), here(ids[1]));

    // A sticky window cannot be sent away, and the console is one
    wm_conn.desk_move_to_workspace(desk, ids[0].0, ids[0].1, other);
    let refused = first_error(&exchange(&mut wm_conn, cl_wm)).is_some();

    // Back again, and the hidden window comes back where it was
    layout::set_kind(layout::Kind::Float);
    let mut wm_conn2 = ClientConn::new();
    let _ = &mut wm_conn2;
    let restored = {
        // The window manager was killed by the refusal above, so the switch
        // back goes through the same call the compositor uses
        workspace::set_workspace(ws);
        here(ids[1])
    };

    t.check(NAME,
            both_before && switched && after == (true, false) && refused && restored,
            format!("both on screen first {}, switched {}, sticky and ordinary \
                     after the switch {:?}, sticky refused a move {}, came back {}",
                    both_before, switched, after, refused, restored));

    for (cl, _, _, _, hnd, _) in clients {
        server::disconnect(cl);
        let _ = shm::destroy(hnd);
    }
    server::disconnect(cl_wm);
    workspace::set_workspace(was_ws);
    layout::set_kind(was);
}

/// The keyboard follows the workspace, not the slide
///
/// A switch takes the keyboard from a window nobody can see, and a window sent
/// to another workspace gives it up. `stack` keeps a window still sliding off
/// the screen, so `next_focusable` has to skip it, or the keys go to the
/// workspace just left
pub(super) fn test_workspace_focus(t: &mut Runner) {
    const NAME: &str = "wm: the keyboard follows a window to its workspace";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    let was_ws = server::workspace();
    layout::set_kind(layout::Kind::Float);

    let Some((cl_wm, mut wm_conn, desk, _wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;
    let mut clients = Vec::new();
    for _ in 0..2 {
        match client_with_pool(bytes, w, h, stride) {
            Some(c) => clients.push(c),
            None => break,
        }
    }
    if clients.len() < 2 {
        for (cl, _, _, _, hnd, _) in clients {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    let mut ids = Vec::new();
    for (cl, conn, comp, xdg, _, buf) in clients.iter_mut() {
        let surface = conn.create_surface(*comp);
        let xs = conn.get_xdg_surface(*xdg, surface);
        conn.get_toplevel(xs);
        let events = exchange(conn, *cl);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        conn.ack_configure(xs, serial);
        conn.attach(surface, *buf, 0, 0);
        conn.damage(surface, 0, 0, w, h);
        conn.commit(surface);
        let _ = exchange(conn, *cl);
        ids.push((*cl, surface));
    }
    let (stay, go) = (ids[0], ids[1]);
    let ws = server::workspace();
    let other = if ws == 1 { 2 } else { 1 };

    // The one that goes has the keyboard when it is sent: the focused window,
    // which is what `movetoworkspace` moves
    wm_conn.desk_focus(desk, go.0, go.1);
    let _ = exchange(&mut wm_conn, cl_wm);
    wm_conn.desk_move_to_workspace(desk, go.0, go.1, other);
    let _ = exchange(&mut wm_conn, cl_wm);
    let after_move = server::focus_key();

    wm_conn.desk_workspace(desk, other);
    let _ = exchange(&mut wm_conn, cl_wm);
    let after_switch = server::focus_key();

    t.check(NAME,
            after_move == Some(stay) && after_switch == Some(go),
            format!("after the move {:?} (wanted {:?}), after the switch {:?} (wanted {:?})",
                    after_move, stay, after_switch, go));

    for (cl, _, _, _, hnd, _) in clients {
        server::disconnect(cl);
        let _ = shm::destroy(hnd);
    }
    server::disconnect(cl_wm);
    workspace::set_workspace(was_ws);
    layout::set_kind(was);
}

/// A window carried by the pointer, and one stretched by a corner
///
/// The grab is the mechanism behind `bindm = ALT, mouse:272, movewindow`.
/// Checked, short of the mouse: a shell can ask for a window to be picked up,
/// the compositor picks up the right one, a resize pulls the edge it was told
/// to and leaves the others, and a closing window lets go
pub(super) fn test_pointer_grab(t: &mut Runner) {
    const NAME: &str = "wm: a window can be carried and stretched by the pointer";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::set_kind(layout::Kind::Float);

    let Some((cl_wm, mut wm_conn, _desk, wm, _, _)) = wm_client() else {
        layout::set_kind(was);
        t.check(NAME, false, "may_wm could not be bound".to_string());
        return;
    };
    let Some((cl, _conn, surface, handle, _)) = one_window() else {
        server::disconnect(cl_wm);
        layout::set_kind(was);
        t.check(NAME, false, "no window to grab".to_string());
        return;
    };
    // A known box to measure against, rather than wherever the layout put it
    layout::set_window_pos(cl, surface, 200, 150);
    wm_conn.wm_resize(wm, cl, surface, 400, 300);
    let _ = exchange(&mut wm_conn, cl_wm);

    // Named rather than `0, 0`: there is no real pointer over it here, and the
    // grab is under test, not the hit test
    wm_conn.wm_begin_move(wm, cl, surface);
    let held = first_error(&exchange(&mut wm_conn, cl_wm)).is_none()
               && crate::mayland::input::grab::dragging() == Some((cl, surface));

    // Stretching the bottom right corner: the origin must not move, because the
    // edges not held stay
    let before = server::window_box(cl, surface);
    wm_conn.wm_begin_resize(wm, cl, surface,
                            protocol::wm::edge::BOTTOM | protocol::wm::edge::RIGHT);
    let _ = exchange(&mut wm_conn, cl_wm);
    let resizing = crate::mayland::input::grab::dragging() == Some((cl, surface));

    // An edge this compositor does not have is a protocol error, not an ignored
    // bit
    wm_conn.wm_begin_resize(wm, cl, surface, 0x80);
    let refused = first_error(&exchange(&mut wm_conn, cl_wm)).is_some();

    // The window goes and the grab goes with it. Left behind, it would swallow
    // every motion event until the next button press
    server::disconnect(cl);
    let _ = shm::destroy(handle);
    let let_go = crate::mayland::input::grab::dragging().is_none();

    server::disconnect(cl_wm);
    crate::mayland::input::grab::end_drag();
    layout::set_kind(was);

    t.check(NAME, held && resizing && refused && let_go,
            format!("picked up {}, took a corner {} (was at {:?}), refused an edge \
                     it does not have {}, let go when the window closed {}",
                    held, resizing, before, refused, let_go));
}
