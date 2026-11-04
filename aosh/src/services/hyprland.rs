// `Hyprland` and `ToplevelManager` globals: the Desktop service's workspaces
// and windows as quickshell's Hyprland objects (`types/hyprland.rs`) and
// Wayland toplevels (`types/wayland.rs`)
//
// Objects persist, so bindings keep the same one: the monitor and workspaces
// for the shell's lifetime, a window's two objects while it is open.
// `workspaces` lists those with windows and the shown one. `publish` rewrites
// them on each `Desktop` change:
// - `lastIpcObject` matches `hyprctl -j` (`clients`, `workspaces`, `monitors`);
// - changes emit Hyprland `rawEvent`s: `openwindow`, `closewindow`,
//   `movewindow`, `windowtitle`, `activewindow`, `workspace`, with their `v2`s.
//
// `dispatch` takes Hyprland and Lua forms: workspace switching (`workspace 3`,
// `r+1`), and focusing, closing, and moving a window by address. Others are
// logged and ignored. `activelayout` is a `rawEvent` from the keyboard layout
// event.

use rquickjs::{Array, Ctx, Object};

use crate::object::build::Builder;
use crate::object::value::{Oid, Value};
use crate::object::{Action, Core};

pub struct Hyprland {
    global: Oid,
    manager: Oid,
    monitor: Oid,
    workspaces: Vec<Oid>,
    toplevels: Vec<Top>,
    /// The workspace and the window last said to be current
    current: u32,
    focus: Option<(u32, u32)>,
}

/// A window's HyprlandToplevel and Toplevel objects and their last reported
/// state
struct Top {
    id: (u32, u32),
    hypr: Oid,
    wayland: Oid,
    workspace: u32,
    title: String,
}

/// A window as `publish` is told of it
pub struct Window<'a> {
    pub id: (u32, u32),
    pub title: &'a str,
    pub app_id: &'a str,
    pub workspace: u32,
    pub focused: bool,
    /// Screen position: x, y, width, height
    pub rect: (i32, i32, i32, i32),
    /// `protocol::wm` flags: floating, fullscreen, pinned
    pub flags: u32,
}

/// Screen edges reserved by layers: left, top, right, bottom, as Hyprland's
/// `reserved`
pub struct Screen {
    pub reserved: [i32; 4],
}

impl Hyprland {
    /// Creates the monitor and `count` workspaces under `global`, and toplevels
    /// under `manager`
    pub fn new(ctx: &Ctx<'_>, core: &Core, global: Oid, manager: Oid, count: u32) -> Result<Hyprland, String> {
        let monitor = Builder::new(core, ctx).make_plain("HyprlandMonitor", Some(global))?;
        let mut workspaces = Vec::new();
        for n in 1..=count {
            let ws = Builder::new(core, ctx).make_plain("HyprlandWorkspace", Some(global))?;
            core.store_layout(ctx, ws, "id", n as f64);
            set(ctx, core, ws, "name", Value::str(&n.to_string()));
            set(ctx, core, ws, "monitor", Value::Obj(monitor));
            workspaces.push(ws);
        }
        set(ctx, core, global, "focusedMonitor", Value::Obj(monitor));
        Ok(Hyprland { global, manager, monitor, workspaces, toplevels: Vec::new(), current: 0, focus: None })
    }

