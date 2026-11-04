// Object model: properties, bindings, signals
//
// One table holds all objects and bindings. A property has a value, a coercion
// type, and subscribed bindings. A binding is a JavaScript function plus the
// properties read on its last run: reads during a binding subscribe it
// (`read`), and a change runs subscribers (`notify`). The set is rebuilt on
// each run, since conditional bindings read different properties.
//
// Nothing is borrowed while JavaScript runs: QuickJS callbacks (`js/`) take the
// same `RefCell`.

pub mod build;
pub mod engine;
pub mod hold;
pub mod sync;
pub mod life;
pub mod table;
pub mod value;

#[cfg(test)]
mod tests;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use rquickjs::function::{Rest, This};
use rquickjs::{Ctx, Function, Object, Persistent};

use crate::qml::ast::Loc;
use crate::types::{Kind, TypeDef};
use value::{Oid, PropType, Value};

pub type Bid = u32;

pub struct Prop {
    pub name: Rc<str>,
    pub ty: PropType,
    pub value: Value,
    pub binding: Option<Bid>,
    pub observers: Vec<Bid>,
    /// `property alias name: target.path`: reads and writes go to the target
    pub alias: Option<(Oid, u32)>,
    /// The aliases of this property, whose change handlers fire with it
    pub aliased_by: Vec<(Oid, u32)>,
    pub readonly: bool,
    /// `Behavior on name { ... }`: a store starts the Behavior's animation
    /// instead (`anim/`)
    pub behavior: Option<Oid>,
    /// Set by the file or a script rather than the layout. The layout does not
    /// override it, so a `Text` with `width: 100` stays 100 wide
    pub explicit: bool,
    /// `required property`: set by a delegate's view from the row
    pub required: bool,
}

pub struct Handler {
    pub func: Persistent<Function<'static>>,
    pub this: Oid,
}

#[derive(Clone)]
pub struct Signal {
    pub name: Rc<str>,
    pub params: Vec<String>,
}

/// Name lookup index, built on first lookup and dropped when the object gains a
/// property or signal (`Obj::renamed`)
#[derive(Default)]
pub struct Names {
    /// First and last property of each name: the type's own and the file's
    /// redeclaration
    props: HashMap<Rc<str>, (u32, u32)>,
    signals: std::collections::HashSet<Rc<str>>,
    /// Every `a` and `a.b` that some `a.b.c` begins with
    groups: std::collections::HashSet<Box<str>>,
}

impl Names {
    fn of(o: &Obj) -> Names {
        let mut n = Names::default();
        for (i, p) in o.props.iter().enumerate() {
            let i = i as u32;
            n.props.entry(p.name.clone()).and_modify(|e| e.1 = i).or_insert((i, i));
            let mut at = 0;
            while let Some(d) = p.name[at..].find('.') {
                n.groups.insert(p.name[..at + d].into());
                at += d + 1;
            }
        }
        for g in &o.signals {
            n.signals.insert(g.name.clone());
        }
        n
    }
}

pub struct Obj {
    /// The type as written: `Bar`, `Rectangle`
    pub ty: String,
    /// The nearest type aosh itself defines
    pub def: &'static TypeDef,
    pub props: Vec<Prop>,
    pub signals: Vec<Signal>,
    /// Signal name to what runs when it is emitted
    pub handlers: Vec<(Rc<str>, Handler)>,
    /// Visual parent: the declaring object, or the Repeater's parent for a
    /// delegate. `Item` also exposes it as `parent`
    pub parent: Option<Oid>,
    pub children: Vec<Oid>,
    /// Destroyed by: the declaring object, or the Repeater or Loader that
    /// created it
    pub owner: Option<Oid>,
    /// Destroyed with it beyond its children: property value objects
    /// (`stdout: StdioCollector {}`) and delegates
    pub owned: Vec<Oid>,
    pub wrapper: Option<Persistent<rquickjs::Value<'static>>>,
    /// Creation id scope
    pub cx: u32,
    pub file: u32,
    pub loc: Loc,
    /// A `Connections`: its handlers for the target's signals and the object
    /// they are on
    pub listens: Vec<(Rc<str>, Persistent<Function<'static>>)>,
    pub listening_on: Option<Oid>,
    /// Initial values are set (`build.rs`, "Complete"). Until then type hooks
    /// wait, so a Process does not start before `command` is set
    pub complete: bool,
    /// A component's `default property alias`: where an instance's children go
    /// (`build.rs`, `default_parent`)
    pub default_into: Option<Rc<str>>,
    /// Store counter (`State::changes`) at the last change of this object, and
    /// the latest over its subtree. Lets a frame lay out and list only what
    /// changed (`Core::touch`)
    pub changed: u64,
    pub below: u64,
    /// Its names, indexed (`Names`)
    pub names: std::cell::OnceCell<Names>,
}

impl Obj {
    pub fn names(&self) -> &Names {
        self.names.get_or_init(|| Names::of(self))
    }

