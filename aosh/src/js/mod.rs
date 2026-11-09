// QuickJS bridge
//
// A QML object is a `Proxy` with a shared handler (`prelude/objects.js`).
// `has`, `get`, and `set` query the object model through `__aosh_kind`:
// properties go through the model (where binding dependencies are recorded),
// grouped names (`anchors`, `font`) are nested proxies, and methods and signals
// are functions. A `function` declared in QML lives on the proxy's target.
//
// Each expression is compiled once per file location into a factory taking the
// id scope and component root:
//
//     with (root) { with (this) { with (ids) { return (expression); } } }
//
// Name lookup order: ids, the object, the file's root, globals. Signal
// parameters belong to the inner function, so properties do not hide them.

pub mod library;
mod md5;

use rquickjs::function::{Rest, This};
use rquickjs::{Coerced, Context, Ctx, Function, Persistent, Runtime};

use crate::object::value::{Oid, PropType, Value};
use crate::object::Core;

/// Runtime that owns the heap and context that owns the globals
pub struct Js {
    // Declared first so it drops before the runtime whose heap it borrows
    context: Context,
    _runtime: Runtime,
}

impl Js {
    pub fn new() -> Result<Self, String> {
        let runtime = Runtime::new().map_err(|e| format!("the JavaScript runtime would not start: {e}"))?;
        let context = Context::full(&runtime)
            .map_err(|e| format!("the JavaScript context would not start: {e}"))?;
        Ok(Self { context, _runtime: runtime })
    }

