// Roles, and what a surface may do while it has one
//
// A surface with no role is pixels nobody has claimed, and Wayland will not
// show one. The cases: taking a role, showing content before agreeing a size,
// two windows and who is active, and the backdrop, which is a window except
// that it is never in a layout or in front of anything.

use super::*;

/// Pixels nobody has claimed do not reach the screen
pub(super) fn test_no_role(t: &mut Runner) {
    let (w, h, stride) = (32i32, 32i32, 128i32);
    let Some((cl, mut conn, comp, _xdg, handle, buffer)) =
        client_with_pool((stride * h) as usize, w, h, stride)
    else {
        t.check("xdg: a surface with no role is not presented", false,
                "setup failed".to_string());
        return;
    };

    let before = mine(&[cl]).len();
    let surface = conn.create_surface(comp);
    conn.attach(surface, buffer, 0, 0);
    conn.commit(surface);
    let events = exchange(&mut conn, cl);
    let after = mine(&[cl]).len();

    t.check("xdg: a surface with no role is not presented",
            after == before && first_error(&events).is_none(),
            format!("{} presentable before, {} after, no error {}",
                    before, after, first_error(&events).is_none()));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A window may not show pixels for a size it never agreed to
pub(super) fn test_unconfigured_buffer(t: &mut Runner) {
    let (w, h, stride) = (32i32, 32i32, 128i32);
    let Some((cl, mut conn, comp, xdg, handle, buffer)) =
        client_with_pool((stride * h) as usize, w, h, stride)
    else {
        t.check("xdg: a buffer before ack_configure is refused", false,
                "setup failed".to_string());
        return;
    };

    let surface = conn.create_surface(comp);
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xdg_surface);
    conn.attach(surface, buffer, 0, 0);
    conn.commit(surface); // deliberately without acknowledging the configure
    let events = exchange(&mut conn, cl);

    let err = first_error(&events);
    t.check("xdg: a buffer before ack_configure is refused", err.is_some(),
            match &err { Some((_, why)) => why.clone(), None => "ACCEPTED".to_string() });

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// Two windows at once: who is on top, who is being typed at, and whether the
/// one that stopped being typed at was told so
///
/// Neither client binds a keyboard. Activation is carried in configure and not
/// in the input events, so a client that never asks for a keyboard still learns
/// it is no longer in front
pub(super) fn test_two_windows(t: &mut Runner) {
    const NAME_STACK: &str = "xdg: a second window opens on top and takes the keyboard";
    const NAME_DEACT: &str = "xdg: the window losing focus is told it is no longer active";
    const NAME_RAISE: &str = "xdg: focusing a window raises it and cycling comes back";

    let (w, h, stride) = (32i32, 32i32, 128i32);
    let bytes = (stride * h) as usize;

    let Some((cl_a, mut ca, comp_a, xdg_a, ha, buf_a)) =
        client_with_pool(bytes, w, h, stride)
    else {
        t.check(NAME_STACK, false, "setup failed".to_string());
        return;
    };
    let Some((cl_b, mut cb, comp_b, xdg_b, hb, buf_b)) =
        client_with_pool(bytes, w, h, stride)
    else {
        server::disconnect(cl_a);
        let _ = shm::destroy(ha);
        t.check(NAME_STACK, false, "the second client could not be set up".to_string());
        return;
    };

    let (sa, active_a) = open_window(&mut ca, cl_a, comp_a, xdg_a, "one", buf_a, w, h);
    let (sb, active_b) = open_window(&mut cb, cl_b, comp_b, xdg_b, "two", buf_b, w, h);

    // Both are on screen, the newer one last, and it has the keyboard
    let stack = mine(&[cl_a, cl_b]);
    let order_ok = stack.len() == 2 && stack[0].surface == sa && stack[1].surface == sb;
    let focus_ok = server::focus().map(|(c, s, _)| (c, s)) == Some((cl_b, sb));
    t.check(NAME_STACK, order_ok && focus_ok && active_a && active_b,
            format!("{} presentable, order {}, focus on the newer {}, each opened active {}",
                    stack.len(), order_ok, focus_ok, active_a && active_b));

    // The first window was told it is no longer the active one
    let a_states = active_states(&ca, cl_a);
    t.check(NAME_DEACT, a_states.last() == Some(&false),
            format!("configures since: {:?}", a_states));

    // Focus the first window back: it rises above the second, both are told,
    // and cycling from there returns to the second
    let refocused = server::focus_window(Some(cl_a), sa);
    let a_now = active_states(&ca, cl_a);
    let b_now = active_states(&cb, cl_b);
    let stack = mine(&[cl_a, cl_b]);
    let raised = stack.len() == 2 && stack[0].surface == sb && stack[1].surface == sa;
    let cycled = server::focus_next() == Some((cl_b, sb));

    t.check(NAME_RAISE,
            refocused && raised && a_now.last() == Some(&true)
                && b_now.last() == Some(&false) && cycled,
            format!("raised {}, one active {:?}, two active {:?}, cycle back {}",
                    raised, a_now.last(), b_now.last(), cycled));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// A window can be put behind the others and stay there
///
/// The console is this: the backdrop, which must not come to the front because
/// it was created last
pub(super) fn test_lower(t: &mut Runner) {
    const NAME: &str = "xdg: a lowered window stays behind the one in front";
    let (w, h, stride) = (32i32, 32i32, 128i32);
    let bytes = (stride * h) as usize;

    let Some((cl_a, mut ca, comp_a, xdg_a, ha, buf_a)) =
        client_with_pool(bytes, w, h, stride)
    else { t.check(NAME, false, "setup failed".to_string()); return };
    let Some((cl_b, mut cb, comp_b, xdg_b, hb, buf_b)) =
        client_with_pool(bytes, w, h, stride)
    else {
        server::disconnect(cl_a);
        let _ = shm::destroy(ha);
        t.check(NAME, false, "the second client could not be set up".to_string());
        return;
    };

    let (sa, _) = open_window(&mut ca, cl_a, comp_a, xdg_a, "front", buf_a, w, h);
    let (sb, _) = open_window(&mut cb, cl_b, comp_b, xdg_b, "back", buf_b, w, h);
    // B is newest, so it is in front. Send it to the back
    let lowered = server::lower_window(cl_b, sb);
    let stack = mine(&[cl_a, cl_b]);
    let order_ok = stack.len() == 2 && stack[0].surface == sb && stack[1].surface == sa;

    // Focus is separate from stacking, and lowering does not touch it, but
    // focusing something else must not bring the lowered one back
    server::focus_window(Some(cl_a), sa);
    let stack = mine(&[cl_a, cl_b]);
    let still = stack.len() == 2 && stack[0].surface == sb && stack[1].surface == sa;

    t.check(NAME, lowered && order_ok && still,
            format!("lowered {}, behind {}, still behind after a focus change {}",
                    lowered, order_ok, still));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// The backdrop is the screen, not a window in the layout
///
/// It can stop being one: a console that was the backdrop can be tiled again
pub(super) fn test_backdrop_excluded_from_layout(t: &mut Runner) {
    const NAME: &str = "geometry: the backdrop is outside the layout, and can leave it";
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;

    let was_layout = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let mut clients = Vec::new();
    for _ in 0..3 {
        match client_with_pool(bytes, w, h, stride) {
            Some(c) => clients.push(c),
            None => break,
        }
    }
    let cleanup = |clients: Vec<(u32, ClientConn, u32, u32, u32, u32)>, was| {
        for (cl, _, _, _, hnd, _) in clients {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
        layout::set_kind(was);
    };
    if clients.len() < 3 {
        cleanup(clients, was_layout);
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    let mut ids = Vec::new();
    for (i, (cl, conn, comp, xdg, _, buf)) in clients.iter_mut().enumerate() {
        let surface = conn.create_surface(*comp);
        // The backdrop says so before it takes a role, as the console does: a
        // surface without a role is not in the layout yet, so it is never tiled
        // even for one frame
        if i == 0 {
            let bytes = conn.take();
            server::submit(*cl, &bytes);
            let _ = server::drain(*cl);
            server::lower_window(*cl, surface);
        }
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

    let at = |c: u32, s: u32| server::presentable().iter()
        .find(|p| p.client == c && p.surface == s).map(|p| p.offset);
    let (sw, sh) = server::output_size().unwrap_or((640, 480));

    // The two ordinary windows divide the whole work area between them, as if
    // the backdrop were not there, and the backdrop is at the origin
    let back_at = at(ids[0].0, ids[0].1);
    let master = at(ids[1].0, ids[1].1);
    let stack = at(ids[2].0, ids[2].1);
    let (gap_out, _) = layout::gaps();
    let split = master == Some((gap_out, gap_out))
        && stack.map(|(x, y)| x > gap_out && y == gap_out).unwrap_or(false);

    // Nothing ever proposed a size for it
    let sized = clients.iter_mut().take(1).any(|(cl, conn, _, _, _, _)| {
        exchange(conn, *cl).iter().any(|e| matches!(
            e, Event::ToplevelConfigure { width, .. } if *width > 0))
    });

    // Back into the layout: three windows now, so the master is half wide
    server::set_backdrop(ids[0].0, ids[0].1, false);
    let rejoined = at(ids[0].0, ids[0].1)
        .map(|(x, y)| x >= gap_out && y >= gap_out
                      && x < sw as i32 && y < sh as i32)
        .unwrap_or(false);
    let moved = clients.iter_mut().any(|(cl, conn, _, _, _, _)| {
        exchange(conn, *cl).iter().any(|e| matches!(
            e, Event::ToplevelConfigure { width, .. } if *width > 0))
    });

    t.check(NAME, split && !sized && rejoined && moved,
            format!("backdrop at {:?}, master {:?}, stack {:?}, never sized {}, \
                     back in the layout at {:?} {}",
                    back_at, master, stack, !sized, at(ids[0].0, ids[0].1), moved));

    cleanup(clients, was_layout);
}

/// The stack keeps a window that is still leaving, even when its workspace is
/// not the one being looked at
pub(super) fn test_stack_keeps_a_leaver(t: &mut Runner) {
    const NAME: &str = "anim: a window still sliding away is still drawn";
    use crate::mayland::shell::anim;

    let Some((cl, _c, surface, handle, _)) = one_window() else {
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    layout::arrange_now();

    // Put it on another workspace by hand and give it a curve, which is what a
    // slide leaves behind
    let was_ws = server::workspace();
    workspace::move_to_workspace_for_test(cl, surface, 9);
    let hidden = mine(&[cl]).len();
    layout::begin_anim_for_test(cl, surface, (0, 0), layout::Rect::new(-500, 0, 64, 32),
                                layout::now_us(), 200, 0, anim::style::SLIDE);
    let while_leaving = mine(&[cl]).len();
    layout::tick(layout::now_us() + 10_000_000);
    let after = mine(&[cl]).len();

    workspace::set_workspace(was_ws);
    server::disconnect(cl);
    let _ = shm::destroy(handle);

    t.check(NAME, hidden == 0 && while_leaving == 1 && after == 0,
            format!("on another workspace and not animating {} on screen, \
                     while its curve runs {}, after it ends {}",
                    hidden, while_leaving, after));
}
