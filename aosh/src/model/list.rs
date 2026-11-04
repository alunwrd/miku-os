// ListModel: rows are `ListElement` objects with roles as properties, plus
// `index`
//
// Delegates read rows through their scope (`model/`), so a role change is an
// ordinary store. `get(i)` returns the row itself. Added and removed rows reach
// views as changes, not as a new list.

use rquickjs::Ctx;

use super::{list_changed, Change};
use crate::object::build::Builder;
use crate::object::value::{Oid, PropType, Value};
use crate::object::Core;
use crate::types::Kind;

/// Initial rows from `ListElement` children
pub fn completed(ctx: &Ctx<'_>, core: &Core, list: Oid) {
    let rows: Vec<Oid> = {
        let s = core.0.borrow();
        s.objs[list].children.iter().copied()
            .filter(|&c| s.objs.get(c).is_some_and(|o| o.def.kind == Kind::Row))
            .collect()
    };
    core.0.borrow_mut().lists.insert(list, rows);
    renumber(ctx, core, list, 0);
    counted(ctx, core, list);
}

pub fn call<'js>(ctx: &Ctx<'js>, core: &Core, list: Oid, name: &str, args: &[rquickjs::Value<'js>]) -> Option<Value> {
    let num = |i: usize| args.get(i).and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)));
    let len = rows(core, list).len();
    match name {
        "append" => {
            let objs = objects(args.first());
            let at = len;
            insert(ctx, core, list, at, objs);
        }
        "insert" => {
            let at = (num(0).unwrap_or(0.0).max(0.0) as usize).min(len);
            insert(ctx, core, list, at, objects(args.get(1)));
        }
        "remove" => {
            let at = num(0)? as usize;
            let n = num(1).unwrap_or(1.0).max(0.0) as usize;
            if at >= len {
                core.log(format!("ListModel.remove: index {at} out of range (count {len})"));
                return Some(Value::Undefined);
            }
            let n = n.min(len - at);
            // The delegates first, while what they read is still there
            list_changed(ctx, core, list, &Change::Removed { at, n });
            let gone: Vec<Oid> = core.0.borrow_mut().lists.get_mut(&list)
                .map(|r| r.drain(at..at + n).collect()).unwrap_or_default();
            for r in gone {
                core.destroy(ctx, r);
            }
            renumber(ctx, core, list, at);
            counted(ctx, core, list);
        }
        "move" => {
            let (from, to) = (num(0)? as usize, num(1)? as usize);
            let n = num(2).unwrap_or(1.0).max(0.0) as usize;
            if from + n > len || to + n > len {
                core.log(format!("ListModel.move: {from}..{} to {to} is out of range (count {len})", from + n));
                return Some(Value::Undefined);
            }
            if let Some(r) = core.0.borrow_mut().lists.get_mut(&list) {
                let moved: Vec<Oid> = r.drain(from..from + n).collect();
                for (k, m) in moved.into_iter().enumerate() {
                    r.insert(to + k, m);
                }
            }
            renumber(ctx, core, list, from.min(to));
            list_changed(ctx, core, list, &Change::Moved { from, to, n });
        }
        "clear" => {
            let gone = core.0.borrow_mut().lists.get_mut(&list).map(std::mem::take).unwrap_or_default();
            list_changed(ctx, core, list, &Change::Reset);
            for r in gone {
                core.destroy(ctx, r);
            }
            counted(ctx, core, list);
        }
        "get" => {
            let i = num(0)? as usize;
            return Some(rows(core, list).get(i).map(|&r| Value::Obj(r)).unwrap_or(Value::Undefined));
        }
        "set" => {
            let i = num(0)? as usize;
            if i == len {
                insert(ctx, core, list, len, objects(args.get(1)));
            } else if let Some(&r) = rows(core, list).get(i) {
                for (k, v) in objects(args.get(1)).into_iter().flatten() {
                    set_role(ctx, core, r, &k, v);
                }
            }
        }
        "setProperty" => {
            let i = num(0)? as usize;
            let role = args.get(1).and_then(|v| v.as_string()).and_then(|s| s.to_string().ok())?;
            let v = args.get(2).map(|v| crate::js::from_js(ctx, v.clone())).unwrap_or(Value::Undefined);
            match rows(core, list).get(i) {
                Some(&r) => set_role(ctx, core, r, &role, v),
                None => core.log(format!("ListModel.setProperty: index {i} out of range (count {len})")),
            }
        }
        "sync" => {}
        _ => return None,
    }
    Some(Value::Undefined)
}

fn rows(core: &Core, list: Oid) -> Vec<Oid> {
    core.0.borrow().lists.get(&list).cloned().unwrap_or_default()
}

/// `append` argument: an object or an array of them
fn objects(v: Option<&rquickjs::Value<'_>>) -> Vec<Vec<(String, Value)>> {
    let Some(v) = v else { return Vec::new() };
    let one = |o: &rquickjs::Object<'_>| -> Vec<(String, Value)> {
        o.props::<String, rquickjs::Value>().flatten()
            .map(|(k, v)| {
                let ctx = v.ctx().clone();
                (k, crate::js::from_js(&ctx, v))
            })
            .collect()
    };
    if let Some(a) = v.as_array() {
        return a.iter::<rquickjs::Value>().flatten().filter_map(|e| e.as_object().map(one)).collect();
    }
    v.as_object().map(|o| vec![one(o)]).unwrap_or_default()
}

fn insert(ctx: &Ctx<'_>, core: &Core, list: Oid, at: usize, objs: Vec<Vec<(String, Value)>>) {
    if objs.is_empty() {
        return;
    }
    let n = objs.len();
    let made: Vec<Oid> = objs.into_iter().map(|roles| {
        let mut all = vec![("index".to_string(), Value::Num(0.0))];
        all.extend(roles);
        Builder::new(core, ctx).make_row(list, all)
    }).collect();
    if let Some(r) = core.0.borrow_mut().lists.get_mut(&list) {
        for (k, m) in made.into_iter().enumerate() {
            r.insert(at + k, m);
        }
    }
    renumber(ctx, core, list, at);
    counted(ctx, core, list);
    list_changed(ctx, core, list, &Change::Inserted { at, n });
}

/// Sets a role on a row, adding it if missing
fn set_role(ctx: &Ctx<'_>, core: &Core, row: Oid, role: &str, v: Value) {
    let i = match core.prop_idx(row, role) {
        Some(i) => i,
        None => core.add_prop(row, role, PropType::Var, false),
    };
    core.store(ctx, row, i, v);
}

/// Each row's `index` is where it is, from `from` on
pub(super) fn renumber(ctx: &Ctx<'_>, core: &Core, list: Oid, from: usize) {
    for (i, &r) in rows(core, list).iter().enumerate().skip(from) {
        let idx = match core.prop_idx(r, "index") {
            Some(i) => i,
            None => core.add_prop(r, "index", PropType::Int, false),
        };
        core.store(ctx, r, idx, Value::Num(i as f64));
    }
}

fn counted(ctx: &Ctx<'_>, core: &Core, list: Oid) {
    let n = rows(core, list).len();
    core.set_layout_value(ctx, list, "count", Value::Num(n as f64));
}
