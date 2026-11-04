// Curves, and the things that ride on them
//
// A curve is arithmetic: a cubic bezier baked into a table, sampled. What rides
// on it needs a window with somewhere to go, a tick and a clock. The ghost has
// to outlive the window by exactly the length of the curve: freeing it a frame
// early is a use-after-free, a frame late is a leak

use super::*;

/// The ring a border is drawn as: four bands outside the window and nothing
/// inside it
///
/// Every pixel of it is written by a copy with no bounds check of its own, so a
/// band reaching inside the window would draw over its contents. The same four
/// rectangles are used to damage and to draw, so an error in them would look
/// right until a window moved
pub(super) fn test_border_ring(t: &mut Runner) {
    const NAME: &str = "border: the ring is four bands outside the window, meeting at the corners";
    use crate::mayland::render::Rect;

    let w = Rect::new(100, 50, 200, 120);
    let n = 2;
    let bands = w.ring(n);

    // Nothing of it is inside the window
    let outside = bands.iter().all(|b| w.intersect(b).is_none());
    // And no two bands share a pixel, or that pixel would be copied twice
    let mut disjoint = true;
    for i in 0..4 {
        for j in i + 1..4 {
            if bands[i].intersect(&bands[j]).is_some() {
                disjoint = false;
            }
        }
    }
    // Together they are exactly the frame: the window grown by `n` on every
    // side, less the window itself
    let area: u64 = bands.iter().map(|b| b.area()).sum();
    let frame = w.outset(n).area() - w.area();
    let covers = area == frame;
    // Every side is touched: a ring with a missing band would pass the three
    // above if the others were made larger
    let touches = bands.iter().any(|b| b.y1 == w.y0)
        && bands.iter().any(|b| b.y0 == w.y1)
        && bands.iter().any(|b| b.x1 == w.x0)
        && bands.iter().any(|b| b.x0 == w.x1);
    // No border at all is no rectangles at all, so a desktop with no borders
    // draws nothing extra
    let none = w.ring(0).iter().all(|b| b.is_empty());

    t.check(NAME, outside && disjoint && covers && touches && none,
            format!("outside {}, disjoint {}, area {} of {} wanted, all four sides {},                      empty at size 0 {}",
                    outside, disjoint, area, frame, touches, none));
}

/// A bezier is a bezier: through both ends, monotone, and ahead of the
/// diagonal early when that is what its control points say
pub(super) fn test_curve_shape(t: &mut Runner) {
    const NAME: &str = "anim: a curve passes through both ends, rises, and leads early";
    use crate::mayland::shell::anim;

    // `easeOut`: a sharp start and a long settle
    let c = anim::Curve::new((0.22, 1.0), (0.36, 1.0));

    let ends = c.y_at(0.0) == 0.0 && c.y_at(1.0) == 1.0;
    // Clamped outside its range rather than extrapolated: an animation past its
    // time is finished
    let clamped = c.y_at(-1.0) == 0.0 && c.y_at(2.0) == 1.0;

    let mut monotone = true;
    let mut prev = 0.0f32;
    for i in 0..=100 {
        let y = c.y_at(i as f32 / 100.0);
        if y < prev - 0.0001 {
            monotone = false;
        }
        prev = y;
    }

    // A quarter of the way through the time, well over half of the distance
    let quarter = c.y_at(0.25);
    let leads = quarter > 0.5;

    // And a curve nobody defined is a straight line
    let linear = anim::Curve::LINEAR;
    let straight = (linear.y_at(0.5) - 0.5).abs() < 0.001;

    t.check(NAME, ends && clamped && monotone && leads && straight,
            format!("ends {}, clamped {}, monotone {}, at a quarter {:.3} (want > 0.5), \
                     undefined is linear {}",
                    ends, clamped, monotone, quarter, straight));
}

/// The other curve leaves late, so a window's exit differs from its entrance
pub(super) fn test_curve_ease_in(t: &mut Runner) {
    const NAME: &str = "anim: easeIn holds back where easeOut runs ahead";
    use crate::mayland::shell::anim;
    let out = anim::Curve::new((0.22, 1.0), (0.36, 1.0));
    let ein = anim::Curve::new((0.64, 0.0), (0.78, 0.0));
    let (a, b) = (out.y_at(0.25), ein.y_at(0.25));
    t.check(NAME, a > b && b < 0.25,
            format!("at a quarter: easeOut {:.3}, easeIn {:.3} (want easeIn below the \
                     diagonal, so under 0.25)", a, b));
}

