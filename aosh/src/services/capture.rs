// `ScreencopyView` object behavior (quickshell's `view.cpp`)
//
// Requests a frame when `captureSource` becomes capturable, on
// `captureFrame()`, and per drawn frame while `live`. A frame sets `hasContent`
// and `sourceSize`; removing the source clears them. Requests go through
// `Action::Capture` to Mayland (`mayland/capture.rs`); answers arrive through
// `took` and `lost`.
//
// Only a Toplevel can be a source (`may_desktop.capture`). Screens are not
// implemented

use rquickjs::Ctx;

use crate::object::value::{Oid, Value};
use crate::object::{Action, Core};
use crate::types::Kind;

fn ask(core: &Core, view: Oid) {
    core.0.borrow_mut().actions.push(Action::Capture(view));
}

/// A view's source changed, or it went live
pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    if kind != Kind::Screencopy || !core.0.borrow().objs.get(oid).is_some_and(|o| o.complete) {
        return;
    }
    match name {
        "captureSource" => {
            if window(core, oid).is_none() {
                lost(ctx, core, oid);
            }
            ask(core, oid);
        }
        "live" if core.flag(oid, "live") => ask(core, oid),
        _ => {}
    }
}

/// Created with a source: requests the first frame
pub fn completed(core: &Core, oid: Oid) {
    if core.0.borrow().objs.get(oid).is_some_and(|o| o.def.kind == Kind::Screencopy) && window(core, oid).is_some() {
        ask(core, oid);
    }
}

/// `captureFrame()`
pub fn call(core: &Core, oid: Oid, kind: Kind, name: &str) -> Option<Value> {
    if kind != Kind::Screencopy || name != "captureFrame" {
        return None;
    }
    if window(core, oid).is_some() {
        ask(core, oid);
    } else {
        core.log_once(oid, "ScreencopyView: cannot capture frame, as no recording context is ready");
    }
    Some(Value::Undefined)
}

/// The window a view's source names, if it names one
pub fn window(core: &Core, view: Oid) -> Option<(u32, u32)> {
    let src = core.peek_named(view, "captureSource").and_then(|v| v.as_obj())?;
    let s = core.0.borrow();
    let o = s.objs.get(src)?;
    if o.def.name != "Toplevel" {
        return None;
    }
    drop(s);
    super::hyprland::window_of_toplevel(core, src)
}

/// A frame arrived, `w` by `h`
pub fn took(ctx: &Ctx<'_>, core: &Core, view: Oid, w: i32, h: i32) {
    core.store_layout(ctx, view, "sourceSize.width", w as f64);
    core.store_layout(ctx, view, "sourceSize.height", h as f64);
    core.set_named(ctx, view, "hasContent", Value::Bool(true));
    core.0.borrow_mut().dirty = true;
}

/// Nothing to show: the source is gone or uncopyable
pub fn lost(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    if !core.flag(view, "hasContent") {
        return;
    }
    core.set_named(ctx, view, "hasContent", Value::Bool(false));
    core.store_layout(ctx, view, "sourceSize.width", 0.0);
    core.store_layout(ctx, view, "sourceSize.height", 0.0);
    core.0.borrow_mut().dirty = true;
}
