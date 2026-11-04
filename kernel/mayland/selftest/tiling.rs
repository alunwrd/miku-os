// Where the layout puts things
//
// The compositor places windows and a client cannot argue, so every case opens
// windows and measures the rectangles they were given. The two algorithms,
// master and dwindle, are tested against their own arithmetic and against what
// changes it underneath them: the screen resizing and the focus moving.

use super::*;

/// The compositor decides where a window goes, and a client cannot argue
///
/// A window that asks to be somewhere is ignored, a window never placed is
/// placed on its first commit, and two windows never open exactly on top of
/// each other
pub(super) fn test_placement(t: &mut Runner) {
    const NAME: &str = "geometry: the compositor places windows, not the client";
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
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

    // open_window pins its windows at the origin, so this one is opened by
    // hand to see what the compositor does when nobody has said anything
    let sa = {
        let surface = ca.create_surface(comp_a);
        let xs = ca.get_xdg_surface(xdg_a, surface);
        ca.get_toplevel(xs);
        let events = exchange(&mut ca, cl_a);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        ca.ack_configure(xs, serial);
        // Asks to be at 500,400 the only way this protocol lets a client ask:
        // the two numbers on attach. They are a buffer offset, and the window
        // must not end up there
        ca.attach(surface, buf_a, 500, 400);
        ca.damage(surface, 0, 0, w, h);
        ca.commit(surface);
        let _ = exchange(&mut ca, cl_a);
        surface
    };

    // Checked against the contract rather than the arithmetic: where a window
    // lands is a policy, but it is never where the client asked and never off
    // the screen
    //
    // Found by client and surface: surface ids are per connection, and two
    // clients that opened one window each both call it 11
    let at = |c: u32, s: u32| server::presentable().iter()
        .find(|p| p.client == c && p.surface == s).map(|p| p.offset);

    let (sw, sh) = server::output_size().unwrap_or((640, 480));
    let at_a = at(cl_a, sa);
    let ignored = at_a != Some((500, 400));
    let placed_a = at_a
        .map(|(x, y)| x >= 0 && y >= 0 && x + w <= sw as i32 && y + h <= sh as i32)
        .unwrap_or(false);

    // A second window does not open exactly on top of the first. Placed by
    // hand rather than through open_window, which pins what it opens
    let sb = {
        let surface = cb.create_surface(comp_b);
        let xs = cb.get_xdg_surface(xdg_b, surface);
        cb.get_toplevel(xs);
        let events = exchange(&mut cb, cl_b);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        cb.ack_configure(xs, serial);
        cb.attach(surface, buf_b, 0, 0);
        cb.damage(surface, 0, 0, w, h);
        cb.commit(surface);
        let _ = exchange(&mut cb, cl_b);
        surface
    };
    let apart = at(cl_b, sb) != at_a;

    // Nothing can be put where it could never be got hold of again
    layout::set_window_pos(cl_b, sb, -4000, -4000);
    let rescued = at(cl_b, sb)
        .map(|(x, y)| x + w > 0 && y >= 0).unwrap_or(false);

    t.check(NAME, ignored && placed_a && apart && rescued,
            format!("asked for 500,400 and got {:?}; on screen {}, \
                     second one apart {}, kept reachable at -4000,-4000 {}",
                    at_a, placed_a, apart, rescued));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
}

