// ScriptModel: quickshell's model of a JavaScript list
//
// Rows are the elements of `values`, each with the role `modelData`, kept like
// ListModel rows (`list.rs`). Unlike an array model, changing `values` keeps
// the delegates of remaining values.
//
// Values match as in quickshell 7511545: `QVariant ==`, or the `objectProp`
// property if present. Plain objects and arrays compare structurally; shell
// objects by identity (`alike`).
//
// - A removed value takes its row and delegate.
// - A new value gets a row at its position.
// - A remaining value keeps its row and delegate and moves; one matched by
//   `objectProp` gets the new `modelData`.
//
// Duplicates match in order, so `Array(n)` grows and shrinks at its end.

use std::rc::Rc;

use rquickjs::Ctx;

use super::{list, list_changed, Change};
use crate::object::build::Builder;
use crate::object::value::{Oid, Value};
use crate::object::Core;

/// Rebuilds rows to match the current `values`
pub fn sync(ctx: &Ctx<'_>, core: &Core, model: Oid) {
    let new = values(ctx, core, model);
    let by = core.text(model, "objectProp");
    let rows: Vec<Oid> = core.0.borrow().lists.get(&model).cloned().unwrap_or_default();
    let old: Vec<Value> = rows.iter().map(|&r| core.peek_named(r, "modelData").unwrap_or(Value::Undefined)).collect();
    let new_keys: Vec<Value> = new.iter().map(|v| key(ctx, core, v, &by)).collect();
    let old_keys: Vec<Value> = old.iter().map(|v| key(ctx, core, v, &by)).collect();

    // Each old row to the first new value it matches that no row has yet
    let mut taken = vec![false; new.len()];
    let matched: Vec<Option<usize>> = old_keys.iter().map(|k| {
        let j = (0..new.len()).find(|&j| !taken[j] && same(ctx, k, &new_keys[j]))?;
        taken[j] = true;
        Some(j)
    }).collect();

    // Removes unmatched rows from the end so indices stay valid
    for at in (0..rows.len()).rev().filter(|&i| matched[i].is_none()) {
        list_changed(ctx, core, model, &Change::Removed { at, n: 1 });
        let gone = core.0.borrow_mut().lists.get_mut(&model).map(|r| r.remove(at));
        if let Some(r) = gone {
            core.destroy(ctx, r);
        }
    }
    // Walks the new order, moving kept rows and creating rows for new values
    let mut kept: Vec<(Oid, usize)> = rows.iter().zip(&matched).filter_map(|(&r, m)| m.map(|j| (r, j))).collect();
    for (i, value) in new.iter().enumerate() {
        match kept.iter().position(|&(_, j)| j == i) {
            Some(from) if from == i => {}
            Some(from) => {
                let moved = kept.remove(from);
                kept.insert(i, moved);
                if let Some(r) = core.0.borrow_mut().lists.get_mut(&model) {
                    let row = r.remove(from);
                    r.insert(i, row);
                }
                list::renumber(ctx, core, model, i);
                list_changed(ctx, core, model, &Change::Moved { from, to: i, n: 1 });
            }
            None => {
                let row = Builder::new(core, ctx).make_row(model, vec![
                    ("index".into(), Value::Num(i as f64)),
                    ("modelData".into(), value.clone()),
                ]);
                kept.insert(i, (row, i));
                if let Some(r) = core.0.borrow_mut().lists.get_mut(&model) {
                    r.insert(i, row);
                }
                list::renumber(ctx, core, model, i);
                list_changed(ctx, core, model, &Change::Inserted { at: i, n: 1 });
            }
        }
    }
    // A row kept by key takes the new value
    for (i, &(row, _)) in kept.iter().enumerate() {
        let now = core.peek_named(row, "modelData").unwrap_or(Value::Undefined);
        if !same(ctx, &now, &new[i]) {
            if let Some(idx) = core.prop_idx(row, "modelData") {
                core.store(ctx, row, idx, new[i].clone());
            }
        }
    }
    list::renumber(ctx, core, model, 0);
}