    /// Drops the name index after a property or signal is added
    pub fn renamed(&mut self) {
        self.names.take();
    }
}

pub struct Binding {
    pub target: (Oid, u32),
    pub func: Persistent<Function<'static>>,
    pub this: Oid,
    pub file: u32,
    pub loc: Loc,
    pub deps: Vec<(Oid, u32)>,
    pub alive: bool,
    /// Suspended by a Binding object (`hold.rs`)
    pub held: bool,
    /// Created with its tree and not run yet (`build.rs`); a read runs it first
    pub pending: bool,
}

/// Id scope: one per instance of a file or component, as in QML
pub struct Context {
    pub ids: HashMap<String, Oid>,
    pub ids_js: Persistent<Object<'static>>,
    /// Enclosing scopes as one lookup object, or `null` (`build.rs`,
    /// `new_context`)
    pub outer_js: Persistent<rquickjs::Value<'static>>,
    pub root: Option<Oid>,
    /// Live objects created in it; the slot is free at zero
    pub objects: u32,
}

#[derive(Default)]
pub struct State {
    /// Native text for shells with `//@ pragma NativeTextRendering`. Otherwise
    /// Qt's distance fields (`scene/text/field.rs`)
    pub native_text: bool,
    pub objs: table::Objs,
    pub binds: Vec<Binding>,
    pub cxs: Vec<Context>,
    /// Running bindings, innermost last, each with what it has read
    pub stack: Vec<(Bid, Vec<(Oid, u32)>)>,
    /// Bindings whose result is being stored. Rerunning one during its store
    /// means a loop
    pub storing: Vec<Bid>,
    pub log: Vec<String>,
    pub files: Vec<String>,
    pub now_ms: u64,
    /// Running timers and when each is next due
    pub timers: Vec<(Oid, u64)>,
    /// Compiled function factories by source location
    pub factories: HashMap<String, Persistent<Function<'static>>>,
    /// Count of stores that changed something. Layout repeats until a round
    /// leaves this unchanged
    pub changes: u64,
    /// Changed since the scene last looked
    pub dirty: bool,
    /// Windows are drawn on the processor (`aosh ipc call aosh render cpu`)
    pub render_cpu: bool,
    /// External requests from scripts, handled by the shell's loop
    pub actions: Vec<Action>,
    /// Started programs not yet reaped
    pub children: Vec<std::process::Child>,
    /// Free binding slots. Only taken while no binding runs, since a killed one
    /// stays on the stack until it returns
    pub free_binds: Vec<Bid>,
    /// Parsed files, available to any builder since delegates are created later
    pub docs: Rc<RefCell<build::Files>>,
    /// Free id scope slots
    pub free_cxs: Vec<u32>,
    /// Destroyed but not freed, still readable by the destroying handler
    /// (`life.rs`, `reap`)
    pub dying: Vec<Oid>,
    /// The Synchronizers watching each property (`sync.rs`)
    pub syncs: HashMap<(Oid, u32), Vec<Oid>>,
    /// How many parts of trees have failed to load, ever (`load_error`)
    pub load_errors: u64,
    pub comps: HashMap<Oid, life::ComponentDef>,
    /// What each Repeater, ListView and Loader has made (`model/`)
    pub views: HashMap<Oid, crate::model::View>,
    /// A ListModel's rows, which are objects (`model/`)
    pub lists: HashMap<Oid, Vec<Oid>>,
    pub anims: crate::anim::Animator,
    /// Running `Process` objects and what they have read (`io/`)
    pub procs: HashMap<Oid, crate::io::Proc>,
    /// `FileView` objects and the inotify watch they and the reload share
    pub io: crate::io::Io,
    /// Who has the keyboard (`input/`)
    pub keys: crate::input::keys::Keys,
    /// The IpcHandlers' functions, and the socket `aosh ipc` reaches
    pub ipc: crate::ipc::Ipc,
    /// Shell modules (`qs` and below), set after the first file loads
    /// (`qml/import.rs`)
    pub modules: Option<Rc<crate::qml::import::Modules>>,
    /// Created singletons by name. Added before their bindings run, so mutual
    /// references resolve
    pub singletons: HashMap<String, Oid>,
    /// Imported JavaScript files' namespaces, by path (`js/library.rs`)
    pub libs: HashMap<std::path::PathBuf, Persistent<Object<'static>>>,
    /// Per-directory `Qt` resolving URLs against it (`build.rs`, `file_scope`)
    pub file_qt: HashMap<std::path::PathBuf, Persistent<Object<'static>>>,
    /// The properties Binding objects hold, by Binding (`hold.rs`)
    pub held: HashMap<Oid, hold::Held>,
    /// `Quickshell.screens`' objects (`engine.rs`)
    pub screens: Vec<Oid>,
    /// Font files FontLoaders have read, for the scene to add (`io/`)
    pub font_files: Vec<std::path::PathBuf>,
    /// Canvases that called `requestPaint`, for the next layout
    pub canvas_due: Vec<Oid>,
    /// Path overrides for `aosh shot --map` (`Core::path`)
    pub path_map: Vec<(std::path::PathBuf, std::path::PathBuf)>,
}

/// Script request that only the shell's loop can carry out
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// `Desktop.switchTo(n)`
    Workspace(u32),
    /// `Desktop.signal("launcher")`: a word to every `may_desktop` listener
    Signal(String),
    /// A window by client and surface: focus, close, or move to a workspace
    /// without following (`Hyprland.dispatch`, a Toplevel's `activate`)
    Focus(u32, u32),
    Close(u32, u32),
    MoveTo(u32, u32, u32),
    /// A `ScreencopyView` wants its source's pixels (again)
    Capture(Oid),
}

impl State {
    pub fn place(&self, file: u32, loc: Loc) -> String {
        let f = self.files.get(file as usize).map(String::as_str).unwrap_or("?");
        format!("{f}:{}", loc.line)
    }
}

/// The table, shared with the functions JavaScript calls back into
#[derive(Clone)]
pub struct Core(pub Rc<RefCell<State>>);

/// What a name is on an object, as the JavaScript side asks (`js/`'s proxy)
pub mod kind {
    pub const NONE: u8 = 0;
    pub const PROP: u8 = 1;
    /// Prefix of a grouped name: `anchors`, `font`, `Layout`
    pub const GROUP: u8 = 2;
    pub const METHOD: u8 = 3;
    pub const SIGNAL: u8 = 4;
}

impl Core {
    pub fn new() -> Core {
        Core(Rc::new(RefCell::new(State::default())))
    }

