// The shell below the windows, and what the effects pass may write over
//
// The folding and the margin rules are decided on the processor from the
// frame's list, so they are checked here as arithmetic: which surfaces fold,
// what the image owes when one of them changes, what the folded surface looks
// like to the rest of the frame, and the three rules about what the pass may
// paint over (`dev/mayland-below-windows.md`). The image itself and the
// chain that draws it need the card.

use super::*;

use crate::mayland::render::{base, compose, effects, Effects};
use crate::mayland::render::damage::Rect;

const SCREEN: Rect = Rect { x0: 0, y0: 0, x1: 1920, y1: 1080 };

/// One surface, as the frame's list carries it
fn surface(client: u32, id: u32, (x, y, w, h): (i32, i32, i32, i32), below: bool, argb: bool)
    -> server::Presentable
{
    server::Presentable {
        client,
        surface: id,
        phys: 0x1000_0000,
        vram: None,
        gpu: None,
        from: None,
        width: w,
        height: h,
        stride: w as usize * 4,
        format: if argb { protocol::shm::FORMAT_ARGB8888 } else { protocol::shm::FORMAT_XRGB8888 },
        offset: (x, y),
        damage: Vec::new(),
        fx: Effects { on: true, ..Effects::DEFAULT },
        decorated: !below,
        composed: !below,
        scale: 1.0,
        animating: false,
        below,
    }
}

/// The wallpaper, negi's background over it, a window, and the bar
fn desktop() -> Vec<server::Presentable> {
    alloc::vec![
        surface(2, 8, (0, 0, 1920, 1080), true, false),
        surface(5, 11, (0, 0, 1920, 1080), true, true),
        surface(3, 7, (4, 44, 954, 1032), false, false),
        surface(5, 79, (0, 0, 1920, 63), false, true),
    ]
}

/// Which surfaces fold: the bottom run of the shell below the windows, and
/// nothing past the first that is not
pub(super) fn test_members(t: &mut Runner) {
    let list = desktop();
    let two = base::members(&list);
    // A window at the bottom: nothing below it to fold
    let mut window_first = desktop();
    window_first.swap(0, 2);
    let none = base::members(&window_first);
    // A layer drawn smaller than itself is the frame's to draw, and the run
    // stops there
    let mut shrinking = desktop();
    shrinking[1].scale = 0.5;
    let one = base::members(&shrinking);
    t.check("below: the wallpaper and the layers under the windows fold, up to the first window",
            two == 2 && none == 0 && one == 1,
            format!("desktop {}, window first {}, a shrinking layer {}", two, none, one));
}

/// What the image owes when a member changes
pub(super) fn test_owed(t: &mut Runner) {
    let list = desktop();
    let was: Vec<base::Member> = list[..2].iter().map(base::Member::of).collect();

    // Still, with a clock redrawn: its damage, where it is on the screen
    let mut clock = desktop();
    clock[1].damage = alloc::vec![(100, 200, 50, 20)];
    let still = base::owed(&was, &clock[..2], &SCREEN);
    let still_ok = still == alloc::vec![Rect::new(100, 200, 50, 20)];

    // Nothing changed: nothing owed
    let quiet = base::owed(&was, &list[..2], &SCREEN).is_empty();

    // A layer that moved: where it was and where it is
    let small_was = alloc::vec![was[0], base::Member { rect: Rect::new(10, 10, 100, 100), ..was[1] }];
    let mut moved = desktop();
    moved[1].offset = (300, 300);
    moved[1].width = 100;
    moved[1].height = 100;
    let mv = base::owed(&small_was, &moved[..2], &SCREEN);
    let mv_ok = mv.iter().any(|r| r.contains(Rect::new(10, 10, 100, 100)))
        && mv.iter().any(|r| r.contains(Rect::new(300, 300, 100, 100)));

    // A layer that went: where it was. One that came: all of it
    let gone = base::owed(&small_was, &list[..1], &SCREEN);
    let gone_ok = gone.iter().any(|r| r.contains(Rect::new(10, 10, 100, 100)));
    let came = base::owed(&was[..1], &list[..2], &SCREEN);
    let came_ok = came == alloc::vec![SCREEN];

    // A curve running on one: all of it, every frame
    let mut fading = desktop();
    fading[1].animating = true;
    let anim = base::owed(&was, &fading[..2], &SCREEN);
    let anim_ok = anim == alloc::vec![SCREEN];

    t.check("below: the image owes a still layer's damage, a moved one's two places, a gone or new or animating one all of it",
            still_ok && quiet && mv_ok && gone_ok && came_ok && anim_ok,
            format!("still {:?}, quiet {}, moved {:?}, gone {:?}, came {:?}, animating {:?}",
                    still, quiet, mv, gone, came, anim));
}