    /// Applies the desktop's state: shown workspace, windows, screen
    pub fn publish(&mut self, ctx: &Ctx<'_>, core: &Core, current: u32, windows: &[Window<'_>], screen: &Screen) {
        let mut said = Vec::new();
        if let Err(e) = self.publish_js(ctx, core, current, windows, screen, &mut said) {
            core.log(format!("Hyprland: {}", crate::js::describe(ctx, e)));
        }
        for (name, data) in said {
            raw_event(ctx, core, self.global, &name, &data);
        }
    }

    fn publish_js(&mut self, ctx: &Ctx<'_>, core: &Core, current: u32, windows: &[Window<'_>], screen: &Screen,
                  said: &mut Vec<(String, String)>) -> rquickjs::Result<()> {
        // The monitor is the screen
        let screens = core.0.borrow().screens.first().copied();
        let (name, width, height) = match screens {
            Some(s) => (core.text(s, "name"), core.num(s, "width"), core.num(s, "height")),
            None => (String::new(), 0.0, 0.0),
        };
        set(ctx, core, self.monitor, "name", Value::str(&name));
        core.store_layout(ctx, self.monitor, "width", width);
        core.store_layout(ctx, self.monitor, "height", height);

        // Two objects for each window open, and none for one that closed
        self.toplevels.retain(|t| {
            let open = windows.iter().any(|w| w.id == t.id);
            if !open {
                said.push(("closewindow".into(), address(t.id)));
                core.emit(ctx, t.wayland, "closed", Vec::new());
                core.destroy(ctx, t.wayland);
                core.destroy(ctx, t.hypr);
            }
            open
        });
        for w in windows {
            if self.toplevels.iter().any(|t| t.id == w.id) {
                continue;
            }
            let made = Builder::new(core, ctx).make_plain("HyprlandToplevel", Some(self.global))
                .and_then(|hypr| Ok((hypr, Builder::new(core, ctx).make_plain("Toplevel", Some(self.manager))?)));
            match made {
                Ok((hypr, wayland)) => {
                    set(ctx, core, hypr, "address", Value::str(&address(w.id)[2..]));
                    set(ctx, core, hypr, "wayland", Value::Obj(wayland));
                    set(ctx, core, wayland, "HyprlandToplevel", Value::Obj(hypr));
                    self.toplevels.push(Top { id: w.id, hypr, wayland, workspace: w.workspace, title: w.title.to_string() });
                    said.push(("openwindow".into(), format!("{},{},{},{}", &address(w.id)[2..], w.workspace, w.app_id, w.title)));
                }
                Err(e) => core.log(format!("Hyprland: {e}")),
            }
        }

        let mut active = None;
        for w in windows {
            let Some(t) = self.toplevels.iter_mut().find(|t| t.id == w.id) else { continue };
            let fullscreen = w.flags & libmay::protocol::wm::FLAG_FULLSCREEN != 0;
            let ws = (w.workspace as usize).checked_sub(1).and_then(|i| self.workspaces.get(i)).copied();
            set(ctx, core, t.hypr, "title", Value::str(w.title));
            set(ctx, core, t.hypr, "activated", Value::Bool(w.focused));
            set(ctx, core, t.hypr, "workspace", ws.map(Value::Obj).unwrap_or(Value::Null));
            set(ctx, core, t.hypr, "monitor", Value::Obj(self.monitor));
            set(ctx, core, t.hypr, "lastIpcObject", client(ctx, w, &name)?);
            set(ctx, core, t.wayland, "appId", Value::str(w.app_id));
            set(ctx, core, t.wayland, "title", Value::str(w.title));
            set(ctx, core, t.wayland, "activated", Value::Bool(w.focused));
            set(ctx, core, t.wayland, "fullscreen", Value::Bool(fullscreen));
            if t.workspace != w.workspace {
                t.workspace = w.workspace;
                said.push(("movewindow".into(), format!("{},{}", &address(w.id)[2..], w.workspace)));
                said.push(("movewindowv2".into(), format!("{},{},{}", &address(w.id)[2..], w.workspace, w.workspace)));
            }
            if t.title != w.title {
                t.title = w.title.to_string();
                said.push(("windowtitle".into(), address(w.id)[2..].to_string()));
                said.push(("windowtitlev2".into(), format!("{},{}", &address(w.id)[2..], w.title)));
            }
            if w.focused {
                active = Some((w, t.hypr, t.wayland));
            }
        }

        for (i, &ws) in self.workspaces.iter().enumerate() {
            let n = i as u32 + 1;
            let on: Vec<&Window<'_>> = windows.iter().filter(|w| w.workspace == n).collect();
            let hypr: Vec<Oid> = on.iter()
                .filter_map(|w| self.toplevels.iter().find(|t| t.id == w.id).map(|t| t.hypr)).collect();
            let fullscreen = on.iter().any(|w| w.flags & libmay::protocol::wm::FLAG_FULLSCREEN != 0);
            set(ctx, core, ws, "active", Value::Bool(n == current));
            set(ctx, core, ws, "focused", Value::Bool(n == current));
            set(ctx, core, ws, "hasFullscreen", Value::Bool(fullscreen));
            set(ctx, core, ws, "toplevels", model(ctx, core, &hypr)?);
            set(ctx, core, ws, "lastIpcObject", workspace(ctx, n, &on, &name)?);
        }
        let shown = (current as usize).checked_sub(1).and_then(|i| self.workspaces.get(i)).copied();
        set(ctx, core, self.monitor, "activeWorkspace", shown.map(Value::Obj).unwrap_or(Value::Null));
        set(ctx, core, self.monitor, "lastIpcObject", monitor(ctx, &name, (width, height), current, screen)?);
        set(ctx, core, self.global, "focusedWorkspace", shown.map(Value::Obj).unwrap_or(Value::Null));
        set(ctx, core, self.global, "activeToplevel", active.map(|a| Value::Obj(a.1)).unwrap_or(Value::Null));
        set(ctx, core, self.global, "monitors", model(ctx, core, &[self.monitor])?);
        // Listed workspaces, as in Hyprland: those with a window and the shown
        // one
        let there: Vec<Oid> = self.workspaces.iter().enumerate()
            .filter(|&(i, _)| i as u32 + 1 == current || windows.iter().any(|w| w.workspace == i as u32 + 1))
            .map(|(_, &ws)| ws).collect();
        set(ctx, core, self.global, "workspaces", model(ctx, core, &there)?);
        let all: Vec<Oid> = self.toplevels.iter().map(|t| t.hypr).collect();
        set(ctx, core, self.global, "toplevels", model(ctx, core, &all)?);
        let all: Vec<Oid> = self.toplevels.iter().map(|t| t.wayland).collect();
        set(ctx, core, self.manager, "toplevels", model(ctx, core, &all)?);
        set(ctx, core, self.manager, "activeToplevel", active.map(|a| Value::Obj(a.2)).unwrap_or(Value::Null));

        if current != self.current {
            self.current = current;
            said.push(("workspace".into(), current.to_string()));
            said.push(("workspacev2".into(), format!("{current},{current}")));
        }
        let focus = active.map(|a| a.0.id);
        if focus != self.focus {
            self.focus = focus;
            let (class, title, addr) = active.map(|a| (a.0.app_id, a.0.title, address(a.0.id)[2..].to_string()))
                .unwrap_or_default();
            said.push(("activewindow".into(), format!("{class},{title}")));
            said.push(("activewindowv2".into(), addr));
        }
        Ok(())
    }

    /// `rawEvent` for a layout change: `activelayout>>KEYBOARD,LAYOUT`
    pub fn layout_changed(&self, ctx: &Ctx<'_>, core: &Core, name: &str) {
        raw_event(ctx, core, self.global, "activelayout", &format!("keyboard,{name}"));
    }
}

