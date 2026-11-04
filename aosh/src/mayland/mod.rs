// Windows over libmay's connection
//
// A `PanelWindow` is one `may_layer` surface:
// - a compositor-requested size is applied between frames, once the buffers are
//   released (`armed`);
// - every committed buffer is whole: the compositor re-reads a buffer beyond
//   its damage, so each buffer tracks what it missed (`debt`) and catches up
//   before attach;
// - an idle window commits nothing and requests no frame callback.

mod capture;
mod slow;

use libmay::conn::{Conn, Event, Pool};
use libmay::protocol::{self, Interface};
use libmay::requests::Requests;

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::paint::Rect;
use crate::scene::{Scene, WinScene};

pub struct Display {
    pub conn: Conn,
    shm: u32,
    comp: u32,
    layer: u32,
    /// Bound `may_layer` version; 2 adds focus grabs
    layer_version: u32,
    pub desk: u32,
    /// Bound `may_desktop` version; 5 adds window capture
    desk_version: u32,
    pub windows: Vec<Window>,
    /// Windows' pixels for the `ScreencopyView`s (`capture.rs`)
    captures: capture::Captures,
    /// Active HyprlandFocusGrabs: object, its `may_focus_grab`, listed surfaces
    grabs: Vec<(Oid, u32, Vec<u32>)>,
    /// Grabs ended by the compositor, reported by `sync_grabs`
    cleared: Vec<u32>,
    /// The slowest frame lately, for the log (`slow.rs`)
    slow: slow::Slowest,
    /// The card, while windows are drawn on it
    gpu: Option<crate::scene::gpu::device::Device>,
    /// Why the card is unavailable; not retried
    no_gpu: Option<String>,
    /// Why rectangles fell back to the processor, logged once each
    refusals: Vec<&'static str>,
}

/// Window setup, compared to detect changes
#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct Asked {
    anchor: u32,
    size: (i32, i32),
    exclusive: i32,
    keyboard: u32,
    band: u32,
}

pub struct Window {
    pub oid: Oid,
    pub surface: u32,
    ls: u32,
    pub w: i32,
    pub h: i32,
    /// Configured and drawable
    pub up: bool,
    /// Requested, not configured yet
    waiting: bool,
    asked: Asked,
    pool: Option<Pool>,
    pool_obj: u32,
    /// Buffers in video memory: card pool, offset, size
    /// (`gpu::device::Device::place`)
    gpu: Option<(usize, usize, usize)>,
    /// (buffer, the compositor has let go of it)
    buffers: [(u32, bool); 2],
    debt: [Option<Rect>; 2],
    frame_cb: u32,
    /// Last frame callback arrived; the next frame may be drawn
    armed: bool,
    /// Compositor-requested size the buffers do not have yet
    pending: Option<(i32, i32)>,
    /// Last input region sent; `None` is the whole window (`input_of`)
    input: Option<Vec<Step>>,
    pub scene: WinScene,
}

impl Window {
    /// Whether the window has buffers in either kind of memory
    fn has_buffers(&self) -> bool {
        self.pool.is_some() || self.gpu.is_some()
    }
}

/// Input region rectangle in window pixels: added or subtracted
pub type Step = (bool, i32, i32, i32, i32);