/// The layout measures the screen, not the console
///
/// A console window's framebuffer is a tile, and a work area measured with it
/// would shrink each time it shrank, each window making the next one smaller
pub(super) fn test_layout_follows_output(t: &mut Runner) {
    const NAME: &str = "geometry: the work area is the output's size, not the console's";
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;

    let was_layout = layout::kind();
    let was_output = server::output_size().unwrap_or((0, 0));
    // Not the console's size, and not a size anything else uses
    let (ow, oh) = (1234i32, 900i32);
    server::set_output(ow, oh, 60_000);
    layout::set_kind(layout::Kind::Master);

    let restore = |t: &mut Runner, why: &str| {
        layout::set_kind(was_layout);
        server::set_output(was_output.0, was_output.1, 60_000);
        t.check(NAME, false, why.to_string());
    };

    let Some((cl_a, mut ca, comp_a, xdg_a, ha, _buf_a)) =
        client_with_pool(bytes, w, h, stride)
    else { restore(t, "setup failed"); return };
    let Some((cl_b, mut cb, comp_b, xdg_b, hb, _buf_b)) =
        client_with_pool(bytes, w, h, stride)
    else {
        server::disconnect(cl_a);
        let _ = shm::destroy(ha);
        restore(t, "the second client could not be set up");
        return;
    };

    let size_in = |events: &[Event]| -> Option<(i32, i32)> {
        events.iter().rev().find_map(|e| match e {
            Event::ToplevelConfigure { width, height, .. } if *width > 0 =>
                Some((*width, *height)),
            _ => None,
        })
    };
    let open = |conn: &mut ClientConn, cl, comp, xdg| -> Option<(i32, i32)> {
        let surface = conn.create_surface(comp);
        let xs = conn.get_xdg_surface(xdg, surface);
        conn.get_toplevel(xs);
        size_in(&exchange(conn, cl))
    };

    let a_alone = open(&mut ca, cl_a, comp_a, xdg_a);
    let b = open(&mut cb, cl_b, comp_b, xdg_b);
    // The master's size now is not the one it was told when it opened: alone it
    // filled the work area, and the second window halved it. That configure
    // went to the first client after its opening handshake, so it is collected
    // on a round trip of its own
    let a = size_in(&exchange(&mut ca, cl_a)).or(a_alone);

    // Master and stack together are the work area: the screen less the outer
    // gap on both sides, less the inner gap between them
    let (gap_out, gap_in) = layout::gaps();
    let want_w = ow - 2 * gap_out - gap_in;
    let want_h = oh - 2 * gap_out;
    let ok = match (a, b) {
        (Some((aw, ah)), Some((bw, bh))) =>
            aw + bw == want_w && ah == want_h && bh == want_h,
        _ => false,
    };

    t.check(NAME, ok,
            format!("output {}x{}: master {:?}, stack {:?}, wanted widths summing \
                     to {} and both {} tall", ow, oh, a, b, want_w, want_h));

    server::disconnect(cl_a);
    server::disconnect(cl_b);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
    layout::set_kind(was_layout);
    server::set_output(was_output.0, was_output.1, 60_000);
}

