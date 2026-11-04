// Object model and JavaScript together: load files, read and write properties,
// emit signals, advance the clock, evaluate expressions

use std::path::Path;

use super::build::{binding_body, Builder};
use super::value::Oid;
#[cfg(test)]
use super::value::Value;
use super::Core;
use crate::js::Js;

pub struct Engine {
    pub core: Core,
    js: Js,
}

impl Engine {
    pub fn new() -> Result<Engine, String> {
        let js = Js::new()?;
        let core = Core::new();
        js.install(&core)?;
        let mut e = Engine { core, js };
        e.make_screens()?;
        Ok(e)
    }

    /// `Quickshell.screens`: the single output, 0 by 0 until `set_screen`
    fn make_screens(&mut self) -> Result<(), String> {
        let holder = self.make_global("__aosh_screens", "AoshScreens")?;
        let core = &self.core;
        self.js.with(|ctx| -> Result<(), String> {
            let screen = Builder::new(core, &ctx).make_plain("ShellScreen", Some(holder))?;
            core.0.borrow_mut().screens = vec![screen];
            if let Some(i) = core.prop_idx(holder, "screens") {
                core.store(&ctx, holder, i, super::value::Value::List(std::rc::Rc::new(vec![super::value::Value::Obj(screen)])));
            }
            Ok(())
        })
    }

    /// The output's size, from the compositor or `aosh shot`
    pub fn set_screen(&self, width: u32, height: u32) {
        let Some(&screen) = self.core.0.borrow().screens.first() else { return };
        self.js.with(|ctx| {
            self.core.store_layout(&ctx, screen, "width", width as f64);
            self.core.store_layout(&ctx, screen, "height", height as f64);
            self.settle(&ctx);
        });
    }

    pub fn load_file(&mut self, path: &Path) -> Result<Oid, String> {
        self.modules_for(path)?;
        let src = std::fs::read_to_string(path).unwrap_or_default();
        self.core.0.borrow_mut().native_text =
            crate::qml::import::shell_pragmas(&src).iter().any(|p| p == "NativeTextRendering");
        let core = &self.core;
        self.js.with(|ctx| Builder::new(core, &ctx).load_file(path))
    }