/// Window address as Hyprland writes it: client and surface in one hex number
/// with `0x`
fn address(id: (u32, u32)) -> String {
    format!("0x{:x}", ((id.0 as u64) << 32) | id.1 as u64)
}

/// Window named by an address.
/// Window of a Toplevel, by its `HyprlandToplevel`'s address
pub(crate) fn window_of_toplevel(core: &Core, toplevel: Oid) -> Option<(u32, u32)> {
    let hypr = core.peek_named(toplevel, "HyprlandToplevel").and_then(|v| v.as_obj())?;
    window_at(&core.text(hypr, "address"))
}

fn window_at(address: &str) -> Option<(u32, u32)> {
    let hex = address.trim().trim_start_matches("address:").trim_start_matches("0x");
    let n = u64::from_str_radix(hex, 16).ok()?;
    Some(((n >> 32) as u32, n as u32))
}

/// `hyprctl clients -j`, for one window
fn client(ctx: &Ctx<'_>, w: &Window<'_>, monitor: &str) -> rquickjs::Result<Value> {
    let o = Object::new(ctx.clone())?;
    let (x, y, wd, ht) = w.rect;
    let f = |bit| w.flags & bit != 0;
    use libmay::protocol::wm::{FLAG_FLOATING, FLAG_FULLSCREEN, FLAG_PINNED};
    o.set("address", address(w.id))?;
    o.set("mapped", true)?;
    o.set("hidden", false)?;
    o.set("at", vec![x, y])?;
    o.set("size", vec![wd, ht])?;
    o.set("workspace", named(ctx, w.workspace)?)?;
    o.set("floating", f(FLAG_FLOATING))?;
    o.set("pseudo", false)?;
    o.set("monitor", 0)?;
    o.set("monitorName", monitor)?;
    o.set("class", w.app_id)?;
    o.set("title", w.title)?;
    o.set("initialClass", w.app_id)?;
    o.set("initialTitle", w.title)?;
    o.set("pid", -1)?;
    o.set("xwayland", false)?;
    o.set("pinned", f(FLAG_PINNED))?;
    o.set("fullscreen", if f(FLAG_FULLSCREEN) { 2 } else { 0 })?;
    o.set("fullscreenClient", if f(FLAG_FULLSCREEN) { 2 } else { 0 })?;
    o.set("grouped", Array::new(ctx.clone())?)?;
    o.set("tags", Array::new(ctx.clone())?)?;
    o.set("swallowing", "0x0")?;
    o.set("focusHistoryID", if w.focused { 0 } else { 1 })?;
    Ok(crate::js::from_js(ctx, o.into_value()))
}