    pub fn with<R>(&self, f: impl FnOnce(Ctx<'_>) -> R) -> R {
        self.context.with(f)
    }

    /// Evaluates `source` and returns `String(value)`, or the exception message
    pub fn eval(&self, source: &str) -> Result<String, String> {
        self.context.with(|ctx| match ctx.eval::<Coerced<String>, _>(source) {
            Ok(Coerced(s)) => Ok(s),
            Err(e) => Err(describe(&ctx, e)),
        })
    }

    /// The functions the prelude calls back into, and the prelude
    pub fn install(&self, core: &Core) -> Result<(), String> {
        self.context.with(|ctx| install(&ctx, core).map_err(|e| describe(&ctx, e)))
    }
}

fn install<'js>(ctx: &Ctx<'js>, core: &Core) -> rquickjs::Result<()> {
    let g = ctx.globals();

    let c = core.clone();
    g.set("__aosh_kind", Function::new(ctx.clone(), move |oid: u32, name: String| c.kind_of(oid, &name))?)?;

    let c = core.clone();
    g.set("__aosh_get", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, name: String| {
        match c.prop_idx(oid, &name) {
            Some(i) => {
                // Flushes a pending binding, like Qt's flushPendingBinding
                c.flush(&ctx, oid, i);
                let v = c.read(oid, i);
                // Color properties are exposed as color objects (`Qt.color` in
                // the prelude)
                if c.prop_type(oid, i) == PropType::Color {
                    if let Value::Str(s) = &v {
                        let of: Function = ctx.globals().get("__aosh_colour_of")?;
                        return of.call((s.to_string(),));
                    }
                }
                to_js(&ctx, &c, &v)
            }
            None => Ok(rquickjs::Value::new_undefined(ctx)),
        }
    })?)?;

    // Color as `#aarrggbb` for `Qt.color`, using the same name table as
    // properties
    g.set("__aosh_colour", Function::new(ctx.clone(), |s: String| crate::object::value::normalise_colour(&s))?)?;

    let c = core.clone();
    g.set("__aosh_set", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, name: String, v: rquickjs::Value<'js>| {
        if let Some(i) = c.prop_idx(oid, &name) {
            // `x = Qt.binding(f)` sets a binding, not a value (`prelude/qt.js`)
            let binding = v.as_object().and_then(|o| o.get::<_, Option<Function>>("__aosh_binding").ok().flatten());
            if let Some(f) = binding {
                c.bind_script(&ctx, oid, i, f);
                return;
            }
            let ty = c.prop_type(oid, i);
            let v = from_js_as(&ctx, v, ty);
            c.assign(&ctx, oid, i, v);
        }
    })?)?;

    let c = core.clone();
    g.set("__aosh_set_group", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, prefix: String, v: rquickjs::Value<'js>| {
        let Some(o) = v.as_object() else { return };
        for (idx, part) in c.group_parts(oid, &prefix) {
            let Ok(x) = o.get::<_, rquickjs::Value>(part.as_str()) else { continue };
            if x.is_undefined() {
                continue;
            }
            let ty = c.prop_type(oid, idx);
            let x = from_js_as(&ctx, x, ty);
            c.assign(&ctx, oid, idx, x);
        }
    })?)?;

    let c = core.clone();
    g.set("__aosh_call", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, name: String, args: Rest<rquickjs::Value<'js>>| {
        let v = c.call_method(&ctx, oid, &name, args.0);
        to_js(&ctx, &c, &v)
    })?)?;

    let c = core.clone();
    g.set("__aosh_emit", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, name: String, args: Rest<rquickjs::Value<'js>>| {
        c.emit(&ctx, oid, &name, args.0);
    })?)?;

    let c = core.clone();
    g.set("__aosh_type", Function::new(ctx.clone(), move |oid: u32| {
        c.0.borrow().objs.get(oid).map(|o| o.ty.clone()).unwrap_or_default()
    })?)?;

    // Whether a property is a typed list (`list<var>`), written back when
    // changed in place (`__aosh_seq`)
    let c = core.clone();
    g.set("__aosh_is_list", Function::new(ctx.clone(), move |oid: u32, name: String| {
        c.prop_idx(oid, &name).is_some_and(|i| c.prop_type(oid, i) == PropType::List)
    })?)?;

    // Nearest window up the parents, or null. Backs `QsWindow.window`
    let c = core.clone();
    g.set("__aosh_window", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32| -> rquickjs::Result<rquickjs::Value<'js>> {
        let win = {
            let s = c.0.borrow();
            let mut at = Some(oid);
            while let Some(o) = at.and_then(|o| s.objs.get(o).map(|x| (o, x))) {
                if o.1.def.kind == crate::types::Kind::Window {
                    break;
                }
                at = o.1.parent;
            }
            at.filter(|&o| s.objs.get(o).is_some_and(|x| x.def.kind == crate::types::Kind::Window))
        };
        to_js(&ctx, &c, &win.map(Value::Obj).unwrap_or(Value::Null))
    })?)?;

    let c = core.clone();
    g.set("__aosh_log", Function::new(ctx.clone(), move |level: String, msg: String| {
        match level.as_str() {
            "warn" | "error" => c.log(format!("{level}: {msg}")),
            _ => println!("{msg}"),
        }
    })?)?;

    // Loads a singleton on first use (`qml/import.rs`). A failed load logs once
    // and stays null
    let c = core.clone();
    g.set("__aosh_singleton", Function::new(ctx.clone(), move |ctx: Ctx<'js>, name: String| -> rquickjs::Result<rquickjs::Value<'js>> {
        match crate::object::build::Builder::new(&c, &ctx).singleton(&name) {
            Ok(oid) => wrapper(&ctx, &c, oid),
            Err(e) => {
                c.load_error(e);
                Ok(rquickjs::Value::new_null(ctx))
            }
        }
    })?)?;

    g.set("__aosh_md5", Function::new(ctx.clone(), move |text: String| md5::hex(text.as_bytes()))?)?;

    // `item.someSignal.connect(f)` adds a handler, `disconnect(f)` removes it
    let c = core.clone();
    g.set("__aosh_connect", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, signal: String, f: Function<'js>| {
        if c.0.borrow().objs.is_live(oid) {
            c.on(oid, &signal, Persistent::save(&ctx, f), oid);
        }
    })?)?;
    let c = core.clone();
    g.set("__aosh_disconnect", Function::new(ctx.clone(), move |ctx: Ctx<'js>, oid: u32, signal: String, f: Function<'js>| {
        let handlers: Vec<(usize, Persistent<Function<'static>>)> = {
            let s = c.0.borrow();
            let Some(o) = s.objs.get(oid) else { return };
            o.handlers.iter().enumerate().filter(|(_, (n, _))| &**n == signal.as_str())
                .map(|(i, (_, h))| (i, h.func.clone())).collect()
        };
        let gone = handlers.into_iter().find(|(_, h)| h.clone().restore(&ctx).is_ok_and(|h| h == f)).map(|(i, _)| i);
        if let Some(i) = gone {
            c.0.borrow_mut().objs[oid].handlers.remove(i);
        }
    })?)?;

    // `Quickshell.env(name)`: the variable, or null if unset
    g.set("__aosh_env", Function::new(ctx.clone(), move |name: String| std::env::var(name).ok())?)?;

    // `Quickshell.shellDir`: directory of the shell's modules (`qs`,
    // `qml/import.rs`)
    let c = core.clone();
    g.set("__aosh_shell_dir", Function::new(ctx.clone(), move || {
        c.0.borrow().modules.as_ref().map(|m| m.root.display().to_string()).unwrap_or_default()
    })?)?;

    // Whether the icon theme has `name` (`scene/icons.rs`). Backs
    // `iconPath(name, true)` and `hasThemeIcon`
    g.set("__aosh_icon_exists", Function::new(ctx.clone(), move |name: String| {
        crate::scene::icons::Icons::global().has(&name)
    })?)?;

    // `StandardPaths.standardLocations(kind)`, as `file://` URLs (`io/xdg.rs`)
    g.set("__aosh_locations", Function::new(ctx.clone(), move |kind: u32| {
        crate::io::xdg::Env::process().locations(kind).iter().map(|p| crate::io::xdg::url(p)).collect::<Vec<_>>()
    })?)?;

    let c = core.clone();
    g.set("__aosh_exec", Function::new(ctx.clone(), move |argv: Vec<String>| {
        let Some((prog, rest)) = argv.split_first() else { return };
        if !crate::io::may_start(&c, &argv) {
            return;
        }
        // Detached; the loop's `try_wait` reaps the child on the next pass.
        //
        // Inherits the shell's descriptors rather than /dev/null: a terminal's
        // shell reads fd 0 from its pty
        let mut cmd = std::process::Command::new(prog);
        cmd.args(rest);
        unprivileged(&mut cmd);
        match cmd.spawn() {
            Ok(child) => c.0.borrow_mut().children.push(child),
            Err(e) => c.log(format!("execDetached {prog}: {e}")),
        }
    })?)?;

    for (name, src) in PRELUDE {
        let mut opts = rquickjs::context::EvalOptions::default();
        opts.filename = Some(format!("aosh:{name}"));
        ctx.eval_with_options::<(), _>(*src, opts)?;
    }
    Ok(())
}