    /// QML warning: to `dmesg` on MikuOS (`crate::say`), stderr on the host
    pub fn log(&self, msg: String) {
        crate::say(&msg);
        self.0.borrow_mut().log.push(msg);
    }

    /// Logged once, for per-frame warnings such as unsupported layouts
    pub fn log_once(&self, oid: Oid, msg: &str) {
        let line = {
            let s = self.0.borrow();
            match s.objs.get(oid) {
                Some(o) => format!("{}: {msg}", s.place(o.file, o.loc)),
                None => msg.to_string(),
            }
        };
        let first = {
            let docs = self.0.borrow().docs.clone();
            let first = docs.borrow_mut().first_said(&line);
            first
        };
        if first {
            self.log(line);
        }
    }

    /// Error that lost part of the tree: an unparsable file or unknown type. A
    /// first load continues without it; a reload with one keeps the old tree
    /// (`shell.rs`).
    ///
    /// Logged once, counted every time
    pub fn load_error(&self, msg: String) {
        let first = {
            let mut s = self.0.borrow_mut();
            s.load_errors += 1;
            let docs = s.docs.clone();
            let first = docs.borrow_mut().first_said(&msg);
            first
        };
        if first {
            self.log(msg);
        }
    }

    /// Property as the file sees it: its redeclaration if any (`add_prop`)
    pub fn prop_idx(&self, oid: Oid, name: &str) -> Option<u32> {
        let s = self.0.borrow();
        s.objs.get(oid)?.names().props.get(name).map(|e| e.1)
    }

    /// Property as aosh reads it (scene, services): the type's own, ignoring a
    /// redeclaration
    pub fn own_idx(&self, oid: Oid, name: &str) -> Option<u32> {
        let s = self.0.borrow();
        s.objs.get(oid)?.names().props.get(name).map(|e| e.0)
    }

    pub fn kind_of(&self, oid: Oid, name: &str) -> u8 {
        let s = self.0.borrow();
        let Some(o) = s.objs.get(oid) else { return kind::NONE };
        let n = o.names();
        if n.props.contains_key(name) {
            return kind::PROP;
        }
        if n.signals.contains(name) {
            return kind::SIGNAL;
        }
        if crate::types::chain(o.def).iter().any(|t| t.methods.contains(&name)) {
            return kind::METHOD;
        }
        if n.groups.contains(name) {
            return kind::GROUP;
        }
        kind::NONE
    }

    /// Adds a property: a `property` declaration or a row's role. Redeclaring a
    /// type's name shadows it for the file (`prop_idx`); the type keeps its own
    /// (`own_idx`)
    pub fn add_prop(&self, oid: Oid, name: &str, ty: PropType, readonly: bool) -> u32 {
        let mut s = self.0.borrow_mut();
        let o = &mut s.objs[oid];
        let own = crate::types::chain(o.def).iter().any(|t| t.props.iter().any(|(n, _, _)| *n == name));
        let matches = o.props.iter().filter(|p| &*p.name == name).count();
        if let Some(i) = o.props.iter().rposition(|p| &*p.name == name).filter(|_| !own || matches > 1) {
            o.props[i].ty = ty;
            o.props[i].value = ty.default_value();
            o.props[i].readonly = readonly;
            return i as u32;
        }
        o.props.push(Prop {
            name: Rc::from(name),
            ty,
            value: ty.default_value(),
            binding: None,
            observers: Vec::new(),
            alias: None,
            aliased_by: Vec::new(),
            readonly,
            behavior: None,
            explicit: false,
            required: false,
        });
        o.renamed();
        (o.props.len() - 1) as u32
    }

    /// Reads a property for a running binding, subscribing it
    pub fn read(&self, oid: Oid, idx: u32) -> Value {
        let mut s = self.0.borrow_mut();
        let (oid, mut idx) = resolve(&s, oid, idx);
        // An unset Rectangle corner (-1) reads as `radius`, as in
        // QQuickRectangle's getter
        if matches!(s.objs[oid].props[idx as usize].value, Value::Num(n) if n < 0.0) {
            if let Some(r) = corner_radius(&s, oid, idx) {
                idx = r;
            }
        }
        if let Some((_, deps)) = s.stack.last_mut() {
            if !deps.contains(&(oid, idx)) {
                deps.push((oid, idx));
            }
        }
        s.objs[oid].props[idx as usize].value.clone()
    }

    /// A property's value, subscribing nothing
    pub fn peek(&self, oid: Oid, idx: u32) -> Value {
        let s = self.0.borrow();
        let (oid, idx) = resolve(&s, oid, idx);
        s.objs[oid].props[idx as usize].value.clone()
    }

    pub fn peek_named(&self, oid: Oid, name: &str) -> Option<Value> {
        let idx = self.own_idx(oid, name)?;
        Some(self.peek(oid, idx))
    }

    pub fn prop_type(&self, oid: Oid, idx: u32) -> PropType {
        let s = self.0.borrow();
        let (oid, idx) = resolve(&s, oid, idx);
        s.objs[oid].props[idx as usize].ty
    }

    /// Assignment from JavaScript (`x = 5` in a handler). Replaces the binding,
    /// as in QML
    pub fn assign(&self, ctx: &Ctx<'_>, oid: Oid, idx: u32, v: Value) {
        {
            let mut s = self.0.borrow_mut();
            let (o, i) = resolve(&s, oid, idx);
            let p = &s.objs[o].props[i as usize];
            if p.readonly {
                let msg = format!("{} is read-only", p.name);
                drop(s);
                self.log(msg);
                return;
            }
            if let Some(b) = s.objs[o].props[i as usize].binding.take() {
                kill_binding(&mut s, b);
            }
            s.objs[o].props[i as usize].explicit = true;
        }
        self.store(ctx, oid, idx, v);
    }

