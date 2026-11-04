// Internal TextInput logic: key event handling and public methods
//
// Cursor offsets are UTF-8 character (scalar value) indices, not byte counts
// (e.g. a Cyrillic character advances the cursor by 1)
// Selection is not implemented; selection APIs are no-ops to preserve
// compatibility with callers

use rquickjs::{Ctx, FromJs};

use super::keys::{
    self, KEY_BACKSPACE, KEY_DELETE, KEY_END, KEY_ENTER, KEY_HOME, KEY_LEFT, KEY_RETURN, KEY_RIGHT,
};
use crate::object::value::{Oid, Value};
use crate::object::Core;

const CTRL: u32 = 1 << 2;

/// The displayed text: masked with bullets for passwords, or empty for `NoEcho`
/// mode
pub fn shown(core: &Core, oid: Oid) -> String {
    let text = core.text(oid, "text");
    match core.num(oid, "echoMode") as i64 {
        1 => String::new(),
        2 | 3 => {
            let dot = core
                .text(oid, "passwordCharacter")
                .chars()
                .next()
                .unwrap_or('\u{2022}');
            std::iter::repeat_n(dot, text.chars().count()).collect()
        }
        _ => text,
    }
}

pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    derived(ctx, core, oid);
    // Matches Qt behavior: moves the cursor to the end of the newly set text
    let len = core.text(oid, "text").chars().count();
    core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(len as f64));
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str) {
    match name {
        // External updates (bindings, scripts) move the cursor to the end
        // In contrast, user edits advance the cursor immediately after the
        // inserted character
        "text" => {
            derived(ctx, core, oid);
            let len = core.text(oid, "text").chars().count();
            core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(len as f64));
        }
        "echoMode" | "passwordCharacter" => derived(ctx, core, oid),
        "cursorPosition" => {
            let len = core.text(oid, "text").chars().count() as f64;
            let at = core.num(oid, "cursorPosition");
            if at < 0.0 || at > len {
                core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(at.clamp(0.0, len)));
            }
            keys::caret_reset(core);
        }
        _ => {}
    }
}

/// Derives `displayText` and `length` from the raw text buffer
fn derived(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let len = core.text(oid, "text").chars().count();
    core.set_layout_value(ctx, oid, "displayText", Value::str(&shown(core, oid)));
    core.set_layout_value(ctx, oid, "length", Value::Num(len as f64));
}

fn is_edit(core: &Core, oid: Oid) -> bool {
    core.0
        .borrow()
        .objs
        .get(oid)
        .is_some_and(|o| crate::types::is_a(o.def, "TextEdit"))
}

/// Fallback for unhandled key events
///
/// Return and Enter mark the event as `accepted` but continue propagating to
/// parent elements. Non-editing keys propagate up the hierarchy as well
pub fn key(ctx: &Ctx<'_>, core: &Core, oid: Oid, key: u32, text: &str, mods: u32) -> bool {
    let old: Vec<char> = core.text(oid, "text").chars().collect();
    let mut chars = old.clone();
    let mut at = (core.num(oid, "cursorPosition").max(0.0) as usize).min(chars.len());
    let editable = !core.flag(oid, "readOnly");
    match key {
        KEY_LEFT => at = at.saturating_sub(1),
        KEY_RIGHT => at = (at + 1).min(chars.len()),
        KEY_HOME => at = 0,
        KEY_END => at = chars.len(),
        // Ctrl+Backspace deletes the word before the cursor
        KEY_BACKSPACE if editable && mods & CTRL != 0 => {
            let mut from = at;
            while from > 0 && chars[from - 1].is_whitespace() {
                from -= 1;
            }
            while from > 0 && !chars[from - 1].is_whitespace() {
                from -= 1;
            }
            chars.drain(from..at);
            at = from;
        }
        KEY_BACKSPACE if editable => {
            if at > 0 {
                chars.remove(at - 1);
                at -= 1;
            }
        }
        KEY_DELETE if editable => {
            if at < chars.len() {
                chars.remove(at);
            }
        }
        // In `TextEdit`, Return/Enter inserts a newline; in `TextInput`, it
        // commits the input
        KEY_RETURN | KEY_ENTER if editable && is_edit(core, oid) => {
            chars.insert(at, '\n');
            at += 1;
        }
        KEY_RETURN | KEY_ENTER => {
            core.emit(ctx, oid, "accepted", Vec::new());
            core.emit(ctx, oid, "editingFinished", Vec::new());
            return false;
        }
        _ if editable && !text.is_empty() => {
            let max = core.num(oid, "maximumLength").max(0.0) as usize;
            for c in text.chars() {
                if chars.len() < max {
                    chars.insert(at, c);
                    at += 1;
                }
            }
        }
        _ => return false,
    }
    if !core.0.borrow().objs.is_live(oid) {
        return true;
    }
    if chars != old {
        let s: String = chars.iter().collect();
        core.set_layout_value(ctx, oid, "text", Value::str(&s));
        core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(at as f64));
        core.emit(ctx, oid, "textEdited", Vec::new());
    } else {
        core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(at as f64));
    }
    keys::caret_reset(core);
    true
}

pub fn call<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    oid: Oid,
    name: &str,
    args: &[rquickjs::Value<'js>],
) -> Option<Value> {
    let int = |i: usize| {
        args.get(i)
            .and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)))
            .map(|n| n.max(0.0) as usize)
    };
    let string = |i: usize| {
        args.get(i).map(|v| {
            rquickjs::Coerced::<String>::from_js(ctx, v.clone())
                .map(|c| c.0)
                .unwrap_or_default()
        })
    };
    let chars: Vec<char> = core.text(oid, "text").chars().collect();
    let put = |chars: Vec<char>, at: usize| {
        let s: String = chars.iter().collect();
        core.set_layout_value(ctx, oid, "text", Value::str(&s));
        core.set_layout_value(ctx, oid, "cursorPosition", Value::Num(at as f64));
    };
    match name {
        "clear" => put(Vec::new(), 0),
        "insert" => {
            let at = int(0).unwrap_or(chars.len()).min(chars.len());
            let add: Vec<char> = string(1).unwrap_or_default().chars().collect();
            let mut c = chars;
            let end = at + add.len();
            c.splice(at..at, add);
            put(c, end);
        }
        "remove" => {
            let (a, b) = (
                int(0).unwrap_or(0).min(chars.len()),
                int(1).unwrap_or(0).min(chars.len()),
            );
            let (a, b) = (a.min(b), a.max(b));
            let mut c = chars;
            c.drain(a..b);
            put(c, a);
        }
        "getText" => {
            let (a, b) = (
                int(0).unwrap_or(0).min(chars.len()),
                int(1).unwrap_or(chars.len()).min(chars.len()),
            );
            let s: String = chars[a.min(b)..a.max(b)].iter().collect();
            return Some(Value::str(&s));
        }
        // Translating X to a cursor position requires font metrics; falling
        // back to text.len() safely clamps it
        "positionAt" => return Some(Value::Num(chars.len() as f64)),
        "selectAll" | "select" | "deselect" | "selectWord" | "ensureVisible" | "copy" | "cut"
        | "paste" | "undo" | "redo" => {}
        _ => return None,
    }
    Some(Value::Undefined)
}