/// Started programs do not keep `CAP_MAY_PRIV`, which aosh needs to bind
/// `may_layer` and `may_desktop`. Also dropped from the bounding set, as in
/// `spawn::run` (`lib/gfx/mayland/src/spawn.rs`)
#[cfg(target_os = "miku")]
pub fn unprivileged(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn miku_capget(buf: *mut u64) -> i64;
        fn miku_capset(permitted: u64, effective: u64, bounding: u64) -> i64;
    }
    const CAP_MAY_PRIV: u64 = 1 << miku_abi::cred::CAP_MAY_PRIV;
    // SAFETY: only system calls between fork and exec, with no allocation or
    // locks. Failures do not block starting the program
    unsafe {
        cmd.pre_exec(|| {
            let mut c = [0u64; 3];
            if miku_capget(c.as_mut_ptr()) >= 0 {
                let keep = !CAP_MAY_PRIV;
                miku_capset(c[0] & keep, c[0] & keep, c[2] & keep);
            }
            Ok(())
        });
    }
}

#[cfg(not(target_os = "miku"))]
pub fn unprivileged(_cmd: &mut std::process::Command) {}

/// Evaluated in order: a file may use an earlier one's names at top level, and
/// any file's names inside a function
const PRELUDE: &[(&str, &str)] = &[
    // What a QML object is in JavaScript, and the scopes names are found in
    ("objects.js", include_str!("prelude/objects.js")),
    // Qt's `color` value type
    ("colour.js", include_str!("prelude/colour.js")),
    // Qt's `Locale`, and dates and numbers formatted with it
    ("locale.js", include_str!("prelude/locale.js")),
    // The `Qt` global
    ("qt.js", include_str!("prelude/qt.js")),
    // Type enums: `Text.AlignLeft`, `Easing.OutCubic`
    ("enums.js", include_str!("prelude/enums.js")),
    // The `Quickshell` global, and QtCore's and Quickshell.Io's singletons
    ("quickshell.js", include_str!("prelude/quickshell.js")),
    // A Canvas's "2d" context (`scene/canvas/`)
    ("canvas.js", include_str!("prelude/canvas.js")),
];

