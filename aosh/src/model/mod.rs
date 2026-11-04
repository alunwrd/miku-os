// Models and their views: Repeater, ListView, Loader
//
// A number gives that many rows, an array one per element, and a ListModel its
// row objects (`list.rs`). Each row has a delegate instance whose scope exposes
// the row (`index`, `modelData`, `model`, roles) through getters, so delegates
// track rows like bindings.
//
// Repeater delegates are children of its parent, after it. ListView, Variants,
// and Instantiator own theirs; the last two are not laid out. ListModel changes
// apply row by row. Replacing a number or array rebuilds every delegate, as in
// Qt. ScriptModel keeps rows like a ListModel (`script.rs`).

pub mod list;
mod loader;
mod script;

use rquickjs::Ctx;

use crate::object::build::{Builder, Scope};
use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

/// What a Repeater, ListView or Loader has made
#[derive(Default)]
pub struct View {
    pub items: Vec<Made>,
    /// Shown ListModel, whose changes arrive by row (`list::changed`)
    pub list: Option<Oid>,
}

pub struct Made {
    pub row: Option<Oid>,
    pub item: Oid,
    /// Row owned by the view (number or array model); destroyed with the
    /// delegate
    pub own_row: bool,
}

/// A ListModel's rows changed
pub enum Change {
    Inserted { at: usize, n: usize },
    Removed { at: usize, n: usize },
    Moved { from: usize, to: usize, n: usize },
    Reset,
}

pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let kind = core.0.borrow().objs[oid].def.kind;
    match kind {
        Kind::Repeater | Kind::ListView | Kind::Instantiator => rebuild(ctx, core, oid),
        Kind::Loader => loader::reload(ctx, core, oid),
        Kind::ListModel => list::completed(ctx, core, oid),
        Kind::ScriptModel => script::completed(ctx, core, oid),
        _ => {}
    }
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    if !core.0.borrow().objs[oid].complete {
        return;
    }
    match (kind, name) {
        (Kind::Repeater | Kind::ListView | Kind::Instantiator, "model" | "delegate") => rebuild(ctx, core, oid),
        (Kind::Instantiator, "active") => rebuild(ctx, core, oid),
        (Kind::ListView, "currentIndex") => current_changed(ctx, core, oid),
        (Kind::ScriptModel, "values" | "objectProp") => script::sync(ctx, core, oid),
        (Kind::Loader, "active" | "source" | "sourceComponent" | "component" | "loading") => {
            loader::reload(ctx, core, oid)
        }
        _ => {}
    }
}

pub fn call<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid, kind: Kind, name: &str, args: &[rquickjs::Value<'js>]) -> Option<Value> {
    let num = |i: usize| args.get(i).and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)));
    match (kind, name) {
        (Kind::ListModel, _) => list::call(ctx, core, oid, name, args),
        (Kind::Repeater | Kind::ListView, "itemAt" | "itemAtIndex") | (Kind::Instantiator, "objectAt") => {
            let i = num(0)? as usize;
            let item = core.0.borrow().views.get(&oid).and_then(|v| v.items.get(i)).map(|m| m.item);
            Some(item.map(Value::Obj).unwrap_or(Value::Null))
        }
        (Kind::ListView, _) => list_view_call(ctx, core, oid, name, args),
        (Kind::Loader, "setSource") => {
            let src = args.first().and_then(|v| v.as_string()).and_then(|s| s.to_string().ok()).unwrap_or_default();
            core.set_named(ctx, oid, "source", Value::str(&src));
            Some(Value::Undefined)
        }
        (Kind::Component, "createObject") => Some(create_object(ctx, core, oid, args)),
        _ => None,
    }
}

/// Everything a view made goes with it
pub fn forget(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let view = core.0.borrow_mut().views.remove(&oid);
    // The view's `owned` items are being destroyed by the caller; only
    // ListModel rows are not the view's
    let _ = (ctx, view);
    core.0.borrow_mut().lists.remove(&oid);
}