    /// Modules of the shell containing `path` (`qs` is its directory,
    /// `qml/import.rs`). Read once per directory; singletons and enums become
    /// global names
    fn modules_for(&mut self, path: &Path) -> Result<(), String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let root = path.parent().map(Path::to_path_buf).unwrap_or_default();
        if self.core.0.borrow().modules.as_ref().is_some_and(|m| m.root == root) {
            return Ok(());
        }
        let m = crate::qml::import::Modules::scan(&root);
        for (name, file) in &m.clashes {
            self.core.log(format!("{}: a second singleton named {name}, which the first one hides", file.display()));
        }
        let names: Vec<String> = m.singletons.keys().cloned().collect();
        let enums = m.enums.clone();
        self.core.0.borrow_mut().modules = Some(std::rc::Rc::new(m));
        self.js.with(|ctx| -> rquickjs::Result<()> {
            let f: rquickjs::Function = ctx.globals().get("__aosh_singletons")?;
            f.call::<_, ()>((names,))?;
            let f: rquickjs::Function = ctx.globals().get("__aosh_enums")?;
            for (ty, list) in enums {
                // `[["Shape", [["Circle", 0], ...]], ...]`. Names are
                // identifiers, so Debug output is valid JSON
                let json = format!("[{}]", list.iter().map(|(e, vals)| format!("[{e:?},[{}]]",
                    vals.iter().map(|(k, v)| format!("[{k:?},{v}]")).collect::<Vec<_>>().join(","))).collect::<Vec<_>>().join(","));
                let v: rquickjs::Value = ctx.json_parse(json)?;
                f.call::<_, ()>((ty, v))?;
            }
            Ok(())
        }).map_err(|e| self.js.with(|ctx| crate::js::describe(&ctx, e)))
    }

    /// A service object, a global in every file (`Desktop`, `Sys`, `Theme`)
    pub fn make_global(&mut self, global: &str, ty: &str) -> Result<Oid, String> {
        let core = &self.core;
        self.js.with(|ctx| Builder::new(core, &ctx).make_global(global, ty))
    }

    /// Non-file source, for tests
    #[cfg(test)]
    pub fn load_source(&mut self, name: &str, src: &str, dir: &Path) -> Result<Oid, String> {
        let core = &self.core;
        self.js.with(|ctx| Builder::new(core, &ctx).load_source(name, src, dir))
    }

    /// Assignment as in JavaScript, removing the property's binding. For tests
    #[cfg(test)]
    pub fn set(&self, oid: Oid, name: &str, v: Value) -> Result<(), String> {
        let idx = self.core.prop_idx(oid, name).ok_or_else(|| format!("no property '{name}'"))?;
        self.js.with(|ctx| {
            self.core.assign(&ctx, oid, idx, v);
            self.settle(&ctx);
        });
        Ok(())
    }

    /// On leaving the engine: runs `Qt.callLater` entries, then frees destroyed
    /// objects (`object/life.rs`)
    fn settle(&self, ctx: &rquickjs::Ctx<'_>) {
        crate::js::run_later(ctx, &self.core);
        self.core.reap();
    }

    /// Runs code that needs the JavaScript context, such as layout
    pub fn with_ctx<R>(&self, f: impl FnOnce(&rquickjs::Ctx<'_>, &Core) -> R) -> R {
        self.js.with(|ctx| {
            let r = f(&ctx, &self.core);
            self.settle(&ctx);
            r
        })
    }

    /// Sets the clock to `now`: fires due timers, steps animations, reports
    /// exited processes
    pub fn advance(&self, now: u64) {
        self.js.with(|ctx| {
            self.core.advance(&ctx, now);
            crate::anim::tick(&ctx, &self.core);
            crate::io::reap(&ctx, &self.core);
            self.settle(&ctx);
        });
    }

    /// The loop woke at `now`. Timers and double-click intervals started by its
    /// events count from here, not from the last `advance`. Nothing fires until
    /// `advance`
    pub fn woke(&self, now: u64) {
        let mut s = self.core.0.borrow_mut();
        s.now_ms = s.now_ms.max(now);
    }

    /// Animations started since the last `advance` begin at `now`
    /// (`anim::begin_fresh`)
    pub fn start_new(&self, now: u64) {
        crate::anim::begin_fresh(&self.core, now);
    }

    /// Whether anything animates; the shell requests frames while true
    pub fn animating(&self) -> bool {
        crate::anim::animating(&self.core)
    }

    /// Whether a Process is running
    pub fn running_processes(&self) -> bool {
        self.core.0.borrow().procs.values().any(|p| p.is_live())
    }

    /// Descriptors polled beside the display: process pipes and the inotify
    /// watch
    pub fn io_fds(&self) -> Vec<(i32, crate::io::Fd)> {
        crate::io::fds(&self.core)
    }

    /// One of them is readable
    pub fn io_ready(&self, fd: crate::io::Fd) {
        self.js.with(|ctx| {
            crate::io::ready(&ctx, &self.core, fd);
            self.settle(&ctx);
        });
    }

    /// Destroys `oid` and everything it owns: the old tree after a reload
    pub fn destroy(&self, oid: Oid) {
        self.js.with(|ctx| {
            self.core.destroy(&ctx, oid);
            self.settle(&ctx);
        });
    }

    /// Key press (`input/keys.rs`): Linux keycode, layout character, modifiers.
    /// Returns whether it was consumed
    pub fn key(&self, code: u32, ch: Option<char>, mods: u32) -> bool {
        self.js.with(|ctx| {
            let r = crate::input::keys::key(&ctx, &self.core, code, ch, mods);
            self.settle(&ctx);
            r
        })
    }

    /// The compositor gave `win` the keyboard, or took it away (`None`)
    pub fn keyboard(&self, win: Option<Oid>) {
        self.js.with(|ctx| {
            crate::input::keys::keyboard(&ctx, &self.core, win);
            self.settle(&ctx);
        });
    }

    pub fn next_due(&self) -> Option<u64> {
        self.core.next_due()
    }

    /// Evaluates an expression as a binding on `oid`, returning text
    pub fn eval(&self, oid: Oid, expr: &str) -> Result<String, String> {
        self.js.with(|ctx| {
            let cx = self.core.0.borrow().objs.get(oid).ok_or("no such object")?.cx;
            let r = crate::js::compile(&ctx, &self.core, "", &[], &binding_body(expr), cx)
                .and_then(|f| crate::js::call0(&ctx, &self.core, &f, oid))
                .and_then(|v| rquickjs::Coerced::<String>::from_js(&ctx, v).map(|c| c.0));
            self.settle(&ctx);
            r.map_err(|e| crate::js::describe(&ctx, e))
        })
    }

    /// What the files have said went wrong, oldest first, and forgotten
    pub fn take_log(&self) -> Vec<String> {
        std::mem::take(&mut self.core.0.borrow_mut().log)
    }
}

use rquickjs::FromJs;

impl Drop for Engine {
    fn drop(&mut self) {
        // Frees all JavaScript values first; QuickJS will not close with any
        // alive
        self.core.clear();
    }
}