/// The surface the members become, and what one update draws
pub(super) fn test_folded(t: &mut Runner) {
    const VRAM: u64 = 0x1e0_0000;
    let damage = alloc::vec![Rect::new(100, 200, 50, 20)];
    let f = base::folded(VRAM, 7680, &SCREEN, &damage, Effects { on: true, ..Effects::DEFAULT });
    let shape = f.vram == Some(VRAM) && f.gpu == Some(VRAM) && f.phys == 0
        && (f.client, f.surface) == base::KEY
        && f.width == 1920 && f.height == 1080 && f.stride == 7680
        && f.format == protocol::shm::FORMAT_XRGB8888
        && f.damage == alloc::vec![(100, 200, 50, 20)]
        && !f.decorated && !f.composed && !f.fx.on && f.below;

    // A clear of the rectangle around what is owed, then each member over it
    // bottom first, cut to where it is
    let mut list = desktop();
    list[1].offset = (120, 0);
    list[1].width = 400;
    let ops = base::ops_for(&list[..2], &damage, &SCREEN);
    let ops_ok = ops.len() == 3
        && ops[0].layer.is_none() && (ops[0].x, ops[0].y, ops[0].w, ops[0].h) == (100, 200, 50, 20)
        && ops[1].layer == Some(0) && (ops[1].x, ops[1].w) == (100, 50)
        && ops[2].layer == Some(1) && (ops[2].x, ops[2].w) == (120, 30);
    // A member that misses the rectangle is not drawn at all
    list[1].offset = (600, 0);
    let missed = base::ops_for(&list[..2], &damage, &SCREEN);
    let ops_ok = ops_ok && missed.len() == 2 && missed[1].layer == Some(0);

    t.check("below: the folded surface is the image in video memory, opaque and screen-sized, and an update clears then lays each member",
            shape && ops_ok,
            format!("vram {:?} gpu {:?} phys {:#x} {}x{} damage {:?}; ops {:?}",
                    f.vram, f.gpu, f.phys, f.width, f.height, f.damage,
                    ops.iter().map(|o| (o.layer, o.x, o.y, o.w, o.h)).collect::<Vec<_>>()));
}