/// Starts with no rows until `values` is read
pub fn completed(ctx: &Ctx<'_>, core: &Core, model: Oid) {
    core.0.borrow_mut().lists.entry(model).or_default();
    sync(ctx, core, model);
}

/// `values` as a list of what it holds
fn values(ctx: &Ctx<'_>, core: &Core, model: Oid) -> Vec<Value> {
    match core.peek_named(model, "values") {
        Some(Value::List(items)) => items.as_ref().clone(),
        Some(Value::Js(p)) => {
            let Ok(v) = p.as_ref().clone().restore(ctx) else { return Vec::new() };
            match v.as_array() {
                Some(a) => (0..a.len())
                    .map(|i| a.get::<rquickjs::Value>(i).map(|e| crate::js::from_js(ctx, e)).unwrap_or(Value::Undefined))
                    .collect(),
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Match key: the `by` property if present
fn key(ctx: &Ctx<'_>, core: &Core, v: &Value, by: &str) -> Value {
    if by.is_empty() {
        return v.clone();
    }
    match v {
        Value::Obj(o) => core.peek_named(*o, by).unwrap_or_else(|| v.clone()),
        Value::Js(p) => {
            let Ok(js) = p.as_ref().clone().restore(ctx) else { return v.clone() };
            match js.as_object() {
                Some(o) if o.contains_key(by).unwrap_or(false) => {
                    o.get::<_, rquickjs::Value>(by).map(|k| crate::js::from_js(ctx, k)).unwrap_or(Value::Undefined)
                }
                _ => v.clone(),
            }
        }
        _ => v.clone(),
    }
}

/// `a == b`, as `QVariant` answers it for what a `QVariantList` holds
fn same(ctx: &Ctx<'_>, a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Obj(x), Value::Obj(y)) => x == y,
        (Value::List(x), Value::List(y)) => Rc::ptr_eq(x, y)
            || (x.len() == y.len() && x.iter().zip(y.iter()).all(|(p, q)| same(ctx, p, q))),
        (Value::Js(x), Value::Js(y)) => Rc::ptr_eq(x, y) || matches!(
            (x.as_ref().clone().restore(ctx), y.as_ref().clone().restore(ctx)),
            (Ok(p), Ok(q)) if alike(&p, &q, DEPTH)),
        _ => false,
    }
}

/// `alike` recursion limit, for self-referencing values
const DEPTH: u32 = 16;

/// `QVariant` equality of two JavaScript values: same value, same shell object,
/// arrays with alike items in order, or plain objects with alike values per
/// key. Functions compare by identity
fn alike<'js>(a: &rquickjs::Value<'js>, b: &rquickjs::Value<'js>, depth: u32) -> bool {
    if a == b {
        return true;
    }
    if let (Some(x), Some(y)) = (a.as_number(), b.as_number()) {
        return x == y;
    }
    if let (Some(x), Some(y)) = (crate::js::wrapped_oid(a), crate::js::wrapped_oid(b)) {
        return x == y;
    }
    if depth == 0 || a.is_function() || b.is_function() {
        return false;
    }
    if let (Some(x), Some(y)) = (a.as_array(), b.as_array()) {
        return x.len() == y.len() && (0..x.len()).all(|i| {
            matches!((x.get::<rquickjs::Value>(i), y.get::<rquickjs::Value>(i)),
                     (Ok(p), Ok(q)) if alike(&p, &q, depth - 1))
        });
    }
    // Shell wrappers compare by identity
    if a.is_array() || b.is_array() || crate::js::is_wrapper(a) || crate::js::is_wrapper(b) {
        return false;
    }
    let (Some(x), Some(y)) = (a.as_object(), b.as_object()) else { return false };
    let keys = |o: &rquickjs::Object<'_>| -> Vec<String> {
        let mut k: Vec<String> = o.keys::<String>().flatten().collect();
        k.sort();
        k
    };
    let kx = keys(x);
    kx == keys(y) && kx.iter().all(|k| {
        matches!((x.get::<_, rquickjs::Value>(k.as_str()), y.get::<_, rquickjs::Value>(k.as_str())),
                 (Ok(p), Ok(q)) if alike(&p, &q, depth - 1))
    })
}