/// JavaScript wrapper of an object, created on first request
pub fn wrapper<'js>(ctx: &Ctx<'js>, core: &Core, oid: Oid) -> rquickjs::Result<rquickjs::Value<'js>> {
    let saved = core.0.borrow().objs.get(oid).and_then(|o| o.wrapper.clone());
    if let Some(p) = saved {
        return p.restore(ctx);
    }
    let make: Function = ctx.globals().get("__aosh_wrap")?;
    let w: rquickjs::Value = make.call((oid,))?;
    if let Some(o) = core.0.borrow_mut().objs.get_mut(oid) {
        o.wrapper = Some(Persistent::save(ctx, w.clone()));
    }
    Ok(w)
}

pub fn to_js<'js>(ctx: &Ctx<'js>, core: &Core, v: &Value) -> rquickjs::Result<rquickjs::Value<'js>> {
    Ok(match v {
        Value::Undefined => rquickjs::Value::new_undefined(ctx.clone()),
        Value::Null => rquickjs::Value::new_null(ctx.clone()),
        Value::Bool(b) => rquickjs::Value::new_bool(ctx.clone(), *b),
        Value::Num(n) => rquickjs::Value::new_number(ctx.clone(), *n),
        Value::Str(s) => rquickjs::String::from_str(ctx.clone(), s)?.into_value(),
        // A destroyed object is `null`, as a QObject property in Qt
        Value::Obj(o) if !core.0.borrow().objs.is_live(*o) => rquickjs::Value::new_null(ctx.clone()),
        Value::Obj(o) => wrapper(ctx, core, *o)?,
        Value::List(items) => {
            let a = rquickjs::Array::new(ctx.clone())?;
            for (i, it) in items.iter().enumerate() {
                a.set(i, to_js(ctx, core, it)?)?;
            }
            a.into_value()
        }
        Value::Js(p) => p.as_ref().clone().restore(ctx)?,
    })
}

/// Whether `v` is a shell wrapper (object or group) rather than a plain JS
/// value
pub(crate) fn is_wrapper(v: &rquickjs::Value<'_>) -> bool {
    v.as_object().is_some_and(|o| o.contains_key("__p").unwrap_or(false))
}

/// Object behind a wrapper, if `v` is one and not a group
pub(crate) fn wrapped_oid<'js>(v: &rquickjs::Value<'js>) -> Option<Oid> {
    let o = v.as_object()?;
    let p: rquickjs::Value = o.get("__p").ok()?;
    if p.as_string().and_then(|s| s.to_string().ok()).as_deref() != Some("") {
        return None;
    }
    let oid: rquickjs::Value = o.get("__oid").ok()?;
    oid.as_number().map(|n| n as Oid)
}