/// The table answers what a class animates with, and inherits the way the
/// config file's own tree does
pub(super) fn test_anim_table(t: &mut Runner) {
    const NAME: &str = "anim: a class inherits from its parent, and off means off";
    use crate::mayland::shell::anim::{self, class, style};

    // Whatever the machine is set to comes back at the end: this runs on a
    // desktop that may already be animating
    let was_on = anim::enabled();
    anim::reset();

    let off_first = !anim::spec(class::WINDOWS).enabled;

    anim::set_spec(class::WINDOWS, anim::Spec {
        enabled: true, ms: 400, curve: 1, style: style::POPIN, arg: 0.8,
    });
    let direct = anim::spec(class::WINDOWS);
    // `windowsOut` was never set, so it takes what `windows` says
    let inherited = anim::spec(class::WINDOWS_OUT);
    // And a class with no parent stays off
    let orphan = anim::spec(class::WORKSPACES);

    anim::set_enabled(false);
    let master_off = !anim::spec(class::WINDOWS).enabled;

    anim::reset();
    anim::set_enabled(was_on);

    t.check(NAME,
            off_first && direct.ms == 400 && direct.arg == 0.8
                && inherited.ms == 400 && !orphan.enabled && master_off,
            format!("off before anything said otherwise {}, windows {} ms, \
                     windowsOut inherited {} ms, workspaces still off {}, \
                     the master switch turns everything off {}",
                    off_first, direct.ms, inherited.ms, !orphan.enabled, master_off));
}

/// A curve walked by the tick starts where it started and lands exactly on the
/// goal
pub(super) fn test_tick_lands(t: &mut Runner) {
    const NAME: &str = "anim: a tick walks from where it began and lands on the goal";
    use crate::mayland::shell::anim;

    let Some((cl, _c, surface, handle, _)) = one_window() else {
        t.check(NAME, false, "setup failed".to_string());
        return;
    };

    // Put it somewhere and animate it elsewhere by hand: `apply` refuses to
    // start one on this path, as the next case checks
    let start = (100i32, 100i32);
    let goal = layout::Rect::new(500, 300, 200, 100);
    let now = layout::now_us();
    layout::begin_anim_for_test(cl, surface, start, goal, now, 100, 0, anim::style::SLIDE);

    // Read through `presentable`, which has a window's position: `window_box`
    // reports the goal
    let where_now = || mine(&[cl]).iter()
        .find(|p| p.surface == surface).map(|p| p.offset);
    let at_start = where_now();
    // A third of the way through
    layout::tick(now + 33_000);
    let middle = where_now();
    // And past the end
    layout::tick(now + 200_000);
    let landed = where_now();
    let still = layout::running();

    let moved = middle.map(|(x, _)| x > start.0 && x < goal.x).unwrap_or(false);
    let exact = landed == Some((goal.x, goal.y));

    t.check(NAME, at_start == Some(start) && moved && exact && still == 0,
            format!("began at {:?}, a third of the way {:?}, landed {:?} against {:?}, \
                     curves still running {}",
                    at_start, middle, landed, (goal.x, goal.y), still));

    server::disconnect(cl);
    let _ = shm::destroy(handle);
}

/// A move starts a curve only when animations are on
pub(super) fn test_move_starts_a_curve(t: &mut Runner) {
    const NAME: &str = "anim: a move starts a curve when animations are on, and not when off";
    use crate::mayland::shell::anim::{self, class, style};

    let was_on = anim::enabled();
    anim::reset();

    let Some((cl, _c, surface, handle, _)) = one_window() else {
        anim::set_enabled(was_on);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };

    // Off: a move lands at once
    layout::set_window_pos(cl, surface, 40, 40);
    layout::arrange_now();
    let when_off = layout::running();

    // On: the same move is a curve
    anim::set_spec(class::WINDOWS, anim::Spec {
        enabled: true, ms: 400, curve: 0, style: style::SLIDE, arg: 1.0,
    });
    layout::set_window_pos(cl, surface, 400, 300);
    layout::arrange_now();
    let when_on = layout::running();

    // It still lands: walking past the end finishes it
    layout::tick(layout::now_us() + 10_000_000);
    let after = layout::running();

    let software = crate::mayland::render::software();
    anim::reset();
    anim::set_enabled(was_on);
    server::disconnect(cl);
    let _ = shm::destroy(handle);

    t.check(NAME,
            when_off == 0 && (when_on == 1 || software) && after == 0,
            format!("with animations off {} running, with them on {} running \
                     (software path {}), after the curve ended {}",
                    when_off, when_on, software, after));
}

