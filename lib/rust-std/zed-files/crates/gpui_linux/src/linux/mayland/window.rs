//! One gpui window: a Mayland surface with the xdg toplevel role.
//!
//! The pixels are a pool of two buffers the size of the window, drawn whole on
//! every frame gpui asks to present and handed to the compositor with a frame
//! callback. A buffer the compositor has not released is not drawn into; a
//! frame with no free buffer is put off to the next tick.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, Capslock, DispatchEventResult, GpuSpecs, Modifiers, Pixels, PlatformAtlas,
    PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point, PromptButton,
    PromptLevel, RequestFrameOptions, Scene, Size, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowParams, WindowVisibility, px, size,
};
use libmay::conn::{Conn, Pool};
use libmay::protocol;
use libmay::requests::Requests;

use super::renderer::{CpuRenderer, Target};

const BUFFERS: usize = 2;

/// The window's pixels at its current size
struct Frame {
    pool: Option<Pool>,
    pool_object: u32,
    /// (buffer id, the compositor has released it)
    buffers: Vec<(u32, bool)>,
    width: i32,
    height: i32,
    bytes: usize,
}

impl Frame {
    fn none() -> Self {
        Frame {
            pool: None,
            pool_object: 0,
            buffers: Vec::new(),
            width: 0,
            height: 0,
            bytes: 0,
        }
    }

    /// Two buffers if the kernel has that much contiguous memory, one if not
    fn make(conn: &mut Conn, shm: u32, width: i32, height: i32) -> Option<Self> {
        if width <= 0 || height <= 0 {
            return None;
        }
        let stride = width * 4;
        let bytes = (stride * height) as usize;
        let (pool, count) = match Pool::new(bytes * BUFFERS) {
            Some(pool) => (pool, BUFFERS),
            None => (Pool::new(bytes)?, 1),
        };
        let pool_object = conn.create_pool(shm, pool.handle, (bytes * count) as i32);
        let buffers = (0..count)
            .map(|i| {
                let id = conn.create_buffer(
                    pool_object,
                    (i * bytes) as i32,
                    width,
                    height,
                    stride,
                    protocol::shm::FORMAT_XRGB8888,
                );
                (id, true)
            })
            .collect();
        Some(Frame {
            pool: Some(pool),
            pool_object,
            buffers,
            width,
            height,
            bytes,
        })
    }

    /// Buffers before the pool, and the pool's memory last: nothing is alive
    /// by the time the shared-memory handle goes
    fn teardown(&mut self, conn: &mut Conn) {
        for (id, _) in self.buffers.drain(..) {
            conn.destroy_buffer(id);
        }
        if self.pool_object != 0 {
            conn.destroy_pool(self.pool_object);
            self.pool_object = 0;
        }
        conn.flush();
        if let Some(mut pool) = self.pool.take() {
            pool.free();
        }
    }

    fn free_slot(&self) -> Option<usize> {
        if self.buffers.len() == 1 {
            return Some(0);
        }
        self.buffers.iter().position(|(_, free)| *free)
    }

    fn released(&mut self, id: u32) -> bool {
        let mut found = false;
        for (buffer, free) in &mut self.buffers {
            if *buffer == id {
                *free = true;
                found = true;
            }
        }
        found
    }
}

#[derive(Default)]
struct Callbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    moved: Option<Box<dyn FnMut()>>,
    should_close: Option<Box<dyn FnMut() -> bool>>,
    close: Option<Box<dyn FnOnce()>>,
    appearance_changed: Option<Box<dyn FnMut()>>,
}

struct State {
    conn: Rc<RefCell<Conn>>,
    shm: u32,
    surface: u32,
    xdg_surface: u32,
    toplevel: u32,
    display: Rc<dyn PlatformDisplay>,
    bounds: Bounds<Pixels>,
    frame: Frame,
    renderer: CpuRenderer,
    title: String,
    input_handler: Option<PlatformInputHandler>,
    active: bool,
    hovered: bool,
    fullscreen: bool,
    mouse_position: Point<Pixels>,
    modifiers: Modifiers,
    capslock: Capslock,
}

