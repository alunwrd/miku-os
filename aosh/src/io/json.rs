////////////////////////////////////////////////////////////////////////////////////////////////////////////////
//                    JsonAdapter & JsonObject: maps JSON to QML properties                                   //
// Top-level properties act as root keys; nested `JsonObject` properties represent child objects              //
//                                                                                                            //
// Loading:                                                                                                   //
// When FileView text updates, `load` parses keys into declared properties (missing keys keep defaults).      //
//   `var`: updated only if the serialized JSON value actually changed.                                       //
//   `JsonObject`: recursively populated, or set to null if the JSON value is `null`.                         //
//    other types: converted to the target property type.                                                      //
//                                                                                                            //
// Saving:                                                                                                    //
// External property changes trigger `adapterUpdated` on the adapter and FileView.                            //
// `save` matches QJsonDocument::Indented output: sorted keys, 4-space indent, and a trailing newline.        //
////////////////////////////////////////////////////////////////////////////////////////////////////////////////
use rquickjs::{Ctx, Object};

use crate::object::value::{Oid, PropType, Value};
use crate::object::Core;
use crate::types::{self, Kind};

/// Checks if `oid` is the adapter itself or a child object
fn is_adapter(core: &Core, oid: Oid) -> bool {
    core.0
        .borrow()
        .objs
        .get(oid)
        .is_some_and(|o| types::is_a(o.def, "JsonAdapter"))
}

fn is_json(core: &Core, oid: Oid) -> bool {
    core.0
        .borrow()
        .objs
        .get(oid)
        .is_some_and(|o| o.def.kind == Kind::Json)
}

/// The nearest ancestor adapter that declared this JsonObject
fn adapter_of(core: &Core, mut oid: Oid) -> Option<Oid> {
    for _ in 0..64 {
        if !is_json(core, oid) {
            return None;
        }
        if is_adapter(core, oid) {
            return Some(oid);
        }
        oid = core.0.borrow().objs.get(oid)?.owner?;
    }
    None
}

/// User-declared keys in declaration order, excluding built-in (`objectName`)
/// and internal aosh properties
fn keys(core: &Core, oid: Oid) -> Vec<(u32, String, PropType, bool)> {
    let s = core.0.borrow();
    let Some(o) = s.objs.get(oid) else {
        return Vec::new();
    };
    let builtin: Vec<&str> = types::chain(o.def)
        .iter()
        .flat_map(|t| t.props.iter().map(|p| p.0))
        .collect();
    o.props
        .iter()
        .enumerate()
        .filter(|(_, p)| !p.name.starts_with("__") && !builtin.contains(&&*p.name))
        .map(|(i, p)| (i as u32, p.name.to_string(), p.ty, p.readonly))
        .collect()
}

/// Raw FileView text passed to the adapter
pub fn load(ctx: &Ctx<'_>, core: &Core, adapter: Oid, text: &str) {
    if text.is_empty() {
        return;
    }
    let what = core
        .0
        .borrow()
        .objs
        .get(adapter)
        .map(|o| core.0.borrow().place(o.file, o.loc))
        .unwrap_or_default();
    let json = match ctx.json_parse(text.to_string()) {
        Ok(v) => v,
        Err(e) => {
            core.log(format!(
                "{what}: JsonAdapter: the file is not JSON: {}",
                crate::js::describe(ctx, e)
            ));
            return;
        }
    };
    let Some(obj) = json.as_object().filter(|_| !json.is_array()).cloned() else {
        core.log(format!(
            "{what}: JsonAdapter: the file is not a JSON object"
        ));
        return;
    };
    core.0.borrow_mut().io.json_loading.push(adapter);
    fill(ctx, core, adapter, &obj, &what);
    core.0
        .borrow_mut()
        .io
        .json_loading
        .retain(|&a| a != adapter);
}

