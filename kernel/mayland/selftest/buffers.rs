// Pools, buffers, and when a buffer goes back
//
// A buffer is busy from the commit that makes it current until a later commit
// takes its place. The compositor reads a current buffer again on every frame
// with damage over it, so a client told it was free after the frame that copied
// it would repaint pixels the next frame is about to read. A client with two
// buffers alternating catches both mistakes: too early tears, too late stalls.
//
// The pool checks are security cases: a buffer running past its pool, and a
// handle belonging to another process.

use super::*;

/// The lifetime rule from `server::surface`, exercised end to end
pub(super) fn test_buffer_lifecycle(t: &mut Runner) {
    let (w, h) = (64i32, 32i32);
    let stride = w * 4;
    // Two buffers' worth: the rule is about one taking the place of another,
    // and one buffer cannot show that
    let one = (stride * h) as usize;
    let bytes = one * 2;

    let handle = match shm::create(0, bytes) {
        Ok(hh) => hh,
        Err(e) => {
            t.check("shm: pool allocation", false, format!("{:?}", e));
            return;
        }
    };

    // Paints the buffer so it is not merely allocated, and checks that the
    // physical address the compositor would hand the copy engine holds it
    let _ = shm::with_bytes_mut(handle, 0, bytes, |b| {
        for px in b.chunks_exact_mut(4) {
            px.copy_from_slice(&0xff30_60c0u32.to_le_bytes());
        }
    });

    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |iface: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == iface => Some(*name),
        _ => None,
    });

    let (shm_name, comp_name, xdg_name) = match (
        name_of("may_shm"), name_of("may_compositor"), name_of("may_xdg"),
    ) {
        (Some(a), Some(b), Some(c)) => (a, b, c),
        _ => {
            t.check("surface: commit and release", false, "no globals".to_string());
            server::disconnect(cl);
            let _ = shm::destroy(handle);
            return;
        }
    };

    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    let comp = conn.bind(registry, comp_name, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xdg_name, Interface::Xdg, 1);
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    let buffer = conn.create_buffer(pool, 0, w, h, stride, protocol::shm::FORMAT_XRGB8888);
    let second = conn.create_buffer(pool, one as i32, w, h, stride,
                                    protocol::shm::FORMAT_XRGB8888);
    let surface = conn.create_surface(comp);

    // A surface has to be told it is a window before it can be shown, and a
    // window has to agree its size before it can show pixels
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    let toplevel = conn.get_toplevel(xdg_surface);
    conn.set_title(toplevel, "selftest");
    let events = exchange(&mut conn, cl);
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    });
    let configured = events.iter().any(|e| matches!(e, Event::ToplevelConfigure { .. }));
    t.check("xdg: a new toplevel is configured and takes the keyboard",
            serial.is_some() && configured && server::focus().is_some(),
            format!("serial {:?}, toplevel configure {}, focus {}",
                    serial, configured, server::focus().is_some()));
    conn.ack_configure(xdg_surface, serial.unwrap_or(0));

    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    let frame_cb = conn.frame(surface);
    conn.commit(surface);
    let events = exchange(&mut conn, cl);

    if let Some((obj, why)) = first_error(&events) {
        t.check("surface: commit and release", false,
                format!("object {} - {}", obj, why));
        server::disconnect(cl);
        let _ = shm::destroy(handle);
        return;
    }

    // After commit and before any frame, the compositor holds the buffer and
    // the callback has not fired. A release here would hand back memory it is
    // about to read
    let early_release = events.iter().any(|e| matches!(e, Event::BufferRelease { .. }));
    let early_done = events.iter().any(|e| matches!(e, Event::Done { id, .. } if *id == frame_cb));

    let visible = mine(&[cl]);
    let seen = visible.iter().find(|p| p.client == cl && p.surface == surface);
    let geometry_ok = seen.map(|p| p.width == w && p.height == h && p.stride == stride as usize)
                          .unwrap_or(false);
    let phys_ok = seen.map(|p| p.phys == shm::phys(handle).unwrap_or(0)).unwrap_or(false);

    t.check("surface: a committed buffer is presentable and not yet released",
            !early_release && !early_done && geometry_ok && phys_ok,
            format!("{} presentable, geometry {}, phys {}",
                    visible.len(), geometry_ok, phys_ok));

    // The frame happens. The callback fires, but the buffer does not come back:
    // it is still what the screen shows, and the next frame copies out of it
    // again for whatever the pointer uncovers
    server::frame_done(1234);
    let reply = server::drain(cl);
    let (events, _) = conn.decode(&reply);
    let early = events.iter().any(|e| matches!(e, Event::BufferRelease { id } if *id == buffer));
    let done = events.iter().any(|e| matches!(e, Event::Done { id, data }
                                             if *id == frame_cb && *data == 1234));

    t.check("surface: the frame fires the callback and keeps the buffer",
            !early && done, format!("released early {}, done {}", early, done));

    // A second frame with nothing new committed must be silent
    server::frame_done(1250);
    let quiet = server::drain(cl).is_empty();
    t.check("surface: an idle frame sends nothing", quiet,
            if quiet { "silent".to_string() } else { "sent events".to_string() });

    // The client draws the next frame into its other buffer. That commit takes
    // the first one's place, and the frame after it hands it back, when nothing
    // is reading it
    conn.attach(surface, second, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    let showing_second = mine(&[cl]).iter()
        .find(|p| p.client == cl && p.surface == surface)
        .map(|p| p.phys == shm::phys(handle).unwrap_or(0) + one as u64)
        .unwrap_or(false);

    server::frame_done(1266);
    let reply = server::drain(cl);
    let (events, _) = conn.decode(&reply);
    let released = events.iter().any(|e| matches!(e, Event::BufferRelease { id } if *id == buffer));
    let kept = !events.iter().any(|e| matches!(e, Event::BufferRelease { id } if *id == second));

    t.check("surface: a commit hands the buffer it replaced back, and only that one",
            showing_second && released && kept,
            format!("showing second {}, first released {}, second kept {}",
                    showing_second, released, kept));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A pool can be memory the kernel already has, and destroying it frees none
/// of it. This is how the console lends its shadow to the protocol
pub(super) fn test_adopted_pool(t: &mut Runner) {
    const NAME: &str = "shm: a pool can borrow memory the kernel already owns";
    const LEN: usize = 4096;
    static mut BORROWED: [u8; LEN] = [0; LEN];

    let ptr = core::ptr::addr_of_mut!(BORROWED) as *mut u8;
    let phys = crate::arch::x86_64::grub::any_virt_to_phys(ptr as u64);

    let handle = match unsafe { shm::adopt(0, phys, ptr, LEN) } {
        Ok(h) => h,
        Err(e) => { t.check(NAME, false, format!("{:?}", e)); return; }
    };

    // The pool reports the memory it was given, and writing through it writes
    // there and nowhere else
    let phys_ok = shm::phys(handle) == Ok(phys);
    let _ = shm::with_bytes_mut(handle, 0, 8, |b| b.copy_from_slice(&[7u8; 8]));
    let wrote = unsafe { core::ptr::read_volatile(ptr) } == 7;

    // Nothing may map kernel memory into a process, whoever asks
    let refused = shm::map_into(handle, 0, 0x1000).is_err();

    // Destroying it leaves the memory alone: it was never the pool's
    let _ = shm::destroy(handle);
    let survived = unsafe { core::ptr::read_volatile(ptr) } == 7;
    let gone = shm::phys(handle).is_err();

    t.check(NAME, phys_ok && wrote && refused && survived && gone,
            format!("phys {}, wrote through {}, mapping refused {}, memory survived {}",
                    phys_ok, wrote, refused, survived));
}

/// What the client declared damaged reaches the compositor, and the geometry
/// done on it is right
///
/// Tested here because it is arithmetic: a wrong intersection is a wrong copy,
/// which on a panel looks like a driver bug
pub(super) fn test_damage(t: &mut Runner) {
    use crate::mayland::render::Rect;

    // the arithmetic
    let screen = Rect::new(0, 0, 100, 100);
    let inside = Rect::new(10, 10, 20, 20);
    let straddling = Rect::new(90, 90, 20, 20);
    let outside = Rect::new(200, 200, 10, 10);

    let clipped = straddling.intersect(&screen);
    let geometry_ok = inside.intersect(&screen) == Some(inside)
        && clipped == Some(Rect { x0: 90, y0: 90, x1: 100, y1: 100 })
        && outside.intersect(&screen).is_none()
        && inside.intersect(&outside).is_none()
        // Touching edge to edge is not overlapping
        && Rect::new(0, 0, 10, 10).intersect(&Rect::new(10, 0, 10, 10)).is_none()
        && inside.area() == 400
        && clipped.map(|r| r.area()) == Some(100);

    t.check("compositor: rectangles clip, miss and measure correctly", geometry_ok,
            format!("{:?} clipped to {:?}", straddling, clipped));

    // A window that moves a little produces where-it-was and where-it-is, which
    // must come out as one copy: the second copy costs a round trip whether or
    // not it overlaps the first
    let was = Rect::new(100, 100, 160, 120);
    let is = Rect::new(107, 105, 160, 120);
    let both = was.union(&is);
    let worth_merging = both.area() <= was.area() + is.area() + 200_000;
    // Opposite corners of a large screen are not merged: the union is most of
    // the screen and neither rectangle is near it
    let far_a = Rect::new(0, 0, 160, 120);
    let far_b = Rect::new(1700, 900, 160, 120);
    let keep_apart = far_a.union(&far_b).area()
        > far_a.area() + far_b.area() + 200_000;

    t.check("compositor: a moving window is one rectangle, two distant ones are two",
            worth_merging && keep_apart,
            format!("moved {} px merged {}, corners {} px kept apart {}",
                    both.area(), worth_merging,
                    far_a.union(&far_b).area(), keep_apart));

    // Nothing may be drawn twice: the background is cut into the pieces the
    // window does not cover, and those must tile the hole exactly. A gap leaves
    // stale pixels; an overlap puts a pixel in two copies, and the raster
    // between them shows as a black band across a window
    let back = Rect::new(0, 0, 100, 100);
    let front = Rect::new(20, 30, 40, 40);
    let mut pieces = alloc::vec::Vec::new();
    back.subtract(&front, &mut pieces);
    let covered: u64 = pieces.iter().map(|p| p.area()).sum();
    let no_overlap = pieces.iter().enumerate().all(|(i, a)|
        pieces[i + 1..].iter().all(|b| a.intersect(b).is_none()));
    let misses_front = pieces.iter().all(|p| p.intersect(&front).is_none());
    let exact = covered + front.area() == back.area();

    // A cut that misses leaves the rectangle whole, and one that swallows it
    // leaves nothing
    let mut apart = alloc::vec::Vec::new();
    back.subtract(&Rect::new(500, 500, 10, 10), &mut apart);
    let mut swallowed = alloc::vec::Vec::new();
    front.subtract(&back, &mut swallowed);

    t.check("compositor: the background is cut around a window without gap or overlap",
            exact && no_overlap && misses_front
                && apart == alloc::vec![back] && swallowed.is_empty(),
            format!("{} pieces covering {} px, {} px hole, no overlap {}",
                    pieces.len(), covered, front.area(), no_overlap));

    // the damage a client declares reaches the compositor
    const NAME: &str = "surface: damage a client declared is carried to the compositor";
    let (w, h, stride) = (32i32, 32i32, 128i32);
    let Some((cl, mut conn, comp, xdg, handle, buffer)) =
        client_with_pool((stride * h) as usize, w, h, stride)
    else { t.check(NAME, false, "setup failed".to_string()); return };

    let surface = conn.create_surface(comp);
    let xdg_surface = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xdg_surface);
    let events = exchange(&mut conn, cl);
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xdg_surface, serial);

    // Two damage rectangles in one commit, with the window at 5,7, so a
    // compositor that forgets to translate them gets a wrong answer. Placed
    // after the commit, because the commit gives a window with no position its
    // first one
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 1, 2, 3, 4);
    conn.damage(surface, 10, 10, 8, 8);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    layout::set_window_pos(cl, surface, 5, 7);

    let seen = server::presentable();
    let ours = seen.iter().find(|p| p.client == cl && p.surface == surface);
    let carried = ours.map(|p| p.damage.clone()).unwrap_or_default();
    let offset_ok = ours.map(|p| p.offset) == Some((5, 7));

    // A frame consumes what it read: a rectangle left behind would be recopied
    // for ever. What the client commits while the frame is composing was not
    // read and belongs to the next frame; on the card that is when a client
    // woken by its frame callback commits
    let frame = server::presentable_for_frame();
    let read = frame.iter()
        .find(|p| p.client == cl && p.surface == surface)
        .map(|p| p.damage.len())
        .unwrap_or(0);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 20, 20, 4, 4);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    server::frame_composed();
    let left = |list: alloc::vec::Vec<server::Presentable>| list.iter()
        .find(|p| p.client == cl && p.surface == surface)
        .map(|p| p.damage.clone())
        .unwrap_or_default();
    let kept = left(server::presentable());
    let _ = server::presentable_for_frame();
    server::frame_composed();
    let after = left(server::presentable()).len();

    t.check(NAME,
            carried == alloc::vec![(1, 2, 3, 4), (10, 10, 8, 8)]
                && offset_ok && read == 2
                && kept == alloc::vec![(20, 20, 4, 4)] && after == 0,
            format!("{:?} at {:?}; the frame read {}, kept {:?} committed while it \
                     composed, {} left after the next one",
                    carried, ours.map(|p| p.offset), read, kept, after));

    // A fence the frame went ahead without: what it read of the surface was not
    // finished, so the damage is read again next frame and the fence waited on
    // again, and the frame after that retires both. Otherwise the shell below
    // the windows would keep the half-drawn pixels until the client next
    // damaged the same place
    const LATE: &str = "surface: damage drawn under a late fence is read again next frame";
    const FENCE: u64 = 0x0000_0003_0bad_f00d;
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 4, 4, 6, 6);
    conn.set_fence(surface, FENCE);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    let late: alloc::vec::Vec<_> = server::take_fences().into_iter().filter(|f| f.fence == FENCE).collect();
    let _ = server::presentable_for_frame();
    server::still_pending(&late);
    server::frame_composed();
    let again = left(server::presentable());
    let rearmed = server::take_fences().iter().any(|f| f.fence == FENCE);
    let _ = server::presentable_for_frame();
    server::frame_composed();
    let retired = left(server::presentable()).len();
    t.check(LATE,
            late.len() == 1 && again == alloc::vec![(4, 4, 6, 6)] && rearmed && retired == 0,
            format!("{} late, kept {:?}, fence again {}, {} left after the next frame",
                    late.len(), again, rearmed, retired));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// Geometry running off the end of the pool is refused before anything can read