/// The items a view made, in row order, for the scene
pub fn items(core: &Core, view: Oid) -> Vec<Oid> {
    core.0.borrow().views.get(&view).map(|v| v.items.iter().map(|m| m.item).collect()).unwrap_or_default()
}

/// Every delegate destroyed and every one made again
fn rebuild(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    clear(ctx, core, view);
    if core.0.borrow().objs[view].def.kind == Kind::Instantiator && core.prop_idx(view, "active").is_some()
        && !core.flag(view, "active")
    {
        counted(ctx, core, view);
        return;
    }
    let model = core.peek_named(view, "model").unwrap_or(Value::Undefined);
    let (rows, list) = rows_of(ctx, core, view, &model);
    core.0.borrow_mut().views.entry(view).or_default().list = list;
    for (i, (row, own)) in rows.into_iter().enumerate() {
        make_item(ctx, core, view, i, row, own);
    }
    place(ctx, core, view);
    counted(ctx, core, view);
    if core.0.borrow().objs[view].def.kind == Kind::ListView {
        let n = core.num(view, "count") as i64;
        let cur = core.num(view, "currentIndex") as i64;
        if n > 0 && cur < 0 {
            core.set_named(ctx, view, "currentIndex", Value::Num(0.0));
        } else {
            current_changed(ctx, core, view);
        }
    }
}

fn clear(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    let items = match core.0.borrow_mut().views.get_mut(&view) {
        Some(v) => std::mem::take(&mut v.items),
        None => return,
    };
    let removed = added_removed(core, view).1;
    for (i, m) in items.iter().enumerate().rev() {
        core.emit_item(ctx, view, removed, i, m.item);
        core.destroy(ctx, m.item);
        if m.own_row {
            if let Some(r) = m.row {
                core.destroy(ctx, r);
            }
        }
    }
}

/// Rows of a model value, plus the ListModel or ScriptModel if it is one
fn rows_of(ctx: &Ctx<'_>, core: &Core, view: Oid, model: &Value) -> (Vec<(Option<Oid>, bool)>, Option<Oid>) {
    let row = |core: &Core, i: usize, data: Value| -> (Option<Oid>, bool) {
        let r = Builder::new(core, ctx).make_row(view, vec![
            ("index".into(), Value::Num(i as f64)),
            ("modelData".into(), data),
        ]);
        (Some(r), true)
    };
    match model {
        Value::Num(n) if n.is_finite() && *n > 0.0 => {
            ((0..*n as usize).map(|i| row(core, i, Value::Num(i as f64))).collect(), None)
        }
        Value::List(items) => (items.iter().enumerate().map(|(i, v)| row(core, i, v.clone())).collect(), None),
        Value::Js(p) => {
            let Ok(v) = p.as_ref().clone().restore(ctx) else { return (Vec::new(), None) };
            // quickshell object models such as `DesktopEntries.applications`
            // use `values`
            let v = match v.as_object().filter(|_| v.as_array().is_none())
                .and_then(|o| o.get::<_, rquickjs::Array>("values").ok())
            {
                Some(values) => values.into_value(),
                None => v,
            };
            match v.as_array() {
                Some(a) => {
                    let elems: Vec<Value> = a.iter::<rquickjs::Value>()
                        .map(|e| e.map(|e| crate::js::from_js(ctx, e)).unwrap_or(Value::Undefined))
                        .collect();
                    (elems.into_iter().enumerate().map(|(i, v)| row(core, i, v)).collect(), None)
                }
                None => (vec![row(core, 0, crate::js::from_js(ctx, v))], None),
            }
        }
        Value::Obj(o) if core.0.borrow().objs.get(*o).is_some_and(|x| matches!(x.def.kind, Kind::ListModel | Kind::ScriptModel)) => {
            let rows = core.0.borrow().lists.get(o).cloned().unwrap_or_default();
            (rows.into_iter().map(|r| (Some(r), false)).collect(), Some(*o))
        }
        Value::Obj(o) if core.0.borrow().objs.is_live(*o) => (vec![row(core, 0, Value::Obj(*o))], None),
        _ => (Vec::new(), None),
    }
}

