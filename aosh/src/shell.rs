// Shell main loop
//
// Single-threaded. Blocks in `poll` on the display socket, Process pipes, and
// the inotify watch until the next `Timer`, service tick, or frame. Each
// iteration handles:
// 1) Window, pointer, and desktop events
// 2) Pipes and file watches
// 3) Timers, animations, and exited processes
// 4) `/proc` sampling and compositor requests
// 5) Reload if a file changed
// 6) A frame for each window with new content

use std::path::{Path, PathBuf};
use std::time::Instant;

use libmay::conn::Event;
use libmay::requests::Requests;

use crate::input::keyboard::Keyboard;
use crate::input::pointer::{self, Pointer};
use crate::io::Fd;
use crate::mayland::Display;
use crate::object::engine::Engine;
use crate::object::value::Oid;
use crate::object::{Action, Core};
use crate::types::Kind;
use crate::scene::text::Fonts;
use crate::scene::Scene;
use crate::services::{Services, SAMPLE_MS};

/// Frame length when no window provides one
const FRAME_MS: u64 = 16;

/// Runs `file`, or the user's shell.qml with the system's as a fallback,
/// resolved again on each reload. `config` (`aosh -c NAME`) selects
/// `~/.config/aosh/NAME`, then `/etc/aosh/NAME`, and gets its own socket
pub fn run(file: Option<&Path>, config: Option<&str>) -> i32 {
    let chosen = if file.is_none() && config.is_none() { default_config() } else { None };
    match go(file, config.or(chosen.as_deref())) {
        Ok(()) => 0,
        Err(e) => {
            crate::say(&e);
            1
        }
    }
}

/// Config run by a bare `aosh`: the word in `~/.config/aosh/default`, then
/// `/etc/aosh/default`, otherwise none (`/etc/aosh/shell.qml`)
pub fn default_config() -> Option<String> {
    let user = std::env::var_os("HOME").map(|h| Path::new(&h).join(".config/aosh/default"));
    let word = |p: &Path| std::fs::read_to_string(p).ok()
        .and_then(|t| t.split_whitespace().next().map(str::to_string))
        .filter(|w| config_name(w));
    user.as_deref().and_then(word).or_else(|| word(Path::new("/etc/aosh/default")))
}

/// `~/.config/aosh`, or a config's directory inside it
fn user_dir(config: Option<&str>) -> Option<PathBuf> {
    let dir = std::env::var_os("HOME").map(|home| Path::new(&home).join(".config/aosh"))?;
    Some(match config {
        Some(name) => dir.join(name),
        None => dir,
    })
}

/// The user's `shell.qml` if present, otherwise the system's
fn shell_file(config: Option<&str>) -> PathBuf {
    let system = match config {
        Some(name) => Path::new("/etc/aosh").join(name).join("shell.qml"),
        None => PathBuf::from("/etc/aosh/shell.qml"),
    };
    user_dir(config).map(|d| d.join("shell.qml")).filter(|p| p.is_file()).unwrap_or(system)
}

/// Whether `name` is a valid config name (a single directory name)
pub fn config_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/')
}

