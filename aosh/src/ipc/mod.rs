// IpcHandler: functions a shell file exposes externally (quickshell's
// `IpcHandler` and `qs ipc`)
//
// A call names a target, a function, and string arguments, which are converted
// to the declared types (`int`, `real`, `bool`, `string`, `color`). The return
// value comes back as text.
//
// Entry points: a compositor keybind word (`signal, launcher toggle`) and
// `aosh ipc call` over a local socket (`socket.rs`).

pub mod socket;

use std::collections::HashMap;

use rquickjs::function::{Rest, This};
use rquickjs::{Ctx, Function};

use crate::object::value::Oid;
use crate::object::Core;
use crate::types::Kind;

/// A function's declared signature, used to check and convert calls
#[derive(Clone, Debug)]
pub struct Sig {
    pub name: String,
    pub params: Vec<String>,
    /// `""` for an untyped parameter, which stays a string
    pub types: Vec<String>,
    pub ret: String,
}

#[derive(Default)]
pub struct Ipc {
    sigs: HashMap<Oid, Vec<Sig>>,
    /// Where `aosh ipc` reaches this shell, while it listens
    pub server: Option<socket::Server>,
}

/// Registers an IpcHandler function (`object/build.rs`)
pub fn declared(core: &Core, oid: Oid, sig: Sig) {
    core.0.borrow_mut().ipc.sigs.entry(oid).or_default().push(sig);
}

pub fn forget(core: &Core, oid: Oid) {
    core.0.borrow_mut().ipc.sigs.remove(&oid);
}

/// Enabled handler for `target`; the most recent one during a reload
fn handler(core: &Core, target: &str) -> Option<Oid> {
    let ids = core.0.borrow().objs.ids();
    ids.into_iter().rev()
        .filter(|&o| core.0.borrow().objs[o].def.kind == Kind::Ipc)
        .find(|&o| core.flag(o, "enabled") && core.text(o, "target") == target)
}

/// `target.function(args...)`, with arguments converted to their declared types
/// and the result as text
pub fn call(ctx: &Ctx<'_>, core: &Core, target: &str, function: &str, args: &[String]) -> Result<String, String> {
    if target == "aosh" && handler(core, target).is_none() {
        return own(core, function, args);
    }
    let h = handler(core, target).ok_or_else(|| format!("no target '{target}'"))?;
    let sig = core.0.borrow().ipc.sigs.get(&h)
        .and_then(|v| v.iter().find(|s| s.name == function).cloned())
        .ok_or_else(|| format!("target '{target}' has no function '{function}'"))?;
    if args.len() > sig.params.len() {
        return Err(format!("{function} takes {} argument(s), not {}", sig.params.len(), args.len()));
    }
    let mut js = Vec::new();
    for (i, a) in args.iter().enumerate() {
        js.push(convert(ctx, a, sig.types.get(i).map(String::as_str).unwrap_or(""), &sig.params[i])?);
    }
    let describe = |e| crate::js::describe(ctx, e);
    let w = crate::js::wrapper(ctx, core, h).map_err(describe)?;
    let f: Function = w.as_object().ok_or("the handler is not an object")?.get(function).map_err(describe)?;
    let r: rquickjs::Value = f.call((This(w), Rest(js))).map_err(describe)?;
    Ok(text_of(ctx, &r))
}

/// Built-in `aosh` target: `render cpu|gpu` selects the renderer, bare `render`
/// reports it
fn own(core: &Core, function: &str, args: &[String]) -> Result<String, String> {
    match (function, args.first().map(String::as_str)) {
        ("render", Some("cpu")) => {
            core.0.borrow_mut().render_cpu = true;
            core.0.borrow_mut().dirty = true;
            Ok("cpu".into())
        }
        ("render", Some("gpu")) => {
            core.0.borrow_mut().render_cpu = false;
            core.0.borrow_mut().dirty = true;
            Ok("gpu".into())
        }
        ("render", None) => Ok(if core.0.borrow().render_cpu { "cpu" } else { "gpu" }.into()),
        ("render", Some(other)) => Err(format!("render {other}: cpu or gpu")),
        _ => Err(format!("target 'aosh' has no function '{function}' - it has render cpu|gpu")),
    }
}

