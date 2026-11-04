// One frame, put on the screen
//
// Reads the flattened scene, works out which rectangles changed, hands them to
// the copy engine in one batch and answers the damage.
//
// Stacking is order: surfaces are copied bottom to top, so the last one written
// is seen where they overlap. No clipping of what is hidden, no blending. A
// redundant copy where two windows overlap, and the order cannot go wrong.
//
// The engine is not waited on here. `present` submits, and the fence is waited
// on at the top of the next frame, so the buffers go back then, and
// `server::frame_composed` and `server::frame_done` are two calls at two ends
// of the frame.

use alloc::vec::Vec;
use core::sync::atomic::{Ordering};

use crate::mayland::server;
use crate::mayland::server::shm;
use crate::drivers::gpu::nvidia::scanout;

use super::compose;
use super::cursor::{CURSOR_DMA, CURSOR_LAST, cursor_patch};
use super::damage::{ANIMATED, FORCE_FULL, LAST, LAST_FOCUS, MAX_PIECES, MAX_RECTS, OWED, Rect, force_full_frame, push_rect, uncovered};
use super::effects::{EFFECTS, border, effect_windows, effects, paint_phys};
use super::{SOFTWARE, STATS, raster, start, stop};

/// Reported once, not once a frame: a frame losing a rectangle usually loses
/// several
static DROPS_SAID: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// The effect-window regions of the previous frame, for damage, as `LAST` and
/// `ANIMATED`
static LAST_EFFECTS: spin::Mutex<alloc::vec::Vec<Rect>> =
    spin::Mutex::new(alloc::vec::Vec::new());

/// The same batch of rectangles, written by the processor.
///
/// The blits carry physical addresses, as the engine takes them; the processor
/// reaches the same memory through the direct map. Returns (copied, refused),
/// so the statistics mean the same on either path
pub(super) fn blit_batch_cpu(blits: &[scanout::ScreenBlit]) -> (u64, u64) {
    let Some(panel) = crate::drivers::gpu::softscan::panel() else {
        return (0, blits.len() as u64);
    };
    let mut ok = 0u64;
    let mut refused = 0u64;
    for b in blits {
        // Video memory is the engine's to read. Nothing should hand the
        // processor one (the copies are made only with an engine), and through
        // the direct map the number would read whatever system memory sits at
        // that address
        if b.src_phys == 0 || b.w == 0 || b.h == 0 || b.src_vram {
            refused += 1;
            continue;
        }
        let src = crate::arch::x86_64::grub::phys_to_virt(b.src_phys) as *const u8;
        unsafe {
            crate::io::console::blit_xrgb_to(
                &panel,
                src, b.src_pitch as usize,
                0, 0,
                b.x as usize, b.y as usize,
                b.w as usize, b.h as usize,
            );
        }
        ok += 1;
    }
    // The panel is mapped write-combining, and a store sits in a fill buffer
    // until something flushes it. Without this the tail of a frame appears when
    // the next frame pushes it out, on an idle desktop not soon. One fence a
    // frame
    unsafe { core::arch::x86_64::_mm_sfence() };
    (ok, refused)
}

/// Wait for last frame's copies to finish
///
/// Everything in a frame reads what the engine reads or writes what it reads:
/// the cursor patch is composed into one reused buffer, and the console draws
/// into the shadow the engine copies out of. So the wait comes before any of
/// it, when the engine has had a whole raster period.
///
/// Ahead of `console::pump`, not only `present`: a resize makes the console
/// blank and repaint the whole shadow from inside `pump`
pub(super) fn wait_for_engines() {
    let (us, done) = scanout::wait_last_blit(200_000);
    // The engine has finished with last frame's addresses, so the pools
    // destroyed while it held them are free. Earlier hands the allocator pages
    // the engine is reading; see `shm::RETIRED`
    crate::mayland::server::shm::reap();
    // And the frames of any channel torn down: the wait above proves the
    // engines are finished with the addresses in flight
    crate::drivers::gpu::nvidia::gsp_common::chan::reap_buffers();
    // And the copies of windows that left the frame
    super::texture::reap();
    // The work this wait was for is the previous frame's, still in STATS
    let (last_px, last_backdrop, last_rects) = {
        let mut st = STATS.lock();
        st.wait_us = us;
        st.wait_worst_us = st.wait_worst_us.max(us);
        if !done {
            st.fence_timeouts += 1;
        }
        (st.last_px, st.last_backdrop_px, st.last_rects)
    };
    // A frame's GPU work outlasting a 240 Hz frame, with the time and how much:
    // against `nvidia log mem debug` it says whether the engines were slow on
    // their own or right after the page tables changed, and whether the slow
    // ones are the big ones. And how much was uploading windows into their
    // copies, the one part of a frame still crossing the bus (`texture`)
    if us >= 4000 {
        crate::kdebug!("[mayland] a frame waited {} us for the engines - it drew {} Kpx, {} Kpx of backdrop, {} rectangle(s), uploaded {} Kpx",
            us, last_px / 1000, last_backdrop / 1000, last_rects,
            super::texture::counts().last_px / 1000);
    }
    if !done {
        // One of the two engines waits for the other and neither will move.
        // Release both fences by hand and stop composing on the GPU: a desktop
        // without blur is preferable to a screen that never updates
        if let Some(b) = super::backend() { b.unstick(); }
        // Out of the frame altogether, first: it remembers whether effects were
        // on, for when the engine comes back. Clearing the preference below is
        // not enough: a fading or scaling window goes down this path regardless
        super::effects::give_up_on_compute("they did not finish a frame");
        EFFECTS.lock().on = false;
        FORCE_FULL.store(true, Ordering::Relaxed);
    }
}