/// `hyprctl workspaces -j`, for workspace `n` and the windows on it
fn workspace(ctx: &Ctx<'_>, n: u32, on: &[&Window<'_>], monitor: &str) -> rquickjs::Result<Value> {
    let o = Object::new(ctx.clone())?;
    let last = on.iter().find(|w| w.focused).or(on.first());
    o.set("id", n)?;
    o.set("name", n.to_string())?;
    o.set("monitor", monitor)?;
    o.set("monitorID", 0)?;
    o.set("windows", on.len() as u32)?;
    o.set("hasfullscreen", on.iter().any(|w| w.flags & libmay::protocol::wm::FLAG_FULLSCREEN != 0))?;
    o.set("lastwindow", last.map(|w| address(w.id)).unwrap_or_else(|| "0x0".into()))?;
    o.set("lastwindowtitle", last.map(|w| w.title).unwrap_or(""))?;
    o.set("ispersistent", false)?;
    Ok(crate::js::from_js(ctx, o.into_value()))
}

/// `hyprctl monitors -j`, for the one screen
fn monitor(ctx: &Ctx<'_>, name: &str, size: (f64, f64), current: u32, screen: &Screen) -> rquickjs::Result<Value> {
    let o = Object::new(ctx.clone())?;
    o.set("id", 0)?;
    o.set("name", name)?;
    o.set("description", "")?;
    o.set("make", "")?;
    o.set("model", "")?;
    o.set("serial", "")?;
    o.set("width", size.0 as i32)?;
    o.set("height", size.1 as i32)?;
    o.set("refreshRate", 60.0)?;
    o.set("x", 0)?;
    o.set("y", 0)?;
    o.set("activeWorkspace", named(ctx, current)?)?;
    let special = Object::new(ctx.clone())?;
    special.set("id", 0)?;
    special.set("name", "")?;
    o.set("specialWorkspace", special)?;
    o.set("reserved", screen.reserved.to_vec())?;
    o.set("scale", 1.0)?;
    o.set("transform", 0)?;
    o.set("focused", true)?;
    o.set("dpmsStatus", true)?;
    o.set("vrr", false)?;
    o.set("disabled", false)?;
    Ok(crate::js::from_js(ctx, o.into_value()))
}

/// `{ id, name }`, as a window and a monitor name their workspace
fn named<'js>(ctx: &Ctx<'js>, n: u32) -> rquickjs::Result<Object<'js>> {
    let o = Object::new(ctx.clone())?;
    o.set("id", n)?;
    o.set("name", n.to_string())?;
    Ok(o)
}