/// Both halves of a workspace switch take one duration and one curve
///
/// A switch is one movement: the half of the screen that leaves and the half
/// that arrives are the same slide seen from two sides, and they must be timed
/// by the same config line or they come apart in the middle.
///
/// Hyprland animates one offset per workspace, so it has one duration. Here two
/// sets of windows walk towards two sets of goals, and `workspacesIn` and
/// `workspacesOut` both fall back to `workspaces`
pub(super) fn test_workspace_anim_halves(t: &mut Runner) {
    const NAME: &str = "anim: both halves of a workspace switch share one duration and curve";
    use crate::mayland::shell::anim::{self, class, style};

    let was_on = anim::enabled();
    anim::reset();

    // One line, as the shipped config writes it: `animation = workspaces, 1,
    // 5, easeOut, slide`
    anim::set_spec(class::WORKSPACES, anim::Spec {
        enabled: true, ms: 500, curve: 1, style: style::SLIDE, arg: 1.0,
    });
    let leaving = anim::spec(class::WORKSPACES_OUT);
    let arriving = anim::spec(class::WORKSPACES_IN);
    let together = leaving.ms == arriving.ms
        && leaving.curve == arriving.curve
        && leaving.style == arriving.style
        && arriving.ms == 500;

    // A config that wants them apart still gets them apart: the fallback is
    // only a default
    anim::set_spec(class::WORKSPACES_OUT, anim::Spec {
        enabled: true, ms: 300, curve: 0, style: style::SLIDE, arg: 1.0,
    });
    let split = anim::spec(class::WORKSPACES_OUT).ms == 300
        && anim::spec(class::WORKSPACES_IN).ms == 500;

    // The `windows` class is beside them and must not be what either takes
    anim::set_spec(class::WINDOWS, anim::Spec {
        enabled: true, ms: 400, curve: 0, style: style::POPIN, arg: 0.8,
    });
    let unrelated = anim::spec(class::WORKSPACES_IN).ms == 500;

    anim::reset();
    anim::set_enabled(was_on);

    t.check(NAME, together && split && unrelated,
            format!("one line times both halves {}, a config may still split them {},                      and `windows` is not what either takes {}",
                    together, split, unrelated));
}

/// A window that closes leaves an afterimage, and the afterimage is freed
/// exactly when its curve ends
pub(super) fn test_ghost_lifetime(t: &mut Runner) {
    const NAME: &str = "anim: a closed window leaves an afterimage that is freed with its curve";
    use crate::mayland::shell::anim::{self, class, style};

    let was_on = anim::enabled();
    anim::reset();
    anim::set_spec(class::WINDOWS_OUT, anim::Spec {
        enabled: true, ms: 200, curve: 0, style: style::POPIN, arg: 0.8,
    });

    let Some((cl, _c, surface, handle, _)) = one_window() else {
        anim::reset();
        anim::set_enabled(was_on);
        t.check(NAME, false, "setup failed".to_string());
        return;
    };
    // It has to have been arranged once, or there is no last frame to keep
    layout::arrange_now();

    let pools_before = shm::list().len();
    let before = mine(&[cl]).len();
    server::disconnect(cl);
    let _ = shm::destroy(handle);

    // The client is gone and the pixels are still there, drawn from memory the
    // compositor owns
    let ghosts = server::presentable().iter().filter(|p| p.client == 0).count();
    let pools_during = shm::list().len();

    // Walk past the end of the curve
    layout::tick(layout::now_us() + 10_000_000);
    let after = server::presentable().iter().filter(|p| p.client == 0).count();
    let pools_after = shm::list().len();

    anim::reset();
    anim::set_enabled(was_on);

    let software = crate::mayland::render::software();
    // With no card the case expects no afterimage
    let ok = if software {
        ghosts == 0 && after == 0
    } else {
        before == 1 && ghosts == 1 && after == 0 && pools_after <= pools_before
            && pools_during > pools_after
    };

    t.check(NAME, ok,
            format!("software {}, on screen before {}, afterimages while the curve ran {}, \
                     after it ended {}, pools {} -> {} -> {}",
                    software, before, ghosts, after,
                    pools_before, pools_during, pools_after));
}