/// Recursively maps an object's keys to its properties
fn fill<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid, json: &Object<'js>, what: &str) {
    for (idx, name, ty, readonly) in keys(core, oid) {
        let Ok(true) = json.contains_key(name.as_str()) else {
            continue;
        };
        let Ok(jval) = json.get::<_, rquickjs::Value>(name.as_str()) else {
            continue;
        };
        if readonly {
            continue;
        }
        let current = core.peek(oid, idx);
        match ty {
            PropType::Var => {
                let old = crate::js::to_js(ctx, core, &current).ok()
                    .and_then(|v| ctx.json_stringify(v).ok().flatten())
                    .and_then(|s| s.to_string().ok());
                let new = ctx.json_stringify(jval.clone()).ok().flatten().and_then(|s| s.to_string().ok());
                if old != new {
                    let v = crate::js::from_js_as(ctx, jval, ty);
                    core.store(ctx, oid, idx, v);
                }
            }
            PropType::Obj => match current.as_obj().filter(|&o| is_json(core, o)) {
                Some(child) if jval.is_object() && !jval.is_array() => {
                    if let Some(o) = jval.as_object() {
                        fill(ctx, core, child, o, what);
                    }
                }
                _ if jval.is_null() => core.store(ctx, oid, idx, Value::Null),
                None if jval.is_object() => {
                    core.log(format!("{what}: JsonAdapter: '{name}' is null here, and the file's object for it is left out"));
                }
                _ => core.log(format!("{what}: JsonAdapter: '{name}' is an object, and the file has something else for it")),
            },
            // `list<JsonObject>`: maps the array item-by-item to existing
            // objects.
            // If the array has more elements than objects, creating a new
            // JsonObject follows the deviation above
            PropType::List if current_is_objects(&current) => {
                let (Value::List(items), Some(arr)) = (&current, jval.as_array()) else { continue };
                for (i, item) in items.iter().enumerate() {
                    let Ok(e) = arr.get::<rquickjs::Value>(i) else { break };
                    if let (Some(o), Some(eo)) = (item.as_obj(), e.as_object()) {
                        fill(ctx, core, o, eo, what);
                    }
                }
            }
            _ => {
                // QVariant::convert: rejects non-numeric strings for `int`, and
                // rejects objects/arrays for non-list types
                let fits = match ty {
                    PropType::List => jval.is_array(),
                    _ => !jval.is_object() || jval.is_null(),
                };
                if !fits {
                    core.log(format!("{what}: JsonAdapter: '{name}' is not what the file has for it"));
                    continue;
                }
                let v = crate::js::from_js_as(ctx, jval, ty);
                core.store(ctx, oid, idx, v);
            }
        }
    }
}

fn current_is_objects(v: &Value) -> bool {
    matches!(v, Value::List(items) if !items.is_empty() && items.iter().all(|i| i.as_obj().is_some()))
}

/// Serializes adapter data into formatted text for writeAdapter()
pub fn save(ctx: &Ctx<'_>, core: &Core, adapter: Oid) -> Option<String> {
    let obj = object(ctx, core, adapter).ok()?;
    let text = ctx
        .json_stringify_replacer_space(obj, rquickjs::Undefined, 4)
        .ok()
        .flatten()?;
    Some(format!("{}\n", text.to_string().ok()?))
}

/// Recursively serializes an object to JSON with sorted keys
fn object<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid) -> rquickjs::Result<rquickjs::Value<'js>> {
    let out = Object::new(ctx.clone())?;
    let mut ks = keys(core, oid);
    ks.sort_by(|a, b| a.1.cmp(&b.1));
    for (idx, name, _, _) in ks {
        let v = core.peek(oid, idx);
        out.set(name.as_str(), value(ctx, core, &v)?)?;
    }
    Ok(out.into_value())
}

fn value<'js>(ctx: &Ctx<'js>, core: &Core, v: &Value) -> rquickjs::Result<rquickjs::Value<'js>> {
    Ok(match v {
        Value::Obj(o) if is_json(core, *o) => object(ctx, core, *o)?,
        // Non-data objects serialize to null, following Qt's QJsonValue
        // behavior for QObject
        Value::Obj(_) => rquickjs::Value::new_null(ctx.clone()),
        Value::List(items) => {
            let a = rquickjs::Array::new(ctx.clone())?;
            for (i, it) in items.iter().enumerate() {
                a.set(i, value(ctx, core, it)?)?;
            }
            a.into_value()
        }
        other => crate::js::to_js(ctx, core, other)?,
    })
}

/// Handles property changes on a JsonAdapter or JsonObject.
/// Only changes to user-declared keys are tracked; built-in QObject properties
/// like `objectName` are ignored
pub fn changed(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str) {
    if name == "objectName" || name.starts_with("__") {
        return;
    }
    let Some(adapter) = adapter_of(core, oid) else {
        return;
    };
    {
        let s = core.0.borrow();
        if s.io.json_loading.contains(&adapter) || !s.objs.get(adapter).is_some_and(|o| o.complete)
        {
            return;
        }
    }
    core.emit(ctx, adapter, "adapterUpdated", Vec::new());
    let view = core
        .0
        .borrow()
        .objs
        .get(adapter)
        .and_then(|o| o.parent)
        .filter(|&p| {
            core.0
                .borrow()
                .objs
                .get(p)
                .is_some_and(|o| o.def.kind == Kind::FileView)
        })
        .filter(|&p| core.peek_named(p, "adapter").and_then(|v| v.as_obj()) == Some(adapter));
    if let Some(v) = view {
        core.emit(ctx, v, "adapterUpdated", Vec::new());
    }
}
