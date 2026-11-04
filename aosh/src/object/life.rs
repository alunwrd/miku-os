// Object destruction at runtime
//
// Creation is `Builder::instantiate`. Destruction undoes everything that
// references the object, children first: binding subscriptions, aliases,
// `Connections` handlers, parent lists, and timers, animations and processes.
// Only then is the slot freed, since a stale id would index whatever is created
// there next (`table.rs`).
//
// The slot is freed later, like Qt's deleteLater (`reap`). Until then the
// object is out of all trees and runs nothing, but the handler that destroyed
// it can still read its values. `onDestruction` runs immediately, as in qml6.

use std::rc::Rc;

use rquickjs::Ctx;

use super::value::{Oid, Value};
use super::{kill_binding, Core, State};

/// A `Component`: a definition and the scope it was written in
#[derive(Clone)]
pub struct ComponentDef {
    pub def: Rc<crate::qml::ast::ObjDef>,
    pub src: super::build::Source,
    /// Definition's id scope; instance scopes chain to it, so inline delegates
    /// see their file's ids
    pub cx: u32,
}

impl Core {
    /// `oid` and everything it owns, destroyed
    pub fn destroy(&self, ctx: &Ctx<'_>, oid: Oid) {
        {
            let s = self.0.borrow();
            if !s.objs.is_live(oid) || s.dying.contains(&oid) {
                return;
            }
        }
        let mut order = Vec::new();
        collect(&self.0.borrow(), oid, &mut order);
        // `Component.onDestruction` while everything is intact, outermost first
        // as in Qt
        for &o in order.iter().rev() {
            self.emit(ctx, o, "destruction", Vec::new());
        }
        for &o in &order {
            self.forget(ctx, o);
        }
        // Removes it from the surviving parent's and owner's lists, which
        // differ for Repeater delegates
        let mut touched: Vec<Oid> = Vec::new();
        {
            let mut s = self.0.borrow_mut();
            for &o in &order {
                let (parent, owner) = (s.objs[o].parent, s.objs[o].owner);
                for p in [parent, owner].into_iter().flatten() {
                    if order.contains(&p) {
                        continue;
                    }
                    if let Some(po) = s.objs.get_mut(p) {
                        let before = po.children.len();
                        po.children.retain(|&c| c != o);
                        po.owned.retain(|&c| c != o);
                        if po.children.len() != before && !touched.contains(&p) {
                            touched.push(p);
                        }
                    }
                }
            }
        }
        for p in touched {
            self.children_changed(ctx, p);
        }
        let mut s = self.0.borrow_mut();
        s.dying.extend(order);
        s.dirty = true;
    }

    /// Frees destroyed objects once nothing that destroyed them is running
    pub fn reap(&self) {
        let dying = std::mem::take(&mut self.0.borrow_mut().dying);
        for o in dying {
            let freed = {
                let mut s = self.0.borrow_mut();
                let freed = s.objs.free(o);
                // Its id scope goes with the last object made in it
                if let Some(obj) = &freed {
                    let cx = obj.cx as usize;
                    s.cxs[cx].objects = s.cxs[cx].objects.saturating_sub(1);
                    if s.cxs[cx].objects == 0 {
                        s.cxs[cx].ids.clear();
                        s.free_cxs.push(cx as u32);
                    }
                }
                freed
            };
            // Releases its JavaScript (wrapper, handlers) outside the borrow
            drop(freed);
        }
    }

    /// Removes one object from everything that refers to it. `destroy` frees
    /// the slot
    fn forget(&self, ctx: &Ctx<'_>, oid: Oid) {
        super::hold::release(ctx, self, oid);
        super::sync::forget(self, oid);
        crate::model::forget(ctx, self, oid);
        crate::anim::forget(self, oid);
        crate::io::forget(self, oid);
        crate::input::forget(self, oid);
        crate::ipc::forget(self, oid);
        let mut s = self.0.borrow_mut();
        let s = &mut *s;
        s.timers.retain(|(o, _)| *o != oid);
        s.comps.remove(&oid);
        let nprops = s.objs[oid].props.len();
        for i in 0..nprops {
            if let Some(b) = s.objs[oid].props[i].binding.take() {
                kill_binding(s, b);
            }
            // Unsubscribes bindings elsewhere that read this property
            let observers = std::mem::take(&mut s.objs[oid].props[i].observers);
            for b in observers {
                s.binds[b as usize].deps.retain(|d| d.0 != oid);
            }
            // Detaches an alias from its target
            if let Some((to, ti)) = s.objs[oid].props[i].alias.take() {
                if let Some(t) = s.objs.get_mut(to) {
                    t.props[ti as usize].aliased_by.retain(|&(ao, ai)| (ao, ai) != (oid, i as u32));
                }
            }
            // Detaches aliases elsewhere pointing here
            for (ao, ai) in std::mem::take(&mut s.objs[oid].props[i].aliased_by) {
                if let Some(a) = s.objs.get_mut(ao) {
                    a.props[ai as usize].alias = None;
                    a.props[ai as usize].value = Value::Undefined;
                }
            }
        }
        // A Connections takes its handlers off its target
        if let Some(t) = s.objs[oid].listening_on.take() {
            if let Some(t) = s.objs.get_mut(t) {
                t.handlers.retain(|(_, h)| h.this != oid);
            }
        }
        s.objs[oid].handlers.clear();
        s.objs[oid].listens.clear();
        s.objs[oid].wrapper = None;
    }

    /// An object's `children` property, from the list the table keeps
    pub fn children_changed(&self, ctx: &Ctx<'_>, oid: Oid) {
        self.touch(oid);
        let Some(i) = self.prop_idx(oid, "children") else { return };
        self.store(ctx, oid, i, Value::List(Rc::new(self.listed(oid, "children"))));
    }

    /// Contents of an object's lists, as on Qt's Item: `children` holds Items,
    /// `resources` the rest, `data` and a Gradient's `stops` all of them
    pub fn listed(&self, oid: Oid, name: &str) -> Vec<Value> {
        let s = self.0.borrow();
        let item = |c: &Oid| s.objs.get(*c).is_some_and(|o| crate::types::is_a(o.def, "Item"));
        s.objs[oid].children.iter()
            .filter(|c| match name {
                "children" => item(c),
                "resources" => !item(c),
                _ => true,
            })
            .map(|&c| Value::Obj(c))
            .collect()
    }
}

/// `oid` and what it owns, the deepest first
fn collect(s: &State, oid: Oid, out: &mut Vec<Oid>) {
    let Some(o) = s.objs.get(oid) else { return };
    if s.dying.contains(&oid) {
        return;
    }
    for &c in o.owned.iter().chain(o.children.iter()) {
        if !out.contains(&c) {
            collect(s, c, out);
        }
    }
    if !out.contains(&oid) {
        out.push(oid);
    }
}