impl Display {
    pub fn open() -> Result<Display, String> {
        let mut c = Conn::open().ok_or("cannot reach the display server - is the compositor running?")?;
        let (registry, globals) = c.handshake();
        if globals.failed {
            return Err("the display server refused the handshake".into());
        }
        for need in [Interface::Shm, Interface::Compositor, Interface::Layer] {
            if !globals.has(need) {
                return Err(format!("the display server has no {}", need.name()));
            }
        }
        let shm = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
        let comp = c.bind(registry, globals.name(Interface::Compositor), Interface::Compositor, 1);
        let layer_version = globals.version(Interface::Layer).clamp(1, Interface::Layer.version());
        let layer = c.bind(registry, globals.name(Interface::Layer), Interface::Layer, layer_version);
        let desk_version = globals.version(Interface::Desktop).clamp(1, Interface::Desktop.version());
        let desk = if globals.has(Interface::Desktop) {
            c.bind(registry, globals.name(Interface::Desktop), Interface::Desktop, desk_version)
        } else {
            0
        };
        if globals.has(Interface::Seat) {
            let seat = c.bind(registry, globals.name(Interface::Seat), Interface::Seat, 1);
            c.get_pointer(seat);
            // Keys arrive only when the compositor grants the keyboard, per
            // `keyboardFocus`
            c.get_keyboard(seat);
        }
        if desk != 0 {
            c.desk_get_windows(desk, 1);
        }
        c.flush();
        Ok(Display {
            conn: c, shm, comp, layer, layer_version, desk, desk_version, windows: Vec::new(),
            captures: capture::Captures::default(), grabs: Vec::new(), cleared: Vec::new(),
            slow: slow::Slowest::default(), gpu: None, no_gpu: None, refusals: Vec::new(),
        })
    }

    /// `view` wants its window's pixels
    pub fn want_capture(&mut self, view: Oid) {
        if self.desk != 0 && self.desk_version >= 5 {
            self.captures.want(view);
        }
    }

    /// Sends the next capture request, queuing live views again after a frame
    pub fn pump_captures(&mut self, engine: &crate::object::engine::Engine, scene: &mut Scene, drawn: bool) {
        if drawn {
            self.captures.frame_drawn();
        }
        // Starts the capture pool at the size of the largest window, about the
        // screen
        let screen = self.windows.iter().map(|w| (w.w.max(0) as usize) * (w.h.max(0) as usize) * 4).max().unwrap_or(0);
        self.captures.pump(&mut self.conn, self.shm, self.desk, engine, scene, screen.max(1 << 20));
    }

    /// `may_desktop.captured`
    #[allow(clippy::too_many_arguments)]
    pub fn captured(&mut self, engine: &crate::object::engine::Engine, scene: &mut Scene, serial: u32, copied: bool,
                    w: i32, h: i32, format: u32) {
        self.captures.answer(&mut self.conn, self.shm, engine, scene, serial, copied, w, h, format);
    }

    /// Keeps one `may_focus_grab` per active HyprlandFocusGrab with windows up.
    /// A grab the compositor ended stores `active = false`, keeping bindings,
    /// and emits `cleared`
    pub fn sync_grabs(&mut self, engine: &crate::object::engine::Engine) {
        let core = &engine.core;
        for object in std::mem::take(&mut self.cleared) {
            let Some(at) = self.grabs.iter().position(|g| g.1 == object) else { continue };
            let (oid, _, _) = self.grabs.remove(at);
            self.conn.focus_grab_destroy(object);
            if core.0.borrow().objs.is_live(oid) {
                engine.with_ctx(|ctx, core| {
                    if let Some(i) = core.own_idx(oid, "active") {
                        core.store(ctx, oid, i, crate::object::value::Value::Bool(false));
                    }
                    core.emit(ctx, oid, "cleared", Vec::new());
                });
            }
        }
        if self.layer_version < 2 {
            return;
        }
        let wanted: Vec<(Oid, Vec<u32>)> = {
            let s = core.0.borrow();
            s.objs.ids().into_iter().filter(|&o| s.objs[o].def.name == "HyprlandFocusGrab").collect::<Vec<_>>()
        }.into_iter().map(|oid| {
            let windows = grab_windows(engine, oid);
            let surfaces = if core.flag(oid, "active") {
                self.windows.iter().filter(|w| w.up && windows.contains(&w.oid)).map(|w| w.surface).collect()
            } else {
                Vec::new()
            };
            (oid, surfaces)
        }).collect();
        // Releases grabs that are gone or empty
        self.grabs.retain(|g| {
            let keep = wanted.iter().any(|(o, s)| *o == g.0 && !s.is_empty());
            if !keep {
                self.conn.focus_grab_destroy(g.1);
            }
            keep
        });
        for (oid, surfaces) in wanted.into_iter().filter(|(_, s)| !s.is_empty()) {
            let at = match self.grabs.iter().position(|g| g.0 == oid) {
                Some(i) => i,
                None => {
                    let object = self.conn.get_focus_grab(self.layer);
                    self.grabs.push((oid, object, Vec::new()));
                    self.grabs.len() - 1
                }
            };
            let (_, object, listed) = &mut self.grabs[at];
            if *listed == surfaces {
                continue;
            }
            for &sf in listed.iter().filter(|s| !surfaces.contains(s)) {
                self.conn.focus_grab_remove(*object, sf);
            }
            for &sf in surfaces.iter().filter(|s| !listed.contains(s)) {
                self.conn.focus_grab_add(*object, sf);
            }
            self.conn.focus_grab_commit(*object);
            *listed = surfaces;
        }
    }