fn go(file: Option<&Path>, config: Option<&str>) -> Result<(), String> {
    let mut path = file.map(Path::to_path_buf).unwrap_or_else(|| shell_file(config));
    let mut engine = Engine::new()?;
    let mut services = Services::new(&mut engine)?;
    let mut root = engine.load_file(&path)?;
    watch_config(&engine);
    // Watches the user's directory even before it exists, so a copy of
    // read-only /etc/aosh takes effect without a restart
    if file.is_none() {
        if let Some(dir) = user_dir(config) {
            crate::say(&format!("{} is followed: a shell.qml there takes over", dir.display()));
            crate::io::watch_user(&engine.core, dir);
        }
    }
    let fonts = Fonts::system();
    if fonts.is_empty() {
        crate::say("no fonts in /etc/aosh/fonts - text will not be drawn");
    }
    let mut scene = Scene::new(fonts);
    for line in engine.take_log() {
        crate::say(&line);
    }
    let mut display = Display::open()?;
    services.sample(&engine);
    crate::say(&format!("{} is read, and the display server answered", path.display()));

    let start = Instant::now();
    let ms = || start.elapsed().as_millis() as u64;
    let mut next_sample = ms() + SAMPLE_MS;
    let mut evs = [Event::Other { object: 0, opcode: 0 }; 64];
    let mut pointer = Pointer::default();
    let mut keyboard = Keyboard::default();
    // Starts serving `aosh ipc call`
    crate::ipc::listen(&engine.core, config);
    // The first frame is drawn even if nothing changed
    engine.core.0.borrow_mut().dirty = true;

    loop {
        display.sync(&engine.core);
        display.sync_grabs(&engine);
        // A closed window loses the keyboard regardless of compositor events
        let kb = engine.core.0.borrow().keys.window();
        if kb.is_some_and(|w| !display.windows.iter().any(|x| x.oid == w)) {
            engine.keyboard(None);
        }
        display.conn.flush();

        let now = ms();
        let mut timeout = next_sample.saturating_sub(now);
        if let Some(due) = engine.next_due() {
            timeout = timeout.min(due.saturating_sub(now));
        }
        if let Some(due) = engine.core.0.borrow().io.reload_due {
            timeout = timeout.min(due.saturating_sub(now));
        }
        let animating = engine.animating();
        if (engine.core.0.borrow().dirty || animating) && display.ready() {
            timeout = 0;
        } else if animating {
            // Fallback wakeup when no window has a frame callback
            timeout = timeout.min(FRAME_MS);
        }
        let io = engine.io_fds();
        let (readable, io_ready) = if display.conn.buffered() {
            (true, Vec::new())
        } else {
            wait(display.conn.fd, &io, timeout)
        };
        engine.woke(ms());
        for fd in io_ready {
            engine.io_ready(fd);
        }

        if readable {
            let n = display.conn.read_events(&mut evs);
            if n == 0 && display.conn.closed {
                return Err(format!("the display server went away: {}", display.conn.death));
            }
            let workspace_was = engine.core.num(services.desktop, "workspace");
            for e in &evs[..n] {
                if display.window_event(e) {
                    if matches!(e, Event::LayerConfigure { .. }) {
                        engine.core.0.borrow_mut().dirty = true;
                    }
                    continue;
                }
                if keyboard.event(&engine, &display, e) {
                    continue;
                }
                match *e {
                    Event::PointerEnter { surface, x, y } => {
                        pointer.win = display.window_of_surface(surface);
                        pointer.x = x as f64;
                        pointer.y = y as f64;
                        pointer::hover(&engine, &scene, &display, &mut pointer);
                    }
                    Event::PointerLeave { .. } => {
                        pointer.win = None;
                        pointer::hover(&engine, &scene, &display, &mut pointer);
                    }
                    Event::PointerMotion { x, y } => {
                        pointer.x = x as f64;
                        pointer.y = y as f64;
                        pointer::hover(&engine, &scene, &display, &mut pointer);
                    }
                    Event::PointerButton { button, pressed, .. } => {
                        pointer::press(&engine, &mut scene, &display, &mut pointer, button, pressed);
                    }
                    Event::PointerAxis { value, .. } => pointer::wheel(&engine, &mut scene, &display, &pointer, value),
                    Event::DeskCaptured { serial, copied, width, height, format } =>
                        display.captured(&engine, &mut scene, serial, copied, width, height, format),
                    Event::Error { ref msg, msg_len, .. } => {
                        let m = String::from_utf8_lossy(&msg[..msg_len]);
                        return Err(format!("the display server refused something: {m}"));
                    }
                    _ => {
                        if services.desk(&engine, e) && display.desk != 0 {
                            display.conn.desk_get_windows(display.desk, 2);
                        }
                    }
                }
            }
            if engine.core.num(services.desktop, "workspace") != workspace_was {
                services.workspace_changed(&engine);
            }
        }

        let now = ms();
        engine.start_new(now);
        engine.advance(now);
        if ms() >= next_sample {
            services.sample(&engine);
            next_sample = ms() + SAMPLE_MS;
        }
        let actions = std::mem::take(&mut engine.core.0.borrow_mut().actions);
        for a in actions {
            match a {
                Action::Workspace(n) if display.desk != 0 => display.conn.desk_workspace(display.desk, n),
                Action::Signal(w) if display.desk != 0 => display.conn.desk_signal(display.desk, &w),
                Action::Focus(c, sf) if display.desk != 0 => display.conn.desk_focus(display.desk, c, sf),
                Action::Close(c, sf) if display.desk != 0 => display.conn.desk_close(display.desk, c, sf),
                Action::MoveTo(c, sf, n) if display.desk != 0 => display.conn.desk_move_to_workspace(display.desk, c, sf, n),
                Action::Capture(view) => display.want_capture(view),
                _ => {}
            }
        }
        engine.core.0.borrow_mut().children.retain_mut(|c| !matches!(c.try_wait(), Ok(Some(_))));

        if engine.core.0.borrow().io.reload_due.is_some_and(|due| ms() >= due) {
            if file.is_none() {
                path = shell_file(config);
            }
            reload(&mut engine, &mut display, &mut root, &path);
        }

        let animating = engine.animating();
        let drawn = (engine.core.0.borrow().dirty || animating) && display.frame(&engine, &mut scene, animating);
        if drawn {
            engine.core.0.borrow_mut().dirty = false;
        }
        display.pump_captures(&engine, &mut scene, drawn);
        // Binding errors and unknown names, with file and line
        for line in engine.take_log() {
            crate::say(&line);
        }
    }
}

