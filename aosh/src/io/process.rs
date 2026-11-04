// Manages a process and its I/O
//
// `running` acts as both the desired state and the actual state. Toggling it
// (`running = false; running = true`) safely awaits the old instance's exit
// before restarting. On exit, parsers drain first, `exited(...)` fires, and
// `running` becomes false (unless a restart was requested in the meantime)
//
// Spawned directly (no shell, unprivileged). I/O is non-blocking via poll

use std::io::{Read, Write};
use std::os::unix::io::AsRawFd;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};

use rquickjs::Ctx;

use super::{strings, Fd};
use crate::object::value::{Oid, Value};
use crate::object::Core;
use crate::types::Kind;

pub struct Proc {
    /// Requested while an older instance was still exiting
    target: bool,
    live: Option<Live>,
}

struct Live {
    child: Child,
    /// Pipes until EOF. Closed ones leave the poll set, otherwise `POLLHUP`
    /// wakes the loop on every pass
    out: Option<ChildStdout>,
    err: Option<ChildStderr>,
    out_parser: Option<Oid>,
    err_parser: Option<Oid>,
    stdin: Option<ChildStdin>,
    out_buf: Vec<u8>,
    err_buf: Vec<u8>,
}

impl Proc {
    pub(super) fn fds(&self, oid: Oid) -> Vec<(i32, Fd)> {
        let Some(l) = &self.live else {
            return Vec::new();
        };
        let mut v = Vec::new();
        if let Some(o) = &l.out {
            v.push((o.as_raw_fd(), Fd::Pipe(oid, false)));
        }
        if let Some(e) = &l.err {
            v.push((e.as_raw_fd(), Fd::Pipe(oid, true)));
        }
        v
    }

    /// `SIGTERM`, and the child to reap
    pub(super) fn terminate(self) -> Child {
        let mut l = self.live.expect("only a live process is terminated");
        // SAFETY: signal to an owned child
        unsafe { libc::kill(l.child.id() as i32, libc::SIGTERM) };
        drop(l.stdin.take());
        l.child
    }

    pub fn is_live(&self) -> bool {
        self.live.is_some()
    }

    /// Kills the child at shell exit, since nothing will reap it
    pub fn kill(self) {
        if let Some(mut l) = self.live {
            let _ = l.child.kill();
            let _ = l.child.wait();
        }
    }
}

fn nonblocking(fd: i32) {
    // SAFETY: fcntl on an owned descriptor
    unsafe {
        let fl = libc::fcntl(fd, libc::F_GETFL);
        if fl >= 0 {
            libc::fcntl(fd, libc::F_SETFL, fl | libc::O_NONBLOCK);
        }
    }
}

pub(super) fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    core.0.borrow_mut().procs.entry(oid).or_insert(Proc {
        target: false,
        live: None,
    });
    if core.flag(oid, "running") {
        want(ctx, core, oid);
    }
}

pub(super) fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, name: &str) {
    match name {
        "running" if core.flag(oid, "running") => want(ctx, core, oid),
        "running" => {
            let mut s = core.0.borrow_mut();
            if let Some(p) = s.procs.get_mut(&oid) {
                p.target = false;
                if let Some(l) = &p.live {
                    // SAFETY: signal to an owned child
                    unsafe { libc::kill(l.child.id() as i32, libc::SIGTERM) };
                }
            }
        }
        // Starts a process that had `running: true` before its command was set
        "command" => start_if_ready(ctx, core, oid),
        _ => {}
    }
}

fn want(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    if let Some(p) = core.0.borrow_mut().procs.get_mut(&oid) {
        p.target = true;
    }
    start_if_ready(ctx, core, oid);
}