    /// Matches surfaces to the `PanelWindow`s that should be up, reconfiguring
    /// on change
    pub fn sync(&mut self, core: &Core) {
        // Closes windows whose object is gone after a reload
        let mut i = 0;
        while i < self.windows.len() {
            if core.0.borrow().objs.is_live(self.windows[i].oid) {
                i += 1;
            } else {
                let mut w = self.windows.remove(i);
                self.close_window(&mut w);
            }
        }
        let wanted: Vec<Oid> = {
            let s = core.0.borrow();
            s.objs.ids().into_iter()
                .filter(|&o| s.objs[o].def.kind == crate::types::Kind::Window)
                .collect()
        };
        for oid in wanted {
            let visible = core.flag(oid, "visible");
            let at = self.windows.iter().position(|w| w.oid == oid);
            match (visible, at) {
                (true, None) => {
                    let w = self.open_window(core, oid);
                    self.windows.push(w);
                }
                (false, Some(i)) => {
                    let mut w = self.windows.remove(i);
                    self.close_window(&mut w);
                }
                (true, Some(i)) => {
                    let asked = asked_of(core, oid);
                    // A different band needs a new surface
                    if asked.band != self.windows[i].asked.band {
                        let mut old = self.windows.remove(i);
                        self.close_window(&mut old);
                        let w = self.open_window(core, oid);
                        self.windows.push(w);
                        continue;
                    }
                    let w = &mut self.windows[i];
                    if asked != w.asked {
                        set_up(&mut self.conn, w.ls, &asked);
                        w.asked = asked;
                        self.conn.commit(w.surface);
                    }
                }
                (false, None) => {}
            }
        }
    }

    fn open_window(&mut self, core: &Core, oid: Oid) -> Window {
        let asked = asked_of(core, oid);
        let surface = self.conn.create_surface(self.comp);
        let ls = self.conn.get_layer_surface(self.layer, surface, asked.band);
        // Set before the first commit; layer rules match on it (`noanim` for
        // `quickshell:overview`)
        let ns = core.text(oid, "WlrLayershell.namespace");
        if !ns.is_empty() {
            self.conn.layer_set_namespace(ls, &ns);
        }
        set_up(&mut self.conn, ls, &asked);
        self.conn.commit(surface);
        if core.num(oid, "margins.top") != 0.0 || core.num(oid, "margins.bottom") != 0.0
            || core.num(oid, "margins.left") != 0.0 || core.num(oid, "margins.right") != 0.0
        {
            core.log("PanelWindow: margins are not there yet - may_layer has no margin request".into());
        }
        Window {
            oid, surface, ls, w: 0, h: 0, up: false, waiting: true, asked,
            pool: None, pool_obj: 0, gpu: None, buffers: [(0, true); 2], debt: [None; 2],
            frame_cb: 0, armed: true, pending: None, input: None, scene: WinScene::new(0, 0),
        }
    }

