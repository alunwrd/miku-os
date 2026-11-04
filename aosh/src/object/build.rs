// Builds objects from a QML tree
//
// Order, as in QML:
// 1) Create all objects, since bindings may name ids declared later
// 2) Resolve aliases
// 3) Compile functions
// 4) Set literal values
// 5) Evaluate bindings in declaration order
// 6) Attach handlers, so initial values are not reported as changes
// 7) Resolve `Connections` targets
// 8) Run `Component.onCompleted`
//
// A file type (`Bar` for `Bar.qml`) creates the file's root in its own id
// scope, then applies the outer declaration's members over it.
//
// Errors are logged with file and line and only skip that location.
//
// `instantiate` creates a `Component` instance at runtime in an id scope
// chained to the component's.
//
// Function scope, innermost first: own id scope, the object (`this`), the file
// root, then enclosing scopes. Delegates get `index`, `modelData`, and `model`
// in their own scope (`factory_source`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use rquickjs::{Ctx, Function, Object, Persistent};

use super::life::ComponentDef;
use super::value::{Oid, PropType, Value};
use super::{capitalise, Context, Core, Obj, Prop, Signal};
use crate::qml::ast::*;
use crate::qml::import::Imports;
use crate::types;

/// Parsed files for later instances of a component, and load errors already
/// reported
#[derive(Default)]
pub struct Files {
    docs: HashMap<PathBuf, Rc<Document>>,
    said: std::collections::HashSet<String>,
    /// Per-file data shared by its objects: the number keying compiled
    /// functions (`js::compile`) and imports. Keyed by the parsed file and
    /// name, since tests reuse names and reloads reparse. The entry holds both
    /// `Rc`s, so addresses are not reused
    sources: HashMap<(usize, usize, String, PathBuf), (Source, Option<Rc<crate::qml::import::Modules>>)>,
}

impl Files {
    /// Forgets parsed files; a reload reads them again and reports errors again
    pub fn clear(&mut self) {
        self.docs.clear();
        self.said.clear();
        self.sources.clear();
    }

    /// True the first time `msg` is seen since files were last read
    pub fn first_said(&mut self, msg: &str) -> bool {
        self.said.insert(msg.to_string())
    }

    fn get(&mut self, path: &Path) -> Result<Rc<Document>, String> {
        if let Some(d) = self.docs.get(path) {
            return Ok(d.clone());
        }
        let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let doc = crate::qml::parse(&src).map_err(|e| format!("{}:{e}", path.display()))?;
        let doc = Rc::new(doc);
        self.docs.insert(path.to_path_buf(), doc.clone());
        Ok(doc)
    }
}

/// Where the file a definition came from is, and what it can see
#[derive(Clone)]
pub struct Source {
    pub file: u32,
    /// Directories searched for a type name: the file's own, then unaliased
    /// imports (`qml/import.rs`)
    pub dirs: Rc<Vec<PathBuf>>,
    /// Its scripts and aliased modules, which its id scope gets
    pub imports: Rc<Imports>,
    /// The file itself, for the inline components it defines
    pub doc: Option<Rc<Document>>,
}

impl Source {
    /// Definition with no file: a service or a row
    fn bare(file: u32) -> Source {
        Source { file, dirs: Rc::new(Vec::new()), imports: Rc::new(Imports::default()), doc: None }
    }
}

/// What a type name turned out to be
enum Found {
    /// A file: `Bar` for `Bar.qml`
    File(PathBuf),
    /// Inline component `component Name: Type { }` and its file, whose imports
    /// its names use
    Inline(ObjDef, Source),
}

/// Delegate scope contents beyond ids: the shown row
#[derive(Default)]
pub struct Scope {
    /// `model`, whose `index` and `modelData` are `index` and `modelData`
    pub row: Option<Oid>,
    /// A ListModel's roles, by name
    pub roles: Vec<String>,
    /// Enclosing ListView, for the attached `ListView.view` and
    /// `ListView.isCurrentItem`
    pub view: Option<Oid>,
}

/// Initial properties of a type's objects, base first, and its signals
struct Template {
    props: Vec<(Rc<str>, PropType, Value)>,
    signals: Vec<Signal>,
}

/// A type's `Template`, built when the first object of it is created
fn template(t: &'static types::TypeDef) -> Rc<Template> {
    thread_local! {
        static MADE: std::cell::RefCell<HashMap<usize, Rc<Template>>> = std::cell::RefCell::new(HashMap::new());
    }
    let key = t as *const types::TypeDef as usize;
    if let Some(m) = MADE.with(|m| m.borrow().get(&key).cloned()) {
        return m;
    }
    let mut props: Vec<(Rc<str>, PropType, Value)> = Vec::new();
    let mut signals = Vec::new();
    for ty in types::chain(t).iter().rev() {
        for (name, pty, default) in ty.props {
            // A type may redeclare a base property, such as `anchors.left` on
            // `PanelWindow`; only the redeclared one is kept
            if let Some(p) = props.iter_mut().find(|p| &*p.0 == *name) {
                p.1 = *pty;
                p.2 = default.value(*pty);
                continue;
            }
            props.push((Rc::from(*name), *pty, default.value(*pty)));
        }
        for (name, params) in ty.signals {
            signals.push(Signal { name: Rc::from(*name), params: params.iter().map(|p| p.to_string()).collect() });
        }
    }
    let m = Rc::new(Template { props, signals });
    MADE.with(|c| c.borrow_mut().insert(key, m.clone()));
    m
}

/// Properties holding a component: an object written here
/// (`delegate: Text { }`) is the definition and is not instantiated
const COMPONENT_PROPS: &[&str] = &["delegate", "sourceComponent", "component", "highlight", "header", "footer"];

enum Work {
    Alias { oid: Oid, idx: u32, target: String, src: Source, cx: u32, loc: Loc },
    Function { oid: Oid, name: String, params: Vec<String>, types: Vec<String>, ret: String, body: String, src: Source, cx: u32, loc: Loc },
    Literal { oid: Oid, idx: u32, value: Value },
    /// `key` names the compiled function when the location does not: a
    /// builder-generated binding
    Bind { oid: Oid, idx: u32, js: String, src: Source, cx: u32, loc: Loc, key: Option<String> },
    Handler { oid: Oid, signal: String, js: String, params: Option<Vec<String>>, src: Source, cx: u32, loc: Loc },
    Completed { oid: Oid, js: String, src: Source, cx: u32, loc: Loc },
    Destruction { oid: Oid, js: String, src: Source, cx: u32, loc: Loc },
    /// `Behavior on prop { }`, `NumberAnimation on prop { }`
    On { oid: Oid, target: Oid, prop: String },
}

pub struct Builder<'a, 'js> {
    core: &'a Core,
    ctx: &'a Ctx<'js>,
    files: Rc<std::cell::RefCell<Files>>,
    work: Vec<Work>,
    made: Vec<Oid>,
}

impl<'a, 'js> Builder<'a, 'js> {
    pub fn new(core: &'a Core, ctx: &'a Ctx<'js>) -> Self {
        let files = core.0.borrow().docs.clone();
        Builder { core, ctx, files, work: Vec::new(), made: Vec::new() }
    }