/// past it. This stops a client reading kernel memory
pub(super) fn test_buffer_past_pool(t: &mut Runner) {
    let bytes = 4096;
    let handle = match shm::create(0, bytes) {
        Ok(h) => h,
        Err(e) => { t.check("shm: pool allocation", false, format!("{:?}", e)); return; }
    };
    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let shm_name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_shm" => Some(*name),
        _ => None,
    }).unwrap_or(0);

    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    // 64 rows of 256 bytes is 16 KiB out of a 4 KiB pool
    conn.create_buffer(pool, 0, 64, 64, 256, protocol::shm::FORMAT_XRGB8888);
    let events = exchange(&mut conn, cl);

    let err = first_error(&events);
    t.check("shm: a buffer past the end of its pool is refused", err.is_some(),
            match &err { Some((_, why)) => why.clone(), None => "ACCEPTED".to_string() });

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A handle belonging to another process is not a capability
pub(super) fn test_foreign_pool(t: &mut Runner) {
    let handle = match shm::create(4242, 4096) {
        Ok(h) => h,
        Err(e) => { t.check("shm: pool allocation", false, format!("{:?}", e)); return; }
    };
    let cl = server::connect(7); // a different pid entirely
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let shm_name = events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == "may_shm" => Some(*name),
        _ => None,
    }).unwrap_or(0);

    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    conn.create_pool(shm_obj, handle, 4096);
    let events = exchange(&mut conn, cl);

    let err = first_error(&events);
    t.check("shm: another process's pool cannot be named", err.is_some(),
            match &err { Some((_, why)) => why.clone(), None => "ACCEPTED".to_string() });

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A commit may carry a GPU fence, and it applies to that commit alone
///
/// Checked without a GPU: whether the wire carries the fence, whether the
/// server stores it where the compositor looks, and whether it stays behind
/// afterwards. A fence persisting across commits would make every later frame
/// wait for finished work
pub(super) fn test_commit_fence(t: &mut Runner) {
    let (w, h) = (32i32, 16i32);
    let bytes = (w * 4 * h) as usize;
    let Ok(handle) = shm::create(0, bytes) else {
        t.check("gpu: pool for the fence case", false, "no pool".to_string());
        return;
    };

    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |iface: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == iface => Some(*name),
        _ => None,
    });
    let (Some(comp), Some(shm_name)) = (name_of("may_compositor"), name_of("may_shm")) else {
        t.check("gpu: the registry offered the compositor and shm", false,
                "missing global".to_string());
        let _ = shm::destroy(handle);
        return;
    };

    let compositor = conn.bind(registry, comp, Interface::Compositor, 1);
    let shm_obj = conn.bind(registry, shm_name, Interface::Shm, 1);
    let surface = conn.create_surface(compositor);
    let pool = conn.create_pool(shm_obj, handle, bytes as i32);
    let buffer = conn.create_buffer(pool, 0, w, h, w * 4, protocol::shm::FORMAT_XRGB8888);

    // A fence with both halves set, so a server reading one word returns a
    // wrong number and not zero
    const FENCE: u64 = 0x0000_0007_dead_beef;
    conn.attach(surface, buffer, 0, 0);
    conn.set_fence(surface, FENCE);
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);

    let stored = server::take_fences();
    t.check("gpu: a commit carries its fence across the wire, both halves",
            stored.iter().any(|f| f.fence == FENCE),
            format!("wrote {:#x}, server had {:x?}", FENCE, stored));

    // Taking it clears it: a compositor reading the same fence every frame
    // would wait for finished work
    t.check("gpu: taking a fence clears it", server::take_fences().is_empty(),
            format!("{:x?}", server::take_fences()));

    // A second commit with nothing new does not bring it back. Buffer state
    // persists across commits in Wayland; a fence must not, since it is about
    // the pixels one commit carried
    conn.commit(surface);
    let _ = exchange(&mut conn, cl);
    t.check("gpu: a fence does not persist into the next commit",
            server::take_fences().is_empty(),
            "committed again with no new pixels".to_string());

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A pool of video memory is a window's buffer, read where it lies
/// (`dev/gpu-window-buffers.md`)
///
/// The pool is made up: an address past any card's memory, in no block the
/// allocator handed out, so no engine is given it and the case runs with no
/// card. Checked: the pool taken, the frame pointing the engines at it and the
/// processor at nothing, the processor leaving it out without a load, and the
/// size a slot allows. The pool goes with `revoke_video`, never `destroy`,
/// which gives a block back to the allocator, and this one is not the
/// allocator's
pub(super) fn test_video_pool(t: &mut Runner) {
    const NAME: &str = "shm: a pool of video memory is a window's buffer, read in place";
    const FAKE: u64 = 0x7f_0000_0000;
    const LEN: usize = 20 << 20;
    const OFFSET: usize = 4096;
    // `revoke_video` takes every video pool there is, and a program's would
    // go with ours
    if crate::drivers::gpu::nvidia::user_vram::held().0 != 0 {
        t.skip(NAME, "a program holds video memory".to_string());
        return;
    }
    let handle = match shm::adopt_video(0, FAKE, 0, LEN, LEN) {
        Ok(h) => h,
        Err(e) => { t.check(NAME, false, format!("{:?}", e)); return; }
    };
    let (w, h) = (64i32, 32i32);
    let stride = w * 4;

    let cl = server::connect(0);
    let mut conn = ClientConn::new();
    let registry = conn.get_registry();
    let events = exchange(&mut conn, cl);
    let name_of = |iface: &str| events.iter().find_map(|e| match e {
        Event::Global { name, interface, .. } if interface == iface => Some(*name),
        _ => None,
    });
    let (Some(sn), Some(cn), Some(xn)) = (name_of("may_shm"), name_of("may_compositor"), name_of("may_xdg"))
    else {
        t.check(NAME, false, "no globals".to_string());
        let _ = shm::revoke_video();
        server::disconnect(cl);
        return;
    };
    let shm_obj = conn.bind(registry, sn, Interface::Shm, 1);
    let comp = conn.bind(registry, cn, Interface::Compositor, 1);
    let xdg = conn.bind(registry, xn, Interface::Xdg, 1);
    let pool = conn.create_pool(shm_obj, handle, LEN as i32);
    let buffer = conn.create_buffer(pool, OFFSET as i32, w, h, stride, protocol::shm::FORMAT_XRGB8888);
    let surface = conn.create_surface(comp);
    let xs = conn.get_xdg_surface(xdg, surface);
    conn.get_toplevel(xs);
    let events = exchange(&mut conn, cl);
    let serial = events.iter().find_map(|e| match e {
        Event::XdgConfigure { serial } => Some(*serial), _ => None,
    }).unwrap_or(0);
    conn.ack_configure(xs, serial);
    conn.attach(surface, buffer, 0, 0);
    conn.damage(surface, 0, 0, w, h);
    conn.commit(surface);
    let events = exchange(&mut conn, cl);
    let refused = first_error(&events).map(|(_, why)| why);

    // The engines are pointed at the buffer, the processor at nothing
    let list = mine(&[cl]);
    let at = list.iter().position(|p| p.surface == surface);
    let want = FAKE + OFFSET as u64;
    let pointed = at.is_some_and(|i| {
        let p = &list[i];
        p.vram == Some(want) && p.gpu == Some(want) && p.phys == 0
    });

    // The processor leaves it out: nothing is read, nothing is written over the
    // black it starts from, and the time is counted
    let (mut untouched, mut counted) = (false, false);
    if let Some(i) = at {
        let (sw, sh) = server::output_size().unwrap_or((640, 480));
        let screen = render::Rect::new(0, 0, sw, sh);
        if let Some(area) = render::compose::placed(&list[i]).intersect(&screen) {
            let aw = (area.x1 - area.x0) as usize;
            let mut dst = alloc::vec![0x5au8; aw * (area.y1 - area.y0) as usize * 4];
            let before = render::texture::counts().left_out;
            render::compose::compose_into(&mut dst, aw, area, core::slice::from_ref(&list[i]),
                                          &screen, render::compose::Dress::Frame);
            untouched = dst.iter().all(|b| *b == 0);
            counted = render::texture::counts().left_out > before;
        }
    }
    t.check(NAME, refused.is_none() && pointed && untouched && counted,
            format!("refused {:?}, vram/gpu/phys {:?}, composed black {}, left out counted {}",
                    refused,
                    at.map(|i| (list[i].vram, list[i].gpu, list[i].phys)),
                    untouched, counted));

    // Larger than a slot of the effects pass: 4096 by 1100 is 17.2 MiB, inside
    // the 20 MiB pool. A refusal ends the connection, so it is asked last
    conn.create_buffer(pool, 0, 4096, 1100, 4096 * 4, protocol::shm::FORMAT_XRGB8888);
    let events = exchange(&mut conn, cl);
    let err = first_error(&events);
    t.check("shm: a buffer in video memory larger than 16 MiB is refused",
            err.as_ref().is_some_and(|(_, why)| why.contains("16 MiB")),
            match &err { Some((_, why)) => why.clone(), None => "ACCEPTED".to_string() });

    // Out of the table before the disconnect can `destroy` it
    let revoked = shm::revoke_video();
    server::disconnect(cl);
    t.check("shm: the made-up video pool goes by revoking, and it is the only one",
            revoked == 1 && shm::size(handle).is_err(),
            format!("{} revoked", revoked));
}
