// FileView: Exposes file contents as a reactive property.
//
// Reads occur on path change, explicit reload(), or via directory inotify
// (which handles atomic editor renames safely). Errors map to FileViewError.
//
// If configured, `adapter` processes new text before the `loaded` signal fires;
// `writeAdapter()` serializes adapter state back to disk.
//
// All I/O is synchronous (intended for small configs and state).

use std::path::PathBuf;

use rquickjs::Ctx;

use crate::object::value::{Oid, Value};
use crate::object::Core;

const NOT_FOUND: f64 = 2.0;
const PERMISSION: f64 = 3.0;
const NOT_A_FILE: f64 = 4.0;
const UNKNOWN: f64 = 1.0;

pub(super) fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    load(ctx, core, oid);
    watch(core, oid);
}

pub(super) fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str) {
    match name {
        "path" => {
            load(ctx, core, oid);
            watch(core, oid);
        }
        "watchChanges" => watch(core, oid),
        _ => {}
    }
}

/// The file changed on disk
pub(super) fn changed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    load(ctx, core, oid);
    core.emit(ctx, oid, "fileChanged", Vec::new());
}

fn path_of(core: &Core, oid: Oid) -> Option<PathBuf> {
    let p = core.text(oid, "path");
    let p = p.strip_prefix("file://").unwrap_or(&p);
    (!p.is_empty()).then(|| core.mapped(PathBuf::from(p)))
}

fn load(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let Some(path) = path_of(core, oid) else {
        core.set_layout_value(ctx, oid, "__text", Value::str(""));
        core.set_layout_value(ctx, oid, "loaded", Value::Bool(false));
        return;
    };
    match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8_lossy(&bytes).into_owned();
            let changed = text != core.text(oid, "__text");
            core.set_layout_value(ctx, oid, "__text", Value::str(&text));
            core.set_layout_value(ctx, oid, "loaded", Value::Bool(true));
            if changed {
                if let Some(a) = adapter(core, oid) {
                    super::json::load(ctx, core, a, &text);
                }
            }
            core.emit(ctx, oid, "loaded", Vec::new());
        }
        Err(e) => {
            let code = match e.kind() {
                std::io::ErrorKind::NotFound => NOT_FOUND,
                std::io::ErrorKind::PermissionDenied => PERMISSION,
                _ if path.is_dir() => NOT_A_FILE,
                _ => UNKNOWN,
            };
            if core.flag(oid, "printErrors") {
                core.log(format!("FileView {}: {e}", path.display()));
            }
            core.set_layout_value(ctx, oid, "__text", Value::str(""));
            core.set_layout_value(ctx, oid, "loaded", Value::Bool(false));
            core.emit(
                ctx,
                oid,
                "loadFailed",
                vec![rquickjs::Value::new_number(ctx.clone(), code)],
            );
        }
    }
}

fn watch(core: &Core, oid: Oid) {
    let path = path_of(core, oid).filter(|_| core.flag(oid, "watchChanges"));
    let mut s = core.0.borrow_mut();
    match path {
        Some(p) => {
            if let Some(dir) = p.parent() {
                s.io.watch_dir(dir);
            }
            s.io.watched.insert(oid, p);
        }
        None => {
            s.io.watched.remove(&oid);
        }
    }
}

pub(super) fn call<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    oid: Oid,
    name: &str,
    args: &[rquickjs::Value<'js>],
) -> Option<Value> {
    match name {
        // Binding to view.text() automatically tracks file changes
        "text" | "data" => {
            let i = core.prop_idx(oid, "__text")?;
            return Some(core.read(oid, i));
        }
        "reload" => load(ctx, core, oid),
        "waitForJob" => return Some(Value::Bool(true)),
        "setText" | "setData" => {
            let text = args
                .first()
                .map(|v| {
                    rquickjs::Coerced::<String>::from_js(ctx, v.clone())
                        .map(|c| c.0)
                        .unwrap_or_default()
                })
                .unwrap_or_default();
            save(ctx, core, oid, &text);
        }
        "writeAdapter" => match adapter(core, oid).and_then(|a| super::json::save(ctx, core, a)) {
            Some(text) => save(ctx, core, oid, &text),
            None => core.log(format!(
                "FileView {}: writeAdapter() with no adapter",
                path_of(core, oid)
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )),
        },
        _ => return None,
    }
    Some(Value::Undefined)
}

/// The JsonAdapter the FileView holds
fn adapter(core: &Core, oid: Oid) -> Option<Oid> {
    core.peek_named(oid, "adapter")
        .and_then(|v| v.as_obj())
        .filter(|&a| core.0.borrow().objs.is_live(a))
}

/// Writes `text` to the file. Skips the write if contents are unchanged
fn save(ctx: &Ctx<'_>, core: &Core, oid: Oid, text: &str) {
    let Some(path) = path_of(core, oid) else {
        return;
    };
    if core.flag(oid, "loaded") && text == core.text(oid, "__text") {
        return;
    }
    if !super::may_write(core, &path) {
        return;
    }
    match write(&path, text.as_bytes(), core.flag(oid, "atomicWrites")) {
        Ok(()) => {
            core.set_layout_value(ctx, oid, "__text", Value::str(text));
            core.set_layout_value(ctx, oid, "loaded", Value::Bool(true));
            core.emit(ctx, oid, "saved", Vec::new());
        }
        Err(e) => {
            core.log(format!("FileView {}: {e}", path.display()));
            let code = if e.kind() == std::io::ErrorKind::PermissionDenied {
                PERMISSION
            } else {
                UNKNOWN
            };
            core.emit(
                ctx,
                oid,
                "saveFailed",
                vec![rquickjs::Value::new_number(ctx.clone(), code)],
            );
        }
    }
}

/// Writes atomically via a temporary file replacement if `atomic` is enabled,
/// preventing readers from seeing partially written data
fn write(path: &std::path::Path, bytes: &[u8], atomic: bool) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if !atomic {
        return std::fs::write(path, bytes);
    }
    let tmp = path.with_extension("aosh-tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

use rquickjs::FromJs;
