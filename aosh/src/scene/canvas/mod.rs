// Canvas: a picture painted by script
//
// - The picture has the item's size in whole pixels and persists: a paint draws
//   over it, only `clearRect` or `reset()` clears it. A new size starts empty.
// - It paints on first size, on size change, and on request (`requestPaint`,
//   `markDirty`), visible or not, since an effect can draw from a hidden
//   Canvas.
// - `getContext("2d")` returns one kept object whose styles, matrix, and path
//   persist.
//
// `onPaint` runs during layout (`Scene::layout`). The context
// (`js/prelude/canvas.js`) records calls and `replay.rs` draws them
// antialiased: flattened curves (`shape.rs`), line outlines (`stroke.rs`),
// FreeType's rasterizer, and thin lines walked pixel by pixel (`thin.rs`). The
// picture draws as an Image.

pub mod replay;
pub mod shape;
pub mod stroke;
pub mod thin;

use std::collections::HashMap;

use rquickjs::{Ctx, Object};

use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::scene::paint::Image;
use crate::types::Kind;
use replay::{Arg, Gradient, Op, Replay};

/// A Canvas's picture, and its context's state
pub struct Picture {
    pub image: Image,
    /// One more each time it is drawn into, for the scene's fingerprint
    pub generation: u64,
    replay: Replay,
}

#[derive(Default)]
pub struct Canvases {
    kept: HashMap<Oid, Picture>,
}

/// `getContext` creates and keeps the context; `requestPaint` and `markDirty`
/// request a paint
pub fn call<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid, kind: Kind, name: &str, _args: &[rquickjs::Value<'js>]) -> Option<Value> {
    if kind != Kind::Canvas {
        return None;
    }
    match name {
        "getContext" => Some(context(ctx, core, oid).unwrap_or(Value::Null)),
        "requestPaint" | "markDirty" => {
            let mut s = core.0.borrow_mut();
            if !s.canvas_due.contains(&oid) {
                s.canvas_due.push(oid);
            }
            s.dirty = true;
            Some(Value::Undefined)
        }
        "requestAnimationFrame" => Some(Value::Num(0.0)),
        "isImageLoaded" | "isImageLoading" | "isImageError" | "save" => Some(Value::Bool(false)),
        "toDataURL" => Some(Value::str("data:,")),
        _ => Some(Value::Undefined),
    }
}

/// The Canvas's context, made the first time it is asked for
fn context(ctx: &Ctx<'_>, core: &Core, oid: Oid) -> Option<Value> {
    if let Some(v @ Value::Js(_)) = core.peek_named(oid, "context") {
        return Some(v);
    }
    let make: rquickjs::Function = ctx.globals().get("__aosh_context2d").ok()?;
    let canvas = crate::js::wrapper(ctx, core, oid).ok()?;
    let c: rquickjs::Value = make.call((canvas,)).ok()?;
    let v = Value::Js(std::rc::Rc::new(rquickjs::Persistent::save(ctx, c)));
    if let Some(i) = core.prop_idx(oid, "context") {
        core.store(ctx, oid, i, v.clone());
    }
    if let Some(i) = core.prop_idx(oid, "contextType") {
        core.store(ctx, oid, i, Value::str("2d"));
    }
    Some(v)
}

impl Canvases {
    /// The picture of a Canvas, if it has painted
    pub fn picture(&self, oid: Oid) -> Option<&Picture> {
        self.kept.get(&oid)
    }