/// Compositor word as a call: `launcher toggle` is `launcher.toggle()`. False
/// if no handler takes the target
pub fn word(ctx: &Ctx<'_>, core: &Core, word: &str) -> bool {
    let parts: Vec<String> = word.split_whitespace().map(str::to_string).collect();
    let [target, function, rest @ ..] = parts.as_slice() else { return false };
    if handler(core, target).is_none() {
        return false;
    }
    if let Err(e) = call(ctx, core, target, function, rest) {
        core.log(format!("signal '{word}': {e}"));
    }
    true
}

/// Listens at `socket::path`. Only the shell does this, not `aosh run` or tests
pub fn listen(core: &Core, config: Option<&str>) {
    let Some(path) = socket::path(config) else { return };
    match socket::Server::open(&path) {
        Ok(s) => core.0.borrow_mut().ipc.server = Some(s),
        Err(e) => core.log(format!("aosh ipc will not reach this shell: {}: {e}", path.display())),
    }
}

/// Answers every waiting request
pub fn serve(ctx: &Ctx<'_>, core: &Core) {
    let waiting = {
        let s = core.0.borrow();
        s.ipc.server.as_ref().map(|x| x.take()).unwrap_or_default()
    };
    for (stream, req) in waiting {
        let r = req.and_then(|f| match f.first().map(String::as_str) {
            Some("call") if f.len() >= 3 => call(ctx, core, &f[1], &f[2], &f[3..]),
            Some("show") => Ok(show(core)),
            _ => Err("that is not a request aosh knows".into()),
        });
        socket::answer(stream, r);
    }
}

/// Output of `aosh ipc show`: each target and its functions
pub fn show(core: &Core) -> String {
    let ids = core.0.borrow().objs.ids();
    let mut out = String::new();
    for o in ids {
        if core.0.borrow().objs[o].def.kind != Kind::Ipc || !core.flag(o, "enabled") {
            continue;
        }
        out.push_str(&format!("target {}\n", core.text(o, "target")));
        for s in core.0.borrow().ipc.sigs.get(&o).into_iter().flatten() {
            let params: Vec<String> = s.params.iter().zip(&s.types)
                .map(|(p, t)| if t.is_empty() { p.clone() } else { format!("{p}: {t}") })
                .collect();
            let ret = if s.ret.is_empty() { "void" } else { &s.ret };
            out.push_str(&format!("  function {}({}): {ret}\n", s.name, params.join(", ")));
        }
    }
    out
}

/// Converts an argument to its declared type, as `qs ipc` does
fn convert<'js>(ctx: &Ctx<'js>, arg: &str, ty: &str, name: &str) -> Result<rquickjs::Value<'js>, String> {
    let bad = |what: &str| format!("{name}: '{arg}' is not {what}");
    Ok(match ty {
        "int" => {
            let n: i64 = arg.trim().parse().map_err(|_| bad("an int"))?;
            rquickjs::Value::new_number(ctx.clone(), n as f64)
        }
        "real" | "double" | "number" => {
            let n: f64 = arg.trim().parse().map_err(|_| bad("a number"))?;
            rquickjs::Value::new_number(ctx.clone(), n)
        }
        "bool" => {
            let b = match arg.trim() {
                "true" => true,
                "false" => false,
                n => n.parse::<i64>().map(|n| n != 0).map_err(|_| bad("true, false or a number"))?,
            };
            rquickjs::Value::new_bool(ctx.clone(), b)
        }
        _ => rquickjs::String::from_str(ctx.clone(), arg).map_err(|e| e.to_string())?.into_value(),
    })
}

/// Return value as `qs ipc` prints it: nothing for void
fn text_of<'js>(ctx: &Ctx<'js>, v: &rquickjs::Value<'js>) -> String {
    if v.is_undefined() || v.is_null() {
        return String::new();
    }
    if let Some(b) = v.as_bool() {
        return b.to_string();
    }
    if let Some(n) = v.as_number() {
        return if n.fract() == 0.0 && n.abs() < 1e15 { format!("{}", n as i64) } else { n.to_string() };
    }
    if v.is_object() && !v.is_function() {
        if let Ok(Some(s)) = ctx.json_stringify(v.clone()) {
            return s.to_string().unwrap_or_default();
        }
    }
    rquickjs::Coerced::<String>::from_js(ctx, v.clone()).map(|c| c.0).unwrap_or_default()
}

use rquickjs::FromJs;

#[cfg(test)]
mod tests;