/// How long a frame holds for a client's own GPU work.
///
/// A tenth of a 60 Hz frame: long enough for a dispatch, microseconds, and too
/// short to lose the frame over
const CLIENT_FENCE_BUDGET_US: u64 = 1_600;

/// Frames that went out before a client's pixels were finished. Should be zero;
/// `mayland status` prints it, as non-zero is a client asking for more work
/// than a frame has room for
pub(super) static LATE_FENCES: core::sync::atomic::AtomicU64 =
    core::sync::atomic::AtomicU64::new(0);

/// Copy what changed onto the screen
///
/// The screen is redrawn only where it has to be: the union of what a client
/// said it drew, where a surface was and is if those differ, and where a
/// surface used to be if gone. A window going away leaves a hole, filled
/// because something below it is recopied.
///
/// Then every surface, bottom to top, is copied where it meets that union.
/// Order is the whole stacking model
pub(super) fn present() -> (u64, u64, usize) {
    let Some((screen_w, screen_h)) = server::output_size() else {
        // `frame_composed` still runs after this and clears every surface's
        // damage, so every window's copy has missed some
        super::texture::missed_frame();
        // And the shell below the windows, built from those copies
        super::base::missed_frame();
        return (0, 0, 0);
    };
    let screen = Rect::new(0, 0, screen_w as i32, screen_h as i32);

    // Pixels a client asked the GPU for and did not wait on itself.
    //
    // The wait is here because the compositor alone knows how much of the frame
    // is left. A budget, not a promise: a fence not arriving in time is drawn
    // anyway, as one frame of flicker beats a vanishing window, and the engines
    // have their own give-up path above for a fence that never comes
    let step = crate::kcore::time::Stopwatch::start();
    let mut late = alloc::vec::Vec::new();
    for f in server::take_fences() {
        if !crate::syscall::adapters::gpu::wait_fence(f.fence, CLIENT_FENCE_BUDGET_US) {
            LATE_FENCES.fetch_add(1, Ordering::Relaxed);
            late.push(f);
        }
    }
    super::phases::add(super::phases::Phase::Fences, step.elapsed_us());

    let step = crate::kcore::time::Stopwatch::start();
    let mut list = server::presentable_for_frame();
    // What the late ones showed is read again next frame, finished by then
    server::still_pending(&late);
    super::phases::add(super::phases::Phase::Scene, step.elapsed_us());

    // What every window's copy in video memory needs from what its client
    // committed, worked out before anything below decides where a pixel is read
    // from. Sent just before the frame's own copies (`texture::submit`)
    let step = crate::kcore::time::Stopwatch::start();
    super::texture::sync(&mut list);
    super::phases::add(super::phases::Phase::Copies, step.elapsed_us());

    // The shell below the windows, folded into one surface in video memory and
    // brought up to date where it changed, before anything decides what blends:
    // a see-through layer under every window would put the whole frame on the
    // processor (`dev/mayland-below-windows.md`)
    let step = crate::kcore::time::Stopwatch::start();
    super::base::fold_for_frame(&mut list, &screen);
    super::phases::add(super::phases::Phase::Copies, step.elapsed_us());

    // Which surfaces really blend with what is under them. Asked first, as it
    // decides which windows the pass may take; who draws the corners and
    // shadows is decided once it has taken them
    let blends = compose::blending(&list);
    compose::reset();

    // Where everything is this frame, clipped to the screen
    let mut now: Vec<(u32, u32, Rect)> = Vec::with_capacity(list.len());
    for p in &list {
        now.push((p.client, p.surface,
                  Rect::new(p.offset.0, p.offset.1, p.width, p.height)));
    }

    let last = LAST.lock();
    let mut regions: Vec<Rect> = Vec::with_capacity(MAX_RECTS + 1);
    let mut full = FORCE_FULL.swap(false, Ordering::Relaxed);

    // A window's border lies outside the window, so every rectangle covering
    // where a window was or is covers its ring as well. Grown here for every
    // surface: at most eight pixels on four sides
    let bord = border();
    // A shadow lies further out still. The widest any surface casts this frame,
    // applied to every rectangle as the border is: a moved window repaints
    // where its shadow was. Zero when the compute pass does the shadows or
    // effects are off
    let reach = list.iter()
        .map(|p| compose::shadow_of(p, &screen, compose::Dress::Frame).1)
        .max()
        .unwrap_or(0);
    let out = bord.size.max(reach);
    let grow = |r: Rect| if out > 0 { r.outset(out) } else { r };

    // A surface that is gone leaves a hole where it was
    for (c, s, r) in last.iter() {
        if !now.iter().any(|(nc, ns, _)| nc == c && ns == s) {
            push_rect(&mut regions, grow(*r), &screen, &mut full);
        }
    }
    for (p, (_, _, r)) in list.iter().zip(now.iter()) {
        match last.iter().find(|(c, s, _)| *c == p.client && *s == p.surface) {
            // New: all of it
            None => push_rect(&mut regions, grow(*r), &screen, &mut full),
            // Moved or resized: where it was and where it is
            Some((_, _, old)) if old != r => {
                push_rect(&mut regions, grow(*old), &screen, &mut full);
                push_rect(&mut regions, grow(*r), &screen, &mut full);
            }
            // Still: only what the client says it redrew, in screen coordinates
            Some(_) => {
                for (dx, dy, dw, dh) in &p.damage {
                    let d = Rect::new(p.offset.0 + dx, p.offset.1 + dy, *dw, *dh);
                    if let Some(d) = d.intersect(r) {
                        push_rect(&mut regions, d, &screen, &mut full);
                    }
                }
            }
        }
    }

    // The cursor is not a surface, so nothing above repaints what it moved off.
    // Where it was and where it is, as for a moved window.
    //
    // Not when the head draws the pointer: the plane composites over the
    // finished viewport and leaves nothing behind, so a mouse over a still
    // desktop damages nothing and this frame does not happen
    if !super::cursor::hardware() {
        // Latched here, once; every later use in this frame, `cursor_patch`
        // included, reads the same position
        let now = crate::mayland::input::pointer::latch_rect()
            .map(|(x, y, w, h)| Rect::new(x, y, w, h));
        let mut was = CURSOR_LAST.lock();
        if *was != now {
            if let Some(r) = *was {
                push_rect(&mut regions, r, &screen, &mut full);
            }
            if let Some(r) = now {
                push_rect(&mut regions, r, &screen, &mut full);
            }
            *was = now;
        }
    }

    // The active window changing changes no pixel anywhere, but the colour of
    // two rings and, with an inactive opacity or a dim
    // (`dev/mayland-window-shape.md`), the two windows whole. Neither redrew or
    // moved, so the one losing the focus would keep its look on an idle
    // desktop. Both whole, border included: it happens once per click. The
    // active window, not the keyboard: the launcher taking the keyboard leaves
    // the window behind it lit (`Server::active`)
    {
        let focus = server::active_key();
        let mut was = LAST_FOCUS.lock();
        if *was != focus {
            for (c, s, r) in now.iter() {
                if Some((*c, *s)) == focus || Some((*c, *s)) == *was {
                    push_rect(&mut regions, grow(*r), &screen, &mut full);
                }
            }
            *was = focus;
        }
    }

    // A window with a curve running is repainted whole, changed or not
    //
    // A `popin` moves neither the rectangle nor a pixel the client drew: what
    // changes is the fraction drawn, the compositor's own arithmetic. The frame
    // after it lands is damaged too, from `ANIMATED`, as the curve stops on the
    // step setting the final values
    {
        let mut was = ANIMATED.lock();
        let mut still: Vec<(u32, u32)> = Vec::new();
        for (p, (_, _, r)) in list.iter().zip(now.iter()) {
            let key = (p.client, p.surface);
            if p.animating {
                still.push(key);
            }
            if p.animating || was.contains(&key) {
                push_rect(&mut regions, grow(*r), &screen, &mut full);
            }
        }
        *was = still;
    }

    // What the surface being drawn into is still owed
    //
    // With one surface, this frame's damage is the whole repair. With two, the
    // one drawn into has been off the screen and is missing what went to the
    // other since.
    //
    // A debt per surface: the two do not take turns strictly every frame, as a
    // frame with nothing to do draws into neither and a refused flip draws the
    // same one twice. A lost erase is a mark staying on the screen.
    //
    // As `gl_renderer_update_renderbuffers` in weston: each frame's damage is
    // unioned into every renderbuffer's own region, and the one about to be
    // rendered pays what it accumulated
    let slot = scanout::draw_slot();

    // A window drawn with effects has its backdrop repainted, changed or not,
    // on a frame with something to change.
    //
    // The blur reads the surface it is about to write into. If that still holds
    // last frame's blend, the blur is fed its own output and the window
    // dissolves into a smear. So the region under an effect window is damaged
    // whole, as every blurring compositor does.
    //
    // Not every frame: two megapixels a frame (`1993 Kpx` in `mayland status`)
    // against a copy engine moving about five hundred a second would saturate
    // it at 240 Hz on an idle desktop.
    //
    // New content and debt differ. If running the pass put its region into
    // `mine` below, both surfaces would take the debt, the next frame would pay
    // it by running the pass, and the cycle would never drain. So only
    // genuinely new damage promotes the region here, where `mine` can see it.
    // Paying an existing debt happens below, after the debt is drained into
    // `regions`, and does not propagate: the other surface holds the same
    // picture.
    //
    // With the software cursor its rectangle is damaged at every mouse move and
    // intersects any window it is over, so this saves little there
    let mut effects = effect_windows(&list, &screen, &blends);
    super::effects::remember_taken(&list, &effects);
    // What the processor blends: what really blends, and the corners and
    // shadows of every window the pass did not take
    let trans = compose::translucent(&list, &screen);
    // What the processor blends is what is see-through and not the pass's. A
    // window fading or popping in is both: in `trans`, and taken by the pass,
    // which composes all of it after the copies. Blending it here as well would
    // cost ~1 Mpx and 10 ms a frame for a half-screen window, only to be
    // overwritten, and would take its area out of the copy path, leaving the
    // backdrop stale
    let by_cpu: Vec<usize> = trans.iter().copied()
        .filter(|t| !effects.iter().any(|w| w.index == *t))
        .collect();
    // A window that opened, closed, moved or resized changes what lies under
    // the others, and not all of that reaches here as damage. Comparing the
    // regions is cheap (a handful), and a wrong answer this way costs a frame
    // of copies, where the other way leaves the wrong picture blurred under a
    // window.
    //
    // Where an effect window's region was is repainted too: the pass drew its
    // shadow out to the margin, further than the border the frame grows a moved
    // window's old rectangle by
    {
        let now_rects: Vec<Rect> = effects.iter().map(|w| w.rect).collect();
        let mut was = LAST_EFFECTS.lock();
        if *was != now_rects {
            super::effects::BACKDROP_DIRTY.store(true, Ordering::Relaxed);
            for old in was.iter().filter(|r| !now_rects.contains(r)) {
                push_rect(&mut regions, *old, &screen, &mut full);
            }
            *was = now_rects;
        }
    }
    let touches = |ws: &[super::effects::EffectWin], rs: &[Rect]| {
        ws.iter().any(|w| rs.iter().any(|r| r.intersect(&w.rect).is_some()))
    };
    // Laying the region down is the backdrop's errand, not the screen's.
    //
    // With a backdrop buffer the screen under an effect window is written by
    // `composite`, every pixel, so the copy engine has nothing to do there. The
    // backdrop needs those pixels when they changed, which ordinary damage from
    // a surface below carries: its pieces land in the backdrop list below.
    // Damage cannot fill the buffer the first time: `BACKDROP_DIRTY`.
    //
    // Under `nobackdrop` the pass reads the surface it writes, so the region
    // goes down on every frame touching it
    let backdrop_live = scanout::backdrop_live();
    let need_region = if backdrop_live {
        full || super::effects::BACKDROP_DIRTY.swap(false, Ordering::Relaxed)
    } else {
        full || touches(&effects, &regions)
    };
    if !effects.is_empty() && need_region {
        for w in &effects {
            push_rect(&mut regions, w.rect, &screen, &mut full);
        }
    }

    if let Some(slot) = slot {
        let mine = regions.clone();
        let mut owed = OWED.lock();
        let mut over = [false, false];
        for (i, o) in owed.iter_mut().enumerate() {
            for r in &mine {
                push_rect(o, *r, &screen, &mut over[i]);
            }
        }
        // A debt too broken up to track is the whole screen, as in `push_rect`
        for (i, o) in owed.iter_mut().enumerate() {
            if over[i] {
                o.clear();
                o.push(screen);
            }
        }
        for r in core::mem::take(&mut owed[slot]) {
            push_rect(&mut regions, r, &screen, &mut full);
        }
    }

    if full {
        regions.clear();
        regions.push(screen);
        if let Some(slot) = slot {
            let mut owed = OWED.lock();
            // This frame pays the whole screen into `slot`, owing nothing; the
            // other owes all of it
            owed[slot].clear();
            owed[1 - slot] = alloc::vec![screen];
        }
    }

    // `regions` is now all this frame will redraw (its own damage and the debt
    // it took on), so the pass runs for what is in it. An empty list stops it
    // being submitted further down: a chain run over a region holding its own
    // output is the feedback the repaint exists to prevent
    if !effects.is_empty() && !touches(&effects, &regions) {
        effects.clear();
    }

    // What each window has to redo.
    //
    // The part of a window's region intersecting `regions` is the part about to
    // differ; the rest holds the last pass's output.
    //
    // Not grown: the blur is the shell below the windows, computed for the
    // whole screen, so nothing is clamped to this rectangle
    for w in effects.iter_mut() {
        let mut hit: Option<Rect> = None;
        for r in &regions {
            if let Some(a) = r.intersect(&w.rect) {
                hit = Some(match hit {
                    Some(p) => p.union(&a),
                    None => a,
                });
            }
        }
        // A window nothing in `regions` touches, while another is touched,
        // keeps the clip it came with, all of it: the whole region is never
        // wrong, once the ones above it have been told (`settle_stacked_clips`,
        // below)
        if let Some(a) = hit.and_then(|a| a.intersect(&w.rect)) {
            w.window.clip = (
                a.x0 as u32,
                a.y0 as u32,
                (a.x1 - a.x0) as u32,
                (a.y1 - a.y0) as u32,
            );
        }
    }
    // A window stacked over another reads what that one writes, so the one
    // below redoes where the one above does, and the one above redoes where the
    // one below writes
    super::effects::settle_stacked_clips(&mut effects);
    // Read for the comparisons above. `now` takes its place at the end of the
    // frame, by move; the pass below needs it to know what covers what
    drop(last);

    // How much this frame has to repair, counted here.
    //
    // Below, a region covered by a see-through surface moves out of `regions`
    // into `blend_regions`, so with a translucent window `regions` can end
    // empty while the frame draws everything
    let damaged = regions.len();

    // Nothing changed, so no frame at all
    //
    // `repaint_needed` in `weston_output_maybe_repaint`: a repaint not needed
    // is skipped whole, with its page flip. Composing and not flipping would
    // put the cursor's erase in the surface nobody scans
    if damaged == 0 {
        // A window committing where nothing of it can be seen (off the screen)
        // still has a copy to keep current
        {
            let step = crate::kcore::time::Stopwatch::start();
            super::texture::submit();
            super::phases::add(super::phases::Phase::Copies, step.elapsed_us());
        }
        *LAST.lock() = now;
        let mut st = STATS.lock();
        st.last_rects = 0;
        st.last_px = 0;
        return (0, 0, 0);
    }

    // Cut each region into the part a see-through surface covers and the part
    // it does not
    //
    // After the debt is settled: what each surface is owed is the damage of the
    // whole frame, and how a region is served is this frame's business.
    //
    // With nothing translucent this is the same list, moved
    let mut blend_regions: Vec<Rect> = Vec::new();
    if !by_cpu.is_empty() {
        // Not one rectangle per surface: a window opaque but rounded
        // contributes its four corners, and its middle stays on the copy path
        let mut covers: Vec<Rect> = Vec::with_capacity(by_cpu.len());
        for &t in by_cpu.iter() {
            compose::covers_of(&list[t], &screen, &mut covers);
        }
        let mut copy_regions: Vec<Rect> = Vec::with_capacity(regions.len());
        let mut plain: Vec<Rect> = Vec::new();
        let mut mixed: Vec<Rect> = Vec::new();
        for r in &regions {
            compose::split(*r, &covers, &mut plain, &mut mixed);
            copy_regions.extend(plain.iter().copied());
            blend_regions.extend(mixed.iter().copied());
        }
        // The pass writes more than `regions`: the blur's reach around the
        // damage, which the chain reads and the compose writes, and the gap
        // between two damaged rectangles, as a window's clip is one box. Those
        // pixels come back as the window, and a see-through surface above it
        // was blended only for `regions`, so it would be gone there (aosh's
        // launcher over the terminal, a 64-pixel band around its search field).
        // So the surfaces above are blended over the rest of the clip too. Only
        // them: growing `regions` would copy the wallpaper over parts of the
        // terminals no clip covers
        for w in &effects {
            let (x, y, pw, ph) = w.window.clip;
            let wrote = if pw == 0 || ph == 0 {
                w.rect
            } else {
                Rect::new(x as i32, y as i32, pw as i32, ph as i32)
            };
            let mut above: Vec<Rect> = Vec::new();
            for &t in by_cpu.iter().filter(|&&t| t > w.index) {
                compose::covers_of(&list[t], &screen, &mut above);
            }
            for c in &above {
                let Some(a) = c.intersect(&wrote) else { continue };
                compose::split(a, &regions, &mut plain, &mut mixed);
                blend_regions.extend(plain.iter().copied());
            }
        }
        let want: usize = blend_regions.iter().map(|r| r.area() as usize * 4).sum();
        if compose::reserve(want) {
            regions = copy_regions;
        } else {
            // No memory to compose in. The bar goes down as an ordinary copy,
            // solid, not see-through: the regions were never split, so every
            // surface is copied bottom to top.
            //
            // The whole frame has to take that path next time: a fallback for
            // the damaged rectangles alone paints them by a different rule than
            // the rest of the window, and the two brightnesses meet along the
            // edge of the damage. So the frame asks for all of itself, and the
            // seam cannot outlive one frame
            blend_regions.clear();
            force_full_frame();
        }
    }

    // Every surface, bottom to top, but not where something above covers the
    // result
    //
    // Writing a pixel twice is visible. The copies go into the surface the
    // raster is scanning, and between the background's copy and the window's
    // there is one engine round trip; a raster crossing in the gap shows a
    // black band across the window. So the background is cut into the pieces
    // the window does not cover.
    //
    // The frame is a list handed to the engine in one submission at the end,
    // keeping the scheduler out of the middle of a frame; see
    // blit_batch_to_screen. Sized up front: one blit per surface per damage
    // region at most, with piece lists capped by MAX_PIECES
    let mut frame: Vec<scanout::ScreenBlit> =
        Vec::with_capacity(list.len().max(1) * regions.len().max(1));
    // The same pieces, for the buffer the effects pass reads.
    //
    // Two lists: what lies under an effect window has to reach the backdrop,
    // which the blur samples, not the screen, where `composite` writes every
    // pixel of that area.
    //
    // Only with a backdrop to read. Under `nobackdrop` the pass samples the
    // surface it writes, so the region is laid down there first
    let mut into_backdrop: Vec<scanout::ScreenBlit> = Vec::new();
    let mut backdrop_px = 0u64;
    // What goes on after the pass: what lies over a window the pass composes
    // (copies here, blended rectangles below), then the rings and the cursor
    let mut after_pass: Vec<scanout::ScreenBlit> = Vec::new();
    let mut pixels = 0u64;
    let mut pieces: Vec<Rect> = Vec::with_capacity(MAX_PIECES + 1);
    let mut next: Vec<Rect> = Vec::with_capacity(MAX_PIECES + 1);
    let picked: Vec<(usize, Rect)> = effects.iter().map(|w| (w.index, w.rect)).collect();
    for (i, p) in list.iter().enumerate() {
        // An effect window is not copied: the GPU composes it after this batch,
        // over the backdrop this batch puts down
        if effects.iter().any(|w| w.index == i) {
            continue;
        }
        let r = Rect::new(p.offset.0, p.offset.1, p.width, p.height);
        // A surface entirely off the screen (a window dragged past an edge) has
        // nothing to copy
        let Some(visible) = r.intersect(&screen) else { continue };
        for region in &regions {
            let Some(area) = visible.intersect(region) else { continue };

            uncovered(area, i, &now, &effects, &mut pieces, &mut next);

            for piece in pieces.iter() {
                // Where inside the window that rectangle begins: in the
                // compositor's copy when there is one, with the client's
                // stride, in the client's buffer when not. The screen, the
                // backdrop and the debt all take it from here: the copy is what
                // the client committed
                let (base, in_vram) = match p.gpu {
                    Some(copy) => (copy, true),
                    None => (p.phys, false),
                };
                let src = base
                    + (piece.y0 - r.y0) as u64 * p.stride as u64
                    + (piece.x0 - r.x0) as u64 * 4;
                let blit = scanout::ScreenBlit {
                    src_phys: src,
                    src_pitch: p.stride as u32,
                    x: piece.x0 as u32,
                    y: piece.y0 as u32,
                    w: (piece.x1 - piece.x0) as u32,
                    h: (piece.y1 - piece.y0) as u32,
                    src_vram: in_vram,
                };
                // Under a window the pass composes over, in two senses. Wholly
                // under: the screen gets this from `composite` and the copy
                // would write a pixel twice. Touching at all: the blur samples
                // it, so the backdrop needs it, including the part sticking
                // out, as a piece is not split.
                //
                // Only windows above this surface count. The panel commits
                // several times a second and sits over the terminal; copying it
                // into a buffer the blur never samples there is thirty-four
                // kilopixels a frame
                let mut hidden = false;
                let mut sampled = false;
                if backdrop_live {
                    for w in effects.iter().filter(|w| w.index > i) {
                        if w.rect.intersect(piece).is_some() {
                            sampled = true;
                            hidden |= w.rect.contains(*piece);
                        }
                    }
                }
                // Over a window the pass composes, it goes on after the pass:
                // the pass writes every pixel of its region, so a piece of a
                // surface above it (a menu, a plain panel) copied first would
                // be painted over
                let over_pass = super::effects::lies_over(i, piece, &picked);
                if !hidden {
                    if over_pass {
                        after_pass.push(blit);
                    } else {
                        frame.push(blit);
                    }
                    pixels += piece.area();
                }
                if sampled {
                    into_backdrop.push(blit);
                    backdrop_px += piece.area();
                }
            }
        }
    }

    // The regions that blend, each composed into one buffer and copied once
    //
    // The rectangles are disjoint from every plain region, so where they land
    // in the batch does not matter; here, so the frame reads bottom to top.
    //
    // A blended rectangle over a window the pass draws goes on after the pass,
    // with the rings and the cursor: composed over everything under it, that
    // window included, and copied first it would be painted over
    // (`effect_windows`, "Not over a see-through surface")
    let step = crate::kcore::time::Stopwatch::start();
    // A window in video memory under one of them is read through a mirror the
    // copy engine fills for these rectangles: the processor does not read the
    // card's memory (`dev/gpu-window-buffers.md`)
    super::texture::mirror(&mut list, &blend_regions);
    for area in &blend_regions {
        super::phases::blended(area.area());
        let Some((phys, pitch)) = compose::compose(*area, &list, &screen) else {
            // The pool ran out part way through the frame. As on the reserve
            // failure above: the whole frame is redrawn, not left as a
            // patchwork
            force_full_frame();
            continue;
        };
        let blit = scanout::ScreenBlit {
            src_phys: phys,
            src_pitch: pitch,
            x: area.x0 as u32,
            y: area.y0 as u32,
            w: (area.x1 - area.x0) as u32,
            h: (area.y1 - area.y0) as u32,
            src_vram: false,
        };
        if effects.iter().any(|w| w.rect.intersect(area).is_some()) {
            after_pass.push(blit);
        } else {
            frame.push(blit);
        }
        pixels += area.area();
    }
    super::phases::add(super::phases::Phase::Blend, step.elapsed_us());

    // The rings, after every window and before the cursor
    //
    // After, as a window with the shader pass is composed later and writes a
    // shadow past its own edge: a border in the batch above would be painted
    // over by its own window. Cut by the windows above them as a window is, so
    // a border does not show through what stands in front of it
    let mut rings: Vec<scanout::ScreenBlit> = Vec::new();
    if bord.size > 0 {
        // The most pixels one band can want: the longer side of the screen
        // across, the border deep, plus the outset, as a ring stands outside
        // its window and the top band of a full-width window is two border
        // widths wider than the screen
        let want = (screen_w.max(screen_h) + 2 * bord.size) as usize * bord.size as usize;
        let focus = server::active_key();
        for (i, p) in list.iter().enumerate() {
            // The backdrop, a fullscreen window and every layer surface are not
            // decorated, and a ring is decoration, the rule keeping a rounded
            // corner off the panel. A dressed window has its border drawn with
            // it, curved and blended, by its composer
            // (`dev/mayland-window-shape.md`): these square fills are for the
            // rest
            if !p.decorated || compose::dressed_by(p, &screen, compose::Dress::Patch, false) {
                continue;
            }
            let colour = if focus == Some((p.client, p.surface)) {
                bord.active
            } else {
                bord.inactive
            };
            let Some(phys) = paint_phys(colour, want) else { continue };
            // Round the window as drawn this frame: smaller while it pops in,
            // by the box the pass and the processor draw it in
            let r = compose::placed(p);
            for band in r.ring(bord.size).iter() {
                let Some(visible) = band.intersect(&screen) else { continue };
                for region in &regions {
                    let Some(area) = visible.intersect(region) else { continue };
                    uncovered(area, i, &now, &effects, &mut pieces, &mut next);
                    for piece in pieces.iter() {
                        let w = (piece.x1 - piece.x0) as u32;
                        let h = (piece.y1 - piece.y0) as u32;
                        rings.push(scanout::ScreenBlit {
                            src_phys: phys,
                            // Any pitch reads the same colour; this one keeps
                            // the copy inside the buffer, `want` pixels
                            src_pitch: w * 4,
                            x: piece.x0 as u32,
                            y: piece.y0 as u32,
                            w,
                            h,
                            src_vram: false,
                        });
                        pixels += piece.area();
                    }
                }
            }
        }
    }

    // The cursor goes on last, over the finished frame, as one opaque patch
    // composed by hand; see `cursor::cursor_patch`, which composes it from this
    // list so the pixels around the arrow are the frame's. Not when the head
    // has a plane of its own
    let mut cursor: Vec<scanout::ScreenBlit> = Vec::new();
    let step = crate::kcore::time::Stopwatch::start();
    // The patch is composed from what is under the pointer, so a window in
    // video memory there needs its mirror too, as for the blend
    if !super::cursor::hardware() {
        if let Some((x, y, w, h)) = crate::mayland::input::pointer::frame_rect() {
            super::texture::mirror(&mut list, &[Rect::new(x, y, w, h)]);
        }
    }
    let patch = (!super::cursor::hardware()).then(|| cursor_patch(&screen, &list)).flatten();
    super::phases::add(super::phases::Phase::Blend, step.elapsed_us());
    if let Some((cx, cy, cw, ch)) = patch {
        cursor.push(scanout::ScreenBlit {
            src_phys: CURSOR_DMA.lock().as_ref().map(|d| d.phys()).unwrap_or(0),
            src_pitch: (cw * 4) as u32,
            x: cx as u32,
            y: cy as u32,
            w: cw as u32,
            h: ch as u32,
            src_vram: false,
        });
        pixels += (cw * ch) as u64;
    }

    // With nothing to compose on the GPU the cursor rides in the batch with
    // everything else: one submission, and the scheduler cannot get into the
    // middle of a frame.
    //
    // With effect windows the frame is three submissions on two engines, in
    // order: the backdrop, the blur and the blend over it, the cursor on top.
    // Each waits for the one before it on the GPU, not through the processor:
    // the copy engine releases a fence, the compute engine acquires it, and the
    // reverse for the cursor.
    //
    // Noted before the list is consumed below: whether this frame asked the
    // compute engine for anything decides what a refusal means
    let had_effects = !effects.is_empty();
    let (ok, dropped) = if SOFTWARE.load(Ordering::Relaxed) {
        // No engine and no second surface: the rectangles go onto the panel in
        // the order built, back to front. Only the blur is dropped: a
        // dual-kawase chain on the processor at sixty frames a second is too
        // costly. The alpha, the rounded corners and the shadow are arithmetic,
        // composed in `compose.rs`.
        //
        // No pass in this frame, so nothing for these to wait for: empty in
        // practice, and never dropped if not
        frame.append(&mut after_pass);
        frame.append(&mut rings);
        frame.append(&mut cursor);
        let step = crate::kcore::time::Stopwatch::start();
        let sent = match super::backend() {
            Some(b) => b.blit_batch(0, &frame),
            None    => (0, frame.len() as u64),
        };
        super::phases::add(super::phases::Phase::Submit, step.elapsed_us());
        sent
    } else if effects.is_empty() {
        // No pass in this frame, so nothing for these to wait for: empty in
        // practice, and never dropped if not
        frame.append(&mut after_pass);
        frame.append(&mut rings);
        frame.append(&mut cursor);
        // The windows' uploads first, on the same channel as the copies that
        // read them
        {
            let step = crate::kcore::time::Stopwatch::start();
            super::texture::submit();
            super::phases::add(super::phases::Phase::Copies, step.elapsed_us());
        }
        let step = crate::kcore::time::Stopwatch::start();
        let sent = match super::backend() {
            Some(b) => b.blit_batch(0, &frame),
            None    => (0, frame.len() as u64),
        };
        super::phases::add(super::phases::Phase::Submit, step.elapsed_us());
        sent
    } else {
        // The rings go with the cursor, in the batch submitted after the
        // compute pass: a border in the first batch is blurred over by its own
        // window. Under both, the blended rectangles over the pass's windows
        after_pass.append(&mut rings);
        after_pass.append(&mut cursor);
        let cursor = after_pass;
        let wins: Vec<_> = effects.into_iter().map(|w| w.window).collect();
        // Maps any copy the pass has not read yet while the engines are idle,
        // before the uploads are queued. The mapping in `compose_frame` is then
        // a lookup, and its result is the one used
        let step = crate::kcore::time::Stopwatch::start();
        let _ = scanout::map_effect_windows(&wins);
        super::phases::add(super::phases::Phase::Submit, step.elapsed_us());
        {
            let step = crate::kcore::time::Stopwatch::start();
            super::texture::submit();
            super::phases::add(super::phases::Phase::Copies, step.elapsed_us());
        }
        let step = crate::kcore::time::Stopwatch::start();
        let sent = scanout::compose_frame(&frame, &into_backdrop, &wins, &cursor);
        super::phases::add(super::phases::Phase::Submit, step.elapsed_us());
        sent
    };

    // A rectangle the engine refused was never written, and the debt of the
    // surface being drawn into was cleared at the top of this frame. Nothing
    // damages those pixels again, so the panel would keep a mix of this frame
    // and an older one.
    //
    // Same remedy as the two compose failures above: the next frame redraws
    // everything and both surfaces settle from scratch. `full` guarantees that
    // frame happens when nothing else changed.
    //
    // `blit_batch_cpu` clips inside `blit_xrgb_to` and only refuses a
    // degenerate rectangle, so the software path does not get here
    if dropped > 0 {
        // Lost rectangles. If they were the windows the compute pass was to
        // compose (it could not map or retarget what it was about to read), the
        // engine will not do better next frame, so the windows go back to being
        // ordinary copies
        if had_effects {
            super::effects::give_up_on_compute("it could not reach what it was to read");
        }
        force_full_frame();
        if !DROPS_SAID.swap(true, Ordering::Relaxed) {
            crate::serial_println!(
                "[mayland] {} of {} rectangles were refused - redrawing whole \
                 frames until it stops ({})",
                dropped, ok + dropped, scanout::refusals()
            );
        }
    }

    // Where everything was this frame, for the next one to compare against
    *LAST.lock() = now;

    {
        let mut st = STATS.lock();
        // What the frame had to repair, not what survived the split into copied
        // and blended: the latter is zero whenever a see-through window covers
        // the damage
        st.last_rects = damaged;
        st.last_px = pixels;
        st.pixels += pixels;
        st.last_backdrop_px = backdrop_px;
        st.backdrop_pixels += backdrop_px;
    }
    // Rectangles of the scene this frame repaired. Not the copies submitted
    // (the cursor patch rides on every frame) and not what is left in `regions`
    // after the blend split. The caller needs to know whether the frame put
    // anything new into the surface, and past the early return above it always
    // did
    (ok, dropped, damaged)
}