/// A JavaScript value into what a property of type `ty` holds
pub fn from_js_as<'js>(ctx: &Ctx<'js>, v: rquickjs::Value<'js>, ty: PropType) -> Value {
    match ty {
        PropType::Str | PropType::Color => {
            if v.is_undefined() || v.is_null() {
                return Value::str("");
            }
            if let Some(s) = v.as_string() {
                return Value::str(&s.to_string().unwrap_or_default());
            }
            match Coerced::<String>::from_js(ctx, v) {
                Ok(Coerced(s)) => Value::str(&s),
                Err(_) => Value::str(""),
            }
        }
        PropType::Int | PropType::Real => {
            if let Some(n) = v.as_number() {
                return Value::Num(n);
            }
            if v.is_undefined() || v.is_null() {
                return Value::Num(0.0);
            }
            match Coerced::<f64>::from_js(ctx, v) {
                Ok(Coerced(n)) => Value::Num(n),
                Err(_) => Value::Num(f64::NAN),
            }
        }
        PropType::Bool => match Coerced::<bool>::from_js(ctx, v) {
            Ok(Coerced(b)) => Value::Bool(b),
            Err(_) => Value::Bool(false),
        },
        PropType::Obj => match wrapped_oid(&v) {
            Some(o) => Value::Obj(o),
            None => Value::Null,
        },
        PropType::List => {
            if let Some(a) = v.as_array() {
                let items: Option<Vec<Value>> = a.iter::<rquickjs::Value>()
                    .map(|x| x.ok().and_then(|x| wrapped_oid(&x)).map(Value::Obj))
                    .collect();
                if let Some(items) = items {
                    return Value::List(std::rc::Rc::new(items));
                }
            }
            from_js(ctx, v)
        }
        PropType::Var => from_js(ctx, v),
    }
}

/// A JavaScript value as it is, for a `var`
pub fn from_js<'js>(ctx: &Ctx<'js>, v: rquickjs::Value<'js>) -> Value {
    if v.is_undefined() {
        return Value::Undefined;
    }
    if v.is_null() {
        return Value::Null;
    }
    if let Some(b) = v.as_bool() {
        return Value::Bool(b);
    }
    if let Some(n) = v.as_number() {
        return Value::Num(n);
    }
    if let Some(s) = v.as_string() {
        return Value::str(&s.to_string().unwrap_or_default());
    }
    if let Some(o) = wrapped_oid(&v) {
        return Value::Obj(o);
    }
    Value::Js(std::rc::Rc::new(Persistent::save(ctx, v)))
}

use rquickjs::FromJs;

/// Error as one line: the exception message or the failure to retrieve it
pub fn describe(ctx: &Ctx<'_>, e: rquickjs::Error) -> String {
    match e {
        rquickjs::Error::Exception => {
            let thrown = ctx.catch();
            if let Some(o) = thrown.as_object() {
                let name: Option<String> = o.get::<_, Coerced<String>>("name").ok().map(|c| c.0);
                let msg: Option<String> = o.get::<_, Coerced<String>>("message").ok().map(|c| c.0);
                if let (Some(n), Some(m)) = (name, msg) {
                    if n != "undefined" {
                        return format!("{n}: {m}");
                    }
                }
            }
            match Coerced::<String>::from_js(ctx, thrown) {
                Ok(Coerced(s)) => s,
                Err(_) => "an exception that would not say what it was".into(),
            }
        }
        other => other.to_string(),
    }
}

/// Factory source taking the id scope and root, returning a function with body
/// `body`.
/// Lookup order: id scope, object, file root, enclosing scopes (`__outer`,
/// `new_context` in `build.rs`)
fn factory_source(params: &[String], body: &str) -> String {
    if params.is_empty() {
        format!("(function(__ids, __root, __outer) {{ return function() {{ with (__outer) {{ with (__root) {{ \
                 with (this) {{ with (__ids) {{ {body}\n}} }} }} }} }}; }})")
    } else {
        let p = params.join(", ");
        format!(
            "(function(__ids, __root, __outer) {{ return function({p}) {{ with (__outer) {{ with (__root) {{ \
             with (this) {{ with (__ids) {{ return (function({p}) {{ {body}\n}}).apply(this, arguments); \
             }} }} }} }} }}; }})"
        )
    }
}

