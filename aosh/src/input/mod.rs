/////////////////////////////////////////////////////////////////////////////////////
//                         Input subsystem:                                        //
//   pointer:    Hit testing and mouse event dispatch (hover, click, scroll)       //
//   keyboard:   Keymaps, modifier state, and compositor focus                     //
//   keys:       Focus management and key event routing                            //
//   text:       TextInput key handling and editing APIs                           //
/////////////////////////////////////////////////////////////////////////////////////
pub mod keyboard;
pub mod keys;
pub mod pointer;
pub mod text;

use rquickjs::Ctx;

use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    if core.0.borrow().objs[oid].def.kind == Kind::Input {
        text::completed(ctx, core, oid);
    }
    keys::completed(ctx, core, oid);
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    keys::hook(ctx, core, oid, kind, name);
}

pub fn call<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    oid: Oid,
    kind: Kind,
    name: &str,
    args: &[rquickjs::Value<'js>],
) -> Option<Value> {
    keys::call(ctx, core, oid, kind, name, args)
}

pub fn forget(core: &Core, oid: Oid) {
    keys::forget(core, oid);
}

#[cfg(test)]
mod tests;