/// A layer opens as Hyprland opens one (`dev/mayland-layers-above.md`, section
/// 4): its alpha on `fadeLayersIn`, its size on `layersIn`, each on its own
/// clock, and not at all when a `noanim` rule names it
pub(super) fn test_layer_open(t: &mut Runner) {
    const NAME: &str = "anim: a layer's alpha and size open on their own clocks, the fade classes fall back up the tree, and a noanim layer is there at once";
    use crate::mayland::shell::anim::{self, class, style};
    use crate::mayland::protocol::wm::layer_rule;

    let was_on = anim::enabled();
    anim::reset();
    // The fade tree: `fadeLayersIn` is two steps below `fade`
    anim::set_spec(class::FADE, anim::Spec { enabled: true, ms: 400, curve: 0, style: style::FADE, arg: 1.0 });
    let from_fade = anim::spec(class::FADE_LAYERS_IN).ms == 400 && anim::spec(class::FADE_IN).ms == 400;
    anim::set_spec(class::FADE_LAYERS, anim::Spec { enabled: true, ms: 300, curve: 0, style: style::FADE, arg: 1.0 });
    let from_layers = anim::spec(class::FADE_LAYERS_OUT).ms == 300 && anim::spec(class::FADE_OUT).ms == 400;
    // A pop from 93% over 270 ms, the alpha in 50
    anim::set_spec(class::LAYERS_IN, anim::Spec { enabled: true, ms: 270, curve: 0, style: style::POPIN, arg: 0.93 });
    anim::set_spec(class::FADE_LAYERS_IN, anim::Spec { enabled: true, ms: 50, curve: 0, style: style::FADE, arg: 1.0 });
    anim::add_layer_rule(layer_rule::NOANIM, "quickshell:overview");
    let matches = anim::layer_has(&libmay::conf::Text::from("quickshell:overview"), layer_rule::NOANIM)
        && !anim::layer_has(&libmay::conf::Text::from("quickshell:bar"), layer_rule::NOANIM);

    let band = protocol::layer::TOP;
    let anchor = protocol::layer_surface::ANCHOR_TOP;
    let shown = |cl: u32, surface: u32| mine(&[cl]).iter().find(|p| p.surface == surface)
        .map(|p| (p.fx.alpha, p.scale, p.animating));
    let (mut early, mut later, mut landed, mut still) = (None, None, None, None);
    if let Some((cl, _c, surface, _ls, handle, _b, _)) = super::layer::layer_client_named(band, anchor, -1, 64, "quickshell:bar") {
        let t0 = layout::now_us();
        layout::tick(t0 + 25_000);
        early = shown(cl, surface);
        layout::tick(t0 + 80_000);
        later = shown(cl, surface);
        layout::tick(t0 + 400_000);
        landed = shown(cl, surface);
        server::disconnect(cl);
        let _ = shm::destroy(handle);
    }
    if let Some((cl, _c, surface, _ls, handle, _b, _)) = super::layer::layer_client_named(band, anchor, -1, 64, "quickshell:overview") {
        still = shown(cl, surface);
        server::disconnect(cl);
        let _ = shm::destroy(handle);
    }
    anim::reset();
    anim::set_enabled(was_on);
    let cleared = !anim::layer_has(&libmay::conf::Text::from("quickshell:overview"), layer_rule::NOANIM);

    // Half way through its 50 ms of alpha and a tenth of its 270 of size;
    // then alpha whole and still growing; then landed
    let clocks = early.is_some_and(|(a, s, run)| a > 0.3 && a < 0.8 && s < 0.95 && run)
        && later.is_some_and(|(a, s, run)| a > 0.999 && s < 0.999 && run)
        && landed.is_some_and(|(a, s, run)| a > 0.999 && s > 0.999 && !run);
    let at_once = still.is_some_and(|(a, s, run)| a > 0.999 && s > 0.999 && !run);
    t.check(NAME, from_fade && from_layers && matches && clocks && at_once && cleared,
            format!("fade tree {} {}, rule matches {}; at 25 ms {:?}, 80 ms {:?}, 400 ms {:?}; \
                     noanim layer {:?}; reset forgets the rule {}",
                    from_fade, from_layers, matches, early, later, landed, still, cleared));
}