/// What the client keeps of a window to route events to it. The window gpui
/// holds is a `MaylandWindow` around the same thing, and dropping that one is
/// what closes it
pub(crate) struct Inner {
    state: RefCell<State>,
    callbacks: RefCell<Callbacks>,
    /// The first configure has been acknowledged: the compositor refuses a
    /// buffer committed before it
    configured: Cell<bool>,
    /// A frame callback is outstanding; the next frame waits for it
    awaiting: Cell<Option<u32>>,
    /// Something changed that gpui has not drawn yet: a resize, or a frame put
    /// off for want of a free buffer
    redraw: Cell<bool>,
    closed: Cell<bool>,
}

pub(crate) struct MaylandWindow(pub(crate) Rc<Inner>);

impl MaylandWindow {
    pub(crate) fn new(
        conn: Rc<RefCell<Conn>>,
        globals: (u32, u32, u32),
        display: Rc<dyn PlatformDisplay>,
        params: WindowParams,
    ) -> Self {
        let (compositor, shm, xdg) = globals;
        let title = params
            .titlebar
            .as_ref()
            .and_then(|titlebar| titlebar.title.as_ref())
            .map(|title| title.to_string())
            .unwrap_or_default();
        let (surface, xdg_surface, toplevel) = {
            let mut c = conn.borrow_mut();
            let surface = c.create_surface(compositor);
            let xdg_surface = c.get_xdg_surface(xdg, surface);
            let toplevel = c.get_toplevel(xdg_surface);
            c.set_title(toplevel, if title.is_empty() { "Zed" } else { &title });
            c.set_app_id(toplevel, "dev.zed.Zed");
            c.flush();
            (surface, xdg_surface, toplevel)
        };
        MaylandWindow(Rc::new(Inner {
            state: RefCell::new(State {
                conn,
                shm,
                surface,
                xdg_surface,
                toplevel,
                display,
                bounds: params.bounds,
                frame: Frame::none(),
                renderer: CpuRenderer::new(),
                title,
                input_handler: None,
                active: false,
                hovered: false,
                fullscreen: false,
                mouse_position: Point::default(),
                modifiers: Modifiers::default(),
                capslock: Capslock::default(),
            }),
            callbacks: RefCell::new(Callbacks::default()),
            configured: Cell::new(false),
            awaiting: Cell::new(None),
            redraw: Cell::new(true),
            closed: Cell::new(false),
        }))
    }
}

impl Inner {
    pub(crate) fn surface(&self) -> u32 {
        self.state.borrow().surface
    }

    /// The objects the compositor's configures and close are sent to, which
    /// is how the client knows which window one is for
    pub(crate) fn xdg_surface(&self) -> u32 {
        self.state.borrow().xdg_surface
    }

    pub(crate) fn toplevel(&self) -> u32 {
        self.state.borrow().toplevel
    }

    pub(crate) fn is_closed(&self) -> bool {
        self.closed.get()
    }

    /// The compositor's proposal: a size (zero means "you choose") and
    /// whether this is the window being typed at
    pub(crate) fn toplevel_configure(&self, width: i32, height: i32, activated: bool) {
        let resized = {
            let mut state = self.state.borrow_mut();
            let current = state.bounds.size;
            if width > 0
                && height > 0
                && (px(width as f32) != current.width || px(height as f32) != current.height)
            {
                state.bounds.size = size(px(width as f32), px(height as f32));
                Some(state.bounds.size)
            } else {
                None
            }
        };
        if let Some(new_size) = resized {
            self.redraw.set(true);
            let callback = self.callbacks.borrow_mut().resize.take();
            if let Some(mut callback) = callback {
                callback(new_size, 1.0);
                self.callbacks.borrow_mut().resize = Some(callback);
            }
        }
        self.set_active(activated);
    }

    pub(crate) fn xdg_configure(&self, serial: u32) {
        let state = self.state.borrow();
        let mut conn = state.conn.borrow_mut();
        conn.ack_configure(state.xdg_surface, serial);
        conn.flush();
        drop(conn);
        drop(state);
        if !self.configured.get() {
            self.configured.set(true);
            self.redraw.set(true);
        }
    }

    pub(crate) fn buffer_released(&self, id: u32) -> bool {
        self.state.borrow_mut().frame.released(id)
    }

    pub(crate) fn frame_done(&self, id: u32) -> bool {
        if self.awaiting.get() == Some(id) {
            self.awaiting.set(None);
            true
        } else {
            false
        }
    }

