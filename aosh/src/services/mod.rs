// Service objects exposed to files
//
// `Desktop`, `Sys`, and `Theme` are globals created before the shell's file
// loads. `Hyprland` is the desktop in quickshell's Hyprland shape
// (`hyprland.rs`). `Mpris` and `UPower` have no backend on MikuOS.
// `SystemClock` is a type files instantiate.

pub mod apps;
pub mod capture;
pub mod hyprland;

use rquickjs::{Array, Ctx, Function, Object};

use crate::object::engine::Engine;
use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

/// How often `/proc` is read and the clocks are told the time
pub const SAMPLE_MS: u64 = 1000;

/// Mayland's workspace count
pub(crate) const WORKSPACES: u32 = libmay::protocol::desktop::WORKSPACES;

struct Win {
    client: u32,
    surface: u32,
    title: String,
    app_id: String,
    workspace: u32,
    rect: (i32, i32, i32, i32),
    flags: u32,
}

pub struct Services {
    pub desktop: Oid,
    pub sys: Oid,
    /// `DesktopEntries`, for the tests to publish their own apps into
    #[cfg(test)]
    pub apps: Oid,
    /// The same desktop in `Quickshell.Hyprland`'s shape
    hyprland: hyprland::Hyprland,
    wins: Vec<Win>,
    focus: (u32, u32),
    /// Usable rectangle and screen size: x, y, width, height, screen width,
    /// screen height
    work: (i32, i32, i32, i32, i32, i32),
    /// Last second reported to the clocks
    last_second: Option<i64>,
}

impl Services {
    pub fn new(engine: &mut Engine) -> Result<Services, String> {
        let desktop = engine.make_global("Desktop", "AoshDesktop")?;
        let sys = engine.make_global("Sys", "AoshSys")?;
        let theme = engine.make_global("Theme", "AoshTheme")?;
        let entries = engine.make_global("DesktopEntries", "AoshDesktopEntries")?;
        let hypr = engine.make_global("Hyprland", "AoshHyprland")?;
        let toplevels = engine.make_global("ToplevelManager", "AoshToplevelManager")?;
        let mpris = engine.make_global("Mpris", "AoshMpris")?;
        let upower = engine.make_global("UPower", "AoshUPower")?;
        let tray = engine.make_global("SystemTray", "AoshSystemTray")?;
        let pipewire = engine.make_global("Pipewire", "AoshPipewire")?;
        let bluetooth = engine.make_global("Bluetooth", "AoshBluetooth")?;
        let hyprland = engine.with_ctx(|ctx, core| hyprland::Hyprland::new(ctx, core, hypr, toplevels, WORKSPACES))?;
        let mut s = Services {
            desktop, sys, #[cfg(test)] apps: entries, hyprland, wins: Vec::new(), focus: (0, 0),
            work: (0, 0, 0, 0, 0, 0), last_second: None,
        };
        engine.with_ctx(|ctx, core| -> Result<(), String> {
            read_theme(ctx, core, theme);
            apps::publish(ctx, core, entries, std::path::Path::new(apps::APPS_D));
            nothing_behind(ctx, core, &[(mpris, "players"), (upower, "devices"), (tray, "items"), (pipewire, "nodes"),
                                        (pipewire, "links"), (pipewire, "linkGroups"), (bluetooth, "adapters"),
                                        (bluetooth, "devices")])?;
            let device = crate::object::build::Builder::new(core, ctx).make_plain("UPowerDevice", Some(upower))?;
            set(ctx, core, upower, "displayDevice", Value::Obj(device));
            s.publish_desktop(ctx, core);
            Ok(())
        })?;
        Ok(s)
    }