    /// After a reload: gives up windows to the new tree's `PanelWindow`s in
    /// declaration order, repaints them whole, and resends changed setup. The
    /// next `sync` closes the rest
    pub fn hand_over(&mut self, core: &Core, new: &[Oid]) {
        for (w, &oid) in self.windows.iter_mut().zip(new.iter()) {
            w.oid = oid;
            w.scene = WinScene::new(w.w, w.h);
            let all = Rect::new(0, 0, w.w, w.h);
            w.debt = [Some(all), Some(all)];
            let asked = asked_of(core, oid);
            if asked != w.asked {
                set_up(&mut self.conn, w.ls, &asked);
                w.asked = asked;
                self.conn.commit(w.surface);
            }
        }
    }

    fn close_window(&mut self, w: &mut Window) {
        self.conn.destroy_layer_surface(w.ls);
        self.conn.destroy_surface(w.surface);
        drop_buffers(&mut self.conn, w, self.gpu.as_mut());
    }

    /// Applies `aosh ipc call aosh render cpu|gpu`: opens or releases the card
    /// and recreates every window's buffers
    fn choose_gpu(&mut self, core: &Core) {
        let want = !core.0.borrow().render_cpu;
        if want && self.gpu.is_none() && self.no_gpu.is_none() {
            match crate::scene::gpu::device::Device::open() {
                Ok(d) => {
                    crate::say("drawing on the card (`aosh ipc call aosh render cpu` for the processor)");
                    self.gpu = Some(d);
                    self.recut();
                }
                Err(e) => {
                    crate::say(&format!("drawing on the processor: {e}"));
                    self.no_gpu = Some(e);
                }
            }
        } else if !want && self.gpu.is_some() {
            self.recut();
            if let Some(d) = self.gpu.take() {
                d.close(&mut self.conn);
            }
            crate::say("drawing on the processor, as asked");
        }
    }

    /// Every window's buffers given up, to be cut again at its next frame
    fn recut(&mut self) {
        for w in &mut self.windows {
            drop_buffers(&mut self.conn, w, self.gpu.as_mut());
            if w.up && w.w > 0 && w.h > 0 {
                w.pending = Some((w.w, w.h));
            }
        }
    }

    /// Handles window events; returns the others to the caller
    pub fn window_event(&mut self, e: &Event) -> bool {
        match *e {
            Event::LayerConfigure { object, serial, width, height } => {
                self.conn.layer_ack_configure(object, serial);
                if let Some(w) = self.windows.iter_mut().find(|w| w.ls == object) {
                    let want = (if width > 0 { width } else { w.w }, if height > 0 { height } else { w.h });
                    w.waiting = false;
                    w.up = true;
                    if want != (w.w, w.h) || !w.has_buffers() {
                        w.pending = Some(want);
                    }
                }
                true
            }
            Event::LayerClosed { object } => {
                if let Some(w) = self.windows.iter_mut().find(|w| w.ls == object) {
                    w.up = false;
                }
                true
            }
            Event::Done { id, .. } => {
                for w in &mut self.windows {
                    if w.frame_cb == id {
                        w.armed = true;
                    }
                }
                true
            }
            Event::FocusGrabCleared { grab } => {
                self.cleared.push(grab);
                true
            }
            Event::BufferRelease { id } => {
                for w in &mut self.windows {
                    for b in w.buffers.iter_mut() {
                        if b.0 == id {
                            b.1 = true;
                        }
                    }
                }
                true
            }
            _ => false,
        }
    }

    pub fn window_of_surface(&self, surface: u32) -> Option<usize> {
        self.windows.iter().position(|w| w.surface == surface)
    }

    /// Whether some window can draw now: up, callback received, and a buffer
    /// released
    pub fn ready(&self) -> bool {
        self.windows.iter().any(|w| w.up && w.armed && (w.pending.is_some() || w.buffers.iter().any(|b| b.1)))
    }