fn start_if_ready(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let ready = core
        .0
        .borrow()
        .procs
        .get(&oid)
        .is_some_and(|p| p.live.is_none() && p.target);
    if !ready {
        return;
    }
    let argv = strings(
        ctx,
        &core.peek_named(oid, "command").unwrap_or(Value::Undefined),
    );
    if argv.is_empty() {
        return;
    }
    if let Some(p) = core.0.borrow_mut().procs.get_mut(&oid) {
        p.target = false;
    }
    let parser = |name: &str| -> Option<Oid> {
        core.peek_named(oid, name)
            .and_then(|v| v.as_obj())
            .filter(|&o| {
                core.0
                    .borrow()
                    .objs
                    .get(o)
                    .is_some_and(|x| matches!(x.def.kind, Kind::Collector | Kind::Splitter))
            })
    };
    let (out_parser, err_parser) = (parser("stdout"), parser("stderr"));
    if !super::may_start(core, &argv) {
        if let Some(i) = core.prop_idx(oid, "running") {
            core.store_raw(ctx, oid, i, Value::Bool(false));
        }
        return;
    }
    let mut cmd = command(ctx, core, oid, &argv);
    cmd.stdin(if core.flag(oid, "stdinEnabled") {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stdout(if out_parser.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    })
    .stderr(if err_parser.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    match cmd.spawn() {
        Ok(mut child) => {
            let out = child.stdout.take();
            let err = child.stderr.take();
            if let Some(o) = &out {
                nonblocking(o.as_raw_fd());
            }
            if let Some(e) = &err {
                nonblocking(e.as_raw_fd());
            }
            let stdin = child.stdin.take();
            let pid = child.id();
            for p in [out_parser, err_parser].into_iter().flatten() {
                reset_parser(ctx, core, p);
            }
            if let Some(p) = core.0.borrow_mut().procs.get_mut(&oid) {
                p.live = Some(Live {
                    child,
                    out,
                    err,
                    out_parser,
                    err_parser,
                    stdin,
                    out_buf: Vec::new(),
                    err_buf: Vec::new(),
                });
            }
            core.set_layout_value(ctx, oid, "processId", Value::Num(pid as f64));
            if !core.flag(oid, "running") {
                if let Some(i) = core.prop_idx(oid, "running") {
                    core.store_raw(ctx, oid, i, Value::Bool(true));
                }
            }
            core.emit(ctx, oid, "started", Vec::new());
        }
        Err(e) => {
            core.log(format!("Process {}: {e}", argv[0]));
            if let Some(i) = core.prop_idx(oid, "running") {
                core.store_raw(ctx, oid, i, Value::Bool(false));
            }
        }
    }
}

/// Builds the program from the properties: directory, environment, and dropped
/// desktop privileges
fn command(ctx: &Ctx<'_>, core: &Core, oid: Oid, argv: &[String]) -> Command {
    let mut cmd = Command::new(argv[0].strip_prefix("file://").unwrap_or(&argv[0]));
    cmd.args(&argv[1..]);
    let dir = core.text(oid, "workingDirectory");
    if !dir.is_empty() {
        cmd.current_dir(dir.strip_prefix("file://").unwrap_or(&dir));
    }
    let clear = core.flag(oid, "clearEnvironment");
    if clear {
        cmd.env_clear();
    }
    if let Some(Value::Js(p)) = core.peek_named(oid, "environment") {
        if let Some(o) = p
            .as_ref()
            .clone()
            .restore(ctx)
            .ok()
            .and_then(|v| v.into_object())
        {
            for (k, v) in o.props::<String, rquickjs::Value>().flatten() {
                if v.is_null() || v.is_undefined() {
                    // Removed, or inherited from aosh when the environment is
                    // cleared
                    if clear {
                        if let Ok(sys) = std::env::var(&k) {
                            cmd.env(&k, sys);
                        }
                    } else {
                        cmd.env_remove(&k);
                    }
                } else {
                    let s = rquickjs::Coerced::<String>::from_js(ctx, v)
                        .map(|c| c.0)
                        .unwrap_or_default();
                    cmd.env(&k, s);
                }
            }
        }
    }
    crate::js::unprivileged(&mut cmd);
    cmd
}

fn reset_parser(ctx: &Ctx<'_>, core: &Core, parser: Oid) {
    if core.0.borrow().objs[parser].def.kind == Kind::Collector && !core.flag(parser, "waitForEnd")
    {
        core.set_layout_value(ctx, parser, "text", Value::str(""));
    }
}

/// Whatever is in the pipe, handed to its parser
pub(super) fn read(ctx: &Ctx<'_>, core: &Core, oid: Oid, err: bool) {
    let mut chunk = Vec::new();
    let parser = {
        let mut s = core.0.borrow_mut();
        let Some(l) = s.procs.get_mut(&oid).and_then(|p| p.live.as_mut()) else {
            return;
        };
        let mut buf = [0u8; 16384];
        let parser = if err { l.err_parser } else { l.out_parser };
        loop {
            let n = if err {
                l.err.as_mut().map(|e| e.read(&mut buf))
            } else {
                l.out.as_mut().map(|o| o.read(&mut buf))
            };
            match n {
                Some(Ok(0)) => {
                    // EOF: closes the descriptor; the parser waits for the exit
                    if err {
                        l.err = None
                    } else {
                        l.out = None
                    }
                    break;
                }
                None | Some(Err(_)) => break,
                Some(Ok(n)) => chunk.extend_from_slice(&buf[..n]),
            }
        }
        parser
    };
    if let Some(p) = parser {
        if !chunk.is_empty() {
            feed(ctx, core, oid, err, p, &chunk);
        }
    }
}

/// Passes output to the parser. SplitParser emits each whole piece;
/// StdioCollector buffers until the end unless `waitForEnd` is off
fn feed(ctx: &Ctx<'_>, core: &Core, oid: Oid, err: bool, parser: Oid, bytes: &[u8]) {
    let kind = core.0.borrow().objs.get(parser).map(|o| o.def.kind);
    let whole = {
        let mut s = core.0.borrow_mut();
        let Some(l) = s.procs.get_mut(&oid).and_then(|p| p.live.as_mut()) else {
            return;
        };
        let buf = if err { &mut l.err_buf } else { &mut l.out_buf };
        buf.extend_from_slice(bytes);
        buf.clone()
    };
    match kind {
        Some(Kind::Splitter) => {
            let marker = core.text(parser, "splitMarker");
            let marker = if marker.is_empty() {
                "\n".to_string()
            } else {
                marker
            };
            let text = String::from_utf8_lossy(&whole).into_owned();
            let mut pieces: Vec<&str> = text.split(marker.as_str()).collect();
            let rest = pieces.pop().unwrap_or("").to_string();
            {
                let mut s = core.0.borrow_mut();
                if let Some(l) = s.procs.get_mut(&oid).and_then(|p| p.live.as_mut()) {
                    let buf = if err { &mut l.err_buf } else { &mut l.out_buf };
                    *buf = rest.into_bytes();
                }
            }
            for piece in pieces {
                emit_text(ctx, core, parser, "read", piece);
            }
        }
        Some(Kind::Collector) if !core.flag(parser, "waitForEnd") => {
            let text = String::from_utf8_lossy(&whole).into_owned();
            core.set_layout_value(ctx, parser, "text", Value::str(&text));
            core.set_layout_value(ctx, parser, "data", Value::str(&text));
        }
        _ => {}
    }
}

fn emit_text(ctx: &Ctx<'_>, core: &Core, oid: Oid, signal: &str, text: &str) {
    if let Ok(s) = rquickjs::String::from_str(ctx.clone(), text) {
        core.emit(ctx, oid, signal, vec![s.into_value()]);
    }
}

/// End of stream: a collector publishes its text, a splitter emits what follows
/// the last marker
fn finish(ctx: &Ctx<'_>, core: &Core, parser: Oid, rest: Vec<u8>) {
    let kind = core.0.borrow().objs.get(parser).map(|o| o.def.kind);
    let text = String::from_utf8_lossy(&rest).into_owned();
    match kind {
        Some(Kind::Collector) => {
            core.set_layout_value(ctx, parser, "text", Value::str(&text));
            core.set_layout_value(ctx, parser, "data", Value::str(&text));
            core.emit(ctx, parser, "streamFinished", Vec::new());
        }
        Some(Kind::Splitter) if !text.is_empty() => emit_text(ctx, core, parser, "read", &text),
        _ => {}
    }
}

pub(super) fn reap(ctx: &Ctx<'_>, core: &Core) {
    let oids: Vec<Oid> = core.0.borrow().procs.keys().copied().collect();
    for oid in oids {
        let status = {
            let mut s = core.0.borrow_mut();
            let Some(l) = s.procs.get_mut(&oid).and_then(|p| p.live.as_mut()) else {
                continue;
            };
            match l.child.try_wait() {
                Ok(Some(st)) => st,
                _ => continue,
            }
        };
        // Output written before the exit
        read(ctx, core, oid, false);
        read(ctx, core, oid, true);
        let live = core
            .0
            .borrow_mut()
            .procs
            .get_mut(&oid)
            .and_then(|p| p.live.take());
        let Some(l) = live else { continue };
        if let Some(p) = l.out_parser {
            finish(ctx, core, p, l.out_buf);
        }
        if let Some(p) = l.err_parser {
            finish(ctx, core, p, l.err_buf);
        }
        use std::os::unix::process::ExitStatusExt;
        // QProcess NormalExit 0, CrashExit 1; a signal death has no code
        let (code, crashed) = match status.code() {
            Some(c) => (c, 0),
            None => (status.signal().unwrap_or(0), 1),
        };
        if !core.0.borrow().objs.is_live(oid) {
            continue;
        }
        core.emit(
            ctx,
            oid,
            "exited",
            vec![
                rquickjs::Value::new_number(ctx.clone(), code as f64),
                rquickjs::Value::new_number(ctx.clone(), crashed as f64),
            ],
        );
        core.set_layout_value(ctx, oid, "processId", Value::Null);
        let again = core.0.borrow().procs.get(&oid).is_some_and(|p| p.target);
        if again {
            start_if_ready(ctx, core, oid);
        } else if let Some(i) = core.prop_idx(oid, "running") {
            core.store_raw(ctx, oid, i, Value::Bool(false));
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
        "signal" => {
            let sig = args
                .first()
                .and_then(|v| v.as_number().or_else(|| v.as_int().map(f64::from)))?
                as i32;
            if let Some(l) = core
                .0
                .borrow()
                .procs
                .get(&oid)
                .and_then(|p| p.live.as_ref())
            {
                // SAFETY: signal to an owned child
                unsafe { libc::kill(l.child.id() as i32, sig) };
            }
        }
        "write" => {
            let text = args.first().map(|v| {
                rquickjs::Coerced::<String>::from_js(ctx, v.clone())
                    .map(|c| c.0)
                    .unwrap_or_default()
            })?;
            if let Some(stdin) = core
                .0
                .borrow_mut()
                .procs
                .get_mut(&oid)
                .and_then(|p| p.live.as_mut())
                .and_then(|l| l.stdin.as_mut())
            {
                let _ = stdin.write_all(text.as_bytes());
            }
        }
        "exec" => {
            // `exec(["cmd", "arg"])` or `exec({ command, environment, ... })`
            core.set_named(ctx, oid, "running", Value::Bool(false));
            if let Some(a) = args.first() {
                if a.is_array() {
                    let v = crate::js::from_js(ctx, a.clone());
                    core.set_named(ctx, oid, "command", v);
                } else if let Some(o) = a.as_object() {
                    for k in [
                        "command",
                        "environment",
                        "clearEnvironment",
                        "workingDirectory",
                    ] {
                        if let Ok(v) = o.get::<_, rquickjs::Value>(k) {
                            if !v.is_undefined() {
                                let ty = core.prop_idx(oid, k).map(|i| core.prop_type(oid, i));
                                let v = match ty {
                                    Some(t) => crate::js::from_js_as(ctx, v, t),
                                    None => crate::js::from_js(ctx, v),
                                };
                                core.set_named(ctx, oid, k, v);
                            }
                        }
                    }
                }
            }
            core.set_named(ctx, oid, "running", Value::Bool(true));
        }
        "startDetached" => {
            let argv = strings(
                ctx,
                &core.peek_named(oid, "command").unwrap_or(Value::Undefined),
            );
            if argv.is_empty() {
                core.log("Process.startDetached: the command is empty".into());
                return Some(Value::Undefined);
            }
            if !super::may_start(core, &argv) {
                return Some(Value::Undefined);
            }
            let mut cmd = command(ctx, core, oid, &argv);
            cmd.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            match cmd.spawn() {
                Ok(child) => core.0.borrow_mut().children.push(child),
                Err(e) => core.log(format!("Process.startDetached {}: {e}", argv[0])),
            }
        }
        _ => return None,
    }
    Some(Value::Undefined)
}

use rquickjs::FromJs;