    /// Once a second: `/proc` and every `SystemClock`
    pub fn sample(&mut self, engine: &Engine) {
        let sys = self.sys;
        engine.with_ctx(|ctx, core| {
            if let Some(t) = read("/proc/meminfo") {
                // Linux names: used is total minus available
                let total = field(&t, "MemTotal:").unwrap_or(0.0);
                let used = total - field(&t, "MemAvailable:").unwrap_or(total);
                core.store_layout(ctx, sys, "memTotal", total);
                core.store_layout(ctx, sys, "memUsed", used);
                let pct = if total > 0.0 { (used * 100.0 / total).floor() } else { 0.0 };
                core.store_layout(ctx, sys, "memPercent", pct);
            }
            if let Some(t) = read("/proc/net") {
                let up = text_field(&t, "link:") == Some("up");
                let ip = text_field(&t, "ip:").unwrap_or("0.0.0.0");
                let shown = if !up || ip == "0.0.0.0" { "offline" } else { ip };
                set(ctx, core, sys, "netIp", Value::str(shown));
            }
            if let Some(t) = read("/proc/uptime") {
                // Linux format, uptime first: `1234.56 4321.09`
                let secs = t.split_whitespace().next()
                    .and_then(|w| w.parse::<f64>().ok())
                    .unwrap_or(0.0) as u64;
                core.store_layout(ctx, sys, "uptime", secs as f64);
                let text = format!("{}h{:02}m", secs / 3600, (secs / 60) % 60);
                set(ctx, core, sys, "uptimeText", Value::str(&text));
            }
            self.tick_clocks(ctx, core);
        });
    }

    /// Every `SystemClock` told the time, as finely as it asked
    fn tick_clocks(&mut self, ctx: &Ctx<'_>, core: &Core) {
        let clocks: Vec<Oid> = {
            let s = core.0.borrow();
            s.objs.ids().into_iter().filter(|&o| s.objs[o].def.kind == Kind::Clock).collect()
        };
        if clocks.is_empty() {
            return;
        }
        let Some(now) = Now::read(ctx) else { return };
        let second = (now.h as i64) * 3600 + (now.m as i64) * 60 + now.s as i64;
        if self.last_second == Some(second) {
            return;
        }
        let was = self.last_second;
        self.last_second = Some(second);
        for c in clocks {
            if !core.flag(c, "enabled") {
                continue;
            }
            let precision = core.num(c, "precision") as i64;
            // A minute clock changes once a minute, since a new `date` reruns
            // its bindings
            let unit = match precision { 0 => 3600, 1 => 60, _ => 1 };
            if was.is_some_and(|w| w / unit == second / unit) && core.peek_named(c, "date").is_some_and(|v| !matches!(v, Value::Undefined)) {
                continue;
            }
            now.tell(ctx, core, c);
        }
    }

    /// One `may_desktop` event. True when the window list must be requested
    /// again: a window opened, or focus moved and a title may have changed
    pub fn desk(&mut self, engine: &Engine, ev: &libmay::conn::Event) -> bool {
        use libmay::conn::Event;
        let desktop = self.desktop;
        let mut stale = false;
        match *ev {
            Event::DeskWindow { client, surface, ref title, title_len, ref app_id, app_id_len, x, y, width, height, flags, workspace } => {
                let title = String::from_utf8_lossy(&title[..title_len]).into_owned();
                let app_id = String::from_utf8_lossy(&app_id[..app_id_len]).into_owned();
                let rect = (x, y, width, height);
                match self.wins.iter_mut().find(|w| w.client == client && w.surface == surface) {
                    Some(w) => {
                        w.title = title;
                        w.app_id = app_id;
                        w.workspace = workspace;
                        w.rect = rect;
                        w.flags = flags;
                    }
                    None => self.wins.push(Win { client, surface, title, app_id, workspace, rect, flags }),
                }
            }
            Event::DeskClosed { client, surface } => {
                self.wins.retain(|w| !(w.client == client && w.surface == surface));
                if self.focus == (client, surface) {
                    self.focus = (0, 0);
                }
            }
            Event::DeskOpened { .. } => return true,
            Event::DeskFocused { client, surface } => {
                self.focus = (client, surface);
                stale = true;
            }
            Event::DeskWorkspace { index } => {
                engine.with_ctx(|ctx, core| core.store_layout(ctx, desktop, "workspace", index as f64));
            }
            Event::DeskWorkArea { x, y, width, height, screen_w, screen_h } => {
                self.work = (x, y, width, height, screen_w, screen_h);
                engine.with_ctx(|ctx, core| {
                    core.store_layout(ctx, desktop, "workTop", y as f64);
                    core.store_layout(ctx, desktop, "screenWidth", screen_w as f64);
                    core.store_layout(ctx, desktop, "screenHeight", screen_h as f64);
                });
                engine.set_screen(screen_w.max(0) as u32, screen_h.max(0) as u32);
            }
            Event::DeskKeyboardLayout { group, groups, ref code, code_len, ref name, name_len } => {
                let code = String::from_utf8_lossy(&code[..code_len]).into_owned();
                let name = String::from_utf8_lossy(&name[..name_len]).into_owned();
                engine.with_ctx(|ctx, core| {
                    set(ctx, core, desktop, "keyboardLayout", Value::str(&code));
                    set(ctx, core, desktop, "keyboardLayoutName", Value::str(&name));
                    core.store_layout(ctx, desktop, "keyboardLayoutIndex", group as f64);
                    core.store_layout(ctx, desktop, "keyboardLayouts", groups as f64);
                    self.hyprland.layout_changed(ctx, core, &name);
                });
                return false;
            }
            Event::DeskSignalled { ref name, name_len } => {
                let word = String::from_utf8_lossy(&name[..name_len]).into_owned();
                engine.with_ctx(|ctx, core| {
                    // `signal, launcher toggle` calls `launcher.toggle()` if an
                    // IpcHandler has that target (`ipc/`), and is a word either
                    // way
                    crate::ipc::word(ctx, core, &word);
                    let w = rquickjs::String::from_str(ctx.clone(), &word).map(|s| s.into_value());
                    if let Ok(w) = w {
                        core.emit(ctx, desktop, "signalled", vec![w]);
                    }
                });
                return false;
            }
            _ => return false,
        }
        engine.with_ctx(|ctx, core| self.publish_desktop(ctx, core));
        stale
    }