    /// Lays out, paints, and commits every window that can take a frame.
    /// Returns false if one must wait for its callback. While animating, an
    /// unchanged window commits with no buffer to get the next callback
    pub fn frame(&mut self, engine: &crate::object::engine::Engine, scene: &mut Scene, animating: bool) -> bool {
        self.choose_gpu(&engine.core);
        // Waits for the previous card frame, whose commands and textures are
        // about to be reused
        if let Some(d) = self.gpu.as_mut() {
            d.wait();
            d.rec.frame();
        }
        let mut all = true;
        // Windows drawn on the card, committed after the launch: index, buffer,
        // damage, cost
        let mut drawn: Vec<(usize, usize, Vec<Rect>, slow::Took)> = Vec::new();
        for (k, w) in self.windows.iter_mut().enumerate() {
            // Its object is gone, and `sync` closes it
            if !w.up || !engine.core.0.borrow().objs.is_live(w.oid) {
                continue;
            }
            if !w.armed {
                all = false;
                continue;
            }
            if let Some((cw, ch)) = w.pending.take() {
                w.w = cw;
                w.h = ch;
                cut_buffers(&mut self.conn, self.shm, w, self.gpu.as_mut());
            }
            let Some(slot) = w.buffers.iter().position(|b| b.1) else {
                all = false;
                continue;
            };
            if !w.has_buffers() {
                continue;
            }
            let (oid, fw, fh) = (w.oid, w.w as f64, w.h as f64);
            let started = std::time::Instant::now();
            engine.with_ctx(|ctx, core| scene.layout(ctx, core, oid, fw, fh));
            let laid = started.elapsed();
            // Input region, sent on change and applied at the next commit
            let input = input_of(&engine.core, oid);
            if input != w.input {
                let region = match &input {
                    Some(steps) => {
                        let r = self.conn.create_region(self.comp);
                        for &(add, x, y, rw, rh) in steps {
                            if add { self.conn.region_add(r, x, y, rw, rh) } else { self.conn.region_subtract(r, x, y, rw, rh) }
                        }
                        r
                    }
                    None => 0,
                };
                self.conn.set_input_region(w.surface, region);
                if region != 0 {
                    self.conn.region_destroy(region);
                }
                self.conn.commit(w.surface);
                w.input = input;
            }
            let painting = std::time::Instant::now();
            let on_card = w.gpu.is_some();
            let damage = if on_card { scene.list(&engine.core, oid, &mut w.scene) } else { scene.paint(&engine.core, oid, &mut w.scene) };
            let mut painted = painting.elapsed();
            if damage.is_empty() {
                self.slow.note(&engine.core, slow::Took {
                    window: oid, layout: laid, paint: painted, copy: Default::default(), damage, by: slow::By::Nothing,
                });
                if animating {
                    w.frame_cb = self.conn.frame(w.surface);
                    self.conn.commit(w.surface);
                    w.armed = false;
                }
                continue;
            }
            let stride = w.w as usize;
            let bytes = (w.w * w.h * 4) as usize;
            let from = slot * bytes;
            let copied;
            let mut by = slow::By::Processor;
            match (self.gpu.as_mut(), w.gpu) {
                // On the card: records damage and buffer debt, paints
                // unrecordable items here (`Scene::record`)
                (Some(d), Some((pool, at, _))) => {
                    let mut rects = damage.clone();
                    rects.extend(w.debt[slot]);
                    let target = d.rec.target(pool as u32, (at + from) as u32, w.w as u32, w.w as u32, w.h as u32);
                    let recording = std::time::Instant::now();
                    let here = scene.record(&engine.core, &mut w.scene, &rects, &mut d.rec, target);
                    painted += recording.elapsed();
                    let copying = std::time::Instant::now();
                    if let Some(mem) = d.pool_bytes(pool) {
                        let buffer = &mut mem[at + from..at + from + bytes];
                        for r in &here {
                            w.scene.image.copy_out(*r, buffer, stride);
                        }
                    }
                    copied = copying.elapsed();
                    by = if here.is_empty() { slow::By::Card } else { slow::By::Both };
                    if let Some(why) = d.rec.refused().filter(|w| !here.is_empty() && !self.refusals.contains(w)) {
                        crate::say(&format!("the processor paints what the card cannot: {why}"));
                        self.refusals.push(why);
                    }
                }
                _ => {
                    let copying = std::time::Instant::now();
                    if let Some(p) = w.pool.as_mut() {
                        let buffer = &mut p.bytes()[from..from + bytes];
                        // Buffer debt, then this frame's damage, so the buffer
                        // is whole before attach
                        if let Some(owed) = w.debt[slot] {
                            w.scene.image.copy_out(owed, buffer, stride);
                        }
                        for &d in &damage {
                            w.scene.image.copy_out(d, buffer, stride);
                        }
                    }
                    copied = copying.elapsed();
                }
            }
            w.debt[slot] = None;
            let other = 1 - slot;
            for &d in &damage {
                w.debt[other] = Some(match w.debt[other] { Some(o) => o.union(&d), None => d });
            }
            let took = slow::Took { window: oid, layout: laid, paint: painted, copy: copied, damage: damage.clone(), by };
            if on_card {
                drawn.push((k, slot, damage, took));
                continue;
            }
            self.conn.attach(w.surface, w.buffers[slot].0, 0, 0);
            // Damage is the difference from the previously shown buffer, which
            // excludes this buffer's debt
            for d in &damage {
                self.conn.damage(w.surface, d.x, d.y, d.w, d.h);
            }
            self.slow.note(&engine.core, took);
            w.buffers[slot].1 = false;
            w.frame_cb = self.conn.frame(w.surface);
            self.conn.commit(w.surface);
            w.armed = false;
        }
        // One launch per frame; each window commits with its fence so the
        // compositor reads after the card
        if !drawn.is_empty() {
            let fence = match self.gpu.as_mut().map(|d| d.submit()) {
                Some(Ok(f)) => f,
                Some(Err(e)) => {
                    // Nothing drawn: falls back to the processor with every
                    // window whole
                    crate::say(&format!("{e} - drawing on the processor"));
                    self.no_gpu = Some(e);
                    engine.core.0.borrow_mut().render_cpu = true;
                    for &(k, _, _, _) in &drawn {
                        self.windows[k].pending = Some((self.windows[k].w, self.windows[k].h));
                    }
                    return false;
                }
                None => None,
            };
            for (k, slot, damage, took) in drawn {
                let w = &mut self.windows[k];
                if let Some(f) = fence {
                    self.conn.set_fence(w.surface, f);
                }
                self.conn.attach(w.surface, w.buffers[slot].0, 0, 0);
                for d in &damage {
                    self.conn.damage(w.surface, d.x, d.y, d.w, d.h);
                }
                self.slow.note(&engine.core, took);
                w.buffers[slot].1 = false;
                w.frame_cb = self.conn.frame(w.surface);
                self.conn.commit(w.surface);
                w.armed = false;
            }
        }
        self.slow.tell();
        all
    }
}

