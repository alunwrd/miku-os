// Window pixels for `ScreencopyView`, over `may_desktop.capture`
//
// One shared pool takes one copy at a time: the compositor copies the window's
// last committed buffer into it and reports `CAPTURED`, and the pixels move to
// the view's picture before the next request. The pool starts at screen size,
// grows up to `shm::MAX_POOL_BYTES` when a window does not fit, and is released
// when no view is waiting.
//
// The copy is not scaled; the scene samples the picture (`Draw::Image`). A live
// view requests again once per drawn frame.

use std::collections::VecDeque;
use std::rc::Rc;

use libmay::conn::{Conn, Pool};
use libmay::protocol;
use libmay::requests::Requests;

use crate::object::engine::Engine;
use crate::object::value::Oid;
use crate::scene::picture::Picture;
use crate::scene::Scene;
use crate::services::capture as view;

/// The most a pool may be (the kernel's `MAX_POOL_BYTES`)
const MOST: usize = 64 * 1024 * 1024;

#[derive(Default)]
pub struct Captures {
    /// The pool and its object on the connection
    pool: Option<(Pool, u32)>,
    /// Views waiting their turn, each once
    queue: VecDeque<Oid>,
    /// The outstanding request: serial, view, window
    asked: Option<(u32, Oid, (u32, u32))>,
    serial: u32,
    /// Live views to ask for again after the next frame
    again: Vec<Oid>,
}

impl Captures {
    /// `view` wants its source's pixels
    pub fn want(&mut self, view: Oid) {
        if !self.queue.contains(&view) {
            self.queue.push_back(view);
        }
    }

    /// Queues the live views again after a frame
    pub fn frame_drawn(&mut self) {
        for v in std::mem::take(&mut self.again) {
            self.want(v);
        }
    }

    /// Sends the next request if none is outstanding. Releases the pool when no
    /// view is waiting or live
    pub fn pump(&mut self, c: &mut Conn, shm: u32, desk: u32, engine: &Engine, scene: &mut Scene, screen: usize) {
        if self.asked.is_some() || desk == 0 {
            return;
        }
        if self.queue.is_empty() && self.again.is_empty() {
            self.drop_pool(c);
            return;
        }
        while let Some(v) = self.queue.pop_front() {
            if !engine.core.0.borrow().objs.is_live(v) {
                scene.set_capture(&engine.core, v, None);
                continue;
            }
            let Some(window) = view::window(&engine.core, v) else {
                scene.set_capture(&engine.core, v, None);
                engine.with_ctx(|ctx, core| view::lost(ctx, core, v));
                continue;
            };
            if self.pool.is_none() && !self.grow(c, shm, screen.max(1)) {
                return;
            }
            let Some((_, obj)) = self.pool.as_ref() else { return };
            self.serial = self.serial.wrapping_add(1);
            c.desk_capture(desk, window.0, window.1, *obj, 0, self.serial);
            self.asked = Some((self.serial, v, window));
            return;
        }
    }

    /// `CAPTURED`: copies the pixels to the view's picture, or grows the pool
    /// and retries, or marks the view empty
    #[allow(clippy::too_many_arguments)]
    pub fn answer(&mut self, c: &mut Conn, shm: u32, engine: &Engine, scene: &mut Scene,
                  serial: u32, copied: bool, w: i32, h: i32, format: u32) {
        let Some((asked, v, _)) = self.asked else { return };
        if asked != serial {
            return;
        }
        self.asked = None;
        let live = engine.core.0.borrow().objs.is_live(v);
        let bytes = (w.max(0) as usize) * (h.max(0) as usize) * 4;
        if !copied {
            if w > 0 && h > 0 && bytes <= MOST && self.grow(c, shm, bytes) && live {
                self.queue.push_front(v);
            } else if live {
                scene.set_capture(&engine.core, v, None);
                engine.with_ctx(|ctx, core| view::lost(ctx, core, v));
            }
            return;
        }
        if !live {
            return;
        }
        let Some((pool, _)) = self.pool.as_ref() else { return };
        let opaque = format == protocol::shm::FORMAT_XRGB8888;
        let px: Vec<u32> = pool.as_slice()[..bytes].chunks_exact(4)
            .map(|b| {
                let p = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if opaque { p | 0xFF00_0000 } else { p }
            })
            .collect();
        scene.set_capture(&engine.core, v, Some(Rc::new(Picture::new(w as usize, h as usize, px))));
        engine.with_ctx(|ctx, core| view::took(ctx, core, v, w, h));
        if engine.core.flag(v, "live") && !self.again.contains(&v) {
            self.again.push(v);
        }
    }

    /// Replaces the pool with one of at least `bytes`
    fn grow(&mut self, c: &mut Conn, shm: u32, bytes: usize) -> bool {
        if self.pool.as_ref().is_some_and(|(p, _)| p.size >= bytes) {
            return true;
        }
        self.drop_pool(c);
        let Some(p) = Pool::new(bytes) else {
            crate::say(&format!("the kernel would not give us {bytes} bytes to copy a window into"));
            return false;
        };
        let obj = c.create_pool(shm, p.handle, bytes as i32);
        self.pool = Some((p, obj));
        true
    }

    fn drop_pool(&mut self, c: &mut Conn) {
        if let Some((mut p, obj)) = self.pool.take() {
            c.destroy_pool(obj);
            // The compositor must see the pool destroyed before the memory is
            // freed
            c.flush();
            p.free();
        }
    }
}