    /// The windows and workspaces as the properties files read
    fn publish_desktop(&mut self, ctx: &Ctx<'_>, core: &Core) {
        let current = core.num(self.desktop, "workspace") as u32;
        let r = (|| -> rquickjs::Result<()> {
            let wins = Array::new(ctx.clone())?;
            for (i, w) in self.wins.iter().enumerate() {
                let o = Object::new(ctx.clone())?;
                o.set("title", w.title.as_str())?;
                o.set("appId", w.app_id.as_str())?;
                o.set("workspace", w.workspace)?;
                o.set("focused", (w.client, w.surface) == self.focus)?;
                wins.set(i, o)?;
            }
            let spaces = Array::new(ctx.clone())?;
            for n in 1..=WORKSPACES {
                let o = Object::new(ctx.clone())?;
                o.set("n", n)?;
                o.set("active", n == current)?;
                o.set("busy", self.wins.iter().any(|w| w.workspace == n))?;
                spaces.set((n - 1) as usize, o)?;
            }
            let title = self.wins.iter().find(|w| (w.client, w.surface) == self.focus)
                .map(|w| w.title.clone()).unwrap_or_default();
            set(ctx, core, self.desktop, "windows", crate::js::from_js(ctx, wins.into_value()));
            set(ctx, core, self.desktop, "workspaces", crate::js::from_js(ctx, spaces.into_value()));
            set(ctx, core, self.desktop, "focusedTitle", Value::str(&title));
            Ok(())
        })();
        if let Err(e) = r {
            core.log(format!("Desktop: {}", crate::js::describe(ctx, e)));
        }
        let windows: Vec<hyprland::Window<'_>> = self.wins.iter().map(|w| hyprland::Window {
            id: (w.client, w.surface),
            title: &w.title,
            app_id: &w.app_id,
            workspace: w.workspace,
            focused: (w.client, w.surface) == self.focus,
            rect: w.rect,
            flags: w.flags,
        }).collect();
        // What the layers keep of each edge, as the work area leaves it
        let (x, y, w, h, sw, sh) = self.work;
        let reserved = if sw > 0 && sh > 0 { [x, y, sw - (x + w), sh - (y + h)].map(|v| v.max(0)) } else { [0; 4] };
        self.hyprland.publish(ctx, core, current, &windows, &hyprland::Screen { reserved });
    }

    /// `workspace` changed under the list: `active` moves
    pub fn workspace_changed(&mut self, engine: &Engine) {
        engine.with_ctx(|ctx, core| self.publish_desktop(ctx, core));
    }
}

/// Sets initial state: a SystemClock has the time from creation, as in
/// quickshell, and a NotificationServer's `trackedNotifications` starts as an
/// empty model
pub fn made(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let (kind, name) = { let s = core.0.borrow(); (s.objs[oid].def.kind, s.objs[oid].def.name) };
    // A notification server tracks nothing, from the start
    if name == "NotificationServer" {
        if let Ok(m) = empty_model(ctx) {
            set(ctx, core, oid, "trackedNotifications", m);
        }
        return;
    }
    if kind != Kind::Clock {
        return;
    }
    if let Some(now) = Now::read(ctx) {
        now.tell(ctx, core, oid);
    }
}