/// Windows a `HyprlandFocusGrab` admits, from its `windows` list. An array with
/// holes stays as given
pub(crate) fn grab_windows(engine: &crate::object::engine::Engine, grab: Oid) -> Vec<Oid> {
    use crate::object::value::Value;
    match engine.core.peek_named(grab, "windows") {
        Some(Value::List(l)) => l.iter().filter_map(|v| v.as_obj()).collect(),
        Some(Value::Js(p)) => engine.with_ctx(|ctx, _| {
            let Ok(v) = p.as_ref().clone().restore(ctx) else { return Vec::new() };
            let Some(a) = v.as_array() else { return Vec::new() };
            a.iter::<rquickjs::Value>().flatten()
                .filter_map(|e| crate::js::from_js(ctx, e).as_obj()).collect()
        }),
        _ => Vec::new(),
    }
}

/// Input region in window pixels, from quickshell's `mask`: a Region (item's
/// box or its own `x`, `y`, `width`, `height`) and nested Regions, each added
/// (`Intersection.Combine`) or subtracted (`Subtract`) in order. An empty mask
/// takes no input; no mask takes all of it. Intersect and Xor are not
/// implemented
pub(crate) fn input_of(core: &Core, win: Oid) -> Option<Vec<Step>> {
    let mask = core.peek_named(win, "mask").and_then(|v| v.as_obj())
        .filter(|&m| core.0.borrow().objs.is_live(m))?;
    let mut steps = Vec::new();
    region_steps(core, mask, true, &mut steps);
    Some(steps)
}