    /// Value type parts, one level down: `sourceSize.width` and
    /// `sourceSize.height`, each with index and name
    pub fn group_parts(&self, oid: Oid, prefix: &str) -> Vec<(u32, String)> {
        let s = self.0.borrow();
        let Some(o) = s.objs.get(oid) else { return Vec::new() };
        let dotted = format!("{prefix}.");
        o.props.iter().enumerate()
            .filter_map(|(i, p)| p.name.strip_prefix(&dotted).filter(|r| !r.contains('.')).map(|r| (i as u32, r.to_string())))
            .collect()
    }

    /// Resolves a QML file reference: strips `file://`, resolves relative to
    /// `oid`'s file, applies `path_map`
    pub fn path(&self, oid: Oid, source: &str) -> std::path::PathBuf {
        let s = source.strip_prefix("file://").unwrap_or(source);
        let mut p = std::path::PathBuf::from(s);
        if !p.is_absolute() {
            let st = self.0.borrow();
            let file = st.objs.get(oid).and_then(|o| st.files.get(o.file as usize)).cloned().unwrap_or_default();
            if let Some(dir) = std::path::Path::new(&file).parent() {
                p = dir.join(p);
            }
        }
        self.mapped(p)
    }

    /// `p`, moved by `path_map` if it is under one of its directories
    pub fn mapped(&self, p: std::path::PathBuf) -> std::path::PathBuf {
        let st = self.0.borrow();
        for (from, to) in &st.path_map {
            if let Ok(rest) = p.strip_prefix(from) {
                return to.join(rest);
            }
        }
        p
    }