/// Compiles `body` once per `key` (a file location) and creates the function
/// for one id scope and root
pub fn compile<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    key: &str,
    params: &[String],
    body: &str,
    cx: u32,
) -> rquickjs::Result<Persistent<Function<'static>>> {
    let cached = core.0.borrow().factories.get(key).cloned();
    let factory = match cached {
        Some(f) => f.restore(ctx)?,
        None => {
            // Sloppy mode: strict mode forbids the `with` that scopes QML names
            let mut opts = rquickjs::context::EvalOptions::default();
            opts.strict = false;
            let f: Function = ctx.eval_with_options(factory_source(params, body), opts)?;
            // Empty key skips the cache, so one-off expressions
            // (`Engine::eval`) do not stay compiled
            if !key.is_empty() {
                core.0.borrow_mut().factories.insert(key.to_string(), Persistent::save(ctx, f.clone()));
            }
            f
        }
    };
    let (ids, root, outer) = {
        let s = core.0.borrow();
        let c = &s.cxs[cx as usize];
        (c.ids_js.clone(), c.root.filter(|&r| s.objs.is_live(r)), c.outer_js.clone())
    };
    let ids = ids.restore(ctx)?;
    // `with (null)` throws, so a missing scope is an empty object
    let none: rquickjs::Value = ctx.globals().get("__aosh_none")?;
    let root = match root {
        Some(r) => wrapper(ctx, core, r)?,
        None => none.clone(),
    };
    let outer = outer.restore(ctx)?;
    let outer = if outer.is_null() { none } else { outer };
    let func: Function = factory.call((ids, root, outer))?;
    Ok(Persistent::save(ctx, func))
}

/// Whether `body` compiles as a function body with these parameters, for
/// `aosh check`
pub fn check_syntax(ctx: &Ctx<'_>, params: &[String], body: &str) -> Result<(), String> {
    let mut opts = rquickjs::context::EvalOptions::default();
    opts.strict = false;
    ctx.eval_with_options::<Function, _>(factory_source(params, body), opts)
        .map(|_| ())
        .map_err(|e| describe(ctx, e))
}

/// Calls a compiled function with `this` and no arguments
pub fn call0<'js>(ctx: &Ctx<'js>, core: &Core, func: &Persistent<Function<'static>>, this: Oid)
    -> rquickjs::Result<rquickjs::Value<'js>>
{
    let f = func.clone().restore(ctx)?;
    let this = wrapper(ctx, core, this)?;
    f.call((This(this),))
}

/// Runs everything `Qt.callLater` queued until the queue is empty
pub fn run_later(ctx: &Ctx<'_>, core: &Core) {
    for _ in 0..64 {
        let queue: rquickjs::Result<rquickjs::Array> = ctx.globals().get("__aosh_later");
        let Ok(queue) = queue else { return };
        if queue.len() == 0 {
            return;
        }
        let _ = ctx.globals().set("__aosh_later", rquickjs::Array::new(ctx.clone()));
        for f in queue.iter::<Function>().flatten() {
            if let Err(e) = f.call::<_, rquickjs::Value>(()) {
                core.log(format!("Qt.callLater: {}", describe(ctx, e)));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Js;

    #[test]
    fn evaluates_and_stringifies() {
        let js = Js::new().unwrap();
        assert_eq!(js.eval("[1,2,3].map(x => x*2).join()").unwrap(), "2,4,6");
        assert_eq!(js.eval("JSON.stringify({a: 1})").unwrap(), r#"{"a":1}"#);
        assert_eq!(js.eval("Math.round(2.5) + Math.floor(-0.5)").unwrap(), "2");
    }

    #[test]
    fn an_exception_is_its_message() {
        let js = Js::new().unwrap();
        let err = js.eval("throw new Error('no')").unwrap_err();
        assert!(err.contains("no"), "{err}");
    }
}