/// A delegate for one row, put at `at` in the view's list
fn make_item(ctx: &Ctx<'_>, core: &Core, view: Oid, at: usize, row: Option<Oid>, own_row: bool) {
    let comp = core.peek_named(view, "delegate").and_then(|v| v.as_obj())
        .filter(|c| core.0.borrow().comps.contains_key(c));
    let Some(comp) = comp else { return };
    let (kind, vparent) = {
        let s = core.0.borrow();
        (s.objs[view].def.kind, s.objs[view].parent)
    };
    let parent = if kind == Kind::Repeater { vparent } else { Some(view) };
    // ListModel roles are row properties; an array element is `modelData` with
    // no roles
    let roles: Vec<String> = match row.filter(|_| !own_row) {
        Some(r) => core.0.borrow().objs[r].props.iter()
            .map(|p| p.name.to_string()).filter(|n| n != "index").collect(),
        None => Vec::new(),
    };
    let scope = Scope { row, roles, view: (kind == Kind::ListView).then_some(view) };
    match Builder::new(core, ctx).instantiate(comp, parent, view, scope) {
        Ok(item) => {
            if let Some(v) = core.0.borrow_mut().views.get_mut(&view) {
                let at = at.min(v.items.len());
                v.items.insert(at, Made { row, item, own_row });
            }
            core.emit_item(ctx, view, added_removed(core, view).0, at, item);
        }
        Err(e) => core.log(e),
    }
}

/// Delegates in the parent's `children`, in row order: a Repeater's follow it,
/// a ListView's follow its other children
fn place(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    let items = items(core, view);
    let (kind, vparent) = {
        let s = core.0.borrow();
        (s.objs[view].def.kind, s.objs[view].parent)
    };
    let parent = if kind == Kind::Repeater { vparent } else { Some(view) };
    let Some(p) = parent.filter(|&p| core.0.borrow().objs.is_live(p)) else { return };
    {
        let mut s = core.0.borrow_mut();
        let children = &mut s.objs[p].children;
        children.retain(|c| !items.contains(c));
        let at = if kind == Kind::Repeater {
            children.iter().position(|&c| c == view).map(|i| i + 1).unwrap_or(children.len())
        } else {
            children.len()
        };
        for (k, &it) in items.iter().enumerate() {
            children.insert(at + k, it);
        }
    }
    core.children_changed(ctx, p);
}

fn counted(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    let n = core.0.borrow().views.get(&view).map(|v| v.items.len()).unwrap_or(0);
    if let Some(i) = core.prop_idx(view, "count") {
        core.store(ctx, view, i, Value::Num(n as f64));
    }
    // Variants' `instances`, Instantiator's first `object`
    let made = items(core, view);
    if let Some(i) = core.prop_idx(view, "instances") {
        core.store(ctx, view, i, Value::List(std::rc::Rc::new(made.iter().map(|&o| Value::Obj(o)).collect())));
    }
    if let Some(i) = core.prop_idx(view, "object").filter(|_| core.0.borrow().objs[view].def.kind == Kind::Instantiator) {
        core.store(ctx, view, i, made.first().map(|&o| Value::Obj(o)).unwrap_or(Value::Null));
    }
}

/// What a view says when it makes a delegate and when one goes
fn added_removed(core: &Core, view: Oid) -> (&'static str, &'static str) {
    match core.0.borrow().objs.get(view).map(|o| o.def.kind) {
        Some(Kind::Instantiator) => ("objectAdded", "objectRemoved"),
        _ => ("itemAdded", "itemRemoved"),
    }
}