    /// `x = Qt.binding(f)`: replaces the property's binding and runs it. Errors
    /// are reported at the object's location
    pub fn bind_script<'js>(&self, ctx: &Ctx<'js>, oid: Oid, idx: u32, f: Function<'js>) {
        let (file, loc, readonly, name) = {
            let s = self.0.borrow();
            let (o, i) = resolve(&s, oid, idx);
            let p = &s.objs[o].props[i as usize];
            (s.objs[oid].file, s.objs[oid].loc, p.readonly, p.name.clone())
        };
        if readonly {
            self.log(format!("{name} is read-only"));
            return;
        }
        self.mark_explicit(oid, idx);
        let b = self.bind((oid, idx), Persistent::save(ctx, f), oid, file, loc);
        self.evaluate(ctx, b);
    }

    /// Value computed by layout or a service (implicit size, positioner `x`,
    /// anchors). Stored without marking the property explicit and past any
    /// Behavior, like Qt's C++ setters
    pub fn store_layout(&self, ctx: &Ctx<'_>, oid: Oid, name: &str, v: f64) {
        if let Some(i) = self.own_idx(oid, name) {
            self.store_raw(ctx, oid, i, Value::Num(v));
        }
    }

    pub fn is_explicit(&self, oid: Oid, name: &str) -> bool {
        let Some(i) = self.own_idx(oid, name) else { return false };
        let s = self.0.borrow();
        let (o, i) = resolve(&s, oid, i);
        s.objs[o].props[i as usize].explicit
    }

    /// Mark a property as given by the file, for the builder
    pub fn mark_explicit(&self, oid: Oid, idx: u32) {
        let mut s = self.0.borrow_mut();
        let (o, i) = resolve(&s, oid, idx);
        s.objs[o].props[i as usize].explicit = true;
    }

    /// A number property, `0` when not a number
    pub fn num(&self, oid: Oid, name: &str) -> f64 {
        let n = self.peek_named(oid, name).map(|v| v.as_num()).unwrap_or(0.0);
        if n.is_finite() { n } else { 0.0 }
    }

    pub fn flag(&self, oid: Oid, name: &str) -> bool {
        self.peek_named(oid, name).is_some_and(|v| v.truthy())
    }

    pub fn text(&self, oid: Oid, name: &str) -> String {
        self.peek_named(oid, name).map(|v| v.to_text()).unwrap_or_default()
    }

    /// Stores a value, keeping any binding, and notifies dependents if it
    /// changed. An enabled Behavior animates instead; a disabled one has its
    /// running animation stopped first
    pub fn store(&self, ctx: &Ctx<'_>, oid: Oid, idx: u32, v: Value) {
        let behavior = {
            let s = self.0.borrow();
            let (o, i) = resolve(&s, oid, idx);
            s.objs[o].props[i as usize].behavior.map(|b| (b, o, i, crate::anim::behavior_on(&s, b)))
        };
        match behavior {
            Some((b, o, i, true)) => {
                crate::anim::behave(ctx, self, b, o, i, v);
                return;
            }
            Some((b, _, _, false)) => crate::anim::behavior_off(self, b),
            None => {}
        }
        self.store_raw(ctx, oid, idx, v);
    }

    /// Marks a non-property change on `oid`: a child added or removed, a Canvas
    /// repaint, a capture, the caret. Counts as a store, so layout runs another
    /// round
    pub fn touch(&self, oid: Oid) {
        let mut s = self.0.borrow_mut();
        s.changes += 1;
        s.dirty = true;
        mark(&mut s, oid);
    }

    /// `store` past any Behavior, for animation steps
    pub fn store_raw(&self, ctx: &Ctx<'_>, oid: Oid, idx: u32, v: Value) {
        let (oid, idx, changed) = {
            let mut s = self.0.borrow_mut();
            let (oid, idx) = resolve(&s, oid, idx);
            let p = &mut s.objs[oid].props[idx as usize];
            let v = p.ty.coerce(v);
            let same = same_value(ctx, &p.value, &v);
            if !same {
                p.value = v;
                s.changes += 1;
                s.dirty = true;
                mark(&mut s, oid);
            }
            (oid, idx, !same)
        };
        if changed {
            self.notify(ctx, oid, idx);
        }
    }

    fn notify(&self, ctx: &Ctx<'_>, oid: Oid, idx: u32) {
        let (observers, aliases, name) = {
            let s = self.0.borrow();
            let p = &s.objs[oid].props[idx as usize];
            (p.observers.clone(), p.aliased_by.clone(), p.name.clone())
        };
        for b in observers {
            self.evaluate(ctx, b);
        }
        // Those bindings may have destroyed this object (a Loader's `active`
        // going false)
        if !self.0.borrow().objs.is_live(oid) {
            return;
        }
        for (ao, ai) in aliases {
            let alias_name = match self.0.borrow().objs.get(ao) {
                Some(o) => o.props[ai as usize].name.clone(),
                None => continue,
            };
            self.emit(ctx, ao, &format!("{alias_name}Changed"), Vec::new());
        }
        self.emit(ctx, oid, &format!("{name}Changed"), Vec::new());
        self.hook(ctx, oid, &name);
        let syncs = self.0.borrow().syncs.get(&(oid, idx)).cloned();
        for sync in syncs.into_iter().flatten() {
            sync::changed(ctx, self, sync, (oid, idx));
        }
    }

    /// Creates a binding for `target` from a compiled function, replacing the
    /// old one, without running it
    pub fn bind(&self, target: (Oid, u32), func: Persistent<Function<'static>>, this: Oid, file: u32, loc: Loc) -> Bid {
        let mut s = self.0.borrow_mut();
        let target = resolve(&s, target.0, target.1);
        if let Some(old) = s.objs[target.0].props[target.1 as usize].binding.take() {
            kill_binding(&mut s, old);
        }
        s.objs[target.0].props[target.1 as usize].explicit = true;
        let binding = Binding { target, func, this, file, loc, deps: Vec::new(), alive: true, held: false, pending: false };
        let reuse = if s.stack.is_empty() && s.storing.is_empty() { s.free_binds.pop() } else { None };
        let bid = match reuse {
            Some(b) => {
                s.binds[b as usize] = binding;
                b
            }
            None => {
                s.binds.push(binding);
                (s.binds.len() - 1) as Bid
            }
        };
        s.objs[target.0].props[target.1 as usize].binding = Some(bid);
        bid
    }

    /// Runs a pending binding of a newly created tree so readers see its value,
    /// like Qt's `QQmlData::flushPendingBinding`
    pub fn flush(&self, ctx: &Ctx<'_>, oid: Oid, idx: u32) {
        let pending = {
            let s = self.0.borrow();
            let (o, i) = resolve(&s, oid, idx);
            s.objs[o].props[i as usize].binding.filter(|&b| s.binds[b as usize].pending)
        };
        if let Some(b) = pending {
            self.evaluate(ctx, b);
        }
    }

    /// Run a binding and store what it comes to
    pub fn evaluate(&self, ctx: &Ctx<'_>, bid: Bid) {
        let (func, this, target, place) = {
            let mut s = self.0.borrow_mut();
            let b = &mut s.binds[bid as usize];
            b.pending = false;
            if !b.alive || b.held {
                return;
            }
            let b = &s.binds[bid as usize];
            let place = s.place(b.file, b.loc);
            if s.stack.iter().any(|(x, _)| *x == bid) || s.storing.contains(&bid) {
                let (o, i) = b.target;
                let name = s.objs[o].props[i as usize].name.clone();
                drop(s);
                self.log(format!("{place}: binding loop on '{name}' - it changes what it reads"));
                return;
            }
            let b = &s.binds[bid as usize];
            let r = (b.func.clone(), b.this, b.target, place);
            s.stack.push((bid, Vec::new()));
            r
        };
        let result = (|| -> rquickjs::Result<rquickjs::Value<'_>> {
            let f = func.restore(ctx)?;
            let this = crate::js::wrapper(ctx, self, this)?;
            f.call((This(this),))
        })();
        let deps = {
            let mut s = self.0.borrow_mut();
            let (_, deps) = s.stack.pop().unwrap_or((bid, Vec::new()));
            resubscribe(&mut s, bid, deps);
            s.binds[bid as usize].alive
        };
        if !deps {
            return;
        }
        match result {
            // A part missing from a value type set whole (`build.rs`)
            Ok(v) if v.as_object().is_some_and(|o| o.contains_key("__aosh_keep").unwrap_or(false)) => {}
            Ok(v) => {
                let ty = self.prop_type(target.0, target.1);
                let v = crate::js::from_js_as(ctx, v, ty);
                self.0.borrow_mut().storing.push(bid);
                self.store(ctx, target.0, target.1, v);
                let mut s = self.0.borrow_mut();
                if let Some(i) = s.storing.iter().rposition(|x| *x == bid) {
                    s.storing.remove(i);
                }
            }
            Err(e) => self.log(format!("{place}: {}", crate::js::describe(ctx, e))),
        }
    }

    /// Whether anything handles `signal` on `oid`
    pub fn has_handler(&self, oid: Oid, signal: &str) -> bool {
        self.0.borrow().objs.get(oid).is_some_and(|o| o.handlers.iter().any(|(n, _)| &**n == signal))
    }

    pub fn on(&self, oid: Oid, signal: &str, func: Persistent<Function<'static>>, this: Oid) {
        let mut s = self.0.borrow_mut();
        s.objs[oid].handlers.push((Rc::from(signal), Handler { func, this }));
    }

    /// Run every handler of a signal, with its arguments
    pub fn emit<'js>(&self, ctx: &Ctx<'js>, oid: Oid, signal: &str, args: Vec<rquickjs::Value<'js>>) {
        let handlers: Vec<(Persistent<Function<'static>>, Oid, String)> = {
            let s = self.0.borrow();
            let Some(o) = s.objs.get(oid) else { return };
            o.handlers.iter()
                .filter(|(n, _)| &**n == signal)
                .map(|(_, h)| (h.func.clone(), h.this, s.place(s.objs[h.this].file, s.objs[h.this].loc)))
                .collect()
        };
        for (func, this, place) in handlers {
            let r = (|| -> rquickjs::Result<()> {
                let f = func.restore(ctx)?;
                let this = crate::js::wrapper(ctx, self, this)?;
                f.call::<_, rquickjs::Value>((This(this), Rest(args.clone())))?;
                Ok(())
            })();
            if let Err(e) = r {
                self.log(format!("{place}: on{}: {}", capitalise(signal), crate::js::describe(ctx, e)));
            }
        }
    }

    /// A method a type defines natively (`Timer.start()`)
    pub fn call_method<'js>(&self, ctx: &Ctx<'js>, oid: Oid, name: &str, _args: Vec<rquickjs::Value<'js>>) -> Value {
        let Some(kind) = self.0.borrow().objs.get(oid).map(|o| o.def.kind) else { return Value::Undefined };
        if let Some(v) = crate::model::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::anim::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::io::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::input::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::services::apps::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::services::hyprland::call(ctx, self, oid, name, &_args) {
            return v;
        }
        if let Some(v) = crate::services::capture::call(self, oid, kind, name) {
            return v;
        }
        if let Some(v) = crate::scene::canvas::call(ctx, self, oid, kind, name, &_args) {
            return v;
        }
        if let Some(v) = crate::scene::control::call(ctx, self, oid, name) {
            return v;
        }
        if matches!(name, "mapToItem" | "mapFromItem" | "mapToGlobal" | "mapFromGlobal") {
            return self.map_point(ctx, oid, name, &_args);
        }
        match (kind, name) {
            (_, "destroy") => self.destroy(ctx, oid),
            (Kind::Timer, "start") => self.set_named(ctx, oid, "running", Value::Bool(true)),
            (Kind::Timer, "stop") => self.set_named(ctx, oid, "running", Value::Bool(false)),
            (Kind::Timer, "restart") => {
                self.set_named(ctx, oid, "running", Value::Bool(false));
                self.set_named(ctx, oid, "running", Value::Bool(true));
            }
            (Kind::Desktop, "switchTo") => {
                let n = _args.first().and_then(|v| v.as_number()).unwrap_or(0.0);
                if n >= 1.0 {
                    self.0.borrow_mut().actions.push(Action::Workspace(n as u32));
                }
            }
            (Kind::Desktop, "signal") => {
                let word = _args.first().and_then(|v| v.as_string()).and_then(|s| s.to_string().ok());
                if let Some(w) = word {
                    self.0.borrow_mut().actions.push(Action::Signal(w));
                }
            }
            // PositionSource has no backend and finds nothing
            // (`types/services.rs`)
            (_, "start" | "stop" | "update") if self.0.borrow().objs[oid].def.name == "PositionSource" => {}
            _ => self.log(format!("{name}() is not there yet on {}", self.0.borrow().objs[oid].ty)),
        }
        Value::Undefined
    }

    /// `mapToItem(other, x, y)` and friends: a point, or a rect with `w` and
    /// `h`, between item coordinates through both transforms (`scene/xf.rs`). A
    /// rect maps to the box around its corners, like QTransform::mapRect.
    /// `null` and "global" mean the window
    fn map_point<'js>(&self, ctx: &Ctx<'js>, oid: Oid, name: &str, args: &[rquickjs::Value<'js>]) -> Value {
        let to = name.starts_with("mapTo");
        let (other, rest) = if name.ends_with("Item") {
            (args.first().and_then(|v| crate::js::from_js_as(ctx, v.clone(), value::PropType::Obj).as_obj()), args.get(1..).unwrap_or(&[]))
        } else {
            (None, args)
        };
        let num = |v: &rquickjs::Value<'js>, k: &str| v.as_object().and_then(|o| o.get::<_, f64>(k).ok());
        // (x, y), (x, y, w, h), a point or a rect
        let (px, py, size) = match rest {
            [p] if p.is_object() => (num(p, "x").unwrap_or(0.0), num(p, "y").unwrap_or(0.0),
                                     num(p, "width").zip(num(p, "height"))),
            [x, y, w, h, ..] => (x.as_number().unwrap_or(0.0), y.as_number().unwrap_or(0.0),
                                 w.as_number().zip(h.as_number())),
            [x, y, ..] => (x.as_number().unwrap_or(0.0), y.as_number().unwrap_or(0.0), None),
            _ => (0.0, 0.0, None),
        };
        use crate::scene::xf::{to_window, Xf};
        let me = to_window(self, oid);
        let them = other.map(|o| to_window(self, o)).unwrap_or(Xf::IDENTITY);
        let (from, into) = if to { (me, them) } else { (them, me) };
        let m = into.inverse().unwrap_or(Xf::IDENTITY).times(&from);
        let (x, y, size) = match size {
            None => {
                let (x, y) = m.map(px, py);
                (x, y, None)
            }
            Some((w, h)) => {
                let corners = [m.map(px, py), m.map(px + w, py), m.map(px, py + h), m.map(px + w, py + h)];
                let (x0, y0) = corners.iter().fold((f64::MAX, f64::MAX), |a, c| (a.0.min(c.0), a.1.min(c.1)));
                let (x1, y1) = corners.iter().fold((f64::MIN, f64::MIN), |a, c| (a.0.max(c.0), a.1.max(c.1)));
                (x0, y0, Some((x1 - x0, y1 - y0)))
            }
        };
        let r = (|| -> rquickjs::Result<rquickjs::Value<'js>> {
            let o = Object::new(ctx.clone())?;
            o.set("x", x)?;
            o.set("y", y)?;
            if let Some((w, h)) = size {
                o.set("width", w)?;
                o.set("height", h)?;
            }
            Ok(o.into_value())
        })();
        r.map(|v| crate::js::from_js(ctx, v)).unwrap_or(Value::Undefined)
    }

    pub fn set_named(&self, ctx: &Ctx<'_>, oid: Oid, name: &str, v: Value) {
        if let Some(i) = self.own_idx(oid, name) {
            self.assign(ctx, oid, i, v);
        }
    }

    /// What a type does when one of its own properties changes
    fn hook(&self, ctx: &Ctx<'_>, oid: Oid, name: &str) {
        let Some(kind) = self.0.borrow().objs.get(oid).map(|o| o.def.kind) else { return };
        match (kind, name) {
            (Kind::Timer, "running") | (Kind::Timer, "interval") => {
                let running = self.peek_named(oid, "running").is_some_and(|v| v.truthy());
                let interval = self.peek_named(oid, "interval").map(|v| v.as_num()).unwrap_or(1000.0);
                let now = self.0.borrow().now_ms;
                let was = {
                    let mut s = self.0.borrow_mut();
                    let was = s.timers.iter().any(|(o, _)| *o == oid);
                    s.timers.retain(|(o, _)| *o != oid);
                    if running {
                        s.timers.push((oid, now + interval.max(0.0) as u64));
                    }
                    was
                };
                if running && !was && name == "running"
                    && self.peek_named(oid, "triggeredOnStart").is_some_and(|v| v.truthy())
                {
                    self.emit(ctx, oid, "triggered", Vec::new());
                }
            }
            (Kind::Connections, "target") => self.follow(oid),
            (_, "font.variableAxes") => self.tags_text(ctx, oid, "font.variableAxes", "font.__axes"),
            (_, "font.features") => self.tags_text(ctx, oid, "font.features", "font.__features"),
            (_, "path") if self.0.borrow().objs.get(oid).is_some_and(|o| o.def.name == "PathPolyline") => self.points_text(ctx, oid),
            (Kind::Binding, _) => hold::update(ctx, self, oid),
            _ => {
                crate::model::hook(ctx, self, oid, kind, name);
                crate::anim::hook(ctx, self, oid, kind, name);
                crate::io::hook(ctx, self, oid, kind, name);
                crate::input::hook(ctx, self, oid, kind, name);
                crate::services::capture::hook(ctx, self, oid, kind, name);
            }
        }
    }

    /// `font.variableAxes` or `font.features` as scene text,
    /// `wdth=100,wght=450`: four-letter tags with numbers, in tag order
    fn tags_text(&self, ctx: &Ctx<'_>, oid: Oid, from: &str, to: &str) {
        let v = self.peek_named(oid, from).unwrap_or(Value::Undefined);
        let mut axes: Vec<(String, f64)> = Vec::new();
        if let Value::Js(p) = &v {
            if let Ok(o) = p.as_ref().clone().restore(ctx).map(|v| v.into_object()) {
                if let Some(o) = o {
                    for (k, x) in o.props::<String, rquickjs::Value>().flatten() {
                        if let Some(n) = x.as_number().or_else(|| x.as_int().map(f64::from)) {
                            if k.len() == 4 {
                                axes.push((k, n));
                            }
                        }
                    }
                }
            }
        }
        axes.sort_by(|a, b| a.0.cmp(&b.0));
        let text = axes.iter().map(|(k, n)| format!("{k}={n}")).collect::<Vec<_>>().join(",");
        if let Some(i) = self.prop_idx(oid, to) {
            self.store_raw(ctx, oid, i, Value::str(&text));
        }
    }

    /// A PathPolyline's `path` as scene text, `x,y x,y`. Non-numeric points are
    /// skipped
    fn points_text(&self, ctx: &Ctx<'_>, oid: Oid) {
        let v = self.peek_named(oid, "path").unwrap_or(Value::Undefined);
        let mut out: Vec<String> = Vec::new();
        if let Value::Js(p) = &v {
            if let Ok(Some(list)) = p.as_ref().clone().restore(ctx).map(|v| v.into_array()) {
                for pt in list.iter::<rquickjs::Value>().flatten() {
                    let Some(o) = pt.as_object() else { continue };
                    let num = |k: &str| o.get::<_, rquickjs::Value>(k).ok().and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)));
                    if let (Some(x), Some(y)) = (num("x"), num("y")) {
                        out.push(format!("{x},{y}"));
                    }
                }
            }
        }
        if let Some(i) = self.prop_idx(oid, "__points") {
            self.store_raw(ctx, oid, i, Value::str(&out.join(" ")));
        }
    }

    /// Moves a `Connections` object's handlers from its old target to the new
    /// one
    pub fn follow(&self, oid: Oid) {
        let target = self.peek_named(oid, "target").and_then(|v| v.as_obj());
        let mut s = self.0.borrow_mut();
        if let Some(old) = s.objs[oid].listening_on.take() {
            if let Some(o) = s.objs.get_mut(old) {
                o.handlers.retain(|(_, h)| h.this != oid);
            }
        }
        let Some(t) = target.filter(|&t| s.objs.is_live(t)) else { return };
        let listens: Vec<_> = s.objs[oid].listens.iter().map(|(n, f)| (n.clone(), f.clone())).collect();
        for (name, func) in listens {
            s.objs[t].handlers.push((name, Handler { func, this: oid }));
        }
        s.objs[oid].listening_on = Some(t);
    }

    /// Sets the clock to `now` and fires due timers in order
    pub fn advance(&self, ctx: &Ctx<'_>, now: u64) {
        self.0.borrow_mut().now_ms = now;
        crate::input::keys::tick(self, now);
        loop {
            let due = {
                let s = self.0.borrow();
                s.timers.iter().filter(|(_, at)| *at <= now).min_by_key(|(_, at)| *at).copied()
            };
            let Some((oid, at)) = due else { break };
            let repeat = self.peek_named(oid, "repeat").is_some_and(|v| v.truthy());
            let interval = self.peek_named(oid, "interval").map(|v| v.as_num()).unwrap_or(1000.0).max(1.0) as u64;
            if repeat {
                let mut s = self.0.borrow_mut();
                if let Some(t) = s.timers.iter_mut().find(|(o, _)| *o == oid) {
                    // Next due time, skipping missed intervals: a repeating
                    // timer fires once after a stall
                    let next = at + interval;
                    t.1 = if next <= now { now + interval } else { next };
                }
            } else {
                self.0.borrow_mut().timers.retain(|(o, _)| *o != oid);
                if let Some(i) = self.prop_idx(oid, "running") {
                    self.store(ctx, oid, i, Value::Bool(false));
                }
            }
            self.emit(ctx, oid, "triggered", Vec::new());
        }
    }

    /// When the next timer is due, for the event loop's poll
    pub fn next_due(&self) -> Option<u64> {
        let timers = self.0.borrow().timers.iter().map(|(_, at)| *at).min();
        match (timers, crate::input::keys::next_due(self)) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Releases all JavaScript values; the runtime will not close with any
    /// alive
    pub fn clear(&self) {
        let mut s = self.0.borrow_mut();
        s.objs.clear();
        s.binds.clear();
        s.free_binds.clear();
        s.comps.clear();
        s.views.clear();
        s.lists.clear();
        s.anims = Default::default();
        s.free_cxs.clear();
        s.dying.clear();
        s.syncs.clear();
        for (_, p) in s.procs.drain() {
            p.kill();
        }
        s.io = Default::default();
        s.keys = Default::default();
        s.ipc = Default::default();
        s.cxs.clear();
        s.factories.clear();
        s.stack.clear();
        s.storing.clear();
        s.singletons.clear();
        s.libs.clear();
        s.file_qt.clear();
        s.held.clear();
    }
}

