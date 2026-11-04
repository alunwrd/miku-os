// may_wm: the desktop's policy, and only that
//
// Where a Hyprland config ends up: the layout, the gaps, the animation curves,
// the border, the key chords and the window rules all cross this interface.
//
// One holder, and no seat to race for
//
// One client holds this interface, decided at bind: the registry hands it over
// once and refuses everybody else until that connection goes. Holding it is
// commanding. The facts a shell needs are in `may_desktop`, and a panel binds
// that alone.

use crate::mayland::protocol;
use crate::mayland::wire::{self, Arg, Reader};
use crate::mayland::shell::layout;

use crate::mayland::server::{focus, scene, surface};
use crate::mayland::server::state::*;
use super::desktop;

/// Whether something already holds the window manager's interface
///
/// For the selftest: with a window manager running, its `may_wm` cases cannot
/// run, and the cases about the desktop are gated on this too, because a client
/// driving the machine makes them unsafe.
///
/// Takes the lock itself: every caller is outside the server
pub fn wm_held() -> bool { holder(&SERVER.lock(), &WM).is_some() }

/// The same question for the console's interface: is anybody drawing any
/// terminal? The selftest asks so it can skip cases that would evict a live one
pub fn console_held() -> bool {
    !crate::mayland::server::state::console_leases(&SERVER.lock()).is_empty()
}

/// Sets one of the desktop's numbers, whoever owns it
///
/// The params are one vocabulary and two owners: the layout keeps the gaps and
/// the compositor keeps the decoration. Both are asked in order
pub(in crate::mayland) fn set_param(s: &mut Server, p: u32, v: f32) {
    use protocol::wm::param;
    match p {
        param::GAPS_OUT => {
            let (_, inner) = layout::gaps();
            layout::set_gaps_in(s, v as i32, inner);
        }
        param::GAPS_IN => {
            let (outer, _) = layout::gaps();
            layout::set_gaps_in(s, outer, v as i32);
        }
        param::SPLIT_RATIO | param::MASTER_FACTOR => { layout::set_param(s, p, v); }
        // `animations { enabled = false }` has to reach the table: the
        // per-class lines below it each set `enabled` themselves
        param::ANIM_ENABLED => crate::mayland::shell::anim::set_enabled(v != 0.0),
        _ => {
            // How the machine behaves under a hand: repeat, sensitivity,
            // whether the keyboard follows the cursor
            if crate::mayland::input::conf::set(p, v) {
                // A keyboard that bound before the config was read was told the
                // old numbers. Everything else here is read when needed; this
                // one was pushed, so it is pushed again
                if p == param::REPEAT_RATE || p == param::REPEAT_DELAY {
                    super::seat::send_repeat_info(s);
                }
                // The same for the layouts: every keymap already sent says the
                // old list, and the bar shows a group of it
                if p == param::KB_LAYOUTS {
                    layouts_changed(s);
                }
                return;
            }
            if crate::mayland::render::set_param(p, v) {
                return;
            }
            // Not ours, and not an error: the window manager may be newer than
            // the compositor
        }
    }
}

/// The seat's layouts are a different list: rewrites every keymap, and tells
/// the focused window and the bar which group is in use now
///
/// The group may have moved as well: a shorter list has no group where the old
/// one was. The drain notices that and sends `modifiers`; the bar is told here
/// because the name of group 1 may have changed
fn layouts_changed(s: &mut Server) {
    super::seat::rewrite_keymaps(s);
    super::desktop::send_keyboard_layout(s);
}

/// Every desktop-wide number, back to what this compositor starts with
///
/// Sent before a config is applied, so what a file no longer says stops being
/// true. Everything it touches keeps its default in one named constant beside
/// the state itself (`Effects::DEFAULT`, `Border::DEFAULT`, `Params::DEFAULT`).
///
/// Not reset: chords, addressed by id and about to be re-bound by the sender;
/// per-window numbers, which belong to windows and would take a rule with them;
/// and the layout, which the sender always states outright
pub(in crate::mayland) fn reset(s: &mut Server) {
    layout::reset(s);
    crate::mayland::render::effects::reset();
    crate::mayland::shell::anim::reset();
    crate::mayland::input::conf::reset();
    // Including the picture: a `wallpaper` line deleted from the config has to
    // take it with it
    crate::mayland::shell::wallpaper::set_image(None);
    // Every keyboard was told a repeat rate that has just changed back, and
    // a list of layouts that may have
    super::seat::send_repeat_info(s);
    layouts_changed(s);
    crate::mayland::render::force_full_frame();
}

