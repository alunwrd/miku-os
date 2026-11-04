// Pointer dispatch: hover, press, and scroll tracking across `MouseArea`s
//
// Split out of `shell.rs` alongside `keys.rs` when keyboard support was added
// Translates window coordinates to local area coordinates

use rquickjs::{Ctx, Object};

use crate::mayland::Display;
use crate::object::engine::Engine;
use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::scene::Scene;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// Scroll distance per wheel notch in a `ListView`: 3 lines of 16px (48px
/// total)
const WHEEL_PX: f64 = 48.0;

/// Maximum interval between two presses to register a double-click
const DOUBLE_CLICK_MS: u64 = 400;

/// Pointer state and interaction tracking across `MouseArea`s
#[derive(Default)]
pub struct Pointer {
    pub win: Option<usize>,
    pub x: f64,
    pub y: f64,
    hovered: Vec<Oid>,
    /// The `MouseArea` where a button press occurred and the kata mouse button
    /// ID
    pressed: Option<(Oid, u32)>,
    /// Previous click state (position, button, and timestamp) used to detect
    /// double-clicks
    last_click: Option<(Oid, u32, u64)>,
    /// Second press of a double click; its release emits no click (matches Qt)
    second: bool,
}

/// `{ x, y, button, buttons, modifiers, accepted }` in the area's own pixels
fn mouse<'js>(
    ctx: &Ctx<'js>,
    x: f64,
    y: f64,
    button: u32,
) -> rquickjs::Result<rquickjs::Value<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("x", x)?;
    o.set("y", y)?;
    o.set("button", button)?;
    o.set("buttons", button)?;
    o.set("modifiers", 0)?;
    o.set("accepted", true)?;
    o.set("wasHeld", false)?;
    Ok(o.into_value())
}

fn local(scene: &Scene, display: &Display, p: &Pointer, area: Oid) -> (f64, f64) {
    let Some(w) = p.win else { return (0.0, 0.0) };
    scene
        .local(&display.windows[w].scene, area, p.x, p.y)
        .unwrap_or((p.x, p.y))
}

/// Updates the hovered areas and notifies the ones the pointer left
pub fn hover(engine: &Engine, scene: &Scene, display: &Display, p: &mut Pointer) {
    let now: Vec<Oid> = match p.win {
        Some(w) => scene.areas_at(&engine.core, &display.windows[w].scene, p.x, p.y),
        None => Vec::new(),
    };
    let left: Vec<Oid> = p
        .hovered
        .iter()
        .copied()
        .filter(|o| !now.contains(o))
        .collect();
    let entered: Vec<Oid> = now
        .iter()
        .copied()
        .filter(|o| !p.hovered.contains(o))
        .collect();
    let moving: Vec<Oid> = now
        .iter()
        .copied()
        .chain(p.pressed.map(|(o, _)| o))
        .collect();
    let held = p.pressed.map(|(o, _)| (o, now.contains(&o)));
    let coords: Vec<(Oid, f64, f64)> = moving
        .iter()
        .map(|&o| {
            let (x, y) = local(scene, display, p, o);
            (o, x, y)
        })
        .collect();
    engine.with_ctx(|ctx, core| {
        // Control uses `hovered`, MouseArea uses `containsMouse`
        let over = |o: Oid| {
            if is_control(core, o) {
                "hovered"
            } else {
                "containsMouse"
            }
        };
        for o in &left {
            set(ctx, core, *o, over(*o), Value::Bool(false));
            core.emit(ctx, *o, "exited", Vec::new());
        }
        for o in &entered {
            set(ctx, core, *o, over(*o), Value::Bool(true));
            core.emit(ctx, *o, "entered", Vec::new());
        }
        // Press state follows whether the pointer is still inside
        if let Some((o, inside)) = held {
            set(ctx, core, o, "containsPress", Value::Bool(inside));
        }
        for &(o, x, y) in &coords {
            core.store_layout(ctx, o, "mouseX", x);
            core.store_layout(ctx, o, "mouseY", y);
            if let Ok(m) = mouse(ctx, x, y, 0) {
                core.emit(ctx, o, "positionChanged", vec![m]);
            }
        }
    });
    p.hovered = now;
}

