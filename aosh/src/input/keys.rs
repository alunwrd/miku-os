// Keyboard focus: tracking the focused item and key event propagation
//
// A window maintains a single focus item: the most recent item granted
// `focus` via QML markup, bindings, or `forceActiveFocus()`. Granting focus
// clears it from
// the previous item. The item gains `activeFocus` only while its parent window
// holds keyboard
// focus, which is granted by the compositor
//
// Key events route to the focused item and bubble up the parent hierarchy to
// the window
// until accepted. At each node:
// 1) `Keys.on<Key>Pressed` fires (accepted by default, matching Qt).
// 2) Otherwise, `Keys.onPressed` fires (not accepted by default).
// 3) Otherwise, the item's default handler executes (e.g. `TextInput` text
//    editing in `text.rs`,
//    or `ListView` row navigation)

use std::collections::HashMap;

use rquickjs::{Ctx, Object};

use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::{self, Kind};

/// Half-cycle duration for the `TextInput` caret blink (Qt's `cursorFlashTime`
/// is 1000ms)
pub const BLINK_MS: u64 = 500;

// Compositor modifier bitmask (`libmay::keymap`)
const SHIFT: u32 = 1 << 0;
const CTRL: u32 = 1 << 2;
const ALT: u32 = 1 << 3;
const LOGO: u32 = 1 << 6;

// kata keysym mappings for non-text keys.
const K: u32 = 0x0100_0000;
pub const KEY_ESCAPE: u32 = K;
pub const KEY_TAB: u32 = K + 1;
pub const KEY_BACKTAB: u32 = K + 2;
pub const KEY_BACKSPACE: u32 = K + 3;
pub const KEY_RETURN: u32 = K + 4;
pub const KEY_ENTER: u32 = K + 5;
pub const KEY_DELETE: u32 = K + 7;
pub const KEY_HOME: u32 = K + 0x10;
pub const KEY_END: u32 = K + 0x11;
pub const KEY_LEFT: u32 = K + 0x12;
pub const KEY_UP: u32 = K + 0x13;
pub const KEY_RIGHT: u32 = K + 0x14;
pub const KEY_DOWN: u32 = K + 0x15;

/// Active keyboard focus holder, stored alongside objects to keep reloads,
/// destruction, and tests in sync
#[derive(Default)]
pub struct Keys {
    /// The focused item for each window
    focus: HashMap<Oid, Oid>,
    /// The window currently holding compositor keyboard focus
    keyboard: Option<Oid>,
    /// Caret visibility and its next blink time
    pub caret_on: bool,
    caret_due: Option<u64>,
}

impl Keys {
    /// The window with keyboard focus
    pub fn window(&self) -> Option<Oid> {
        self.keyboard
    }
}

/// Items with `focus: true` grab focus once the window is known;
pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    if is_item(core, oid) && core.flag(oid, "focus") {
        claim(ctx, core, oid);
    }
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    if !core.0.borrow().objs[oid].complete {
        return;
    }
    if name == "focus" && is_item(core, oid) {
        if core.flag(oid, "focus") {
            claim(ctx, core, oid);
        } else {
            release(ctx, core, oid);
        }
    }
    if kind == Kind::Input {
        super::text::hook(ctx, core, oid, name);
    }
}

pub fn call<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    oid: Oid,
    kind: Kind,
    name: &str,
    args: &[rquickjs::Value<'js>],
) -> Option<Value> {
    match name {
        "forceActiveFocus" if is_item(core, oid) => {
            force(ctx, core, oid);
            Some(Value::Undefined)
        }
        "nextItemInFocusChain" if is_item(core, oid) => Some(Value::Null),
        _ if kind == Kind::Input => super::text::call(ctx, core, oid, name, args),
        _ => None,
    }
}

/// Dropped object loses focus
pub fn forget(core: &Core, oid: Oid) {
    let mut s = core.0.borrow_mut();
    s.keys.focus.retain(|w, i| *w != oid && *i != oid);
    if s.keys.keyboard == Some(oid) {
        s.keys.keyboard = None;
    }
}

/// Forces focus even if already active, in case another item took window focus
pub fn force(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    core.set_layout_value(ctx, oid, "focus", Value::Bool(true));
    claim(ctx, core, oid);
}

/// Tracks whether the compositor granted (`Some`) or revoked (`None`) keyboard
/// focus for `win`
pub fn keyboard(ctx: &Ctx<'_>, core: &Core, win: Option<Oid>) {
    let was = std::mem::replace(&mut core.0.borrow_mut().keys.keyboard, win);
    if let Some(w) = was.filter(|w| Some(*w) != win) {
        refresh(ctx, core, w);
    }
    if let Some(w) = win {
        refresh(ctx, core, w);
    }
}

