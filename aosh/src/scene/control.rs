// Control layout, following QQuickControl in the Basic style
// (`controls_are_qts`, `types/controls.rs`)
//
// - `background` and `contentItem` are children, the background below (`z` -1
//   unless set).
// - The background fills the control minus insets, unless it has its own size.
// - The content sits at the left and top padding and fills the rest
//   (`availableWidth`, `availableHeight`), unless anchors hold it.
// - Unset paddings inherit: `leftPadding` from `horizontalPadding` from
//   `padding`. A Button's `horizontalPadding` is 2 more.
// - With no size, a control is the larger of the background's implicit size
//   plus insets and the content's plus padding.
// - A ScrollBar's content is its handle: `size` of its length, at `position`.

use rquickjs::Ctx;

use crate::object::value::Oid;
use crate::object::Core;
use crate::types;

/// The control's background and content items, if it has them
fn parts(core: &Core, oid: Oid) -> (Option<Oid>, Option<Oid>) {
    let item = |name: &str| {
        core.peek_named(oid, name).and_then(|v| v.as_obj())
            .filter(|&o| core.0.borrow().objs.get(o).is_some_and(|x| types::is_a(x.def, "Item")))
    };
    (item("background"), item("contentItem"))
}

/// Axes on which the control sizes `item`: the background where it has no size,
/// the content where anchors do not size it
pub fn sizes(core: &Core, oid: Oid, item: Oid) -> Option<(bool, bool)> {
    let (bg, content) = parts(core, oid);
    if bg == Some(item) {
        return Some((!core.is_explicit(item, "width"), !core.is_explicit(item, "height")));
    }
    if content == Some(item) {
        let held = super::held(core, item);
        return Some((!held.width, !held.height));
    }
    None
}

/// Makes the background and content children, the background below
pub fn adopt(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let (bg, content) = parts(core, oid);
    let mut changed = false;
    for part in [bg, content].into_iter().flatten() {
        let mut s = core.0.borrow_mut();
        if !s.objs[oid].children.contains(&part) {
            s.objs[oid].children.push(part);
            s.objs[part].parent = Some(oid);
            changed = true;
        }
    }
    if changed {
        core.children_changed(ctx, oid);
    }
    if let Some(b) = bg {
        if core.num(b, "z") == 0.0 && !core.is_explicit(b, "z") {
            core.store_layout(ctx, b, "z", -1.0);
        }
    }
}

/// A padding: its value, or the inherited one
pub struct Pads {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

/// Paddings of a control, TextInput, or TextEdit (0 unless set). Basic style: a
/// Button's `horizontalPadding` is `padding` + 2, a TextField's or TextArea's
/// `leftPadding` is 4 more
pub fn pads(core: &Core, oid: Oid) -> Pads {
    let given = |n: &str| core.is_explicit(oid, n).then(|| core.num(oid, n));
    let padding = core.num(oid, "padding");
    let name = core.0.borrow().objs.get(oid).map(|o| o.def.name).unwrap_or("");
    let horizontal = given("horizontalPadding").unwrap_or(if name == "Button" { padding + 2.0 } else { padding });
    let vertical = given("verticalPadding").unwrap_or(padding);
    let field = matches!(name, "TextField" | "TextArea");
    Pads {
        left: given("leftPadding").unwrap_or(if field { padding + 4.0 } else { horizontal }),
        top: given("topPadding").unwrap_or(vertical),
        right: given("rightPadding").unwrap_or(horizontal),
        bottom: given("bottomPadding").unwrap_or(vertical),
    }
}

/// Writes computed paddings back where the file set none
pub fn store_pads(ctx: &Ctx<'_>, core: &Core, oid: Oid) -> Pads {
    let p = pads(core, oid);
    let padding = core.num(oid, "padding");
    let button = core.0.borrow().objs.get(oid).is_some_and(|o| o.def.name == "Button");
    for (n, v) in [("leftPadding", p.left), ("topPadding", p.top), ("rightPadding", p.right), ("bottomPadding", p.bottom),
                   ("horizontalPadding", if button { padding + 2.0 } else { padding }), ("verticalPadding", padding)] {
        if !core.is_explicit(oid, n) {
            core.store_layout(ctx, oid, n, v);
        }
    }
    p
}

/// Implicit background size of a TextField or TextArea: the given one, none if
/// empty, else Basic style's (TextField 200x40, TextArea none)
pub fn field_background(core: &Core, oid: Oid) -> (f64, f64) {
    if !core.is_explicit(oid, "background") {
        let field = core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "TextField"));
        return if field { (200.0, 40.0) } else { (0.0, 0.0) };
    }
    parts(core, oid).0.map(|b| (core.num(b, "implicitWidth"), core.num(b, "implicitHeight"))).unwrap_or((0.0, 0.0))
}

/// Writes back paddings, insets, and implicit sizes; returns the control's
/// implicit size
pub fn implicit(ctx: &Ctx<'_>, core: &Core, oid: Oid) -> (f64, f64) {
    let p = store_pads(ctx, core, oid);
    let (bg, content) = parts(core, oid);
    let size = |o: Option<Oid>| o.map(|o| (core.num(o, "implicitWidth"), core.num(o, "implicitHeight"))).unwrap_or((0.0, 0.0));
    let (bw, bh) = size(bg);
    let (cw, ch) = size(content);
    core.store_layout(ctx, oid, "implicitBackgroundWidth", bw);
    core.store_layout(ctx, oid, "implicitBackgroundHeight", bh);
    core.store_layout(ctx, oid, "implicitContentWidth", cw);
    core.store_layout(ctx, oid, "implicitContentHeight", ch);
    let inset = |n: &str| core.num(oid, n);
    let w = (bw + inset("leftInset") + inset("rightInset")).max(cw + p.left + p.right);
    let h = (bh + inset("topInset") + inset("bottomInset")).max(ch + p.top + p.bottom);
    (w, h)
}

