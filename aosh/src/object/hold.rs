// QtQml Binding: holds a property at `value` while `when` is true
//
// The property's own binding is suspended meanwhile (`Binding::held`). When
// `when` turns false, the target changes, or the Binding is destroyed,
// `restoreMode` applies (Qt 6 values):
// - RestoreNone (0): keeps the value, drops the old binding;
// - RestoreBinding (1): runs its binding again, if any;
// - RestoreValue (2): restores the old value;
// - RestoreBindingOrValue (3, default): the binding if any, otherwise the
//   value.
//
// `Binding on visible { ... }` names its target like a Behavior (`anim::on`).

use rquickjs::Ctx;

use super::value::{Oid, Value};
use super::{kill_binding, Bid, Core};

/// A property a Binding holds, and what it had before
pub struct Held {
    pub target: (Oid, u32),
    binding: Option<Bid>,
    value: Value,
}

const RESTORE_NONE: i64 = 0;
const RESTORE_BINDING: i64 = 1;
const RESTORE_VALUE: i64 = 2;

/// Binding target: `target` and `property`, or what it is `on`
fn aim(core: &Core, b: Oid) -> Option<(Oid, u32)> {
    let on = core.0.borrow().anims.on_target(b);
    let (obj, prop) = match on {
        Some((o, p)) => (o, p),
        None => {
            let o = core.peek_named(b, "target").and_then(|v| v.as_obj())?;
            (o, core.text(b, "property"))
        }
    };
    if !core.0.borrow().objs.is_live(obj) || prop.is_empty() {
        return None;
    }
    let idx = core.prop_idx(obj, &prop)?;
    // An alias holds where it points
    let s = core.0.borrow();
    Some(super::resolve(&s, obj, idx))
}

/// Applies the Binding's state: hold, follow the value, or release
pub fn update(ctx: &Ctx<'_>, core: &Core, b: Oid) {
    if !core.0.borrow().objs.get(b).is_some_and(|o| o.complete) {
        return;
    }
    let aim = aim(core, b);
    let active = aim.is_some() && core.flag(b, "when");
    let held = core.0.borrow().held.get(&b).map(|h| h.target);
    if held.is_some() && (!active || held != aim) {
        release(ctx, core, b);
    }
    let Some((o, i)) = aim.filter(|_| active) else { return };
    if !core.0.borrow().held.contains_key(&b) {
        let mut s = core.0.borrow_mut();
        let binding = s.objs[o].props[i as usize].binding;
        if let Some(bid) = binding {
            s.binds[bid as usize].held = true;
        }
        let value = s.objs[o].props[i as usize].value.clone();
        s.held.insert(b, Held { target: (o, i), binding, value });
    }
    let v = core.peek_named(b, "value").unwrap_or(Value::Undefined);
    core.store(ctx, o, i, v);
}

/// Let go of the property a Binding holds
pub fn release(ctx: &Ctx<'_>, core: &Core, b: Oid) {
    let Some(h) = core.0.borrow_mut().held.remove(&b) else { return };
    let (o, i) = h.target;
    if !core.0.borrow().objs.is_live(o) {
        return;
    }
    let mode = if core.0.borrow().objs.is_live(b) { core.num(b, "restoreMode") as i64 } else { RESTORE_NONE };
    // Restores the old binding only if nothing replaced it
    let binding = h.binding.filter(|&bid| {
        let s = core.0.borrow();
        s.binds[bid as usize].alive && s.objs[o].props[i as usize].binding == Some(bid)
    });
    match (mode, binding) {
        (RESTORE_BINDING, Some(bid)) | (3, Some(bid)) => {
            core.0.borrow_mut().binds[bid as usize].held = false;
            core.evaluate(ctx, bid);
        }
        (RESTORE_VALUE, bid) | (3, bid @ None) => {
            if let Some(bid) = bid {
                let mut s = core.0.borrow_mut();
                s.objs[o].props[i as usize].binding = None;
                kill_binding(&mut s, bid);
            }
            core.store(ctx, o, i, h.value);
        }
        (_, Some(bid)) => {
            let mut s = core.0.borrow_mut();
            s.objs[o].props[i as usize].binding = None;
            kill_binding(&mut s, bid);
        }
        _ => {}
    }
}