/// Handles a key press: `code` (Linux keycode), `ch` (mapped char), and `mods`
/// (compositor modifiers). Returns whether it was consumed
pub fn key(ctx: &Ctx<'_>, core: &Core, code: u32, ch: Option<char>, mods: u32) -> bool {
    let Some(win) = core
        .0
        .borrow()
        .keys
        .keyboard
        .filter(|&w| core.0.borrow().objs.is_live(w))
    else {
        return false;
    };
    let start = core
        .0
        .borrow()
        .keys
        .focus
        .get(&win)
        .copied()
        .filter(|&o| core.0.borrow().objs.is_live(o))
        .unwrap_or(win);
    let key = qt_key(code, ch, mods);
    let text = ch
        .filter(|c| !c.is_control() && mods & (CTRL | ALT | LOGO) == 0)
        .map(String::from)
        .unwrap_or_default();
    let Ok(ev) = event(ctx, key, &text, qt_modifiers(mods), code) else {
        return false;
    };
    let accepted = |ev: &Object<'_>| ev.get::<_, bool>("accepted").unwrap_or(false);
    let mut at = Some(start);
    while let Some(item) = at {
        if !core.0.borrow().objs.is_live(item) {
            return true;
        }
        if core.flag(item, "Keys.enabled") {
            if let Some(h) = handler_of(key).filter(|h| core.has_handler(item, h)) {
                let _ = ev.set("accepted", true);
                core.emit(ctx, item, h, vec![ev.clone().into_value()]);
                if accepted(&ev) {
                    return true;
                }
            }
            if core.has_handler(item, "Keys.pressed") {
                let _ = ev.set("accepted", false);
                core.emit(ctx, item, "Keys.pressed", vec![ev.clone().into_value()]);
                if accepted(&ev) {
                    return true;
                }
            }
        }
        if !core.0.borrow().objs.is_live(item) {
            return true;
        }
        if own(ctx, core, item, key, &text, mods) {
            return true;
        }
        if item == win {
            break;
        }
        at = core.0.borrow().objs.get(item).and_then(|o| o.parent);
    }
    false
}

/// The caret blinks only while TextInput has active focus
pub fn tick(core: &Core, now: u64) {
    let input = active_input(core);
    let active = input.is_some();
    let blinked = {
        let mut s = core.0.borrow_mut();
        match s.keys.caret_due {
            Some(due) if active && now >= due => {
                s.keys.caret_on = !s.keys.caret_on;
                s.keys.caret_due = Some(now + BLINK_MS);
                s.dirty = true;
                true
            }
            Some(_) if !active => {
                s.keys.caret_due = None;
                false
            }
            _ => false,
        }
    };
    // Caret is drawn manually, not bound to a property
    if let Some(i) = input.filter(|_| blinked) {
        core.touch(i);
    }
}

pub fn next_due(core: &Core) -> Option<u64> {
    active_input(core).and(core.0.borrow().keys.caret_due)
}

/// Typing immediately reveals the caret and resets its blink cycle
pub fn caret_reset(core: &Core) {
    {
        let mut s = core.0.borrow_mut();
        s.keys.caret_on = true;
        s.keys.caret_due = Some(s.now_ms + BLINK_MS);
        s.dirty = true;
    }
    if let Some(i) = active_input(core) {
        core.touch(i);
    }
}

/// The `TextInput` currently receiving keyboard input, if any
pub fn active_input(core: &Core) -> Option<Oid> {
    let s = core.0.borrow();
    let win = s.keys.keyboard?;
    let item = *s.keys.focus.get(&win)?;
    s.objs
        .get(item)
        .filter(|o| o.def.kind == Kind::Input)
        .map(|_| item)
}

fn is_item(core: &Core, oid: Oid) -> bool {
    core.0
        .borrow()
        .objs
        .get(oid)
        .is_some_and(|o| types::is_a(o.def, "Item"))
}

/// The window an item is in, by its parents
fn window_of(core: &Core, oid: Oid) -> Option<Oid> {
    let s = core.0.borrow();
    let mut at = Some(oid);
    for _ in 0..256 {
        let o = s.objs.get(at?)?;
        if o.def.kind == Kind::Window {
            return at;
        }
        at = o.parent;
    }
    None
}

/// Moves window focus to `item`, revoking it from the previous one
fn claim(ctx: &Ctx<'_>, core: &Core, item: Oid) {
    let Some(win) = window_of(core, item) else {
        return;
    };
    let prev = core.0.borrow_mut().keys.focus.insert(win, item);
    if let Some(p) = prev.filter(|&p| p != item && core.0.borrow().objs.is_live(p)) {
        core.set_layout_value(ctx, p, "focus", Value::Bool(false));
        core.set_layout_value(ctx, p, "activeFocus", Value::Bool(false));
    }
    refresh(ctx, core, win);
}

/// The window's focused item lost `focus`
fn release(ctx: &Ctx<'_>, core: &Core, item: Oid) {
    let Some(win) = window_of(core, item) else {
        return;
    };
    let mine = core.0.borrow().keys.focus.get(&win) == Some(&item);
    if mine {
        core.0.borrow_mut().keys.focus.remove(&win);
        core.set_layout_value(ctx, item, "activeFocus", Value::Bool(false));
    }
}

