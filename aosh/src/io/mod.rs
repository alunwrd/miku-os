// External I/O integration: Process pipes and the inotify descriptor are polled
// non-blockingly via the shell loop (`fds`). Data is read only when ready,
// keeping
// the event loop unblocked. Exits are handled per-iteration by `reap`

mod fileview;
mod json;
pub mod kdeglobals;
mod process;
pub mod xdg;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rquickjs::Ctx;

use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

pub use process::Proc;

/// Represents a polled file descriptor in the event loop
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fd {
    /// Standard output (`false`) or standard error (`true`)
    Pipe(Oid, bool),
    Inotify,
    /// The IPC socket used by `aosh ipc` (see `ipc/`)
    Ipc,
}

/// The single inotify descriptor monitoring both FileViews and reload
/// directories
#[derive(Default)]
pub struct Io {
    inotify: Option<i32>,
    /// Watch descriptor to directory
    dirs: HashMap<i32, PathBuf>,
    /// Active FileViews with `watchChanges` enabled, keyed by target file
    watched: HashMap<Oid, PathBuf>,
    /// Configuration directories; changes in any directory trigger a reload
    pub config_dirs: Vec<PathBuf>,
    /// Scheduled reload time. Editor saves emit multiple events, so the reload
    /// waits for the final one
    pub reload_due: Option<u64>,
    /// The user configuration directory when it doesn't yet exist
    /// Watches the nearest existing parent directory until this directory is
    /// created
    user: Option<PathBuf>,
    /// Supplies file contents to JsonAdapters. Updates triggered here originate
    /// from
    /// the file itself and suppress `adapterUpdated` signals (see `json.rs`)
    json_loading: Vec<Oid>,
}

impl Drop for Io {
    fn drop(&mut self) {
        if let Some(fd) = self.inotify.take() {
            // SAFETY: owned descriptor, closed once
            unsafe { libc::close(fd) };
        }
    }
}

impl Io {
    /// Watch `dir`, once
    fn watch_dir(&mut self, dir: &Path) {
        if self.dirs.values().any(|d| d == dir) {
            return;
        }
        let fd = match self.inotify {
            Some(fd) => fd,
            None => {
                // SAFETY: no arguments to validate
                let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
                if fd < 0 {
                    return;
                }
                self.inotify = Some(fd);
                fd
            }
        };
        let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()) else {
            return;
        };
        let mask = libc::IN_CLOSE_WRITE
            | libc::IN_MODIFY
            | libc::IN_CREATE
            | libc::IN_DELETE
            | libc::IN_MOVED_TO
            | libc::IN_MOVED_FROM;
        // SAFETY: valid descriptor and NUL-terminated path
        let wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), mask) };
        if wd >= 0 {
            self.dirs.insert(wd, dir.to_path_buf());
        }
    }

    /// Watches the user directory directly if it exists (`true`), or its
    /// nearest existing parent otherwise
    fn follow_user(&mut self) -> bool {
        let Some(dir) = self.user.clone() else {
            return false;
        };
        if dir.is_dir() {
            self.watch_dir(&dir);
            if !self.config_dirs.contains(&dir) {
                self.config_dirs.push(dir);
            }
            self.user = None;
            return true;
        }
        if let Some(up) = dir.ancestors().skip(1).find(|d| d.is_dir()) {
            self.watch_dir(up);
        }
        false
    }

    /// The paths that changed since the last read
    fn changed(&mut self) -> Vec<PathBuf> {
        let Some(fd) = self.inotify else {
            return Vec::new();
        };
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            // SAFETY: reads into an owned buffer
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let mut at = 0usize;
            let head = std::mem::size_of::<libc::inotify_event>();
            while at + head <= n as usize {
                // SAFETY: the kernel writes whole records; read unaligned
                let ev: libc::inotify_event =
                    unsafe { std::ptr::read_unaligned(buf[at..].as_ptr().cast()) };
                let name_bytes = &buf[at + head..(at + head + ev.len as usize).min(n as usize)];
                let name = name_bytes.split(|&b| b == 0).next().unwrap_or(&[]);
                if let Some(dir) = self.dirs.get(&ev.wd) {
                    let p = if name.is_empty() {
                        dir.clone()
                    } else {
                        dir.join(String::from_utf8_lossy(name).as_ref())
                    };
                    if !out.contains(&p) {
                        out.push(p);
                    }
                }
                at += head + ev.len as usize;
            }
        }
        out
    }
}

// Refuses to start programs outside MikuOS and tests unless AOSH_HOST_EXEC=1 is
// set
pub fn may_start(core: &Core, argv: &[String]) -> bool {
    if cfg!(target_os = "miku")
        || cfg!(test)
        || std::env::var_os("AOSH_HOST_EXEC").is_some_and(|v| v == "1")
    {
        return true;
    }
    core.log(format!(
        "not started on this host (AOSH_HOST_EXEC=1 would): {}",
        argv.join(" ")
    ));
    false
}

/// Outside MikuOS, restricts file writes to `$HOME`
pub fn may_write(core: &Core, path: &Path) -> bool {
    if cfg!(target_os = "miku")
        || cfg!(test)
        || std::env::var_os("AOSH_HOST_EXEC").is_some_and(|v| v == "1")
    {
        return true;
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    if home
        .as_deref()
        .is_some_and(|h| h != Path::new("/") && path.starts_with(h))
    {
        return true;
    }
    core.log(format!(
        "not written on this host (outside $HOME): {}",
        path.display()
    ));
    false
}

pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let kind = core.0.borrow().objs[oid].def.kind;
    match kind {
        Kind::Process => process::completed(ctx, core, oid),
        Kind::FileView => fileview::completed(ctx, core, oid),
        Kind::FontLoader => font(ctx, core, oid),
        _ => {}
    }
}