/// The time, as JavaScript's `new Date()` says it, and its parts
struct Now<'js> {
    date: Object<'js>,
    h: f64,
    m: f64,
    s: f64,
}

impl<'js> Now<'js> {
    fn read(ctx: &Ctx<'js>) -> Option<Now<'js>> {
        let date = ctx.eval::<Object, _>("new Date()").ok()?;
        let call = |name: &str| -> f64 {
            date.get::<_, Function>(name)
                .and_then(|f| f.call::<_, f64>((rquickjs::function::This(date.clone()),)))
                .unwrap_or(0.0)
        };
        let (h, m, s) = (call("getHours"), call("getMinutes"), call("getSeconds"));
        Some(Now { date, h, m, s })
    }

    fn tell(&self, ctx: &Ctx<'js>, core: &Core, clock: Oid) {
        core.store_layout(ctx, clock, "hours", self.h);
        core.store_layout(ctx, clock, "minutes", self.m);
        core.store_layout(ctx, clock, "seconds", self.s);
        let v = crate::js::from_js(ctx, self.date.clone().into_value());
        set(ctx, core, clock, "date", v);
    }
}

/// `Mpris` and `UPower` with no backend: no players, no devices, a display
/// device not ready
fn nothing_behind(ctx: &Ctx<'_>, core: &Core, models: &[(Oid, &str)]) -> Result<(), String> {
    for &(oid, name) in models {
        set(ctx, core, oid, name, empty_model(ctx).map_err(|e| crate::js::describe(ctx, e))?);
    }
    Ok(())
}

/// Empty object model: `values` is an empty list
fn empty_model(ctx: &Ctx<'_>) -> rquickjs::Result<Value> {
    let m = Object::new(ctx.clone())?;
    m.set("values", Array::new(ctx.clone())?)?;
    Ok(crate::js::from_js(ctx, m.into_value()))
}

fn set(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str, v: Value) {
    if let Some(i) = core.prop_idx(oid, name) {
        core.store(ctx, oid, i, v);
    }
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn text_field<'a>(text: &'a str, key: &str) -> Option<&'a str> {
    text.lines().find_map(|l| l.trim().strip_prefix(key)).map(str::trim)
}

fn field(text: &str, key: &str) -> Option<f64> {
    text_field(text, key)?.split_whitespace().next()?.parse().ok()
}

/// The desktop's `theme { }` block, from the config files in order
fn read_theme(ctx: &Ctx<'_>, core: &Core, theme: Oid) {
    struct Sink(Vec<(String, String)>);
    impl libmay::conf::Sink for Sink {
        fn key(&mut self, _at: libmay::conf::Where, section: &str, key: &str, value: &str) {
            if section == "theme" {
                self.0.push((key.to_string(), value.to_string()));
            }
        }
        fn note(&mut self, _at: libmay::conf::Where, _note: libmay::conf::Note, _text: &str) {}
    }
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for dir in ["/etc/mayland/default.d", "/etc/mayland/conf.d"] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            let mut v: Vec<_> = rd.filter_map(Result::ok).map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|e| e == "conf")).collect();
            v.sort();
            files.extend(v);
        }
        if dir.ends_with("default.d") {
            files.push("/etc/mayland/may.conf".into());
        }
    }
    let mut sink = Sink(Vec::new());
    let vars = libmay::conf::Vars::new();
    for (i, f) in files.iter().enumerate() {
        if let Ok(text) = std::fs::read_to_string(f) {
            libmay::conf::parse_into(&text, &mut sink, &vars, i as u8);
        }
    }
    for (key, value) in sink.0 {
        let prop = match key.as_str() {
            "on_accent" => "onAccent",
            "text_muted" => "textMuted",
            "wallpaper" => {
                set(ctx, core, theme, "wallpaper", Value::str(value.trim()));
                continue;
            }
            k => k,
        };
        if let Some(c) = libmay::conf::colour(&value) {
            set(ctx, core, theme, prop, Value::str(&format!("#{c:08x}")));
        }
    }
}