/// Places the background inside the insets and the content inside the padding
pub fn place(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let (w, h) = (core.num(oid, "width"), core.num(oid, "height"));
    let p = pads(core, oid);
    let (aw, ah) = ((w - p.left - p.right).max(0.0), (h - p.top - p.bottom).max(0.0));
    core.store_layout(ctx, oid, "availableWidth", aw);
    core.store_layout(ctx, oid, "availableHeight", ah);
    let (bg, content) = parts(core, oid);
    if let Some(b) = bg {
        let inset = |n: &str| core.num(oid, n);
        if !core.is_explicit(b, "width") {
            core.store_layout(ctx, b, "x", inset("leftInset"));
            core.store_layout(ctx, b, "width", w - inset("leftInset") - inset("rightInset"));
        }
        if !core.is_explicit(b, "height") {
            core.store_layout(ctx, b, "y", inset("topInset"));
            core.store_layout(ctx, b, "height", h - inset("topInset") - inset("bottomInset"));
        }
    }
    let Some(c) = content else { return };
    let scroll_bar = core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "ScrollBar"));
    if scroll_bar {
        handle(ctx, core, oid, c, &p, (aw, ah));
        return;
    }
    // What its anchors hold, they hold
    let held = super::held(core, c);
    if !held.across {
        core.store_layout(ctx, c, "x", p.left);
    }
    if !held.down {
        core.store_layout(ctx, c, "y", p.top);
    }
    if !held.width {
        core.store_layout(ctx, c, "width", aw);
    }
    if !held.height {
        core.store_layout(ctx, c, "height", ah);
    }
}

/// ScrollBar handle: across, the space inside the padding; along, `size` of its
/// length at `position`, at least `minimumSize`
fn handle(ctx: &Ctx<'_>, core: &Core, oid: Oid, c: Oid, p: &Pads, (aw, ah): (f64, f64)) {
    let (w, h) = (core.num(oid, "width"), core.num(oid, "height"));
    let vertical = core.num(oid, "orientation") as i64 != 1;
    let least = if core.is_explicit(oid, "minimumSize") {
        core.num(oid, "minimumSize")
    } else if vertical {
        if h > 0.0 { w / h } else { 0.0 }
    } else if w > 0.0 {
        h / w
    } else {
        0.0
    };
    let size = core.num(oid, "size").clamp(0.0, 1.0);
    let shown = size.max(least).min(1.0);
    let at = core.num(oid, "position").clamp(0.0, 1.0 - shown);
    core.store_layout(ctx, oid, "visualSize", shown);
    core.store_layout(ctx, oid, "visualPosition", at);
    core.store_layout(ctx, oid, "minimumSize", least);
    if vertical {
        core.store_layout(ctx, c, "x", p.left);
        core.store_layout(ctx, c, "y", p.top + at * ah);
        core.store_layout(ctx, c, "width", aw);
        core.store_layout(ctx, c, "height", shown * ah);
    } else {
        core.store_layout(ctx, c, "x", p.left + at * aw);
        core.store_layout(ctx, c, "y", p.top);
        core.store_layout(ctx, c, "width", shown * aw);
        core.store_layout(ctx, c, "height", ah);
    }
}

/// Native button methods. `click()` is a press and release as in qml6:
/// `pressed` and `down` go true, `pressed()` emits, they go false, a checkable
/// button toggles, then `released()` and `clicked()` emit. Disabled buttons
/// emit nothing. `toggle()` flips `checked` and emits only that
pub fn call<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid, name: &str) -> Option<crate::object::value::Value> {
    if !core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "AbstractButton")) {
        return None;
    }
    use crate::object::value::Value;
    match name {
        "click" | "animateClick" => {
            if !core.flag(oid, "enabled") {
                return Some(Value::Undefined);
            }
            // Visual state only: no binding is broken, and an explicit `down`
            // stays
            let told = core.is_explicit(oid, "down");
            let put = |name: &str, v: bool| {
                if let Some(i) = core.own_idx(oid, name) {
                    core.store(ctx, oid, i, Value::Bool(v));
                }
            };
            put("pressed", true);
            if !told {
                put("down", true);
            }
            core.emit(ctx, oid, "pressed", Vec::new());
            put("pressed", false);
            if !told {
                put("down", false);
            }
            turn_over(ctx, core, oid);
            core.emit(ctx, oid, "released", Vec::new());
            core.emit(ctx, oid, "clicked", Vec::new());
            Some(Value::Undefined)
        }
        "toggle" => {
            core.set_named(ctx, oid, "checked", Value::Bool(!core.flag(oid, "checked")));
            Some(Value::Undefined)
        }
        _ => None,
    }
}

/// A checkable button toggles before `released()`: `checked`, then `toggled()`
pub fn turn_over(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    if core.flag(oid, "checkable") {
        core.set_named(ctx, oid, "checked", crate::object::value::Value::Bool(!core.flag(oid, "checked")));
        core.emit(ctx, oid, "toggled", Vec::new());
    }
}