/// Target storage of an alias.
/// `radius`, when `idx` is one of the four corners of an object that has it
fn corner_radius(s: &State, oid: Oid, idx: u32) -> Option<u32> {
    let o = &s.objs[oid];
    let name = &*o.props[idx as usize].name;
    if !matches!(name, "topLeftRadius" | "topRightRadius" | "bottomLeftRadius" | "bottomRightRadius") {
        return None;
    }
    o.props.iter().position(|p| &*p.name == "radius").map(|i| i as u32)
}

pub(crate) fn resolve(s: &State, mut oid: Oid, mut idx: u32) -> (Oid, u32) {
    for _ in 0..16 {
        match s.objs[oid].props[idx as usize].alias {
            Some((o, i)) => {
                oid = o;
                idx = i;
            }
            None => break,
        }
    }
    (oid, idx)
}

pub(crate) fn kill_binding(s: &mut State, b: Bid) {
    if !s.binds[b as usize].alive {
        return;
    }
    let deps = std::mem::take(&mut s.binds[b as usize].deps);
    s.binds[b as usize].alive = false;
    for (o, i) in deps {
        if let Some(obj) = s.objs.get_mut(o) {
            obj.props[i as usize].observers.retain(|x| *x != b);
        }
    }
    s.free_binds.push(b);
}