/// Font family and the backing file providing its typefaces for the scene
fn font(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let src = core.text(oid, "source");
    let src = src.strip_prefix("file://").unwrap_or(&src);
    if src.is_empty() {
        core.set_named(ctx, oid, "status", Value::Num(0.0));
        return;
    }
    let path = core.mapped(PathBuf::from(src));
    let names = std::fs::read(&path)
        .ok()
        .and_then(|b| crate::scene::text::index::names(&b));
    match names {
        Some(n) => {
            core.set_named(ctx, oid, "name", Value::str(&n.family));
            core.set_named(ctx, oid, "status", Value::Num(1.0));
            let mut s = core.0.borrow_mut();
            if !s.font_files.contains(&path) {
                s.font_files.push(path);
            }
        }
        None => {
            core.log(format!(
                "FontLoader {}: not a font that can be read",
                path.display()
            ));
            core.set_named(ctx, oid, "status", Value::Num(3.0));
        }
    }
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    if !core.0.borrow().objs[oid].complete {
        return;
    }
    match kind {
        Kind::Process => process::hook(ctx, core, oid, name),
        Kind::FileView => fileview::hook(ctx, core, oid, name),
        Kind::Json => json::changed(ctx, core, oid, name),
        Kind::FontLoader if name == "source" => font(ctx, core, oid),
        _ => {}
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
    match kind {
        Kind::Process => process::call(ctx, core, oid, name, args),
        Kind::FileView => fileview::call(ctx, core, oid, name, args),
        _ => None,
    }
}

pub fn forget(core: &Core, oid: Oid) {
    let p = core.0.borrow_mut().procs.remove(&oid);
    if let Some(p) = p {
        // Killed when the owning object is destroyed
        // The event loop reaps the child process just like a detached one
        let child = p.terminate();
        core.0.borrow_mut().children.push(child);
    }
    core.0.borrow_mut().io.watched.remove(&oid);
}

/// Descriptors registered with the poll loop alongside their respective event
/// types or roles
pub fn fds(core: &Core) -> Vec<(i32, Fd)> {
    let s = core.0.borrow();
    let mut out: Vec<(i32, Fd)> = s.procs.iter().flat_map(|(&o, p)| p.fds(o)).collect();
    if let Some(fd) = s.io.inotify {
        out.push((fd, Fd::Inotify));
    }
    if let Some(srv) = s.ipc.server.as_ref() {
        out.push((srv.fd(), Fd::Ipc));
    }
    out
}

// `fd` has data ready to read
pub fn ready(ctx: &Ctx<'_>, core: &Core, fd: Fd) {
    match fd {
        Fd::Pipe(oid, err) => process::read(ctx, core, oid, err),
        Fd::Ipc => crate::ipc::serve(ctx, core),
        Fd::Inotify => {
            let changed = core.0.borrow_mut().io.changed();
            for path in changed {
                let views: Vec<Oid> = core
                    .0
                    .borrow()
                    .io
                    .watched
                    .iter()
                    .filter(|(_, p)| **p == path)
                    .map(|(&o, _)| o)
                    .collect();
                for v in views {
                    fileview::changed(ctx, core, v);
                }
                let is_config = path.extension().is_some_and(|e| e == "qml")
                    && path
                        .parent()
                        .is_some_and(|d| core.0.borrow().io.config_dirs.iter().any(|c| c == d));
                if is_config {
                    let now = core.0.borrow().now_ms;
                    core.0.borrow_mut().io.reload_due = Some(now + 100);
                }
            }
            // User directory created either populated via `cp -r` or empty via
            // `mkdir`
            // Triggers a reload either way so the shell re-evaluates its target
            // file
            let mut s = core.0.borrow_mut();
            if s.io.user.is_some() && s.io.follow_user() {
                s.io.reload_due = Some(s.now_ms + 100);
            }
        }
    }
}

/// Exited processes whose output streams have closed and exit status has been
/// emitted
pub fn reap(ctx: &Ctx<'_>, core: &Core) {
    process::reap(ctx, core);
}

/// Monitored config directories: saving any `.qml` file triggers a reload
pub fn watch_config(core: &Core, dirs: &[PathBuf]) {
    let mut s = core.0.borrow_mut();
    for d in dirs {
        s.io.watch_dir(d);
        if !s.io.config_dirs.contains(d) {
            s.io.config_dirs.push(d.clone());
        }
    }
}

/// Monitored user configuration directory: reloads upon creation and on
/// subsequent `.qml` saves
pub fn watch_user(core: &Core, dir: PathBuf) {
    let mut s = core.0.borrow_mut();
    s.io.user = Some(dir);
    s.io.follow_user();
}

/// Reads a JS property value as a list of strings (e.g. a Process's `command`)
fn strings(ctx: &Ctx<'_>, v: &Value) -> Vec<String> {
    match v {
        Value::List(items) => items.iter().map(|i| i.to_text()).collect(),
        Value::Js(p) => {
            let Ok(v) = p.as_ref().clone().restore(ctx) else {
                return Vec::new();
            };
            match v.as_array() {
                Some(a) => a
                    .iter::<rquickjs::Value>()
                    .flatten()
                    .map(|e| {
                        rquickjs::Coerced::<String>::from_js(ctx, e)
                            .map(|c| c.0)
                            .unwrap_or_default()
                    })
                    .collect(),
                None => Vec::new(),
            }
        }
        Value::Str(s) if !s.is_empty() => vec![s.to_string()],
        _ => Vec::new(),
    }
}

use rquickjs::FromJs;

#[cfg(test)]
mod tests;