/// A window's margin, and what the pass may write over
pub(super) fn test_margins(t: &mut Runner) {
    // Against the screen: four pixels from the left edge is four of shadow
    let edge = effects::margin_for(Rect::new(4, 300, 400, 300), 24, &SCREEN, &[]);
    // Floating in the middle, nothing below: all of it
    let free = effects::margin_for(Rect::new(600, 300, 400, 300), 24, &SCREEN, &[]);
    // Ten pixels right of an effect window below it: ten
    let beside = effects::margin_for(Rect::new(600, 300, 400, 300), 24, &SCREEN,
                                     &[Rect::new(90, 300, 500, 300)]);
    // Diagonally off one, eight across and thirty down: the wider gap
    let diagonal = effects::margin_for(Rect::new(600, 300, 400, 300), 24, &SCREEN,
                                       &[Rect::new(92, 0, 500, 270)]);
    let margins = edge == 4 && free == 24 && beside == 10 && diagonal == 24;

    // Overlapping an effect window below is `Over`; beside one is not
    let below = [Rect::new(90, 300, 500, 300), Rect::new(1200, 100, 300, 300)];
    let over = effects::first_overlap(&below, &Rect::new(1100, 200, 300, 300));
    let clear = effects::first_overlap(&below, &Rect::new(600, 300, 400, 300));

    // Something above an effect window goes after the pass; something below
    // it, or beside it, goes before
    let picked = [(2usize, Rect::new(0, 20, 958, 1056))];
    let after = effects::lies_over(3, &Rect::new(0, 0, 1920, 63), &picked);
    let before = effects::lies_over(1, &Rect::new(0, 0, 1920, 1080), &picked);
    let beside_ok = !effects::lies_over(3, &Rect::new(1000, 0, 100, 10), &picked);

    t.check("below: a margin is cut to the screen and to the effect windows below; an overlap is refused; what lies over the pass goes after it",
            margins && over == Some(1) && clear.is_none() && after && !before && beside_ok,
            format!("margins edge {} free {} beside {} diagonal {}; overlap {:?} clear {:?}; after {} before {} beside {}",
                    edge, free, beside, diagonal, over, clear, after, before, beside_ok));
}

/// A window the pass refused is rounded by the processor, and one it took is
/// left to the pass
pub(super) fn test_refused_is_dressed(t: &mut Runner) {
    let list = desktop();
    let w = &list[2];
    let frame = compose::Dress::Frame;
    let taken = compose::dressed_by(w, &SCREEN, frame, true);
    let refused = compose::dressed_by(w, &SCREEN, frame, false);
    // The cursor patch reproduces the screen, whoever dressed the window
    let patch = compose::dressed_by(w, &SCREEN, compose::Dress::Patch, true);
    let blends = compose::blending(&list);
    t.check("below: the processor dresses a window the pass refused and not one it took; only what really blends decides the pass",
            !taken && refused && patch && blends == alloc::vec![1, 3],
            format!("taken {}, refused {}, patch {}, blending {:?}", taken, refused, patch, blends));
}