/// Fullscreen, and the arrangement that follows from it
///
/// A state rather than a size, because leaving it puts the window back into
/// whatever the rest of the desktop has become, and only the layout knows that
pub(in crate::mayland) fn set_fullscreen(s: &mut Server, si: usize, on: bool) {
    if s.surfaces[si].fullscreen == on {
        return;
    }
    s.surfaces[si].fullscreen = on;
    if on {
        // It is over everything by being at the top of the compositor's one
        // stack
        focus::raise(s, si);
    }
    layout::arrange(s);
    crate::mayland::render::force_full_frame();
}

/// Which window a grab is about
///
/// `(0, 0)` means the one under the pointer, which a mouse binding uses: the
/// window manager knows a button was pressed and only the compositor knows
/// where. A named window is the other case: a client asking to be moved through
/// `may_toplevel`, or a shell with a window in mind
fn grab_target(s: &Server, client: u32, surface: u32) -> Option<(u32, u32)> {
    let key = if client == 0 && surface == 0 {
        s.pointer_focus?
    } else {
        (client, surface)
    };
    // A window that is not there, or is the wallpaper, or a layer surface:
    // none of them is something to carry about
    let si = s.surface_idx(key)?;
    (s.surfaces[si].role == Role::Toplevel).then_some(key)
}

/// Which corner a resize is pulling, when nobody said
///
/// The quadrant the pointer is in: grab the near half of each axis. A pointer
/// dead in the middle gets the bottom right
pub(super) fn edges_from_pointer(g: layout::Rect) -> u32 {
    use protocol::wm::edge;
    let (px, py) = crate::mayland::input::pointer::position();
    let horizontal = if px < g.x + g.w / 2 { edge::LEFT } else { edge::RIGHT };
    let vertical = if py < g.y + g.h / 2 { edge::TOP } else { edge::BOTTOM };
    horizontal | vertical
}

