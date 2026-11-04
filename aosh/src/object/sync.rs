// Qt.labs.synchronizer Synchronizer: keeps properties equal in both directions
//
// Kept properties: the one it is `on` (or `targetObject`'s `targetProperty`),
// `sourceObject`'s `sourceProperty`, and every alias declared in it. Behavior
// matches qml6 6.12:
// - initially all take the source's value (an alias named `source` counts),
//   otherwise the target's;
// - after that, a change is copied to the others, leaving their bindings in
//   place.
//
// Changes arrive through `State::syncs` from `Core::notify`.

use rquickjs::Ctx;

use super::value::{Oid, Value};
use super::Core;

/// Kept properties with aliases resolved, source first if there is one
fn kept(core: &Core, sync: Oid) -> (Option<(Oid, u32)>, Vec<(Oid, u32)>) {
    let named = |obj: &str, prop: &str| -> Option<(Oid, u32)> {
        let o = core.peek_named(sync, obj).and_then(|v| v.as_obj())?;
        let p = core.text(sync, prop);
        if p.is_empty() || !core.0.borrow().objs.is_live(o) {
            return None;
        }
        let i = core.prop_idx(o, &p)?;
        let s = core.0.borrow();
        Some(super::resolve(&s, o, i))
    };
    let target = match core.0.borrow().anims.on_target(sync) {
        Some((o, p)) => core.prop_idx(o, &p).map(|i| {
            let s = core.0.borrow();
            super::resolve(&s, o, i)
        }),
        None => None,
    }.or_else(|| named("targetObject", "targetProperty"));
    let mut source = named("sourceObject", "sourceProperty");
    let mut all: Vec<(Oid, u32)> = target.into_iter().collect();
    let aliases: Vec<(String, (Oid, u32))> = {
        let s = core.0.borrow();
        s.objs[sync].props.iter()
            .filter_map(|p| p.alias.map(|a| (p.name.to_string(), super::resolve(&s, a.0, a.1))))
            .collect()
    };
    for (name, at) in aliases {
        if name == "source" && source.is_none() {
            source = Some(at);
        }
        all.push(at);
    }
    if let Some(s) = source {
        all.push(s);
    }
    all.dedup();
    let mut seen = Vec::new();
    all.retain(|p| if seen.contains(p) { false } else { seen.push(*p); true });
    (source, all)
}

/// Copies the initial value to all and starts watching
pub fn completed(ctx: &Ctx<'_>, core: &Core, sync: Oid) {
    let (source, all) = kept(core, sync);
    {
        let mut s = core.0.borrow_mut();
        for p in &all {
            let list = s.syncs.entry(*p).or_default();
            if !list.contains(&sync) {
                list.push(sync);
            }
        }
    }
    let Some(&first) = source.as_ref().or(all.first()) else { return };
    changed(ctx, core, sync, first);
}

/// `from` changed: copies its value to the others
pub fn changed(ctx: &Ctx<'_>, core: &Core, sync: Oid, from: (Oid, u32)) {
    if !core.0.borrow().objs.get(sync).is_some_and(|o| o.complete) {
        return;
    }
    let (_, all) = kept(core, sync);
    let v: Value = {
        let s = core.0.borrow();
        match s.objs.get(from.0) {
            Some(o) => o.props[from.1 as usize].value.clone(),
            None => return,
        }
    };
    for p in all.into_iter().filter(|&p| p != from) {
        if core.0.borrow().objs.is_live(p.0) {
            core.store(ctx, p.0, p.1, v.clone());
        }
    }
}

/// Removes a destroyed Synchronizer or watched object from the table
pub fn forget(core: &Core, oid: Oid) {
    let mut s = core.0.borrow_mut();
    s.syncs.retain(|k, list| {
        list.retain(|&o| o != oid);
        k.0 != oid && !list.is_empty()
    });
}