    fn doc(&self, path: &Path) -> Result<Rc<Document>, String> {
        self.files.borrow_mut().get(path)
    }

    /// A file's root object, made and finished
    pub fn load_file(mut self, path: &Path) -> Result<Oid, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let doc = self.doc(&path)?;
        let name = path.display().to_string();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        self.load_doc(&doc, &name, &dir)
    }

    /// Non-file source from a test, with `dir` as the component search path
    #[cfg(test)]
    pub fn load_source(mut self, name: &str, src: &str, dir: &Path) -> Result<Oid, String> {
        let doc = Rc::new(crate::qml::parse(src).map_err(|e| format!("{name}:{e}"))?);
        self.load_doc(&doc, name, dir)
    }

    /// The singleton `name`, created on first use by a script
    /// (`__aosh_singletons`)
    pub fn singleton(mut self, name: &str) -> Result<Oid, String> {
        let made = self.core.0.borrow().singletons.get(name).copied();
        if let Some(o) = made.filter(|&o| self.core.0.borrow().objs.is_live(o)) {
            return Ok(o);
        }
        let path = self.core.0.borrow().modules.as_ref().and_then(|m| m.singletons.get(name).cloned())
            .ok_or_else(|| format!("there is no singleton '{name}'"))?;
        let doc = self.doc(&path)?;
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let src = self.source(&path.display().to_string(), &dir, &doc);
        let cx = self.new_context(None, None).map_err(|e| self.js_error(e))?;
        self.file_scope(cx, &src).map_err(|e| self.js_error(e))?;
        let oid = self.make(&doc.root, &src, cx, None)?;
        // Registered before its bindings run, so references to it find this one
        self.core.0.borrow_mut().singletons.insert(name.to_string(), oid);
        self.finish();
        Ok(oid)
    }

    /// Object of a built-in type, exposed to every file as the global `global`:
    /// the services
    pub fn make_global(mut self, global: &str, ty: &str) -> Result<Oid, String> {
        let t = types::find(ty).ok_or_else(|| format!("there is no type '{ty}'"))?;
        let src = Source::bare(0);
        let cx = self.new_context(None, None).map_err(|e| self.js_error(e))?;
        let def = ObjDef { ty: ty.to_string(), ..ObjDef::default() };
        let oid = self.new_object(&def, t, &src, cx, None);
        self.core.0.borrow_mut().cxs[cx as usize].root = Some(oid);
        let w = crate::js::wrapper(self.ctx, self.core, oid).map_err(|e| self.js_error(e))?;
        self.ctx.globals().set(global, w).map_err(|e| self.js_error(e))?;
        Ok(oid)
    }

    /// Object of a built-in type with no file and no global: a `ShellScreen`
    pub fn make_plain(mut self, ty: &str, owner: Option<Oid>) -> Result<Oid, String> {
        let t = types::find(ty).ok_or_else(|| format!("there is no type '{ty}'"))?;
        let src = Source::bare(0);
        let cx = match owner {
            Some(o) => self.core.0.borrow().objs[o].cx,
            None => self.new_context(None, None).map_err(|e| self.js_error(e))?,
        };
        let def = ObjDef { ty: ty.to_string(), ..ObjDef::default() };
        let oid = self.new_object(&def, t, &src, cx, owner);
        self.core.0.borrow_mut().objs[oid].complete = true;
        Ok(oid)
    }

    fn load_doc(&mut self, doc: &Rc<Document>, name: &str, dir: &Path) -> Result<Oid, String> {
        let src = self.source(name, dir, doc);
        let cx = self.new_context(None, None).map_err(|e| self.js_error(e))?;
        self.file_scope(cx, &src).map_err(|e| self.js_error(e))?;
        let root = self.make(&doc.root, &src, cx, None)?;
        self.finish();
        Ok(root)
    }

    /// Creates one instance of the `Component` `comp`, shown in `parent`,
    /// destroyed by `owner`
    pub fn instantiate(mut self, comp: Oid, parent: Option<Oid>, owner: Oid, scope: Scope) -> Result<Oid, String> {
        let cd: ComponentDef = self.core.0.borrow().comps.get(&comp).cloned()
            .ok_or_else(|| "that is not a component".to_string())?;
        // The instance's scope, with the instance as root (QML's context
        // object), so a delegate's properties are bare names inside it. The
        // file root and ids follow in the `new_context` chain
        let cx = self.new_context(Some(cd.cx), Some(&scope)).map_err(|e| self.js_error(e))?;
        let oid = self.make(&cd.def, &cd.src, cx, parent)?;
        self.required(oid, &scope, &cd.src, cx);
        self.belongs(oid, parent, owner);
        self.finish();
        Ok(oid)
    }

    /// A delegate's `required` properties are bound by name to the row
    /// (`index`, `modelData`, `model`, roles) in the instance's scope, unless
    /// the instance sets them. Done here since a file delegate's own scope has
    /// no row
    fn required(&mut self, oid: Oid, scope: &Scope, src: &Source, cx: u32) {
        let given: Vec<u32> = self.work.iter().filter_map(|w| match w {
            Work::Bind { oid: o, idx, .. } | Work::Literal { oid: o, idx, .. } if *o == oid => Some(*idx),
            _ => None,
        }).collect();
        let wanted: Vec<(u32, String)> = {
            let s = self.core.0.borrow();
            s.objs[oid].props.iter().enumerate()
                .filter(|(i, p)| p.required && !given.contains(&(*i as u32)))
                .filter(|(_, p)| matches!(&*p.name, "index" | "modelData" | "model") || scope.roles.iter().any(|r| **r == *p.name))
                .map(|(i, p)| (i as u32, p.name.to_string()))
                .collect()
        };
        // Added first: the rest read them, as Qt sets required properties first
        for (k, (idx, name)) in wanted.into_iter().enumerate() {
            let key = Some(format!("required:{name}"));
            self.work.insert(k, Work::Bind { oid, idx, js: name, src: src.clone(), cx, loc: Loc::default(), key });
        }
    }

    /// A file's root as an instance: a Loader's `source`
    pub fn instantiate_file(mut self, path: &Path, parent: Option<Oid>, owner: Oid, outer: Option<u32>) -> Result<Oid, String> {
        let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let doc = self.doc(&path)?;
        let name = path.display().to_string();
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let src = self.source(&name, &dir, &doc);
        let cx = self.new_context(outer, None).map_err(|e| self.js_error(e))?;
        self.file_scope(cx, &src).map_err(|e| self.js_error(e))?;
        let oid = self.make(&doc.root, &src, cx, parent)?;
        self.belongs(oid, parent, owner);
        self.finish();
        Ok(oid)
    }

    /// An instance is its owner's to destroy, whoever shows it
    fn belongs(&mut self, oid: Oid, parent: Option<Oid>, owner: Oid) {
        {
            let mut s = self.core.0.borrow_mut();
            if let Some(p) = parent {
                s.objs[p].owned.retain(|&c| c != oid);
            }
            s.objs[oid].owner = Some(owner);
            if !s.objs[owner].owned.contains(&oid) {
                s.objs[owner].owned.push(oid);
            }
        }
        if let (Some(p), Some(i)) = (parent, self.core.prop_idx(oid, "parent")) {
            self.work.push(Work::Literal { oid, idx: i, value: Value::Obj(p) });
        }
    }

    fn source(&mut self, name: &str, dir: &Path, doc: &Rc<Document>) -> Source {
        let modules = self.core.0.borrow().modules.clone();
        let key = (Rc::as_ptr(doc) as usize, modules.as_ref().map_or(0, |m| Rc::as_ptr(m) as usize),
                   name.to_string(), dir.to_path_buf());
        if let Some((src, _)) = self.files.borrow().sources.get(&key) {
            return src.clone();
        }
        let file = {
            let mut s = self.core.0.borrow_mut();
            s.files.push(name.to_string());
            (s.files.len() - 1) as u32
        };
        let mut imports = crate::qml::import::imports(doc, dir, modules.as_deref());
        let dirs = std::mem::take(&mut imports.dirs);
        let src = Source { file, dirs: Rc::new(dirs), imports: Rc::new(imports), doc: Some(doc.clone()) };
        self.files.borrow_mut().sources.insert(key, (src.clone(), modules));
        src
    }

    /// Adds each script namespace and aliased module to the file's id scope
    /// under its alias
    fn file_scope(&mut self, cx: u32, src: &Source) -> rquickjs::Result<()> {
        let ids = self.core.0.borrow().cxs[cx as usize].ids_js.clone().restore(self.ctx)?;
        // The file's own `Qt`, whose `resolvedUrl` is relative to its
        // directory; shared per directory
        if let Some(dir) = src.dirs.first() {
            let cached = self.core.0.borrow().file_qt.get(dir).cloned();
            let qt = match cached {
                Some(q) => q.restore(self.ctx)?,
                None => {
                    let make: Function = self.ctx.globals().get("__aosh_file_qt")?;
                    let q: Object = make.call((dir.display().to_string(),))?;
                    self.core.0.borrow_mut().file_qt.insert(dir.clone(), Persistent::save(self.ctx, q.clone()));
                    q
                }
            };
            ids.set("Qt", qt)?;
        }
        for (alias, path) in &src.imports.js {
            let ns = crate::js::library::namespace(self.ctx, self.core, path)?;
            ids.set(alias.as_str(), ns)?;
        }
        let modules = self.core.0.borrow().modules.clone();
        for (alias, dir) in &src.imports.spaces {
            let names = modules.as_ref().map(|m| m.singletons_in(dir)).unwrap_or_default();
            let space: Function = self.ctx.globals().get("__aosh_space")?;
            let o: rquickjs::Value = space.call((names,))?;
            ids.set(alias.as_str(), o)?;
        }
        Ok(())
    }

    fn js_error(&self, e: rquickjs::Error) -> String {
        crate::js::describe(self.ctx, e)
    }

    fn place(&self, src: &Source, loc: Loc) -> String {
        self.core.0.borrow().place(src.file, loc)
    }

    /// New id scope, chained to `outer`: the file using this one as a type, or
    /// the file a delegate was written in
    fn new_context(&mut self, outer: Option<u32>, scope: Option<&Scope>) -> rquickjs::Result<u32> {
        let null = || rquickjs::Value::new_null(self.ctx.clone());
        let (row, view, roles) = match scope {
            Some(sc) => (
                match sc.row { Some(r) => crate::js::wrapper(self.ctx, self.core, r)?, None => null() },
                match sc.view { Some(v) => crate::js::wrapper(self.ctx, self.core, v)?, None => null() },
                sc.roles.clone(),
            ),
            None => (null(), null(), Vec::new()),
        };
        let make: Function = self.ctx.globals().get("__aosh_scope")?;
        let ids: Object = make.call((row, roles, view))?;
        let outer_js = match outer {
            Some(c) => {
                let (ids, root, next) = {
                    let s = self.core.0.borrow();
                    let c = &s.cxs[c as usize];
                    (c.ids_js.clone(), c.root, c.outer_js.clone())
                };
                let ids = ids.restore(self.ctx)?;
                let root = match root { Some(r) => crate::js::wrapper(self.ctx, self.core, r)?, None => null() };
                let next = next.restore(self.ctx)?;
                let chain: Function = self.ctx.globals().get("__aosh_chain")?;
                chain.call::<_, rquickjs::Value>((ids, root, next))?
            }
            None => null(),
        };
        let cx = Context {
            ids: HashMap::new(),
            ids_js: Persistent::save(self.ctx, ids),
            outer_js: Persistent::save(self.ctx, outer_js),
            root: None,
            objects: 0,
        };
        let mut s = self.core.0.borrow_mut();
        Ok(match s.free_cxs.pop() {
            Some(i) => {
                s.cxs[i as usize] = cx;
                i
            }
            None => {
                s.cxs.push(cx);
                (s.cxs.len() - 1) as u32
            }
        })
    }

    /// Resolves a type name: inline component of this file, file in its
    /// directories, file in an aliased module (`Bar.Clock`), or inline
    /// component of another file
    fn find_component(&mut self, ty: &str, src: &Source) -> Option<Found> {
        let Some((head, tail)) = ty.split_once('.') else {
            if let Some(doc) = &src.doc {
                if let Some((_, def)) = doc.inline.iter().find(|(n, _)| n == ty) {
                    return Some(Found::Inline(def.clone(), src.clone()));
                }
            }
            return src.dirs.iter().map(|d| d.join(format!("{ty}.qml"))).find(|p| p.is_file()).map(Found::File);
        };
        if tail.contains('.') {
            return None;
        }
        if let Some((_, dir)) = src.imports.spaces.iter().find(|(a, _)| a == head) {
            let p = dir.join(format!("{tail}.qml"));
            return p.is_file().then_some(Found::File(p));
        }
        let file = src.dirs.iter().map(|d| d.join(format!("{head}.qml"))).find(|p| p.is_file())?;
        let doc = self.doc(&file).ok()?;
        let (_, def) = doc.inline.iter().find(|(n, _)| n == tail)?;
        let def = def.clone();
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let fsrc = self.source(&file.display().to_string(), &dir, &doc);
        Some(Found::Inline(def, fsrc))
    }

    /// Creates one object and its subtree, queuing its members
    fn make(&mut self, def: &ObjDef, src: &Source, cx: u32, parent: Option<Oid>) -> Result<Oid, String> {
        let found = self.find_component(&def.ty, src);
        if found.is_none() && types::find(&def.ty).is_some_and(|t| t.kind == types::Kind::Component) {
            return self.make_component(def, src, cx, parent);
        }
        let oid = match found {
            Some(Found::File(path)) => {
                let doc = self.doc(&path)?;
                let name = path.display().to_string();
                let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                let inner_src = self.source(&name, &dir, &doc);
                let inner_cx = self.new_context(Some(cx), None).map_err(|e| self.js_error(e))?;
                self.file_scope(inner_cx, &inner_src).map_err(|e| self.js_error(e))?;
                let oid = self.make(&doc.root, &inner_src, inner_cx, parent)?;
                self.into_default(oid, &doc.root);
                self.core.0.borrow_mut().objs[oid].ty = def.ty.clone();
                oid
            }
            // An inline component is a type with its own id scope, using its
            // file's imports
            Some(Found::Inline(idef, isrc)) => {
                let inner_cx = self.new_context(Some(cx), None).map_err(|e| self.js_error(e))?;
                self.file_scope(inner_cx, &isrc).map_err(|e| self.js_error(e))?;
                let oid = self.make(&idef, &isrc, inner_cx, parent)?;
                self.into_default(oid, &idef);
                self.core.0.borrow_mut().objs[oid].ty = def.ty.clone();
                oid
            }
            None => match types::find(&def.ty) {
                Some(t) => self.new_object(def, t, src, cx, parent),
                // Unknown type: an inert Item stands in, so the file still
                // loads. Logged once per type and file
                None => {
                    let msg = format!("{}: there is no type '{}' yet - an inert item stands in for it",
                                      self.core.0.borrow().files.get(src.file as usize).cloned().unwrap_or_default(), def.ty);
                    if self.files.borrow_mut().first_said(&msg) {
                        self.core.log(msg);
                    }
                    let t = types::find(types::INERT).expect("the inert type is a type");
                    self.new_object(def, t, src, cx, parent)
                }
            },
        };

        {
            let mut s = self.core.0.borrow_mut();
            if s.cxs[cx as usize].root.is_none() {
                s.cxs[cx as usize].root = Some(oid);
            }
        }
        if let Some(id) = &def.id {
            self.register_id(cx, id, oid).map_err(|e| self.js_error(e))?;
        }
        self.members(oid, def, src, cx);
        Ok(oid)
    }

    /// `Component { Item { } }`: keeps the definition without instantiating
    fn make_component(&mut self, def: &ObjDef, src: &Source, cx: u32, parent: Option<Oid>) -> Result<Oid, String> {
        let inner = def.members.iter().find_map(|m| match m {
            Member::Child(c) => Some(c.clone()),
            _ => None,
        });
        let oid = match inner {
            Some(d) => self.component_of(&d, src, cx, parent),
            None => {
                let t = types::find("Component").expect("Component is a type");
                self.new_object(def, t, src, cx, parent)
            }
        };
        if let Some(id) = &def.id {
            self.register_id(cx, id, oid).map_err(|e| self.js_error(e))?;
        }
        Ok(oid)
    }

    /// A Component object holding `def`, written in scope `cx`
    fn component_of(&mut self, def: &ObjDef, src: &Source, cx: u32, owner: Option<Oid>) -> Oid {
        let t = types::find("Component").expect("Component is a type");
        let cdef = ObjDef { ty: "Component".into(), loc: def.loc, ..ObjDef::default() };
        let c = self.new_object(&cdef, t, src, cx, owner);
        self.core.0.borrow_mut().comps.insert(c, ComponentDef { def: Rc::new(def.clone()), src: src.clone(), cx });
        c
    }

    fn new_object(&mut self, def: &ObjDef, t: &'static types::TypeDef, src: &Source, cx: u32, parent: Option<Oid>) -> Oid {
        let start = template(t);
        let mut props: Vec<Prop> = start.props.iter().map(|(name, ty, value)| Prop {
            name: name.clone(),
            ty: *ty,
            value: value.clone(),
            binding: None,
            observers: Vec::new(),
            alias: None,
            aliased_by: Vec::new(),
            readonly: false,
            behavior: None,
            explicit: false,
            required: false,
        }).collect();
        let signals = start.signals.clone();
        let mut s = self.core.0.borrow_mut();
        let oid = s.objs.reserve();
        // The anchor lines another item's anchors may name
        for line in ANCHOR_LINES {
            if let Some(p) = props.iter_mut().find(|p| &*p.name == line) {
                if p.ty == PropType::Str {
                    p.value = Value::str(&format!("anchor:{oid}:{line}"));
                    p.readonly = true;
                }
            }
        }
        s.objs.put(oid, Obj {
            ty: def.ty.clone(),
            def: t,
            props,
            signals,
            handlers: Vec::new(),
            parent,
            children: Vec::new(),
            owner: parent,
            owned: Vec::new(),
            complete: false,
            wrapper: None,
            cx,
            file: src.file,
            loc: def.loc,
            listens: Vec::new(),
            listening_on: None,
            default_into: None,
            changed: 0,
            below: 0,
            names: Default::default(),
        });
        // New object, so it counts as changed in every window it is in
        s.changes += 1;
        super::mark(&mut s, oid);
        s.cxs[cx as usize].objects += 1;
        if let Some(p) = parent {
            s.objs[p].owned.push(oid);
        }
        drop(s);
        self.made.push(oid);
        oid
    }

    fn register_id(&mut self, cx: u32, id: &str, oid: Oid) -> rquickjs::Result<()> {
        let w = crate::js::wrapper(self.ctx, self.core, oid)?;
        let ids = self.core.0.borrow().cxs[cx as usize].ids_js.clone().restore(self.ctx)?;
        ids.set(id, w)?;
        self.core.0.borrow_mut().cxs[cx as usize].ids.insert(id.to_string(), oid);
        Ok(())
    }

    fn add_prop(&mut self, oid: Oid, name: &str, ty: PropType, readonly: bool) -> u32 {
        self.core.add_prop(oid, name, ty, readonly)
    }

    /// Row object (`ListElement`) with the given roles, owned by `owner`. Used
    /// by `ListModel.append` and Repeater array elements
    pub fn make_row(mut self, owner: Oid, roles: Vec<(String, Value)>) -> Oid {
        let t = types::find("ListElement").expect("ListElement is a type");
        let (file, cx) = {
            let s = self.core.0.borrow();
            (s.objs[owner].file, s.objs[owner].cx)
        };
        let src = Source::bare(file);
        let def = ObjDef { ty: "ListElement".into(), ..ObjDef::default() };
        let oid = self.new_object(&def, t, &src, cx, Some(owner));
        self.core.0.borrow_mut().objs[oid].complete = true;
        for (k, v) in roles {
            let i = self.core.add_prop(oid, &k, PropType::Var, false);
            self.core.store(self.ctx, oid, i, v);
        }
        oid
    }

    fn members(&mut self, oid: Oid, def: &ObjDef, src: &Source, cx: u32) {
        for m in &def.members {
            match m {
                Member::Property { name, ty, value, readonly, required, loc, .. } => {
                    if ty.is_empty() {
                        // `required name` on an existing property: nothing to
                        // declare
                        if let Some(i) = self.core.prop_idx(oid, name).filter(|_| *required) {
                            self.core.0.borrow_mut().objs[oid].props[i as usize].required = true;
                        }
                        continue;
                    }
                    let idx = self.add_prop(oid, name, PropType::parse(ty), *readonly);
                    if *required {
                        self.core.0.borrow_mut().objs[oid].props[idx as usize].required = true;
                    }
                    if let Some(v) = value {
                        self.value(oid, idx, v, src, cx, *loc);
                    }
                }
                Member::Alias { name, target, loc, .. } => {
                    // `property alias font: symbol.font`: each part aliases the
                    // same part of the target, so `font.pixelSize: 20` reaches
                    // `symbol`
                    if let Some(parts) = self.alias_group(target, src) {
                        for (part, ty) in parts {
                            let idx = self.add_prop(oid, &format!("{name}.{part}"), ty, false);
                            self.work.push(Work::Alias { oid, idx, target: format!("{target}.{part}"), src: src.clone(), cx, loc: *loc });
                        }
                        continue;
                    }
                    let idx = self.add_prop(oid, name, PropType::Var, false);
                    self.work.push(Work::Alias { oid, idx, target: target.clone(), src: src.clone(), cx, loc: *loc });
                }
                Member::Signal { name, params, .. } => {
                    let mut s = self.core.0.borrow_mut();
                    s.objs[oid].signals.push(Signal { name: Rc::from(name.as_str()), params: params.clone() });
                    s.objs[oid].renamed();
                }
                Member::Function { name, params, body, loc, .. }
                    if is_handler_name(name)
                        && self.core.0.borrow().objs[oid].def.kind == types::Kind::Connections =>
                {
                    // `function onPing(n) { ... }` in a Connections handles the
                    // target's `ping`, with its own parameter names
                    self.work.push(Work::Handler {
                        oid, signal: lower_first(&name[2..]), js: body.clone(), params: Some(params.clone()),
                        src: src.clone(), cx, loc: *loc,
                    });
                }
                Member::Function { name, params, types, ret, body, loc } => {
                    self.work.push(Work::Function {
                        oid, name: name.clone(), params: params.clone(), types: types.clone(), ret: ret.clone(),
                        body: body.clone(),
                        src: src.clone(), cx, loc: *loc,
                    });
                }
                Member::Assign { name, value, loc } => self.assign(oid, name, value, src, cx, *loc),
                // A Repeater's child is its delegate, a LazyLoader's its
                // component
                Member::Child(child) if self.default_component(oid).is_some() => {
                    let prop = self.default_component(oid).unwrap_or("delegate");
                    let c = self.component_of(child, src, cx, Some(oid));
                    if let Some(idx) = self.core.prop_idx(oid, prop) {
                        self.work.push(Work::Literal { oid, idx, value: Value::Obj(c) });
                    }
                }
                Member::Child(child) => {
                    let into = self.default_parent(oid);
                    match self.make(child, src, cx, Some(into)) {
                        Ok(c) => self.adopt(into, c),
                        Err(e) => self.core.load_error(e),
                    }
                }
                Member::On { prop, obj, .. } => {
                    // Behavior or value source: created, kept, and told its
                    // property (`anim/`)
                    match self.make(obj, src, cx, Some(oid)) {
                        Ok(c) => {
                            self.core.0.borrow_mut().objs[oid].children.push(c);
                            self.work.push(Work::On { oid: c, target: oid, prop: prop.clone() });
                        }
                        Err(e) => self.core.load_error(e),
                    }
                }
            }
        }
    }

    /// Property a child defines, on types whose default property is a component
    fn default_component(&self, oid: Oid) -> Option<&'static str> {
        let s = self.core.0.borrow();
        let def = s.objs[oid].def;
        match def.kind {
            types::Kind::Repeater | types::Kind::Instantiator => Some("delegate"),
            types::Kind::Loader if types::is_a(def, "LazyLoader") => Some("component"),
            _ => None,
        }
    }

    /// `default property alias data: box.data` puts an instance's children in
    /// `box`
    fn into_default(&mut self, oid: Oid, root: &ObjDef) {
        let target = root.members.iter().find_map(|m| match m {
            Member::Alias { target, default: true, .. } => Some(target.clone()),
            _ => None,
        });
        if let Some(t) = target {
            self.core.0.borrow_mut().objs[oid].default_into = Some(Rc::from(t.as_str()));
        }
    }

    /// Where a child of `oid` goes: `oid` or the target of its default alias
    /// chain
    fn default_parent(&self, oid: Oid) -> Oid {
        let mut at = oid;
        for _ in 0..8 {
            let next = {
                let s = self.core.0.borrow();
                let o = &s.objs[at];
                o.default_into.as_ref().and_then(|t| {
                    let head = t.split('.').next().unwrap_or(t);
                    s.cxs.get(o.cx as usize).and_then(|c| c.ids.get(head).copied())
                })
            };
            match next {
                Some(n) if n != at => at = n,
                _ => break,
            }
        }
        at
    }

    fn adopt(&mut self, parent: Oid, child: Oid) {
        self.core.0.borrow_mut().objs[parent].children.push(child);
        if let Some(i) = self.core.prop_idx(child, "parent") {
            self.work.push(Work::Literal { oid: child, idx: i, value: Value::Obj(parent) });
        }
        // Types with one object as default property: FileView's `adapter`,
        // Behavior's `animation`
        let one = match self.core.0.borrow().objs[parent].def.kind {
            types::Kind::FileView => Some("adapter"),
            types::Kind::Behavior => Some("animation"),
            _ => None,
        };
        if let Some(i) = one.and_then(|p| self.core.prop_idx(parent, p)) {
            self.work.push(Work::Literal { oid: parent, idx: i, value: Value::Obj(child) });
        }
    }

    fn assign(&mut self, oid: Oid, name: &str, value: &Value_, src: &Source, cx: u32, loc: Loc) {
        let js = match value {
            Value_::Js { src, .. } => Some(src.clone()),
            _ => None,
        };
        if name == "Component.onCompleted" {
            if let Some(js) = js {
                self.work.push(Work::Completed { oid, js, src: src.clone(), cx, loc });
            }
            return;
        }
        // A ScriptAction's `script` is a handler for a private signal, not a
        // value
        if name == "script" && self.core.0.borrow().objs[oid].def.name == "ScriptAction" {
            if let Some(js) = js {
                self.work.push(Work::Handler {
                    oid, signal: crate::anim::SCRIPT.to_string(), js, params: None, src: src.clone(), cx, loc,
                });
            }
            return;
        }
        if name == "Component.onDestruction" {
            if let Some(js) = js {
                self.work.push(Work::Destruction { oid, js, src: src.clone(), cx, loc });
            }
            return;
        }
        let last = name.rsplit('.').next().unwrap_or(name);
        if let Some(rest) = last.strip_prefix("on") {
            if rest.chars().next().is_some_and(|c| c.is_ascii_uppercase()) {
                // `Keys.onReturnPressed`: a signal on every item, named with
                // its attached object (`input/keys.rs`)
                let attached = name.strip_suffix(last).and_then(|p| p.strip_suffix('.'));
                if attached.is_some_and(|a| a != "Keys") {
                    let place = self.place(src, loc);
                    self.core.log(format!("{place}: attached handlers ('{name}') are not there yet"));
                    return;
                }
                let Some(js) = js else {
                    let place = self.place(src, loc);
                    self.core.log(format!("{place}: a handler is JavaScript, not an object"));
                    return;
                };
                let signal = match attached {
                    Some(a) => format!("{a}.{}", lower_first(rest)),
                    None => lower_first(rest),
                };
                self.work.push(Work::Handler { oid, signal, js, params: None, src: src.clone(), cx, loc });
                return;
            }
        }
        // Rows and stand-ins for missing types accept any property
        if self.core.prop_idx(oid, name).is_none()
            && matches!(self.core.0.borrow().objs[oid].def.kind, types::Kind::Row | types::Kind::Inert)
        {
            self.add_prop(oid, name, PropType::Var, false);
        }
        match self.core.prop_idx(oid, name) {
            Some(idx) => self.value(oid, idx, value, src, cx, loc),
            // `sourceSize: Qt.size(w, h)`, `font: editor.font`: each part binds
            // to that part of the value. Parts missing from the value keep
            // theirs (`__aosh_keep`)
            None if self.core.kind_of(oid, name) == super::kind::GROUP && matches!(value, Value_::Js { .. }) => {
                let Value_::Js { src: js, loc: jloc } = value else { return };
                let whole = binding_body(js);
                for (idx, part) in self.core.group_parts(oid, name) {
                    let body = format!(
                        "const __v = (function () {{ {whole} \n}}).call(this);\n\
                         return __v === undefined || __v === null || __v[{part:?}] === undefined ? __aosh_keep : __v[{part:?}];"
                    );
                    let key = format!("{}:{}:{}:bind:{part}", src.file, jloc.line, jloc.col);
                    self.work.push(Work::Bind { oid, idx, js: body, src: src.clone(), cx, loc: *jloc, key: Some(key) });
                }
            }
            None => {
                let place = self.place(src, loc);
                let ty = self.core.0.borrow().objs[oid].ty.clone();
                self.core.log(format!("{place}: {ty} has no property '{name}'"));
            }
        }
    }

    fn value(&mut self, oid: Oid, idx: u32, value: &Value_, src: &Source, cx: u32, loc: Loc) {
        match value {
            Value_::Js { src: js, loc } => match value.literal() {
                Some(l) => {
                    let v = match l {
                        Literal::Bool(b) => Value::Bool(b),
                        Literal::Number(n) => Value::Num(n),
                        Literal::Str(s) => Value::str(&s),
                    };
                    self.work.push(Work::Literal { oid, idx, value: v });
                }
                None => self.work.push(Work::Bind { oid, idx, js: js.clone(), src: src.clone(), cx, loc: *loc, key: None }),
            },
            Value_::Object(def) if self.holds_component(oid, idx) && def.ty != "Component" => {
                let c = self.component_of(def, src, cx, Some(oid));
                self.work.push(Work::Literal { oid, idx, value: Value::Obj(c) });
            }
            Value_::Object(def) => match self.make(def, src, cx, Some(oid)) {
                Ok(c) => {
                    // A Control's background and content are children from the
                    // start, as Qt parents them, since their bindings read
                    // `parent`
                    let part = {
                        let st = self.core.0.borrow();
                        let o = &st.objs[oid];
                        matches!(&*o.props[idx as usize].name, "background" | "contentItem") && types::is_a(o.def, "Control")
                    };
                    if let (true, Some(i)) = (part, self.core.prop_idx(c, "parent")) {
                        self.work.push(Work::Literal { oid: c, idx: i, value: Value::Obj(oid) });
                    }
                    self.work.push(Work::Literal { oid, idx, value: Value::Obj(c) });
                }
                Err(e) => self.core.load_error(e),
            },
            Value_::Objects(defs) => {
                let mut items = Vec::new();
                for d in defs {
                    match self.make(d, src, cx, Some(oid)) {
                        Ok(c) => items.push(Value::Obj(c)),
                        Err(e) => self.core.load_error(e),
                    }
                }
                let _ = loc;
                self.work.push(Work::Literal { oid, idx, value: Value::List(Rc::new(items)) });
            }
        }
    }

    fn holds_component(&self, oid: Oid, idx: u32) -> bool {
        let s = self.core.0.borrow();
        let o = &s.objs[oid];
        let name = &*o.props[idx as usize].name;
        // `layer.effect` is a per-layer component, and a SwipeDelegate's behind
        // item is created on swipe
        (COMPONENT_PROPS.contains(&name)
            && matches!(o.def.kind, types::Kind::Repeater | types::Kind::ListView | types::Kind::Loader | types::Kind::Instantiator))
            || (name == "layer.effect" && types::is_a(o.def, "Item"))
            || (matches!(name, "swipe.left" | "swipe.right" | "swipe.behind") && types::is_a(o.def, "SwipeDelegate"))
    }

    fn compile(&self, key: String, params: &[String], body: &str, cx: u32) -> rquickjs::Result<Persistent<Function<'static>>> {
        crate::js::compile(self.ctx, self.core, &key, params, body, cx)
    }

    /// Everything queued, in QML's order (the top of this file)
    fn finish(&mut self) {
        let work = std::mem::take(&mut self.work);
        let ctx = self.ctx;
        let core = self.core;

        // Initial values types have before any read: a SystemClock's time
        for &oid in &self.made {
            crate::services::made(ctx, core, oid);
        }

        for w in &work {
            if let Work::Alias { oid, idx, target, src, cx, loc } = w {
                self.alias(*oid, *idx, target, src, *cx, *loc);
            }
        }
        // Writing through an alias removes the target's own value or binding
        // (Qt's RemoveBindingOnAliasWrite)
        let given: Vec<(Oid, u32)> = {
            let st = core.0.borrow();
            work.iter().filter_map(|w| match w {
                Work::Literal { oid, idx, .. } | Work::Bind { oid, idx, .. }
                    if st.objs[*oid].props[*idx as usize].alias.is_some() => Some(super::resolve(&st, *oid, *idx)),
                _ => None,
            }).collect()
        };
        let work: Vec<Work> = if given.is_empty() {
            work
        } else {
            work.into_iter().filter(|w| match w {
                Work::Literal { oid, idx, .. } | Work::Bind { oid, idx, .. } => !given.contains(&(*oid, *idx)),
                _ => true,
            }).collect()
        };
        // A property set twice keeps the last value; instance members are
        // pushed after the file's
        let last: std::collections::HashMap<(Oid, u32), usize> = work.iter().enumerate()
            .filter_map(|(k, w)| match w {
                Work::Literal { oid, idx, .. } | Work::Bind { oid, idx, .. } => Some(((*oid, *idx), k)),
                _ => None,
            })
            .collect();
        let work: Vec<Work> = work.into_iter().enumerate().filter(|(k, w)| match w {
            Work::Literal { oid, idx, .. } | Work::Bind { oid, idx, .. } => last.get(&(*oid, *idx)) == Some(k),
            _ => true,
        }).map(|(_, w)| w).collect();
        for w in &work {
            if let Work::Function { oid, name, params, types, ret, body, src, cx, loc } = w {
                let key = format!("{}:{}:{}:fn", src.file, loc.line, loc.col);
                let r = self.compile(key, params, body, *cx).and_then(|f| {
                    let f = f.restore(ctx)?;
                    let w = crate::js::wrapper(ctx, core, *oid)?;
                    w.as_object().map(|o| o.set(name.as_str(), f)).transpose()?;
                    Ok(())
                });
                // Registers an IpcHandler's functions and their argument types
                // for `aosh ipc`
                if r.is_ok() && core.0.borrow().objs[*oid].def.kind == types::Kind::Ipc {
                    crate::ipc::declared(core, *oid, crate::ipc::Sig {
                        name: name.clone(), params: params.clone(), types: types.clone(), ret: ret.clone(),
                    });
                }
                if let Err(e) = r {
                    core.log(format!("{}: function {name}: {}", self.place(src, *loc), self.js_error(e)));
                }
            }
        }
        for w in &work {
            if let Work::Literal { oid, idx, value } = w {
                // `parent` comes from the builder, so it does not mark the
                // child as explicitly placed
                let is_parent = &*core.0.borrow().objs[*oid].props[*idx as usize].name == "parent";
                if !is_parent {
                    core.mark_explicit(*oid, *idx);
                }
                core.store(ctx, *oid, *idx, value.clone());
            }
        }
        let mut made = Vec::new();
        for w in &work {
            if let Work::Bind { oid, idx, js, src, cx, loc, key } = w {
                let key = key.clone().unwrap_or_else(|| format!("{}:{}:{}:bind", src.file, loc.line, loc.col));
                let mut r = self.compile(key.clone(), &[], &binding_body(js), *cx);
                if r.is_err() {
                    if let Some(alt) = binding_fallback(js) {
                        // Drops the block's error; the object literal's is
                        // reported if that fails too
                        let _ = self.js_error(r.err().unwrap());
                        r = self.compile(format!("{key}:obj"), &[], &alt, *cx);
                    }
                }
                match r {
                    Ok(f) => made.push(core.bind((*oid, *idx), f, *oid, src.file, *loc)),
                    Err(e) => core.log(format!("{}: {}", self.place(src, *loc), self.js_error(e))),
                }
            }
        }
        // Last created runs first, as in QQmlObjectCreator::finalize: an
        // instance's bindings run before its type's, a child's before its
        // parent's. Reading a pending binding's property runs it first
        // (`flush`)
        for &b in &made {
            core.0.borrow_mut().binds[b as usize].pending = true;
        }
        for b in made.into_iter().rev() {
            core.evaluate(ctx, b);
        }
        for w in &work {
            if let Work::Handler { oid, signal, js, params, src, cx, loc } = w {
                self.handler(*oid, signal, js, params.clone(), src, *cx, *loc);
            }
        }
        // Stores children lists once all children exist, so bindings that read
        // them (`box.children.length`) run again
        for &oid in &self.made {
            // A Gradient's `stops` are its children too
            for name in ["children", "resources", "data", "stops"] {
                // Item's own list, not a file's `property var data`
                if let Some(i) = core.prop_idx(oid, name).filter(|&i| core.prop_type(oid, i) == PropType::List) {
                    core.store_raw(ctx, oid, i, Value::List(Rc::new(core.listed(oid, name))));
                }
            }
        }
        for &oid in &self.made {
            if core.0.borrow().objs[oid].def.kind == types::Kind::Connections {
                core.follow(oid);
            }
        }
        for w in &work {
            if let Work::Destruction { oid, js, src, cx, loc } = w {
                let key = format!("{}:{}:{}:gone", src.file, loc.line, loc.col);
                match self.compile(key, &[], &handler_body(js), *cx) {
                    Ok(f) => core.on(*oid, "destruction", f, *oid),
                    Err(e) => core.log(format!("{}: Component.onDestruction: {}", self.place(src, *loc), self.js_error(e))),
                }
            }
        }
        for w in &work {
            if let Work::On { oid, target, prop } = w {
                crate::anim::on(core, *oid, *target, prop);
            }
        }
        // Completion hooks: a Repeater creates delegates, a Behavior starts
        // watching, a Process with `running: true` starts. Deferred until all
        // bindings have run
        let made = std::mem::take(&mut self.made);
        for &oid in &made {
            if let Some(o) = core.0.borrow_mut().objs.get_mut(oid) {
                o.complete = true;
            }
        }
        for &oid in &made {
            if !core.0.borrow().objs.is_live(oid) {
                continue;
            }
            crate::model::completed(ctx, core, oid);
            crate::anim::completed(ctx, core, oid);
            crate::io::completed(ctx, core, oid);
            crate::input::completed(ctx, core, oid);
            crate::services::capture::completed(core, oid);
            if core.0.borrow().objs.get(oid).is_some_and(|o| o.def.kind == types::Kind::Binding) {
                super::hold::update(ctx, core, oid);
            }
            if core.0.borrow().objs.get(oid).is_some_and(|o| o.def.kind == types::Kind::Synchronizer) {
                super::sync::completed(ctx, core, oid);
            }
        }
        self.made = made;
        for w in &work {
            if let Work::Completed { oid, js, src, cx, loc } = w {
                let key = format!("{}:{}:{}:done", src.file, loc.line, loc.col);
                let r = self.compile(key, &[], &handler_body(js), *cx)
                    .and_then(|f| crate::js::call0(ctx, core, &f, *oid).map(|_| ()));
                if let Err(e) = r {
                    core.log(format!("{}: Component.onCompleted: {}", self.place(src, *loc), self.js_error(e)));
                }
            }
        }
        crate::js::run_later(ctx, core);
    }

    fn alias(&mut self, oid: Oid, idx: u32, target: &str, src: &Source, cx: u32, loc: Loc) {
        let place = self.place(src, loc);
        let (head, rest) = match target.split_once('.') {
            Some((h, r)) => (h, Some(r)),
            None => (target, None),
        };
        let obj = self.core.0.borrow().cxs[cx as usize].ids.get(head).copied();
        let Some(obj) = obj else {
            self.core.log(format!("{place}: alias '{target}': there is no id '{head}' here"));
            return;
        };
        match rest {
            // `property alias button: okButton`: the object itself
            None => self.work_literal_now(oid, idx, Value::Obj(obj)),
            Some(prop) => match self.core.prop_idx(obj, prop) {
                Some(ti) => {
                    let mut s = self.core.0.borrow_mut();
                    let ty = s.objs[obj].props[ti as usize].ty;
                    let p = &mut s.objs[oid].props[idx as usize];
                    p.alias = Some((obj, ti));
                    p.ty = ty;
                    s.objs[obj].props[ti as usize].aliased_by.push((oid, idx));
                }
                None => self.core.log(format!("{place}: alias '{target}': no property '{prop}' there")),
            },
        }
    }

    /// Parts of the value type an alias names. The target object does not exist
    /// yet when the alias is declared, so its type is followed to the built-in
    /// one
    fn alias_group(&mut self, target: &str, src: &Source) -> Option<Vec<(String, PropType)>> {
        let (id, group) = target.split_once('.')?;
        if group.contains('.') {
            return None;
        }
        let doc = src.doc.clone()?;
        let def = std::iter::once(&doc.root).chain(doc.inline.iter().map(|(_, d)| d)).find_map(|d| def_with_id(d, id))?;
        let (mut ty, mut tsrc) = (def.ty.clone(), src.clone());
        let t = loop {
            match self.find_component(&ty, &tsrc) {
                Some(Found::File(path)) => {
                    let doc = self.doc(&path).ok()?;
                    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
                    tsrc = self.source(&path.display().to_string(), &dir, &doc);
                    ty = doc.root.ty.clone();
                }
                Some(Found::Inline(idef, isrc)) => {
                    ty = idef.ty.clone();
                    tsrc = isrc;
                }
                None => break types::find(&ty)?,
            }
        };
        let dotted = format!("{group}.");
        let parts: Vec<(String, PropType)> = types::chain(t).iter()
            .flat_map(|t| t.props.iter())
            .filter_map(|(n, ty, _)| n.strip_prefix(&dotted).map(|p| (p.to_string(), *ty)))
            .collect();
        let plain = types::chain(t).iter().any(|t| t.props.iter().any(|(n, _, _)| *n == group));
        (!parts.is_empty() && !plain).then_some(parts)
    }

    fn work_literal_now(&mut self, oid: Oid, idx: u32, v: Value) {
        let mut s = self.core.0.borrow_mut();
        s.objs[oid].props[idx as usize].value = v;
    }

    #[allow(clippy::too_many_arguments)]
    fn handler(&self, oid: Oid, signal: &str, js: &str, params: Option<Vec<String>>, src: &Source, cx: u32, loc: Loc) {
        let core = self.core;
        let place = self.place(src, loc);
        let is_connections = core.0.borrow().objs[oid].def.kind == types::Kind::Connections;
        // Signal owner: the object, or a Connections' target
        let owner = if is_connections {
            core.peek_named(oid, "target").and_then(|v| v.as_obj())
        } else {
            Some(oid)
        };
        let known = owner.and_then(|o| self.signal_params(o, signal));
        // A stand-in has no signals, and a handler on one waits for its type
        if known.is_none() && core.0.borrow().objs[oid].def.kind == types::Kind::Inert {
            return;
        }
        if known.is_none() && !is_connections {
            let ty = core.0.borrow().objs[oid].ty.clone();
            core.log(format!("{place}: {ty} has no signal '{signal}' for on{}", capitalise(signal)));
            return;
        }
        let params = params.or(known).unwrap_or_default();
        let key = format!("{}:{}:{}:on", src.file, loc.line, loc.col);
        match self.compile(key, &params, &handler_body(js), cx) {
            Ok(f) => {
                if is_connections {
                    core.0.borrow_mut().objs[oid].listens.push((Rc::from(signal), f));
                } else {
                    core.on(oid, signal, f, oid);
                }
            }
            Err(e) => core.log(format!("{place}: on{}: {}", capitalise(signal), self.js_error(e))),
        }
    }

    /// Parameter names of an object's signal; `nameChanged` has none
    fn signal_params(&self, oid: Oid, signal: &str) -> Option<Vec<String>> {
        let s = self.core.0.borrow();
        let o = &s.objs[oid];
        if let Some(g) = o.signals.iter().find(|g| &*g.name == signal) {
            return Some(g.params.clone());
        }
        let prop = signal.strip_suffix("Changed")?;
        o.props.iter().any(|p| &*p.name == prop).then(Vec::new)
    }
}