pub fn press(
    engine: &Engine,
    scene: &mut Scene,
    display: &Display,
    p: &mut Pointer,
    button: u32,
    down: bool,
) {
    let qt = match button {
        BTN_LEFT => 1,
        BTN_RIGHT => 2,
        BTN_MIDDLE => 4,
        _ => return,
    };
    if down {
        let Some(w) = p.win else { return };
        let ws = &display.windows[w].scene;
        // A press on an uncovered TextInput sets focus and cursor
        if let Some((input, true)) = scene.press_target(&engine.core, ws, p.x, p.y) {
            if qt == 1 && engine.core.flag(input, "activeFocusOnPress") {
                let (x, _) = local(scene, display, p, input);
                let at = scene.cursor_at(&engine.core, input, x);
                engine.with_ctx(|ctx, core| {
                    crate::input::keys::force(ctx, core, input);
                    core.set_layout_value(ctx, input, "cursorPosition", Value::Num(at as f64));
                });
            }
            return;
        }
        let target = scene
            .hit(&engine.core, ws, p.x, p.y)
            .filter(|&o| accepted(&engine.core, o) & qt != 0);
        let Some(area) = target else { return };
        let (x, y) = local(scene, display, p, area);
        let now = engine.core.0.borrow().now_ms;
        let second = p.last_click.is_some_and(|(o, b, at)| {
            o == area && b == qt && now.saturating_sub(at) <= DOUBLE_CLICK_MS
        });
        engine.with_ctx(|ctx, core| {
            set(ctx, core, area, "pressed", Value::Bool(true));
            set(ctx, core, area, "containsPress", Value::Bool(true));
            set(ctx, core, area, "pressedButtons", Value::Num(qt as f64));
            // A button is down while it is pressed, unless it is told
            if crate::scene::is_button(core, area) {
                if !core.is_explicit(area, "down") {
                    set(ctx, core, area, "down", Value::Bool(true));
                }
                core.store_layout(ctx, area, "pressX", x);
                core.store_layout(ctx, area, "pressY", y);
            }
            if let Ok(m) = mouse(ctx, x, y, qt) {
                core.emit(ctx, area, "pressed", vec![m.clone()]);
                if second {
                    core.emit(ctx, area, "doubleClicked", vec![m]);
                }
            }
        });
        p.pressed = Some((area, qt));
        p.second = second;
        p.last_click = None;
    } else {
        let Some((area, b)) = p.pressed.take() else {
            return;
        };
        if b != qt {
            p.pressed = Some((area, b));
            return;
        }
        let (x, y) = local(scene, display, p, area);
        let (w, h) = (
            engine.core.num(area, "width"),
            engine.core.num(area, "height"),
        );
        let inside = p.win.is_some() && x >= 0.0 && y >= 0.0 && x < w && y < h;
        let second = std::mem::take(&mut p.second);
        engine.with_ctx(|ctx, core| {
            set(ctx, core, area, "pressed", Value::Bool(false));
            set(ctx, core, area, "containsPress", Value::Bool(false));
            set(ctx, core, area, "pressedButtons", Value::Num(0.0));
            let button = crate::scene::is_button(core, area);
            if button && !core.is_explicit(area, "down") {
                set(ctx, core, area, "down", Value::Bool(false));
            }
            // Checkable buttons toggle on click, before `released`
            if button && inside && !second {
                crate::scene::control::turn_over(ctx, core, area);
            }
            if let Ok(m) = mouse(ctx, x, y, qt) {
                core.emit(ctx, area, "released", vec![m.clone()]);
                if inside && !second {
                    core.emit(ctx, area, "clicked", vec![m]);
                }
            }
        });
        if inside && !second {
            p.last_click = Some((area, qt, engine.core.0.borrow().now_ms));
        }
    }
}

pub fn wheel(engine: &Engine, scene: &mut Scene, display: &Display, p: &Pointer, value: i32) {
    let Some(w) = p.win else { return };
    let Some((target, is_area)) =
        scene.wheel_target(&engine.core, &display.windows[w].scene, p.x, p.y)
    else {
        return;
    };
    if !is_area {
        // ListView content scrolls within its bounds. Mayland reports 15 units
        // per notch, positive down
        scroll(engine, target, value as f64 / 15.0 * WHEEL_PX);
        return;
    }
    let area = target;
    let (x, y) = local(scene, display, p, area);
    engine.with_ctx(|ctx, core| {
        let ev = (|| -> rquickjs::Result<rquickjs::Value> {
            let o = Object::new(ctx.clone())?;
            let d = Object::new(ctx.clone())?;
            d.set("x", 0)?;
            // Mayland: 15 per notch, positive down. Qt: 120 per notch, positive
            // away from the user
            d.set("y", -value * 8)?;
            o.set("angleDelta", d)?;
            o.set("x", x)?;
            o.set("y", y)?;
            o.set("accepted", true)?;
            Ok(o.into_value())
        })();
        if let Ok(ev) = ev {
            core.emit(ctx, area, "wheel", vec![ev]);
        }
    });
}

/// A ListView moved by `by` pixels along its length, kept within its ends
fn scroll(engine: &Engine, view: Oid, by: f64) {
    let core = &engine.core;
    if !core.flag(view, "interactive") {
        return;
    }
    let vertical = core.num(view, "orientation") as i64 != 1;
    let (pos, extent, size) = if vertical {
        (
            "contentY",
            core.num(view, "contentHeight"),
            core.num(view, "height"),
        )
    } else {
        (
            "contentX",
            core.num(view, "contentWidth"),
            core.num(view, "width"),
        )
    };
    let (before, after) = crate::model::margins(core, view, vertical);
    let to = (core.num(view, pos) + by).clamp(0.0, (extent + before + after - size).max(0.0));
    engine.with_ctx(|ctx, core| core.set_layout_value(ctx, view, pos, Value::Num(to)));
}

/// Buttons an area accepts: `acceptedButtons` for a MouseArea, left for a
/// button
fn accepted(core: &Core, oid: Oid) -> u32 {
    if crate::scene::is_button(core, oid) {
        1
    } else {
        core.num(oid, "acceptedButtons") as u32
    }
}

fn is_control(core: &Core, oid: Oid) -> bool {
    core.0
        .borrow()
        .objs
        .get(oid)
        .is_some_and(|o| crate::types::is_a(o.def, "Control"))
}

pub(super) fn set(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str, v: Value) {
    if let Some(i) = core.prop_idx(oid, name) {
        core.store(ctx, oid, i, v);
    }
}