/// Subscribes a binding to exactly what it just read
fn resubscribe(s: &mut State, b: Bid, deps: Vec<(Oid, u32)>) {
    if !s.binds[b as usize].alive {
        return;
    }
    let old = std::mem::take(&mut s.binds[b as usize].deps);
    for d in &old {
        if !deps.contains(d) {
            if let Some(o) = s.objs.get_mut(d.0) {
                o.props[d.1 as usize].observers.retain(|x| *x != b);
            }
        }
    }
    // Skips destroyed objects
    let deps: Vec<(Oid, u32)> = deps.into_iter().filter(|d| s.objs.is_live(d.0)).collect();
    for d in &deps {
        let obs = &mut s.objs[d.0].props[d.1 as usize].observers;
        if !obs.contains(&b) {
            obs.push(b);
        }
    }
    s.binds[b as usize].deps = deps;
}

/// Whether storing `new` over `old` changes anything. JavaScript values compare
/// as `===`, except NaN equals NaN so a NaN binding settles.
/// `oid` changed: marks it and every ancestor up to the root, following the
/// owner where there is no parent (a layer's effect). Each window has its own
/// counter, so the walk cannot stop early
fn mark(s: &mut State, oid: Oid) {
    let now = s.changes;
    let mut at = Some(oid);
    let mut first = true;
    while let Some(a) = at {
        let Some(o) = s.objs.get_mut(a) else { break };
        if first {
            o.changed = now;
            first = false;
        }
        if o.below == now {
            break;
        }
        o.below = now;
        at = o.parent.or(o.owner);
    }
}

fn same_value(ctx: &Ctx<'_>, old: &Value, new: &Value) -> bool {
    match (old, new) {
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        (Value::Bool(a), Value::Bool(b)) => a == b,
        (Value::Num(a), Value::Num(b)) => a == b || (a.is_nan() && b.is_nan()),
        (Value::Str(a), Value::Str(b)) => a == b,
        (Value::Obj(a), Value::Obj(b)) => a == b,
        (Value::List(a), Value::List(b)) => {
            a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| matches!((x, y), (Value::Obj(p), Value::Obj(q)) if p == q))
        }
        (Value::Js(a), Value::Js(b)) => {
            Rc::ptr_eq(a, b)
                || match (a.as_ref().clone().restore(ctx), b.as_ref().clone().restore(ctx)) {
                    (Ok(x), Ok(y)) => x == y,
                    _ => false,
                }
        }
        _ => false,
    }
}

pub fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}