    /// One tick of the frame clock. gpui is asked for a frame unless the last
    /// one is still on its way to the screen; whether it draws is its call
    pub(crate) fn tick(&self) {
        if !self.configured.get() || self.awaiting.get().is_some() || self.closed.get() {
            return;
        }
        let force_render = self.redraw.get();
        let mut callbacks = self.callbacks.borrow_mut();
        if let Some(request_frame) = callbacks.request_frame.as_mut() {
            request_frame(RequestFrameOptions {
                require_presentation: force_render,
                force_render,
            });
        }
    }

    pub(crate) fn set_active(&self, active: bool) {
        {
            let mut state = self.state.borrow_mut();
            if state.active == active {
                return;
            }
            state.active = active;
        }
        let callback = self.callbacks.borrow_mut().active_status_change.take();
        if let Some(mut callback) = callback {
            callback(active);
            self.callbacks.borrow_mut().active_status_change = Some(callback);
        }
    }

    pub(crate) fn set_hovered(&self, hovered: bool) {
        self.state.borrow_mut().hovered = hovered;
        let callback = self.callbacks.borrow_mut().hover_status_change.take();
        if let Some(mut callback) = callback {
            callback(hovered);
            self.callbacks.borrow_mut().hover_status_change = Some(callback);
        }
    }

    pub(crate) fn set_input_state(
        &self,
        position: Option<Point<Pixels>>,
        modifiers: Modifiers,
        capslock: Capslock,
    ) {
        let mut state = self.state.borrow_mut();
        if let Some(position) = position {
            state.mouse_position = position;
        }
        state.modifiers = modifiers;
        state.capslock = capslock;
    }

    /// An event for this window. What gpui does not take and that types a
    /// character goes to the focused text input, which is how a keystroke
    /// becomes text on every Linux backend
    pub(crate) fn handle_input(&self, input: PlatformInput) {
        let callback = self.callbacks.borrow_mut().input.take();
        if let Some(mut callback) = callback {
            let result = callback(input.clone());
            self.callbacks.borrow_mut().input = Some(callback);
            if !result.propagate {
                return;
            }
        }
        if let PlatformInput::KeyDown(event) = input
            && event.keystroke.modifiers.is_subset_of(&Modifiers::shift())
            && let Some(key_char) = &event.keystroke.key_char
        {
            let handler = self.state.borrow_mut().input_handler.take();
            if let Some(mut handler) = handler {
                handler.replace_text_in_range(None, key_char);
                self.state.borrow_mut().input_handler = Some(handler);
            }
        }
    }

    /// The compositor would like the window gone; gpui decides
    pub(crate) fn close_requested(&self) {
        let callback = self.callbacks.borrow_mut().should_close.take();
        let close = match callback {
            Some(mut callback) => {
                let close = callback();
                self.callbacks.borrow_mut().should_close = Some(callback);
                close
            }
            None => true,
        };
        if close {
            self.close();
        }
    }

    /// Closed from this side: the window is taken down and gpui is told.
    /// Only for a close gpui did not start - see `teardown`
    fn close(&self) {
        if !self.teardown() {
            return;
        }
        let callback = self.callbacks.borrow_mut().close.take();
        if let Some(callback) = callback {
            callback();
        }
    }

    /// The surface and its roles gone, and nothing said to gpui. What a drop
    /// does: gpui drops a window it is closing itself, from inside its own
    /// update, and its close callback would re-enter the app ("RefCell already
    /// borrowed"). False when it was already down
    fn teardown(&self) -> bool {
        if self.closed.replace(true) {
            return false;
        }
        let mut state = self.state.borrow_mut();
        let conn = state.conn.clone();
        let mut conn = conn.borrow_mut();
        state.frame.teardown(&mut conn);
        conn.destroy_toplevel(state.toplevel);
        conn.destroy_xdg_surface(state.xdg_surface);
        conn.destroy_surface(state.surface);
        conn.flush();
        true
    }