/// A ListModel's change, applied to every view showing it
pub fn list_changed(ctx: &Ctx<'_>, core: &Core, list: Oid, change: &Change) {
    let views: Vec<Oid> = core.0.borrow().views.iter()
        .filter(|(_, v)| v.list == Some(list)).map(|(&o, _)| o).collect();
    let rows = core.0.borrow().lists.get(&list).cloned().unwrap_or_default();
    for view in views {
        match *change {
            Change::Reset => rebuild(ctx, core, view),
            Change::Inserted { at, n } => {
                for k in 0..n {
                    if let Some(&r) = rows.get(at + k) {
                        make_item(ctx, core, view, at + k, Some(r), false);
                    }
                }
            }
            Change::Removed { at, n } => {
                let gone: Vec<Made> = match core.0.borrow_mut().views.get_mut(&view) {
                    Some(v) => {
                        let end = (at + n).min(v.items.len());
                        v.items.drain(at.min(end)..end).collect()
                    }
                    None => continue,
                };
                for (k, m) in gone.iter().enumerate().rev() {
                    core.emit_item(ctx, view, added_removed(core, view).1, at + k, m.item);
                    core.destroy(ctx, m.item);
                }
            }
            Change::Moved { from, to, n } => {
                if let Some(v) = core.0.borrow_mut().views.get_mut(&view) {
                    if from + n <= v.items.len() {
                        let moved: Vec<Made> = v.items.drain(from..from + n).collect();
                        let to = to.min(v.items.len());
                        for (k, m) in moved.into_iter().enumerate() {
                            v.items.insert(to + k, m);
                        }
                    }
                }
            }
        }
        if !matches!(change, Change::Reset) {
            place(ctx, core, view);
            counted(ctx, core, view);
            if core.0.borrow().objs.get(view).is_some_and(|o| o.def.kind == Kind::ListView) {
                current_changed(ctx, core, view);
            }
        }
    }
}

/// `currentItem` follows `currentIndex`
fn current_changed(ctx: &Ctx<'_>, core: &Core, view: Oid) {
    let i = core.num(view, "currentIndex") as i64;
    let item = if i >= 0 {
        core.0.borrow().views.get(&view).and_then(|v| v.items.get(i as usize)).map(|m| m.item)
    } else {
        None
    };
    core.set_layout_value(ctx, view, "currentItem", item.map(Value::Obj).unwrap_or(Value::Null));
}

/// Margins along a ListView's axis: `topMargin` and `bottomMargin`, or
/// `leftMargin` and `rightMargin` when horizontal. Not part of `contentHeight`.
///
/// `contentY` starts at 0 at the top margin, where Qt starts at `-topMargin`
pub fn margins(core: &Core, view: Oid, vertical: bool) -> (f64, f64) {
    if vertical {
        (core.num(view, "topMargin"), core.num(view, "bottomMargin"))
    } else {
        (core.num(view, "leftMargin"), core.num(view, "rightMargin"))
    }
}

