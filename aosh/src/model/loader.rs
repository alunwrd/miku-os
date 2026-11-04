// Loader and LazyLoader: one component instance while `active`
//
// `sourceComponent` (Loader) or `component` (LazyLoader) is a Component;
// `source` is a file relative to the Loader's file. Clearing `active` destroys
// the item, losing its state, as in Qt.
//
// A Loader's item is its child. A LazyLoader holds non-items such as windows,
// with no parent.


use rquickjs::Ctx;

use super::{Made, View};
use crate::object::build::{Builder, Scope};
use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types;

pub fn reload(ctx: &Ctx<'_>, core: &Core, loader: Oid) {
    let old = core.0.borrow_mut().views.get_mut(&loader).map(|v| std::mem::take(&mut v.items));
    for m in old.into_iter().flatten() {
        core.destroy(ctx, m.item);
    }
    let is_item = types::is_a(core.0.borrow().objs[loader].def, "Item");
    let wanted = core.flag(loader, "active") || (!is_item && core.flag(loader, "loading"));
    let comp_prop = if is_item { "sourceComponent" } else { "component" };
    let comp = core.peek_named(loader, comp_prop).and_then(|v| v.as_obj())
        .filter(|c| core.0.borrow().comps.contains_key(c));
    let source = core.text(loader, "source");
    let parent = is_item.then_some(loader);

    if !wanted || (comp.is_none() && source.is_empty()) {
        core.set_layout_value(ctx, loader, "item", Value::Null);
        set_status(ctx, core, loader, is_item, 0);
        return;
    }
    let made = match comp {
        Some(c) => Builder::new(core, ctx).instantiate(c, parent, loader, Scope::default()),
        None => {
            let path = core.path(loader, &source);
            let outer = Some(core.0.borrow().objs[loader].cx);
            Builder::new(core, ctx).instantiate_file(&path, parent, loader, outer)
        }
    };
    match made {
        Ok(item) => {
            core.0.borrow_mut().views.entry(loader).or_insert_with(View::default)
                .items.push(Made { row: None, item, own_row: false });
            if is_item {
                core.0.borrow_mut().objs[loader].children.push(item);
                core.children_changed(ctx, loader);
            }
            core.set_layout_value(ctx, loader, "item", Value::Obj(item));
            set_status(ctx, core, loader, is_item, 1);
            core.emit(ctx, loader, "loaded", Vec::new());
        }
        Err(e) => {
            core.log(e);
            core.set_layout_value(ctx, loader, "item", Value::Null);
            set_status(ctx, core, loader, is_item, 3);
        }
    }
}

fn set_status(ctx: &Ctx<'_>, core: &Core, loader: Oid, is_item: bool, status: u32) {
    if is_item {
        core.set_layout_value(ctx, loader, "status", Value::Num(status as f64));
    }
}