/// Clicking a window does not rearrange the screen
///
/// `raise` runs on every click and focus change, so a layout ranking tiles by
/// stacking order would make the clicked window the master and move every other
/// one out from under the pointer
pub(super) fn test_tiles_stable_under_focus(t: &mut Runner) {
    const NAME: &str = "geometry: focusing a window does not rearrange the layout";
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
    if clients.len() < 3 {
        for (cl, _, _, _, hnd, _) in clients {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
        layout::set_kind(was_layout);
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    // Opened all the way to a committed buffer, because a surface with no
    // buffer is not presentable and the rectangles below would all be missing
    let size_in = |events: &[Event]| -> Option<(i32, i32)> {
        events.iter().rev().find_map(|e| match e {
            Event::ToplevelConfigure { width, height, .. } if *width > 0 =>
                Some((*width, *height)),
            _ => None,
        })
    };

    let mut ids = Vec::new();
    let mut sizes: Vec<Option<(i32, i32)>> = Vec::new();
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
        sizes.push(size_in(&events));
    }
    // Everything each client has been told so far is past. Windows opened
    // earlier were resized by later ones, so this is also where each one's
    // current size arrives
    for (i, (cl, conn, _, _, _, _)) in clients.iter_mut().enumerate() {
        if let Some(sz) = size_in(&exchange(conn, *cl)) {
            sizes[i] = Some(sz);
        }
    }

    let boxes = |ids: &[(u32, u32)]| -> Vec<(i32, i32, i32, i32)> {
        let list = server::presentable();
        ids.iter().map(|(c, s)| list.iter()
            .find(|p| p.client == *c && p.surface == *s)
            .map(|p| (p.offset.0, p.offset.1, p.width, p.height))
            .unwrap_or((-1, -1, -1, -1))).collect()
    };
    let before = boxes(&ids);

    // The oldest window: the master, and the one furthest from the top of the
    // stack
    server::focus_window(Some(ids[0].0), ids[0].1);
    let after = boxes(&ids);

    // And nobody was sent a new size
    //
    // Both the window losing the focus and the one taking it are configured,
    // because the activated state travels inside a configure. What must not
    // change is the size in it
    let resized = clients.iter_mut().enumerate().any(|(i, (cl, conn, _, _, _, _))| {
        exchange(conn, *cl).iter().any(|e| match e {
            Event::ToplevelConfigure { width, height, .. } if *width > 0 =>
                Some((*width, *height)) != sizes[i],
            _ => false,
        })
    });

    t.check(NAME, before == after && !resized,
            format!("before {:?}, after {:?}, sizes {:?}, resized {}",
                    before, after, sizes, resized));

    for (cl, _, _, _, hnd, _) in clients {
        server::disconnect(cl);
        let _ = shm::destroy(hnd);
    }
    layout::set_kind(was_layout);
}

/// The compositor decides how big a window is, and says so in configure
///
/// A tiled window is told a size it never asked for, two of them divide the
/// screen instead of overlapping, and closing one gives the other its space
/// back
pub(super) fn test_tiling(t: &mut Runner) {
    const NAME: &str = "geometry: a tiling layout sizes windows through configure";
    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let bytes = (stride * h) as usize;

    let was = layout::kind();
    layout::set_kind(layout::Kind::Master);

    let Some((cl_a, mut ca, comp_a, xdg_a, ha, buf_a)) =
        client_with_pool(bytes, w, h, stride)
    else { layout::set_kind(was); t.check(NAME, false, "setup failed".to_string()); return };
    let Some((cl_b, mut cb, comp_b, xdg_b, hb, buf_b)) =
        client_with_pool(bytes, w, h, stride)
    else {
        server::disconnect(cl_a);
        let _ = shm::destroy(ha);
        layout::set_kind(was);
        t.check(NAME, false, "the second client could not be set up".to_string());
        return;
    };

    // The size in the configure, not the size the client drew: `open` attaches
    // a 64x32 buffer and the layout ignores that.
    //
    // Read from the events the opening handshake produced: a configure is sent
    // once, when something changes, so it cannot be found by a later exchange
    let size_in = |events: &[Event]| -> Option<(i32, i32)> {
        events.iter().rev().find_map(|e| match e {
            Event::ToplevelConfigure { width, height, .. } if *width > 0 =>
                Some((*width, *height)),
            _ => None,
        })
    };
    let open = |conn: &mut ClientConn, cl, comp, xdg| -> Option<(i32, i32)> {
        let surface = conn.create_surface(comp);
        let xs = conn.get_xdg_surface(xdg, surface);
        conn.get_toplevel(xs);
        let events = exchange(conn, cl);
        let serial = events.iter().find_map(|e| match e {
            Event::XdgConfigure { serial } => Some(*serial), _ => None,
        }).unwrap_or(0);
        conn.ack_configure(xs, serial);
        size_in(&events)
    };

    let alone = open(&mut ca, cl_a, comp_a, xdg_a);
    let bs = open(&mut cb, cl_b, comp_b, xdg_b);
    // The second one appearing resizes the first, and that configure lands in
    // the first client's queue with nothing else to prompt it
    let shared = size_in(&exchange(&mut ca, cl_a));

    // One window fills the work area; two divide it and neither is the size it
    // drew itself
    let (sw, _) = server::output_size().unwrap_or((640, 480));
    let filled = alone.map(|(aw, _)| aw > sw as i32 / 2).unwrap_or(false);
    let divided = match (shared, bs) {
        (Some((aw, _)), Some((bw, _))) => aw < sw as i32 && bw < sw as i32
            && aw != w && bw != w,
        _ => false,
    };

    // And closing one gives its space back
    server::disconnect(cl_b);
    let after = size_in(&exchange(&mut ca, cl_a));
    let regrown = match (after, alone) { (Some(a), Some(b)) => a == b, _ => false };

    t.check(NAME, filled && divided && regrown,
            format!("alone {:?}, sharing {:?} and {:?}, after the other closed {:?}",
                    alone, shared, bs, after));

    server::disconnect(cl_a);
    let _ = shm::destroy(ha);
    let _ = shm::destroy(hb);
    layout::set_kind(was);
}

/// Dwindle divides the work area and loses nothing
///
/// Which window is where depends on opening order. What is checked is that the
/// boxes tile the area: none overlaps another, none hangs outside, and the
/// pixels they cover plus the gaps add up to the work area. Failing this shows
/// as a strip of stale video memory nobody repaints
pub(super) fn test_dwindle_tiles(t: &mut Runner) {
    const NAME: &str = "dwindle: the splits tile the work area, no overlap and nothing lost";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::reset_trees();
    layout::set_kind(layout::Kind::Dwindle);

    let (w, h, stride) = (64i32, 32i32, 64 * 4);
    let mut opened = Vec::new();
    for _ in 0..4 {
        match one_window() {
            Some(x) => opened.push(x),
            None => break,
        }
    }
    let _ = (w, h, stride);
    if opened.len() < 4 {
        for (cl, _, _, hnd, _) in opened {
            server::disconnect(cl);
            let _ = shm::destroy(hnd);
        }
        layout::set_kind(was);
        layout::reset_trees();
        t.check(NAME, false, "setup failed".to_string());
        return;
    }

    let area = layout::work_area_now();
    let gap = layout::gaps().1;
    let boxes: Vec<(i32, i32, i32, i32)> = opened.iter()
        .filter_map(|(cl, _, sf, _, _)| server::window_box(*cl, *sf))
        .collect();

    // Every box inside the work area, and none of them empty
    let inside = boxes.iter().all(|&(x, y, bw, bh)|
        bw > 0 && bh > 0 && x >= area.x && y >= area.y
        && x + bw <= area.x + area.w && y + bh <= area.y + area.h);

    // No two overlap
    let mut overlap = false;
    for i in 0..boxes.len() {
        for j in i + 1..boxes.len() {
            let (ax, ay, aw, ah) = boxes[i];
            let (bx, by, bw, bh) = boxes[j];
            if ax < bx + bw && bx < ax + aw && ay < by + bh && by < ay + ah {
                overlap = true;
            }
        }
    }

    // What they cover, plus the gaps, is the whole area. Counted as area rather
    // than rectangle by rectangle: the split points are integers and the halves
    // may differ by a pixel, but the total may not drift
    let covered: i64 = boxes.iter().map(|&(_, _, bw, bh)| bw as i64 * bh as i64).sum();
    let whole = area.w as i64 * area.h as i64;
    // Three splits between four windows, each eating one gap across the box it
    // divides. The bound is generous: the check is that nothing was lost, not
    // the exact arithmetic of the gaps
    let lost = whole - covered;
    let plausible = lost >= 0 && lost <= (gap as i64 + 1) * (area.w as i64 + area.h as i64);

    t.check(NAME, boxes.len() == 4 && inside && !overlap && plausible,
            format!("{} boxes in {:?}, inside {}, overlap {}, {} px of {} covered \
                     ({} px in gaps)",
                    boxes.len(), (area.x, area.y, area.w, area.h), inside, overlap,
                    covered, whole, lost));

    for (cl, _, _, hnd, _) in opened {
        server::disconnect(cl);
        let _ = shm::destroy(hnd);
    }
    layout::set_kind(was);
    layout::reset_trees();
}

/// A window that closes gives its space back to its sibling
pub(super) fn test_dwindle_collapses(t: &mut Runner) {
    const NAME: &str = "dwindle: closing a window gives its space to the one beside it";
    if t.needs(NAME, Interface::Wm) { return; }
    let was = layout::kind();
    layout::reset_trees();
    layout::set_kind(layout::Kind::Dwindle);

    let (a, b) = (one_window(), one_window());
    let (Some((cl_a, _ca, sa, ha, _)), Some((cl_b, _cb, sb, hb, _))) = (a, b) else {
        layout::set_kind(was);
        layout::reset_trees();
        t.check(NAME, false, "setup failed".to_string());
        return;
    };

    let area = layout::work_area_now();
    let split = server::window_box(cl_a, sa);
    // The second window closes, and the first should have the whole area back
    server::disconnect(cl_b);
    let _ = shm::destroy(hb);
    let whole = server::window_box(cl_a, sa);

    let ok = split.map(|(_, _, w, h)| w < area.w || h < area.h).unwrap_or(false)
             && whole == Some((area.x, area.y, area.w, area.h));

    t.check(NAME, ok,
            format!("shared {:?}, alone {:?}, work area {:?}",
                    split, whole, (area.x, area.y, area.w, area.h)));

    server::disconnect(cl_a);
    let _ = shm::destroy(ha);
    layout::set_kind(was);
    layout::reset_trees();
}