fn region_steps(core: &Core, region: Oid, add: bool, out: &mut Vec<Step>) {
    let item = core.peek_named(region, "item").and_then(|v| v.as_obj())
        .filter(|&i| core.0.borrow().objs.is_live(i));
    let (x, y, w, h) = match item {
        Some(i) => {
            let b = crate::scene::xf::to_window(core, i).bounds(0.0, 0.0, core.num(i, "width"), core.num(i, "height"));
            (b.x, b.y, b.w, b.h)
        }
        None => (core.num(region, "x") as i32, core.num(region, "y") as i32,
                 core.num(region, "width") as i32, core.num(region, "height") as i32),
    };
    if w > 0 && h > 0 {
        out.push((add, x, y, w, h));
    }
    // Regions from `regions` or children
    let listed: Vec<Oid> = match core.peek_named(region, "regions") {
        Some(crate::object::value::Value::List(l)) => l.iter().filter_map(|v| v.as_obj()).collect(),
        _ => Vec::new(),
    };
    let inside: Vec<Oid> = if listed.is_empty() {
        let s = core.0.borrow();
        s.objs[region].children.iter().copied().filter(|&c| s.objs.get(c).is_some_and(|o| o.def.name == "Region")).collect()
    } else {
        listed
    };
    for child in inside {
        match core.num(child, "intersection") as i64 {
            0 => region_steps(core, child, true, out),
            1 => region_steps(core, child, false, out),
            _ => core.log_once(child, "Region: Intersect and Xor are not there yet"),
        }
    }
}

/// Layer-shell exclusive zone from quickshell's `exclusionMode`: Normal uses
/// `exclusiveZone`, Ignore is -1, Auto is the size along the anchored edge or 0
/// if there is no single edge. An explicit zone implies Normal, as in
/// quickshell's `setExclusiveZone`
pub(crate) fn exclusive_of(core: &Core, oid: Oid) -> i32 {
    let (top, bottom, left, right) = (
        core.flag(oid, "anchors.top"), core.flag(oid, "anchors.bottom"),
        core.flag(oid, "anchors.left"), core.flag(oid, "anchors.right"),
    );
    let w = if left && right { 0 } else { core.num(oid, "implicitWidth") as i32 };
    let h = if top && bottom { 0 } else { core.num(oid, "implicitHeight") as i32 };
    let zone = core.num(oid, "exclusiveZone") as i32;
    let mode = if core.is_explicit(oid, "exclusiveZone") {
        0
    } else if core.is_explicit(oid, "exclusionMode") {
        core.num(oid, "exclusionMode") as i64
    } else {
        2
    };
    match mode {
        0 => zone.max(0),
        1 => -1,
        _ if top != bottom && (left == right) => h,
        _ if left != right && (top == bottom) => w,
        _ => 0,
    }
}