/// Syncs `activeFocus` for `win`'s focused item with keyboard state
fn refresh(ctx: &Ctx<'_>, core: &Core, win: Oid) {
    let (item, has) = {
        let s = core.0.borrow();
        (
            s.keys.focus.get(&win).copied(),
            s.keys.keyboard == Some(win),
        )
    };
    let Some(item) = item.filter(|&i| core.0.borrow().objs.is_live(i)) else {
        return;
    };
    core.set_layout_value(ctx, item, "activeFocus", Value::Bool(has));
    if has && core.0.borrow().objs[item].def.kind == Kind::Input {
        caret_reset(core);
    }
}

/// Default key handling for unconsumed events
fn own(ctx: &Ctx<'_>, core: &Core, item: Oid, key: u32, text: &str, mods: u32) -> bool {
    let kind = core.0.borrow().objs[item].def.kind;
    match kind {
        Kind::Input => super::text::key(ctx, core, item, key, text, mods),
        // Arrow keys navigate ListView items along its scroll direction
        Kind::ListView if core.flag(item, "keyNavigationEnabled") => {
            let vertical = core.num(item, "orientation") as i64 != 1;
            let step = match key {
                KEY_DOWN if vertical => "incrementCurrentIndex",
                KEY_UP if vertical => "decrementCurrentIndex",
                KEY_RIGHT if !vertical => "incrementCurrentIndex",
                KEY_LEFT if !vertical => "decrementCurrentIndex",
                _ => return false,
            };
            core.call_method(ctx, item, step, Vec::new());
            true
        }
        _ => false,
    }
}

/// Matches kata's `KeyEvent` payload:
/// `{ key, text, modifiers, accepted, isAutoRepeat, count }`
fn event<'js>(
    ctx: &Ctx<'js>,
    key: u32,
    text: &str,
    modifiers: u32,
    code: u32,
) -> rquickjs::Result<Object<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("key", key)?;
    o.set("text", text)?;
    o.set("modifiers", modifiers)?;
    o.set("accepted", false)?;
    // Auto-repeat is hardware/OS-driven: holding a key just emits repeated
    // press events with no way to distinguish them
    o.set("isAutoRepeat", false)?;
    o.set("count", 1)?;
    o.set("nativeScanCode", code)?;
    Ok(o)
}

/// Maps a Linux keycode to a kata key code: named keys map by code, printable
/// keys by layout character (uppercased, matching kata's `Key_A` style)
pub fn qt_key(code: u32, ch: Option<char>, mods: u32) -> u32 {
    match code {
        1 => KEY_ESCAPE,
        15 if mods & SHIFT != 0 => KEY_BACKTAB,
        15 => KEY_TAB,
        14 => KEY_BACKSPACE,
        28 => KEY_RETURN,
        96 => KEY_ENTER,
        110 => K + 6,
        111 => KEY_DELETE,
        119 => K + 8,
        102 => KEY_HOME,
        107 => KEY_END,
        105 => KEY_LEFT,
        103 => KEY_UP,
        106 => KEY_RIGHT,
        108 => KEY_DOWN,
        104 => K + 0x16,
        109 => K + 0x17,
        42 | 54 => K + 0x20,
        29 | 97 => K + 0x21,
        125 | 126 => K + 0x22,
        56 | 100 => K + 0x23,
        58 => K + 0x24,
        59..=68 => K + 0x30 + (code - 59),
        87 => K + 0x3a,
        88 => K + 0x3b,
        57 => 0x20,
        _ => match ch {
            Some(c) => c.to_uppercase().next().unwrap_or(c) as u32,
            None => K + 0xff_ffff,
        },
    }
}

fn qt_modifiers(mods: u32) -> u32 {
    let mut m = 0;
    if mods & SHIFT != 0 {
        m |= 0x0200_0000;
    }
    if mods & CTRL != 0 {
        m |= 0x0400_0000;
    }
    if mods & ALT != 0 {
        m |= 0x0800_0000;
    }
    if mods & LOGO != 0 {
        m |= 0x1000_0000;
    }
    m
}

/// The `Keys` signal corresponding to this key's dedicated handler
fn handler_of(key: u32) -> Option<&'static str> {
    const DIGITS: [&str; 10] = [
        "Keys.digit0Pressed",
        "Keys.digit1Pressed",
        "Keys.digit2Pressed",
        "Keys.digit3Pressed",
        "Keys.digit4Pressed",
        "Keys.digit5Pressed",
        "Keys.digit6Pressed",
        "Keys.digit7Pressed",
        "Keys.digit8Pressed",
        "Keys.digit9Pressed",
    ];
    Some(match key {
        KEY_RETURN => "Keys.returnPressed",
        KEY_ENTER => "Keys.enterPressed",
        KEY_ESCAPE => "Keys.escapePressed",
        KEY_UP => "Keys.upPressed",
        KEY_DOWN => "Keys.downPressed",
        KEY_LEFT => "Keys.leftPressed",
        KEY_RIGHT => "Keys.rightPressed",
        KEY_TAB => "Keys.tabPressed",
        KEY_BACKTAB => "Keys.backtabPressed",
        KEY_DELETE => "Keys.deletePressed",
        0x20 => "Keys.spacePressed",
        0x2a => "Keys.asteriskPressed",
        0x30..=0x39 => DIGITS[(key - 0x30) as usize],
        _ => return None,
    })
}