/// Hyprland event `NAME>>DATA` as quickshell's `rawEvent`: name, data, and
/// `parse(n)`
fn raw_event(ctx: &Ctx<'_>, core: &Core, global: Oid, name: &str, data: &str) {
    let made = ctx.eval::<rquickjs::Function, _>(
        "(name, data) => ({ name, data, parse(n) { const p = data.split(','); \
         return n > 0 ? p.slice(0, n - 1).concat([p.slice(n - 1).join(',')]) : p; } })",
    ).and_then(|f| f.call::<_, rquickjs::Value>((name, data)));
    match made {
        Ok(ev) => core.emit(ctx, global, "rawEvent", vec![ev]),
        Err(e) => core.log(format!("Hyprland: {}", crate::js::describe(ctx, e))),
    }
}

/// Object model with `objs` as its `values`
fn model(ctx: &Ctx<'_>, core: &Core, objs: &[Oid]) -> rquickjs::Result<Value> {
    let values = Array::new(ctx.clone())?;
    for (i, &o) in objs.iter().enumerate() {
        values.set(i, crate::js::to_js(ctx, core, &Value::Obj(o))?)?;
    }
    let m = Object::new(ctx.clone())?;
    m.set("values", values)?;
    Ok(crate::js::from_js(ctx, m.into_value()))
}

fn set(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str, v: Value) {
    if let Some(i) = core.prop_idx(oid, name) {
        core.store(ctx, oid, i, v);
    }
}

/// `dispatch`, `monitorFor`, and refreshes on the global; a workspace's
/// `activate`; a Toplevel's `activate` and `close`
pub fn call<'js>(_ctx: &Ctx<'js>, core: &Core, oid: Oid, name: &str, args: &[rquickjs::Value<'js>]) -> Option<Value> {
    let ty = core.0.borrow().objs.get(oid).map(|o| o.def.name)?;
    let push = |a: Action| core.0.borrow_mut().actions.push(a);
    match (ty, name) {
        ("AoshHyprland", "dispatch") => {
            let text = args.first().and_then(|v| v.as_string()).and_then(|s| s.to_string().ok()).unwrap_or_default();
            let current = core.peek_named(oid, "focusedWorkspace").and_then(|v| v.as_obj()).map_or(1, |w| core.num(w, "id") as u32);
            match request(&text, current, crate::services::WORKSPACES) {
                Some(a) => push(a),
                None => core.log(format!("Hyprland.dispatch: not on MikuOS: {text}")),
            }
            Some(Value::Undefined)
        }
        ("AoshHyprland", "monitorFor") => Some(core.peek_named(oid, "focusedMonitor").unwrap_or(Value::Null)),
        ("AoshHyprland", "refreshMonitors" | "refreshWorkspaces" | "refreshToplevels") => Some(Value::Undefined),
        ("HyprlandWorkspace", "activate") => {
            let n = core.num(oid, "id");
            if n >= 1.0 {
                push(Action::Workspace(n as u32));
            }
            Some(Value::Undefined)
        }
        ("Toplevel", "activate" | "close") => {
            if let Some(id) = window_of_toplevel(core, oid) {
                push(if name == "activate" { Action::Focus(id.0, id.1) } else { Action::Close(id.0, id.1) });
            }
            Some(Value::Undefined)
        }
        ("Toplevel", _) => {
            core.log(format!("Toplevel.{name}: not on MikuOS"));
            Some(Value::Undefined)
        }
        _ => None,
    }
}