    fn draw(&self, scene: &Scene) {
        if !self.configured.get() || self.closed.get() {
            return;
        }
        let mut guard = self.state.borrow_mut();
        let state = &mut *guard;
        let width = f32::from(state.bounds.size.width).round() as i32;
        let height = f32::from(state.bounds.size.height).round() as i32;
        if state.frame.width != width || state.frame.height != height || state.frame.pool.is_none() {
            let conn = state.conn.clone();
            let mut conn = conn.borrow_mut();
            state.frame.teardown(&mut conn);
            match Frame::make(&mut conn, state.shm, width, height) {
                Some(frame) => state.frame = frame,
                None => {
                    log::error!("mayland: no memory for a {width}x{height} window");
                    return;
                }
            }
        }
        let Some(slot) = state.frame.free_slot() else {
            // Both buffers are the compositor's. Drawing is put off, not
            // dropped: the next tick asks again with `force_render`
            self.redraw.set(true);
            return;
        };
        let (bytes, frame_width, frame_height) =
            (state.frame.bytes, state.frame.width as usize, state.frame.height as usize);
        let buffer = state.frame.buffers[slot].0;
        let Some(pool) = state.frame.pool.as_mut() else {
            return;
        };
        let pixels = &mut pool.bytes()[slot * bytes..(slot + 1) * bytes];
        state.renderer.draw(
            scene,
            &mut Target {
                pixels,
                width: frame_width,
                height: frame_height,
            },
        );
        let mut conn = state.conn.borrow_mut();
        conn.attach(state.surface, buffer, 0, 0);
        conn.damage(state.surface, 0, 0, frame_width as i32, frame_height as i32);
        let callback = conn.frame(state.surface);
        conn.commit(state.surface);
        conn.flush();
        if state.frame.buffers.len() > 1 {
            state.frame.buffers[slot].1 = false;
        }
        self.awaiting.set(Some(callback));
        self.redraw.set(false);
    }
}

impl Drop for MaylandWindow {
    fn drop(&mut self) {
        self.0.teardown();
    }
}

impl raw_window_handle::HasWindowHandle for MaylandWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        // A Mayland surface is not a native window anything outside this
        // backend could draw into
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl raw_window_handle::HasDisplayHandle for MaylandWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl PlatformWindow for MaylandWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        self.0.state.borrow().bounds
    }

    fn is_maximized(&self) -> bool {
        // A tiling desktop: every window has the size it was given
        true
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.bounds().size
    }

    fn resize(&mut self, size: Size<Pixels>) {
        // The compositor decides a window's size; asking for one it has not
        // proposed would draw a buffer it did not configure
        log::debug!("mayland: resize to {size:?} ignored, the compositor sizes windows");
    }

    fn scale_factor(&self) -> f32 {
        1.0
    }

    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Dark
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.0.state.borrow().display.clone())
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.0.state.borrow().mouse_position
    }

    fn modifiers(&self) -> Modifiers {
        self.0.state.borrow().modifiers
    }

    fn capslock(&self) -> Capslock {
        self.0.state.borrow().capslock
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.state.borrow_mut().input_handler.take()
    }

    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        // gpui draws its own prompts when the platform has none
        None
    }

    fn activate(&self) {}

    fn is_active(&self) -> bool {
        self.0.state.borrow().active
    }

    fn visibility(&self) -> WindowVisibility {
        WindowVisibility::Visible
    }

    fn is_hovered(&self) -> bool {
        self.0.state.borrow().hovered
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_title(&mut self, title: &str) {
        let mut state = self.0.state.borrow_mut();
        if state.title == title {
            return;
        }
        state.title = title.to_owned();
        let mut conn = state.conn.borrow_mut();
        conn.set_title(state.toplevel, title);
        conn.flush();
    }

    fn get_title(&self) -> String {
        self.0.state.borrow().title.clone()
    }

    fn set_background_appearance(&self, _background: WindowBackgroundAppearance) {}

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {
        let mut state = self.0.state.borrow_mut();
        state.fullscreen = !state.fullscreen;
    }

    fn is_fullscreen(&self) -> bool {
        self.0.state.borrow().fullscreen
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    fn on_visibility_change(&self, _callback: Box<dyn FnMut(WindowVisibility)>) {}

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.callbacks.borrow_mut().resize = Some(callback);
    }

    fn on_moved(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().moved = Some(callback);
    }

    fn on_should_close(&self, callback: Box<dyn FnMut() -> bool>) {
        self.0.callbacks.borrow_mut().should_close = Some(callback);
    }

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.0.callbacks.borrow_mut().close = Some(callback);
    }

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        self.0.draw(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.state.borrow().renderer.sprite_atlas()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }
}