/// The lines an item's anchors attach to, as `Item` names them
pub const ANCHOR_LINES: [&str; 6] = ["left", "right", "top", "bottom", "horizontalCenter", "verticalCenter"];

// The AST's `Value`, as distinct from the object model's
use crate::qml::ast::Value as Value_;

/// Finds the object with `id: name` in a file
fn def_with_id<'d>(d: &'d ObjDef, name: &str) -> Option<&'d ObjDef> {
    if d.id.as_deref() == Some(name) {
        return Some(d);
    }
    d.members.iter().find_map(|m| match m {
        Member::Child(c) | Member::On { obj: c, .. } => def_with_id(c, name),
        Member::Assign { value: Value_::Object(c), .. } | Member::Property { value: Some(Value_::Object(c)), .. } => def_with_id(c, name),
        Member::Assign { value: Value_::Objects(cs), .. } | Member::Property { value: Some(Value_::Objects(cs)), .. } => {
            cs.iter().find_map(|c| def_with_id(c, name))
        }
        _ => None,
    })
}

/// `onClicked`, `onWidthChanged`: `on` and a capital
fn is_handler_name(name: &str) -> bool {
    name.strip_prefix("on").is_some_and(|r| r.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Binding source as a function body: expressions are returned, blocks return
/// for themselves.
///
/// Statement bindings (`width: if (wide) 200; else 100`) compile as a function
/// body as in Qt. With no `return`, the value is the completion value, from
/// `eval` in the binding's scope
pub fn binding_body(js: &str) -> String {
    let t = js.trim_start();
    if t.starts_with('{') {
        js.to_string()
    } else if starts_statement(t) {
        if has_word(t, "return") {
            js.to_string()
        } else {
            format!("return eval({});", quote_js(js))
        }
    } else {
        format!("return ({js}\n);")
    }
}

/// Whether a value is a statement rather than an expression
fn starts_statement(t: &str) -> bool {
    const WORDS: &[&str] = &["switch", "if", "for", "while", "do", "try", "var", "let", "const", "throw", "return"];
    WORDS.iter().any(|w| {
        t.strip_prefix(w).is_some_and(|r| r.is_empty() || !r.starts_with(|c: char| c.is_alphanumeric() || c == '_' || c == '$'))
    })
}

/// `word` as a whole word in `src`, including strings and comments. False
/// matches only compile the binding as a body
fn has_word(src: &str, word: &str) -> bool {
    let is_id = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    src.match_indices(word).any(|(i, _)| {
        let before = src[..i].chars().next_back();
        let after = src[i + word.len()..].chars().next();
        !before.is_some_and(is_id) && !after.is_some_and(is_id)
    })
}

/// A JavaScript string literal holding `s`
fn quote_js(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Fallback for a binding starting with `{` that fails as a block: an object
/// literal. The block is tried first, as in QML, so `{}` is an empty block
pub fn binding_fallback(js: &str) -> Option<String> {
    js.trim_start().starts_with('{').then(|| format!("return ({js}\n);"))
}

/// Handler source as a function body. A function value (`(mouse) => { ... }`,
/// `function (mouse) { ... }`) is called with the signal's arguments; anything
/// else is statements
pub fn handler_body(js: &str) -> String {
    let t = js.trim_start();
    if t.starts_with('{') {
        return js.to_string();
    }
    if is_function_value(t) {
        return format!("return ({js}\n).apply(this, arguments);");
    }
    format!("{js}\n;")
}

fn is_function_value(t: &str) -> bool {
    if t.starts_with("function") && t[8..].trim_start().starts_with(['(', '{']) || t.starts_with("function ") {
        return true;
    }
    // `(a, b) =>` or `a =>`
    if let Some(rest) = t.strip_prefix('(') {
        let mut depth = 1;
        for (i, c) in rest.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        return rest[i + 1..].trim_start().starts_with("=>");
                    }
                }
                _ => {}
            }
        }
        return false;
    }
    let ident_end = t.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(t.len());
    ident_end > 0 && t[ident_end..].trim_start().starts_with("=>")
}