/// What a window should be set up with, from its properties
fn asked_of(core: &Core, oid: Oid) -> Asked {
    use protocol::layer_surface as ls;
    let (top, bottom, left, right) = (
        core.flag(oid, "anchors.top"), core.flag(oid, "anchors.bottom"),
        core.flag(oid, "anchors.left"), core.flag(oid, "anchors.right"),
    );
    let mut anchor = 0;
    if top { anchor |= ls::ANCHOR_TOP; }
    if bottom { anchor |= ls::ANCHOR_BOTTOM; }
    if left { anchor |= ls::ANCHOR_LEFT; }
    if right { anchor |= ls::ANCHOR_RIGHT; }
    // Zero means stretched when anchored at both ends
    let w = if left && right { 0 } else { core.num(oid, "implicitWidth") as i32 };
    let h = if top && bottom { 0 } else { core.num(oid, "implicitHeight") as i32 };
    let exclusive = exclusive_of(core, oid);
    // `WlrLayershell.keyboardFocus`, default on demand
    let keyboard = if core.is_explicit(oid, "WlrLayershell.keyboardFocus") {
        match core.num(oid, "WlrLayershell.keyboardFocus") as i64 {
            1 => ls::KEYBOARD_EXCLUSIVE,
            2 => ls::KEYBOARD_ON_DEMAND,
            _ => ls::KEYBOARD_NONE,
        }
    } else if core.flag(oid, "focusable") {
        ls::KEYBOARD_ON_DEMAND
    } else {
        ls::KEYBOARD_NONE
    };
    // `WlrLayershell.layer`, default `aboveWindows`
    let band = if core.is_explicit(oid, "WlrLayershell.layer") {
        (core.num(oid, "WlrLayershell.layer") as u32).min(protocol::layer::OVERLAY)
    } else if core.flag(oid, "aboveWindows") {
        protocol::layer::TOP
    } else {
        protocol::layer::BOTTOM
    };
    Asked { anchor, size: (w, h), exclusive, keyboard, band }
}

fn set_up(c: &mut Conn, ls: u32, a: &Asked) {
    c.layer_set_anchor(ls, a.anchor);
    c.layer_set_size(ls, a.size.0, a.size.1);
    c.layer_set_exclusive(ls, a.exclusive);
    c.layer_set_keyboard(ls, a.keyboard);
}

/// Two buffers of the size the compositor named, in one pool
fn cut_buffers(c: &mut Conn, shm: u32, w: &mut Window, mut gpu: Option<&mut crate::scene::gpu::device::Device>) {
    drop_buffers(c, w, gpu.as_deref_mut());
    if w.w <= 0 || w.h <= 0 {
        return;
    }
    let stride = w.w * 4;
    let bytes = (stride * w.h) as usize;
    // In video memory, where the card draws it and the compositor reads it
    if let Some(d) = gpu {
        if let Some((pool, at)) = d.place(c, shm, bytes) {
            let obj = d.pool_obj(pool);
            for i in 0..2 {
                w.buffers[i] = (
                    c.create_buffer(obj, (at + i * bytes) as i32, w.w, w.h, stride, protocol::shm::FORMAT_ARGB8888),
                    true,
                );
            }
            w.gpu = Some((pool, at, bytes));
            w.scene = WinScene::new(w.w, w.h);
            let all = Rect::new(0, 0, w.w, w.h);
            w.debt = [Some(all), Some(all)];
            return;
        }
    }
    let Some(p) = Pool::new(bytes * 2) else {
        crate::say(&format!("the kernel would not give us a pool for a {}x{} window", w.w, w.h));
        return;
    };
    w.pool_obj = c.create_pool(shm, p.handle, (bytes * 2) as i32);
    for i in 0..2 {
        w.buffers[i] = (
            c.create_buffer(w.pool_obj, (i * bytes) as i32, w.w, w.h, stride, protocol::shm::FORMAT_ARGB8888),
            true,
        );
    }
    w.pool = Some(p);
    // New memory: the image is painted whole and neither buffer holds it
    w.scene = WinScene::new(w.w, w.h);
    let all = Rect::new(0, 0, w.w, w.h);
    w.debt = [Some(all), Some(all)];
}

fn drop_buffers(c: &mut Conn, w: &mut Window, gpu: Option<&mut crate::scene::gpu::device::Device>) {
    for b in w.buffers.iter_mut() {
        if b.0 != 0 {
            c.destroy_buffer(b.0);
        }
        *b = (0, true);
    }
    if let Some((pool, at, bytes)) = w.gpu.take() {
        if let Some(d) = gpu {
            d.release(c, pool, at, bytes);
        }
    }
    if w.pool_obj != 0 {
        c.destroy_pool(w.pool_obj);
        w.pool_obj = 0;
    }
    if let Some(mut p) = w.pool.take() {
        // The compositor must see the pool destroyed before the memory is freed
        c.flush();
        p.free();
    }
}