/// The pass composes in stacking order (`dev/mayland-layers-above.md`,
/// section 3): what it takes, what it stacks, and what it still refuses
pub(super) fn test_stacked(t: &mut Runner) {
    // The shell below the windows, the see-through console, the overview over
    // it and the bar over both
    let console = surface(3, 7, (4, 44, 1912, 1152), false, true);
    let mut overview = surface(5, 90, (0, 40, 1920, 1160), false, true);
    overview.decorated = false;
    let mut bar = surface(5, 79, (0, 0, 1920, 63), false, true);
    bar.decorated = false;
    let list = alloc::vec![surface(2, 8, (0, 0, 1920, 1080), true, false), console, overview, bar];
    let trans = [1usize, 2, 3];
    let mut refused = Vec::new();
    let taken = effects::choose(&list, &SCREEN, &trans, &mut refused);
    let shape: Vec<(usize, bool)> = taken.iter().map(|w| (w.index, w.window.stacked)).collect();
    let all_three = shape == alloc::vec![(1, false), (2, true), (3, true)] && refused.is_empty();

    // A plain layer with nothing see-through about it stays a copy
    let mut opaque = surface(5, 91, (100, 100, 300, 200), false, false);
    opaque.decorated = false;
    let list2 = alloc::vec![surface(2, 8, (0, 0, 1920, 1080), true, false), opaque];
    let mut refused2 = Vec::new();
    let none = effects::choose(&list2, &SCREEN, &[], &mut refused2).is_empty() && refused2.len() == 1;

    // Between the console and the overview, an opaque surface the pass does not
    // draw (a plain one, copied after the pass because it lies over an effect
    // window): the overview would be laid under it, so it is refused. A
    // see-through one there is `Under`, which is asked first
    let mut middle = surface(6, 12, (200, 200, 400, 300), false, false);
    middle.decorated = false;
    let mut over = surface(5, 92, (300, 250, 400, 300), false, true);
    over.decorated = false;
    let list3 = alloc::vec![surface(2, 8, (0, 0, 1920, 1080), true, false),
                            surface(3, 7, (4, 44, 1912, 1152), false, true), middle, over];
    let mut refused3 = Vec::new();
    let taken3 = effects::choose(&list3, &SCREEN, &[1, 3], &mut refused3);
    let between = taken3.len() == 1 && taken3[0].index == 1
        && refused3.iter().any(|&(i, r)| i == 3 && r == effects::Refusal::Between(6, 12));

    // Over a see-through surface the pass does not draw, with no effect window
    // below: refused
    let mut under_it = surface(6, 13, (200, 200, 400, 300), false, true);
    under_it.composed = false;
    let mut top = surface(5, 93, (300, 250, 400, 300), false, true);
    top.decorated = false;
    let list4 = alloc::vec![surface(2, 8, (0, 0, 1920, 1080), true, false), under_it, top];
    let mut refused4 = Vec::new();
    let under = effects::choose(&list4, &SCREEN, &[1, 2], &mut refused4).is_empty()
        && refused4.iter().any(|&(i, r)| i == 2 && r == effects::Refusal::Under(6, 13));

    // The clips: the bar redoes a strip across the top, and the console
    // under it has to redo the same strip, or the bar reads last frame's
    let mut clipped = taken;
    clipped[0].window.clip = (900, 600, 100, 100);
    clipped[1].window.clip = (0, 0, 0, 0);
    clipped[2].window.clip = (0, 0, 1920, 63);
    effects::settle_stacked_clips(&mut clipped);
    let c0 = clipped[0].clip_rect();
    let c1 = clipped[1].clip_rect();
    // The console's clip now takes in the bar's strip where they meet, and
    // the overview's whole region, which is its clip
    let settled = c0.x0 <= 4 && c0.y0 <= 44 && c0.x1 >= 1916 && c0.y1 >= 700
        && c1 == clipped[1].rect;

    // And upwards. With a click on the bar's magnifier as the overview opens,
    // the ripple and the overview's edge make the bar's clip one wide box. The
    // screen's corner over it, cut from the ripple alone, must not keep a clip
    // smaller than what the bar writes over it
    let mut bar2 = surface(5, 79, (0, 0, 1920, 63), false, true);
    bar2.decorated = false;
    let mut corner = surface(5, 80, (0, 0, 23, 23), false, true);
    corner.decorated = false;
    let list5 = alloc::vec![surface(2, 8, (0, 0, 1920, 1080), true, false), bar2, corner];
    let mut refused5 = Vec::new();
    let mut up = effects::choose(&list5, &SCREEN, &[1, 2], &mut refused5);
    up[0].window.clip = (0, 9, 1920, 54);
    up[1].window.clip = (12, 9, 11, 14);
    effects::settle_stacked_clips(&mut up);
    let (bar_clip, corner_clip) = (up[0].clip_rect(), up[1].clip_rect());
    let upwards = up[1].window.stacked
        && corner_clip == Rect { x0: 0, y0: 9, x1: 23, y1: 23 }
        && bar_clip == Rect { x0: 0, y0: 9, x1: 1920, y1: 63 };

    t.check("below: see-through layers over a see-through window are stacked in the pass; an opaque plain one, one with an undrawn surface between and one over an undrawn see-through one are not; clips settle downwards and upwards",
            all_three && none && between && under && settled && upwards,
            format!("taken {:?} refused {}; opaque layer refused {}; between {} ({} taken); under {}; console clip {:?}; bar clip {:?}, corner clip {:?}",
                    shape, refused.len(), none, between, taken3.len(), under,
                    (c0.x0, c0.y0, c0.x1, c0.y1), bar_clip, corner_clip));
}