pub(in crate::mayland) fn on_wm(s: &mut Server, idx: usize, _sender: u32, opcode: u16, r: &mut Reader)
    -> Result<(), &'static str>
{
    // No arbitration here: a client holding this object is the one driving,
    // because the registry gave it to nobody else. `idx` is still the sender's,
    // which everything below writes back through
    let _ = idx;

    // The pair every request but four names its window by
    let mut window = || -> Result<(u32, u32), &'static str> {
        let c = r.uint().map_err(|_| "may_wm: truncated")?;
        let sf = r.uint().map_err(|_| "may_wm: truncated")?;
        Ok((c, sf))
    };

    match opcode {
        protocol::wm::MOVE => {
            let (c, sf) = window()?;
            let x = r.int().map_err(|_| "move: truncated")?;
            let y = r.int().map_err(|_| "move: truncated")?;
            if let Some(si) = s.surface_idx((c, sf)) {
                let size = scene::size_of(s, si).unwrap_or((0, 0));
                let (x, y) = layout::clamp_pos(s, (x, y), size);
                layout::put(s, si, x, y);
            }
            Ok(())
        }
        protocol::wm::RESIZE => {
            let (c, sf) = window()?;
            let w = r.int().map_err(|_| "resize: truncated")?;
            let h = r.int().map_err(|_| "resize: truncated")?;
            if let Some(si) = s.surface_idx((c, sf)) {
                if w > 0 && h > 0 {
                    surface::configure(s, si, w, h);
                }
            }
            Ok(())
        }
        protocol::wm::SET_LAYOUT => {
            let l = r.uint().map_err(|_| "set_layout: truncated")?;
            let k = match l {
                protocol::wm::LAYOUT_MASTER => layout::Kind::Master,
                protocol::wm::LAYOUT_DWINDLE => layout::Kind::Dwindle,
                _ => layout::Kind::Float,
            };
            layout::set_kind_in(s, k);
            Ok(())
        }
        protocol::wm::SET_GAPS => {
            let outer = r.int().map_err(|_| "set_gaps: truncated")?;
            let inner = r.int().map_err(|_| "set_gaps: truncated")?;
            layout::set_gaps_in(s, outer, inner);
            Ok(())
        }
        protocol::wm::SET_PARAM => {
            let p = r.uint().map_err(|_| "set_param: truncated")?;
            let v = r.fixed().map_err(|_| "set_param: truncated")?;
            set_param(s, p, wire::fixed_to_f32(v));
            Ok(())
        }
        protocol::wm::SET_WINDOW_PARAM => {
            let (c, sf) = window()?;
            let p = r.uint().map_err(|_| "set_window_param: truncated")?;
            let v = r.fixed().map_err(|_| "set_window_param: truncated")?;
            let set = r.uint().map_err(|_| "set_window_param: truncated")? != 0;
            let Some(si) = s.surface_idx((c, sf)) else { return Ok(()) };
            // The layout is asked first, because one param is about a window's
            // place rather than its paint: `split_ratio` on a window means the
            // split above it, as Hyprland's dispatcher moves. Unknown params
            // are ignored: a config written for a compositor with one more knob
            // should still put a desktop on the screen
            if p == protocol::wm::param::SPLIT_RATIO {
                layout::set_window_split(s, (c, sf), wire::fixed_to_f32(v));
            } else if s.surfaces[si].fx.set(p, wire::fixed_to_f32(v), set) {
                crate::mayland::render::force_full_frame();
            }
            Ok(())
        }
        protocol::wm::SET_WINDOW_STATE => {
            let (c, sf) = window()?;
            let mask = r.uint().map_err(|_| "set_window_state: truncated")?;
            let flags = r.uint().map_err(|_| "set_window_state: truncated")?;
            let Some(si) = s.surface_idx((c, sf)) else { return Ok(()) };
            if mask & !protocol::wm::state::ALL != 0 {
                return Err("set_window_state: a bit this compositor does not have");
            }
            if mask & protocol::wm::state::FLOATING != 0 {
                s.surfaces[si].floating = flags & protocol::wm::state::FLOATING != 0;
            }
            if mask & protocol::wm::state::PINNED != 0 {
                s.surfaces[si].pinned = flags & protocol::wm::state::PINNED != 0;
            }
            if mask & protocol::wm::state::NO_FOCUS != 0 {
                focus::set_no_focus_at(s, si,
                    flags & protocol::wm::state::NO_FOCUS != 0);
            }
            if mask & protocol::wm::state::FULLSCREEN != 0 {
                set_fullscreen(s, si, flags & protocol::wm::state::FULLSCREEN != 0);
            } else {
                // `set_fullscreen` arranges on its own; the other two need it
                // here, and arranging twice would configure every window for
                // one flag
                layout::arrange(s);
            }
            desktop::send_window(s, si);
            Ok(())
        }
        protocol::wm::SWAP => {
            let (ca, sa) = window()?;
            let (cb, sb) = window()?;
            let (Some(a), Some(b)) = (s.surface_idx((ca, sa)), s.surface_idx((cb, sb)))
                else { return Ok(()) };
            if a == b {
                return Ok(());
            }
            // Trading places in `surfaces` rather than in a list of the tiled
            // ones: the order of `surfaces` is the order a layout reads
            // ("oldest window is master")
            s.surfaces.swap(a, b);
            layout::arrange(s);
            Ok(())
        }
        protocol::wm::SET_CURVE => {
            let id = r.uint().map_err(|_| "set_curve: truncated")?;
            let x1 = wire::fixed_to_f32(r.fixed().map_err(|_| "set_curve: truncated")?);
            let y1 = wire::fixed_to_f32(r.fixed().map_err(|_| "set_curve: truncated")?);
            let x2 = wire::fixed_to_f32(r.fixed().map_err(|_| "set_curve: truncated")?);
            let y2 = wire::fixed_to_f32(r.fixed().map_err(|_| "set_curve: truncated")?);
            crate::mayland::shell::anim::set_curve(id, x1, y1, x2, y2);
            Ok(())
        }
        protocol::wm::SET_ANIMATION => {
            let class = r.uint().map_err(|_| "set_animation: truncated")?;
            let enabled = r.uint().map_err(|_| "set_animation: truncated")? != 0;
            let speed = wire::fixed_to_f32(r.fixed().map_err(|_| "set_animation: truncated")?);
            let curve = r.uint().map_err(|_| "set_animation: truncated")?;
            let style = r.uint().map_err(|_| "set_animation: truncated")?;
            let arg = wire::fixed_to_f32(r.fixed().map_err(|_| "set_animation: truncated")?);
            if class >= protocol::wm::anim_class::COUNT {
                return Err("set_animation: no such class");
            }
            // Speed is in hundreds of milliseconds, as every config of this
            // shape writes it. Clamped rather than refused: a curve of zero
            // length is a warp, and one of a minute is somebody experimenting
            let ms = ((speed * 100.0) as i32).clamp(0, 60_000) as u32;
            crate::mayland::shell::anim::set_spec(class, crate::mayland::shell::anim::Spec {
                enabled, ms, curve, style, arg,
            });
            Ok(())
        }
        protocol::wm::SET_BORDER => {
            let size = r.uint().map_err(|_| "set_border: truncated")?;
            let active = r.uint().map_err(|_| "set_border: truncated")?;
            let inactive = r.uint().map_err(|_| "set_border: truncated")?;
            // Clamped rather than refused, as a speed is: a config asking for a
            // border half the window wide should still start the desktop. The
            // colours keep their alpha: a border is blended
            // (`dev/mayland-window-shape.md`), and an alpha of 0 is a client
            // that wrote `0xRRGGBB`, which is solid
            let said = |c: u32| if c >> 24 == 0 { c | 0xFF00_0000 } else { c };
            crate::mayland::render::set_border(size as i32, said(active), said(inactive));
            Ok(())
        }
        protocol::wm::BEGIN_MOVE => {
            let (c, sf) = window()?;
            let Some(key) = grab_target(s, c, sf) else { return Ok(()) };
            let Some(si) = s.surface_idx(key) else { return Ok(()) };
            // Picking a window up brings it to the front
            focus::raise(s, si);
            crate::mayland::input::grab::begin_move(key.0, key.1);
            Ok(())
        }
        protocol::wm::BEGIN_RESIZE => {
            let (c, sf) = window()?;
            let edges = r.uint().map_err(|_| "begin_resize: truncated")?;
            if edges & !protocol::wm::edge::ALL != 0 {
                return Err("begin_resize: an edge this compositor does not have");
            }
            let Some(key) = grab_target(s, c, sf) else { return Ok(()) };
            let Some(si) = s.surface_idx(key) else { return Ok(()) };
            // The size it starts from is the goal rather than what is on
            // screen: a window still animating towards a size would otherwise
            // jump back to where the animation had got to
            let g = s.surfaces[si].goal;
            let edges = if edges != 0 { edges } else { edges_from_pointer(g) };
            focus::raise(s, si);
            crate::mayland::input::grab::begin_resize(key.0, key.1, edges, g.w, g.h);
            Ok(())
        }
        protocol::wm::BIND_CHORD => {
            let id = r.uint().map_err(|_| "bind_chord: truncated")?;
            let mods = r.uint().map_err(|_| "bind_chord: truncated")?;
            let key = r.uint().map_err(|_| "bind_chord: truncated")?;
            // The letters after `bind` in the line this came from. Bits with no
            // meaning here are dropped rather than refused, as for an unknown
            // param: a window manager may be newer than the compositor under it
            let flags = r.uint().map_err(|_| "bind_chord: truncated")?;
            crate::mayland::input::bind_chord(id, mods, key, flags)
        }
        protocol::wm::RESET => {
            reset(s);
            Ok(())
        }
        protocol::wm::SET_WALLPAPER => {
            let path = r.string().map_err(|_| "set_wallpaper: truncated")?
                .unwrap_or("").trim();
            crate::mayland::shell::wallpaper::set_image(
                (!path.is_empty()).then_some(path));
            Ok(())
        }
        protocol::wm::SET_LAYER_RULE => {
            let rule = r.uint().map_err(|_| "set_layer_rule: truncated")?;
            let pattern = r.string().map_err(|_| "set_layer_rule: truncated")?.unwrap_or("").trim();
            if rule != protocol::wm::layer_rule::NOANIM {
                return Err("set_layer_rule: no such rule");
            }
            if !pattern.is_empty() {
                crate::mayland::shell::anim::add_layer_rule(rule, pattern);
            }
            Ok(())
        }
        protocol::wm::WARP_POINTER => {
            let x = r.int().map_err(|_| "warp_pointer: truncated")?;
            let y = r.int().map_err(|_| "warp_pointer: truncated")?;
            // Clamped rather than refused: a config that asked for the corner
            // of a bigger screen than this one wants the corner of this one
            crate::mayland::input::pointer::place(
                x.clamp(0, (s.width - 1).max(0)),
                y.clamp(0, (s.height - 1).max(0)));
            // The pointer has crossed into something else without a mouse
            // having moved, which `pointer_motion` never sees. `repoint`
            // handles that, as for a surface arriving or leaving under a still
            // cursor
            focus::repoint(s);
            Ok(())
        }
        protocol::wm::UNBIND_CHORD => {
            let id = r.uint().map_err(|_| "unbind_chord: truncated")?;
            crate::mayland::input::unbind_chord(id);
            Ok(())
        }
        _ => Err("may_wm: unknown request"),
    }
}

/// A chord the window manager asked for was pressed
///
/// Called from the compositor thread by `input::drain`, never from the keyboard
/// interrupt: this takes the server lock, and the interrupt cannot wait for it
pub fn wm_chord(id: u32) {
    let mut s = SERVER.lock();
    // Delivered to whoever bound it, the one client holding this interface
    let Some(l) = holder(&s, &WM) else { return };
    let Some(idx) = s.client_idx(l.client) else { return };
    s.clients[idx].queue(l.object, protocol::wm::CHORD, &[Arg::Uint(id)]);
}