    /// Paints every Canvas in `win` that is new, resized, or requested
    pub fn paint_due(&mut self, ctx: &Ctx<'_>, core: &Core, win: Oid, since: u64) {
        let mut shown = Vec::new();
        canvases_in(core, win, since, &mut shown);
        let asked = std::mem::take(&mut core.0.borrow_mut().canvas_due);
        // Requested Canvases in unchanged parts of the window, which the walk
        // skipped
        for &oid in &asked {
            if !shown.contains(&oid) && is_under(core, oid, win) {
                shown.push(oid);
            }
        }
        let mut later = Vec::new();
        for &oid in &asked {
            if !shown.contains(&oid) && core.0.borrow().objs.is_live(oid) {
                later.push(oid);
            }
        }
        for oid in shown {
            let (w, h) = (core.num(oid, "width").round().max(0.0) as i32, core.num(oid, "height").round().max(0.0) as i32);
            if w == 0 || h == 0 {
                continue;
            }
            let fresh = self.kept.get(&oid).is_none_or(|p| (p.image.w, p.image.h) != (w, h));
            if !fresh && !asked.contains(&oid) {
                continue;
            }
            let pic = self.kept.entry(oid).or_insert_with(|| Picture { image: Image::new(w, h), generation: 0, replay: Replay::default() });
            if fresh {
                pic.image = Image::new(w, h);
            }
            let region = (|| -> rquickjs::Result<rquickjs::Value<'_>> {
                let o = Object::new(ctx.clone())?;
                o.set("x", 0)?;
                o.set("y", 0)?;
                o.set("width", core.num(oid, "width"))?;
                o.set("height", core.num(oid, "height"))?;
                Ok(o.into_value())
            })();
            core.emit(ctx, oid, "paint", region.into_iter().collect());
            let ops = take_ops(ctx, core, oid);
            pic.replay.play(&ops, &mut pic.image);
            pic.image.renew();
            pic.generation += 1;
            // The picture is not a property, so the item is marked directly
            core.touch(oid);
            core.emit(ctx, oid, "painted", Vec::new());
        }
        // Requested from another window; stays queued
        let mut s = core.0.borrow_mut();
        for oid in later {
            if !s.canvas_due.contains(&oid) {
                s.canvas_due.push(oid);
            }
        }
        drop(s);
        self.kept.retain(|&o, _| core.0.borrow().objs.is_live(o));
    }
}

/// Canvases under `oid`, visible or not, in what changed since `since`
fn canvases_in(core: &Core, oid: Oid, since: u64, out: &mut Vec<Oid>) {
    let (kind, kids) = {
        let s = core.0.borrow();
        let Some(o) = s.objs.get(oid) else { return };
        if since > 0 && o.below <= since {
            return;
        }
        (o.def.kind, o.children.clone())
    };
    if kind == Kind::Canvas {
        out.push(oid);
    }
    for c in kids {
        canvases_in(core, c, since, out);
    }
}

/// Whether `oid` is in the tree under `win`
fn is_under(core: &Core, oid: Oid, win: Oid) -> bool {
    let s = core.0.borrow();
    let mut at = Some(oid);
    for _ in 0..256 {
        match at {
            Some(a) if a == win => return true,
            Some(a) => at = s.objs.get(a).and_then(|o| o.parent),
            None => return false,
        }
    }
    false
}

/// Takes the context's recorded calls
fn take_ops(ctx: &Ctx<'_>, core: &Core, oid: Oid) -> Vec<Op> {
    let Some(Value::Js(p)) = core.peek_named(oid, "context") else { return Vec::new() };
    let Ok(v) = p.as_ref().clone().restore(ctx) else { return Vec::new() };
    let Some(o) = v.as_object() else { return Vec::new() };
    let Ok(list) = o.get::<_, rquickjs::Array>("__ops") else { return Vec::new() };
    let ops: Vec<Op> = list.iter::<rquickjs::Value>().filter_map(|op| op.ok()).filter_map(|op| {
        let a = op.as_array()?;
        let name: String = a.get(0).ok()?;
        let args = (1..a.len()).filter_map(|i| a.get::<rquickjs::Value>(i).ok()).map(|v| arg(&v)).collect();
        Some((name, args))
    }).collect();
    if let Ok(empty) = rquickjs::Array::new(ctx.clone()) {
        let _ = o.set("__ops", empty);
    }
    ops
}

fn arg(v: &rquickjs::Value<'_>) -> Arg {
    if let Some(n) = v.as_number() {
        return Arg::Num(n);
    }
    if let Some(b) = v.as_bool() {
        return Arg::Num(b as u8 as f64);
    }
    if let Some(s) = v.as_string().and_then(|s| s.to_string().ok()) {
        return Arg::Str(s);
    }
    if let Some(a) = v.as_array() {
        return Arg::List(a.iter::<f64>().filter_map(|x| x.ok()).collect());
    }
    if let Some(o) = v.as_object() {
        let kind: String = o.get("kind").unwrap_or_default();
        let args: Vec<f64> = o.get::<_, rquickjs::Array>("args").map(|a| a.iter::<f64>().filter_map(|x| x.ok()).collect()).unwrap_or_default();
        let stops = o.get::<_, rquickjs::Array>("stops").map(|a| {
            a.iter::<rquickjs::Array>().filter_map(|s| s.ok()).filter_map(|s| {
                let at: f64 = s.get(0).ok()?;
                let c: String = s.get(1).ok()?;
                Some((at, replay::colour(&c)?))
            }).collect::<Vec<_>>()
        }).unwrap_or_default();
        let mut stops = stops;
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        return Arg::Gradient(Gradient { kind, args, stops });
    }
    Arg::Str(String::new())
}