fn list_view_call<'js>(ctx: &Ctx<'js>, core: &Core, view: Oid, name: &str, args: &[rquickjs::Value<'js>]) -> Option<Value> {
    let num = |i: usize| args.get(i).and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)));
    let n = core.num(view, "count") as i64;
    let vertical = core.num(view, "orientation") as i64 != 1;
    let (pos, extent, size) = if vertical {
        ("contentY", core.num(view, "contentHeight"), core.num(view, "height"))
    } else {
        ("contentX", core.num(view, "contentWidth"), core.num(view, "width"))
    };
    let (before, after) = margins(core, view, vertical);
    let furthest = (extent + before + after - size).max(0.0);
    match name {
        "positionViewAtBeginning" => core.set_layout_value(ctx, view, pos, Value::Num(0.0)),
        "positionViewAtEnd" => core.set_layout_value(ctx, view, pos, Value::Num(furthest)),
        "positionViewAtIndex" => {
            let i = num(0)? as usize;
            let mode = num(1).unwrap_or(0.0) as i64;
            let item = core.0.borrow().views.get(&view).and_then(|v| v.items.get(i)).map(|m| m.item)?;
            let (at, len) = if vertical {
                (core.num(item, "y") + core.num(view, "contentY"), core.num(item, "height"))
            } else {
                (core.num(item, "x") + core.num(view, "contentX"), core.num(item, "width"))
            };
            let cur = core.num(view, pos);
            let to = match mode {
                // Center
                1 => at - (size - len) / 2.0,
                // End
                2 => at + len - size,
                // Visible, Contain: scrolls the minimum distance
                3 | 4 => {
                    if at < cur {
                        at
                    } else if at + len > cur + size {
                        at + len - size
                    } else {
                        cur
                    }
                }
                _ => at,
            };
            core.set_layout_value(ctx, view, pos, Value::Num(to.clamp(0.0, furthest)));
        }
        "incrementCurrentIndex" | "decrementCurrentIndex" => {
            let cur = core.num(view, "currentIndex") as i64;
            let next = if name.starts_with("incr") { (cur + 1).min(n - 1) } else { (cur - 1).max(0) };
            if n > 0 {
                core.set_named(ctx, view, "currentIndex", Value::Num(next as f64));
            }
        }
        "indexAt" => {
            let (x, y) = (num(0)?, num(1)?);
            let items = items(core, view);
            let hit = items.iter().position(|&it| {
                let (ix, iy) = (core.num(it, "x"), core.num(it, "y"));
                x >= ix && y >= iy && x < ix + core.num(it, "width") && y < iy + core.num(it, "height")
            });
            return Some(Value::Num(hit.map(|i| i as f64).unwrap_or(-1.0)));
        }
        "forceLayout" => {}
        _ => return None,
    }
    Some(Value::Undefined)
}

/// `component.createObject(parent, { props })`
fn create_object<'js>(ctx: &Ctx<'js>, core: &Core, comp: Oid, args: &[rquickjs::Value<'js>]) -> Value {
    let parent = args.first().map(|v| crate::js::from_js(ctx, v.clone())).and_then(|v| v.as_obj())
        .filter(|&p| core.0.borrow().objs.is_live(p));
    // Owned by the parent if any, otherwise by the component
    let owner = parent.unwrap_or(comp);
    let made = Builder::new(core, ctx).instantiate(comp, parent, owner, Scope::default());
    let item = match made {
        Ok(i) => i,
        Err(e) => {
            core.log(format!("createObject: {e}"));
            return Value::Null;
        }
    };
    if let Some(p) = parent {
        core.0.borrow_mut().objs[p].children.push(item);
        core.children_changed(ctx, p);
    }
    if let Some(props) = args.get(1).and_then(|v| v.as_object()) {
        for (k, v) in props.props::<String, rquickjs::Value>().flatten() {
            if let Some(i) = core.prop_idx(item, &k) {
                let ty = core.prop_type(item, i);
                core.assign(ctx, item, i, crate::js::from_js_as(ctx, v, ty));
            }
        }
    }
    Value::Obj(item)
}

impl Core {
    /// `itemAdded(index, item)` and `itemRemoved(index, item)`
    fn emit_item(&self, ctx: &Ctx<'_>, view: Oid, signal: &str, index: usize, item: Oid) {
        let args = (|| -> rquickjs::Result<Vec<rquickjs::Value>> {
            Ok(vec![
                rquickjs::Value::new_number(ctx.clone(), index as f64),
                crate::js::to_js(ctx, self, &Value::Obj(item))?,
            ])
        })();
        if let Ok(a) = args {
            self.emit(ctx, view, signal, a);
        }
    }

    /// Stores a computed value (`count`, `currentItem`) without breaking a
    /// binding
    pub fn set_layout_value(&self, ctx: &Ctx<'_>, oid: Oid, name: &str, v: Value) {
        if let Some(i) = self.prop_idx(oid, name) {
            self.store(ctx, oid, i, v);
        }
    }
}

#[cfg(test)]
mod tests;