/// Waits up to `ms` for the display socket or `io`. Returns whether the socket
/// is ready, and which `io` entries are
fn wait(display: i64, io: &[(i32, Fd)], ms: u64) -> (bool, Vec<Fd>) {
    let mut fds = vec![libc::pollfd { fd: display as i32, events: libc::POLLIN, revents: 0 }];
    fds.extend(io.iter().map(|&(fd, _)| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }));
    // SAFETY: pollfd array of the given length
    let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms.min(i32::MAX as u64) as i32) };
    if r <= 0 {
        return (false, Vec::new());
    }
    let hit = |p: &libc::pollfd| p.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
    let ready = fds[1..].iter().zip(io.iter()).filter(|(p, _)| hit(p)).map(|(_, &(_, f))| f).collect();
    (hit(&fds[0]), ready)
}

/// Watches every directory a file was read from; saving a `.qml` reloads the
/// shell
fn watch_config(engine: &Engine) {
    let dirs: Vec<std::path::PathBuf> = {
        let s = engine.core.0.borrow();
        let mut d: Vec<std::path::PathBuf> = Vec::new();
        for f in &s.files {
            if let Some(p) = Path::new(f).parent() {
                if !p.as_os_str().is_empty() && !d.iter().any(|x| x == p) {
                    d.push(p.to_path_buf());
                }
            }
        }
        d
    };
    crate::io::watch_config(&engine.core, &dirs);
}

/// Builds the new tree beside the old one and destroys the old one if loading
/// succeeds. Windows are handed over in order to avoid flicker. On failure the
/// current tree stays
fn reload(engine: &mut Engine, display: &mut Display, root: &mut Oid, path: &Path) {
    {
        let mut s = engine.core.0.borrow_mut();
        s.io.reload_due = None;
        s.docs.borrow_mut().clear();
    }
    let errors_before = engine.core.0.borrow().load_errors;
    let loaded = engine.load_file(path).and_then(|new| {
        // Any load error (parse failure, unknown type) rejects the new tree
        if engine.core.0.borrow().load_errors != errors_before {
            engine.destroy(new);
            return Err("part of it did not load (above)".to_string());
        }
        Ok(new)
    });
    match loaded {
        Ok(new) => {
            let windows = windows_in(&engine.core, new);
            display.hand_over(&engine.core, &windows);
            engine.destroy(*root);
            *root = new;
            // Closes unclaimed windows before a frame is drawn for a destroyed
            // object
            display.sync(&engine.core);
            engine.core.0.borrow_mut().dirty = true;
            for line in engine.take_log() {
                crate::say(&line);
            }
            watch_config(engine);
            crate::say(&format!("{} reloaded", path.display()));
        }
        Err(e) => {
            for line in engine.take_log() {
                crate::say(&line);
            }
            crate::say(&format!("the reload did not load, so what was up stays up: {e}"));
        }
    }
}

/// The windows in a tree, in the order they are written
fn windows_in(core: &Core, root: Oid) -> Vec<Oid> {
    fn walk(core: &Core, o: Oid, out: &mut Vec<Oid>) {
        let (kind, next) = {
            let s = core.0.borrow();
            let Some(obj) = s.objs.get(o) else { return };
            let mut next = obj.children.clone();
            next.extend(obj.owned.iter().copied().filter(|c| !obj.children.contains(c)));
            (obj.def.kind, next)
        };
        if kind == Kind::Window && !out.contains(&o) {
            out.push(o);
        }
        for c in next {
            walk(core, c, out);
        }
    }
    let mut out = Vec::new();
    walk(core, root, &mut out);
    out
}