/// Parses a dispatch in Hyprland or Lua form, with `current` the shown
/// workspace and `count` the total:
///   - `workspace 3`, `workspace r+1`, `hl.dsp.focus({ workspace = 3 })`;
///   - `focuswindow address:0x…`, `hl.dsp.focus({ window = "address:0x…" })`;
///   - `closewindow address:0x…`, `hl.dsp.window.close({ window = … })`;
/// - `movetoworkspacesilent 3,address:0x…`,
///   `hl.dsp.window.move({ workspace = 3, follow = false, window = … })`
fn request(text: &str, current: u32, count: u32) -> Option<Action> {
    let r = text.trim();
    let space = |v: &str| -> Option<u32> {
        let v = v.trim().trim_matches('"');
        let n = match v.strip_prefix('r') {
            Some(rel) => current as i64 + rel.parse::<i64>().ok()?,
            None => v.parse::<i64>().ok()?,
        };
        (n >= 1).then(|| n.min(count as i64) as u32)
    };
    if let Some(rest) = r.strip_prefix("hl.dsp.") {
        let (verb, table) = rest.split_once('(')?;
        let table = table.strip_suffix(')')?.trim().strip_prefix('{')?.strip_suffix('}')?;
        let field = |key: &str| table.split(',').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            (k.trim() == key).then(|| v.trim().trim_matches('"').to_string())
        });
        let window = field("window").and_then(|w| window_at(&w));
        return match verb.trim() {
            "focus" => match window {
                Some(id) => Some(Action::Focus(id.0, id.1)),
                None => Some(Action::Workspace(space(&field("workspace")?)?)),
            },
            "window.close" => window.map(|id| Action::Close(id.0, id.1)),
            // Only a silent move to a workspace; moves on screen belong to the
            // window manager
            "window.move" if field("follow").as_deref() == Some("false") => {
                let (id, n) = (window?, space(&field("workspace")?)?);
                Some(Action::MoveTo(id.0, id.1, n))
            }
            _ => None,
        };
    }
    let (word, arg) = r.split_once(' ').unwrap_or((r, ""));
    match word {
        "workspace" => space(arg).map(Action::Workspace),
        "focuswindow" => window_at(arg).map(|id| Action::Focus(id.0, id.1)),
        "closewindow" => window_at(arg).map(|id| Action::Close(id.0, id.1)),
        "movetoworkspacesilent" => {
            let (n, w) = arg.split_once(',')?;
            let id = window_at(w)?;
            Some(Action::MoveTo(id.0, id.1, space(n)?))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{address, request, window_at};
    use crate::object::Action;

    #[test]
    fn the_requests_end4_makes_are_read() {
        let w = address((3, 7));
        assert_eq!(w, "0x300000007");
        assert_eq!(window_at(&format!("address:{w}")), Some((3, 7)));
        assert_eq!(request("workspace 3", 1, 10), Some(Action::Workspace(3)));
        assert_eq!(request("workspace r+1", 4, 10), Some(Action::Workspace(5)));
        assert_eq!(request("workspace r-1", 1, 10), None);
        assert_eq!(request("hl.dsp.focus({ workspace = 4 })", 1, 10), Some(Action::Workspace(4)));
        assert_eq!(request("hl.dsp.focus({workspace = 2})", 1, 10), Some(Action::Workspace(2)));
        assert_eq!(request(r#"hl.dsp.focus({workspace = "r+1"})"#, 2, 10), Some(Action::Workspace(3)));
        assert_eq!(request(&format!(r#"hl.dsp.focus({{ window = "address:{w}" }})"#), 1, 10), Some(Action::Focus(3, 7)));
        assert_eq!(request(&format!(r#"hl.dsp.window.close({{ window = "address:{w}" }})"#), 1, 10), Some(Action::Close(3, 7)));
        assert_eq!(request(&format!(r#"hl.dsp.window.move({{ workspace = 6, follow = false, window = "address:{w}" }})"#), 1, 10),
                   Some(Action::MoveTo(3, 7, 6)));
        assert_eq!(request(&format!("movetoworkspacesilent 2,address:{w}"), 1, 10), Some(Action::MoveTo(3, 7, 2)));
        assert_eq!(request(&format!(r#"hl.dsp.window.move({{ x = "10", y = "20", window = "address:{w}" }})"#), 1, 10), None);
        assert_eq!(request(r#"hl.dsp.global("quickshell:wallpaperSelectorToggle")"#, 1, 10), None);
    }
}
