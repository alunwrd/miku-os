//! The connection to Mayland and everything that is not one window: the
//! globals, the seat, the frame clock, the clipboard, and routing each event
//! to the window it is about.

use std::cell::RefCell;
use std::os::fd::{AsFd, BorrowedFd, RawFd};
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{EventLoop, Interest, LoopHandle, Mode, PostAction};
use gpui::{
    AnyWindowHandle, Bounds, Capslock, ClipboardItem, CursorStyle, DisplayId, KeyDownEvent,
    KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton, MouseDownEvent,
    MouseExitEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, PlatformDisplay,
    PlatformInput, PlatformKeyboardLayout, PlatformWindow, Point, ScrollDelta, ScrollWheelEvent,
    TouchPhase, WindowParams, point, px, size,
};
use gpui_util::ResultExt;
use libmay::conn::{Conn, Event, Pool};
use libmay::keymap::{self, Named};
use libmay::protocol::{self, Interface};
use libmay::requests::Requests;
use uuid::Uuid;

use super::window::{Inner, MaylandWindow};
use crate::linux::{
    DOUBLE_CLICK_INTERVAL, LinuxClient, LinuxCommon, LinuxKeyboardLayout, SCROLL_LINES,
    is_within_click_distance,
};

/// How often a window is offered a frame. The compositor's frame callback paces
/// drawing: a window with one outstanding is skipped
const TICK: Duration = Duration::from_millis(16);

/// How long a paste waits for the compositor to hand the selection over
const CLIPBOARD_WAIT: Duration = Duration::from_millis(500);

const MIME_TEXT: &str = "text/plain;charset=utf-8";

#[derive(Debug)]
pub(crate) struct MaylandDisplay {
    bounds: Bounds<Pixels>,
}

impl PlatformDisplay for MaylandDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(0)
    }

    fn uuid(&self) -> anyhow::Result<Uuid> {
        Ok(Uuid::nil())
    }

    fn bounds(&self) -> Bounds<Pixels> {
        self.bounds
    }
}

/// The connection's descriptor, lent to calloop to wait on. `Conn` owns and
/// closes it
struct ConnFd(RawFd);

impl AsFd for ConnFd {
    fn as_fd(&self) -> BorrowedFd<'_> {
        // Valid as long as the client, which holds the `Conn`, is alive; the
        // event loop holding this is the client's
        unsafe { BorrowedFd::borrow_raw(self.0) }
    }
}

struct Globals {
    compositor: u32,
    shm: u32,
    xdg: u32,
    clipboard: u32,
}

#[derive(Default)]
struct Clicks {
    last_click: Option<Instant>,
    last_button: Option<MouseButton>,
    last_location: Point<Pixels>,
    count: usize,
}

struct Repeat {
    keycode: u32,
    generation: u64,
}

pub(crate) struct MaylandState {
    event_loop: Option<EventLoop<'static, MaylandClient>>,
    loop_handle: LoopHandle<'static, MaylandClient>,
    common: LinuxCommon,
    conn: Rc<RefCell<Conn>>,
    globals: Globals,
    display: Rc<MaylandDisplay>,
    windows: Vec<Rc<Inner>>,
    keyboard_focus: Option<Rc<Inner>>,
    pointer_focus: Option<Rc<Inner>>,
    layout: Option<Pool>,
    mods: u32,
    /// The keyboard layout in use, an index into the keymap's groups: the last
    /// field of the compositor's `modifiers`
    group: u32,
    modifiers: Modifiers,
    capslock: Capslock,
    /// The compositor's number for the last key or button, which setting and
    /// getting the selection quote
    serial: u32,
    mouse_position: Point<Pixels>,
    pressed_button: Option<MouseButton>,
    clicks: Clicks,
    /// (rate per second, delay in ms)
    repeat_info: (i32, i32),
    repeat: Option<Repeat>,
    repeat_generation: u64,
    /// What the compositor last said is in each selection
    offered: [bool; 2],
    /// Events read while waiting for something else (the selection), not yet
    /// dispatched
    pending: Vec<Event>,
    events: Vec<Event>,
}

#[derive(Clone)]
pub(crate) struct MaylandClient(Rc<RefCell<MaylandState>>);

fn empty_events(n: usize) -> Vec<Event> {
    vec![Event::Other { object: 0, opcode: 0 }; n]
}

impl MaylandClient {
    pub(crate) fn new() -> Self {
        let event_loop = EventLoop::try_new().expect("an event loop");
        let (common, main_receiver, power_receiver) = LinuxCommon::new(event_loop.get_signal());
        let handle = event_loop.handle();

        handle
            .insert_source(main_receiver, |event, _, _: &mut MaylandClient| {
                if let calloop::channel::Event::Msg(runnable) = event {
                    runnable.run();
                }
            })
            .log_err();
        handle
            .insert_source(power_receiver, |event, _, client: &mut MaylandClient| {
                if let calloop::channel::Event::Msg(event) = event {
                    client.with_common(|common| common.handle_system_power_event(event));
                }
            })
            .log_err();

        let mut conn = Conn::open().expect("no Mayland display server to connect to");
        let (registry, advertised) = conn.handshake();
        if advertised.failed {
            panic!("the Mayland display server refused the handshake");
        }
        let bind = |conn: &mut Conn, iface: Interface| {
            let name = advertised.name(iface);
            if name == 0 { 0 } else { conn.bind(registry, name, iface, 1) }
        };
        let compositor = bind(&mut conn, Interface::Compositor);
        let shm = bind(&mut conn, Interface::Shm);
        let xdg = bind(&mut conn, Interface::Xdg);
        let clipboard = bind(&mut conn, Interface::Clipboard);
        let seat = bind(&mut conn, Interface::Seat);
        if seat != 0 {
            conn.get_keyboard(seat);
            conn.get_pointer(seat);
        }
        bind(&mut conn, Interface::Output);
        conn.sync();
        conn.flush();
        if compositor == 0 || shm == 0 || xdg == 0 {
            panic!("the Mayland display server has no compositor, shm or xdg global");
        }

        // The output's size and the keymap arrive in answer to the binds
        let mut screen = (1280, 800);
        let mut layout = None;
        let mut pending = Vec::new();
        let mut events = empty_events(64);
        let mut settled = false;
        while !settled {
            let n = conn.read_events(&mut events);
            if n == 0 && conn.closed {
                panic!("the Mayland display server closed the connection");
            }
            for event in &events[..n] {
                match *event {
                    Event::OutputMode { width, height, .. } if width > 0 && height > 0 => {
                        screen = (width, height);
                    }
                    Event::Done { .. } => settled = true,
                    Event::Keymap { format, handle, size } => {
                        layout = adopt_layout(format, handle, size);
                    }
                    other => pending.push(other),
                }
            }
        }

        let fd = conn.fd as RawFd;
        let conn = Rc::new(RefCell::new(conn));
        let client = MaylandClient(Rc::new(RefCell::new(MaylandState {
            event_loop: None,
            loop_handle: handle.clone(),
            common,
            conn,
            globals: Globals {
                compositor,
                shm,
                xdg,
                clipboard,
            },
            display: Rc::new(MaylandDisplay {
                bounds: Bounds {
                    origin: Point::default(),
                    size: size(px(screen.0 as f32), px(screen.1 as f32)),
                },
            }),
            windows: Vec::new(),
            keyboard_focus: None,
            pointer_focus: None,
            layout,
            mods: 0,
            group: 0,
            modifiers: Modifiers::default(),
            capslock: Capslock::default(),
            serial: 0,
            mouse_position: Point::default(),
            pressed_button: None,
            clicks: Clicks::default(),
            repeat_info: (25, 600),
            repeat: None,
            repeat_generation: 0,
            offered: [false; 2],
            pending,
            events: empty_events(256),
        })));

        handle
            .insert_source(
                Generic::new(ConnFd(fd), Interest::READ, Mode::Level),
                |_, _, client: &mut MaylandClient| {
                    client.read_and_dispatch();
                    Ok(PostAction::Continue)
                },
            )
            .expect("the display server connection in the event loop");
        handle
            .insert_source(Timer::from_duration(TICK), |_, _, client: &mut MaylandClient| {
                client.tick();
                TimeoutAction::ToDuration(TICK)
            })
            .log_err();

        client.0.borrow_mut().event_loop = Some(event_loop);
        client
    }

    /// Everything the socket has, and everything already read ahead of it
    fn read_and_dispatch(&self) {
        let pending = std::mem::take(&mut self.0.borrow_mut().pending);
        for event in pending {
            self.dispatch(event);
        }
        loop {
            let (events, n) = {
                let mut state = self.0.borrow_mut();
                let mut events = std::mem::take(&mut state.events);
                let n = state.conn.borrow_mut().read_events(&mut events);
                (events, n)
            };
            for event in &events[..n] {
                self.dispatch(*event);
            }
            let more = {
                let mut state = self.0.borrow_mut();
                state.events = events;
                let conn = state.conn.borrow();
                if conn.closed {
                    log::error!("mayland: {}", conn.death);
                    drop(conn);
                    state.common.signal.stop();
                    return;
                }
                conn.buffered()
            };
            if !more {
                break;
            }
        }
    }

    fn tick(&self) {
        let windows = {
            let mut state = self.0.borrow_mut();
            state.windows.retain(|window| !window.is_closed());
            state.windows.clone()
        };
        for window in windows {
            window.tick();
        }
    }

    fn window_for(&self, surface: u32) -> Option<Rc<Inner>> {
        self.window_by(|window| window.surface() == surface)
    }

    /// The open window that `which` picks out
    fn window_by(&self, which: impl Fn(&Inner) -> bool) -> Option<Rc<Inner>> {
        self.0
            .borrow()
            .windows
            .iter()
            .find(|window| !window.is_closed() && which(window))
            .cloned()
    }

    fn dispatch(&self, event: Event) {
        match event {
            // Each to the window whose object it was sent to. Acknowledging a
            // configure on any other is a protocol error, and the compositor
            // disconnects for it
            Event::XdgConfigure { xdg_surface, serial } => {
                if let Some(window) = self.window_by(|w| w.xdg_surface() == xdg_surface) {
                    window.xdg_configure(serial);
                }
            }
            Event::ToplevelConfigure {
                toplevel,
                width,
                height,
                activated,
            } => {
                if let Some(window) = self.window_by(|w| w.toplevel() == toplevel) {
                    window.toplevel_configure(width, height, activated);
                }
            }
            Event::BufferRelease { id } => {
                let windows = self.0.borrow().windows.clone();
                for window in windows {
                    if window.buffer_released(id) {
                        break;
                    }
                }
            }
            Event::Done { id, .. } => {
                let windows = self.0.borrow().windows.clone();
                for window in windows {
                    if window.frame_done(id) {
                        break;
                    }
                }
            }
            Event::Close { toplevel } => {
                if let Some(window) = self.window_by(|w| w.toplevel() == toplevel) {
                    window.close_requested();
                }
            }
            Event::Keymap { format, handle, size } => {
                self.0.borrow_mut().layout = adopt_layout(format, handle, size);
            }
            Event::RepeatInfo { rate, delay } => {
                self.0.borrow_mut().repeat_info = (rate, delay);
            }
            Event::KeyboardEnter { surface } => {
                let window = self.window_for(surface);
                self.0.borrow_mut().keyboard_focus = window.clone();
                if let Some(window) = window {
                    window.set_active(true);
                }
            }
            Event::KeyboardLeave { surface } => {
                self.0.borrow_mut().repeat = None;
                let focus = self.0.borrow_mut().keyboard_focus.take();
                if let Some(window) = focus.or_else(|| self.window_for(surface)) {
                    window.set_active(false);
                }
            }
            Event::Modifiers { mask, group } => {
                self.modifiers_changed(mask);
                self.group_changed(group);
            }
            Event::Key {
                keycode,
                pressed,
                serial,
            } => {
                self.0.borrow_mut().serial = serial;
                self.key(keycode, pressed, false);
            }
            Event::PointerEnter { surface, x, y } => {
                let window = self.window_for(surface);
                let position = point(px(x as f32), px(y as f32));
                {
                    let mut state = self.0.borrow_mut();
                    state.pointer_focus = window.clone();
                    state.mouse_position = position;
                }
                if let Some(window) = window {
                    window.set_hovered(true);
                    self.mouse_moved(&window, position);
                }
            }
            Event::PointerLeave { .. } => {
                let (window, position, pressed_button, modifiers) = {
                    let mut state = self.0.borrow_mut();
                    (
                        state.pointer_focus.take(),
                        state.mouse_position,
                        state.pressed_button,
                        state.modifiers,
                    )
                };
                if let Some(window) = window {
                    window.set_hovered(false);
                    window.handle_input(PlatformInput::MouseExited(MouseExitEvent {
                        position,
                        pressed_button,
                        modifiers,
                    }));
                }
            }
            Event::PointerMotion { x, y } => {
                let position = point(px(x as f32), px(y as f32));
                let window = {
                    let mut state = self.0.borrow_mut();
                    state.mouse_position = position;
                    state.pointer_focus.clone()
                };
                if let Some(window) = window {
                    self.mouse_moved(&window, position);
                }
            }
            Event::PointerButton {
                button,
                pressed,
                serial,
            } => self.button(button, pressed, serial),
            Event::PointerAxis { axis, value } => self.axis(axis, value),
            Event::ClipboardOffer { which, .. } => {
                if let Some(slot) = self.0.borrow_mut().offered.get_mut(which as usize) {
                    *slot = true;
                }
            }
            Event::ClipboardLost { which } => {
                if let Some(slot) = self.0.borrow_mut().offered.get_mut(which as usize) {
                    *slot = false;
                }
            }
            Event::Error { msg, msg_len, .. } => {
                let text = String::from_utf8_lossy(&msg[..msg_len.min(msg.len())]).into_owned();
                log::error!("mayland: protocol error: {text}");
            }
            _ => {}
        }
    }

    fn modifiers_changed(&self, mask: u32) {
        let modifiers = Modifiers {
            control: mask & keymap::MOD_CTRL != 0,
            alt: mask & keymap::MOD_ALT != 0,
            shift: mask & keymap::MOD_SHIFT != 0,
            platform: mask & keymap::MOD_LOGO != 0,
            function: false,
        };
        let capslock = Capslock {
            on: mask & keymap::MOD_CAPS != 0,
        };
        let window = {
            let mut state = self.0.borrow_mut();
            state.mods = mask;
            state.modifiers = modifiers;
            state.capslock = capslock;
            state.keyboard_focus.clone()
        };
        if let Some(window) = window {
            window.set_input_state(None, modifiers, capslock);
            window.handle_input(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
                modifiers,
                capslock,
            }));
        }
    }

    /// The seat moved to another layout. Zed is told as the Wayland backend
    /// tells it, through the layout-change callback, so a layout-dependent
    /// keymap is re-read
    fn group_changed(&self, group: u32) {
        let mut state = self.0.borrow_mut();
        if state.group == group {
            return;
        }
        state.group = group;
        if let Some(mut callback) = state.common.callbacks.keyboard_layout_change.take() {
            drop(state);
            callback();
            self.0.borrow_mut().common.callbacks.keyboard_layout_change = Some(callback);
        }
    }

    /// The code of the layout in use, "us" or "ru", from the keymap
    fn layout_code(&self) -> String {
        let state = self.0.borrow();
        state
            .layout
            .as_ref()
            .and_then(|pool| keymap::Table::new(pool.as_slice()))
            .map(|table| table.code(state.group as usize % table.groups().max(1)).to_owned())
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| "us".to_owned())
    }

    /// A keystroke as the other Linux backends build one: `key` is what is
    /// printed on the key (lowercase for a letter, the shifted symbol for a
    /// symbol, whose shift is then implied), `key_char` what it would type,
    /// absent under Ctrl and for keys that type nothing
    fn keystroke(&self, keycode: u32) -> Option<Keystroke> {
        let state = self.0.borrow();
        let code = keycode as u16;
        let table = state
            .layout
            .as_ref()
            .and_then(|pool| keymap::Table::new(pool.as_slice()));
        let named = match &table {
            Some(table) => table.to_named(code),
            None => keymap::to_named(code),
        };
        let typed = |mods: u32, group: u32| match &table {
            Some(table) => table.to_char_in(code, mods, group),
            None => keymap::to_char(code, mods),
        };
        let mut modifiers = state.modifiers;
        let name = |n: &str| Some(n.to_owned());
        let key = match named {
            Named::Escape => name("escape"),
            Named::Backspace => name("backspace"),
            Named::Enter => name("enter"),
            Named::Tab => name("tab"),
            Named::Up => name("up"),
            Named::Down => name("down"),
            Named::Left => name("left"),
            Named::Right => name("right"),
            Named::Home => name("home"),
            Named::End => name("end"),
            Named::PageUp => name("pageup"),
            Named::PageDown => name("pagedown"),
            Named::Insert => name("insert"),
            Named::Delete => name("delete"),
            Named::Function(n) => Some(format!("f{n}")),
            Named::Other => None,
        };
        let text_mods = state.mods & (keymap::MOD_SHIFT | keymap::MOD_CAPS);
        // A shortcut is named in group 0, US, whatever is being typed: Ctrl+С
        // is ctrl-c to a keymap, as it is to the compositor's binds. What it
        // would type comes from the layout in use
        let shortcut = modifiers.control || modifiers.alt || modifiers.platform;
        let printed_group = if shortcut { 0 } else { state.group };
        let (key, key_char) = match key {
            Some(key) => (key, None),
            None => {
                let text = typed(text_mods, state.group);
                let typed = typed(text_mods, printed_group)?;
                if typed == ' ' {
                    ("space".to_owned(), Some(" ".to_owned()))
                } else {
                    let key = if typed.is_alphabetic() {
                        typed.to_lowercase().to_string()
                    } else {
                        // Shift is implied by the symbol, as it is on the
                        // other backends: shift-1 is "!", not shift-!
                        if modifiers.shift {
                            modifiers.shift = false;
                        }
                        typed.to_string()
                    };
                    let key_char = if modifiers.control { None } else { text.map(|c| c.to_string()) };
                    (key, key_char)
                }
            }
        };
        Some(Keystroke {
            modifiers,
            key,
            key_char,
        })
    }

    fn key(&self, keycode: u32, pressed: bool, is_held: bool) {
        let Some(window) = self.0.borrow().keyboard_focus.clone() else {
            return;
        };
        let Some(keystroke) = self.keystroke(keycode) else {
            // A modifier or a lock: `modifiers` has already said so
            return;
        };
        if pressed {
            if !is_held {
                self.arm_repeat(keycode);
            }
            window.handle_input(PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held,
                prefer_character_input: false,
            }));
        } else {
            {
                let mut state = self.0.borrow_mut();
                if state.repeat.as_ref().is_some_and(|r| r.keycode == keycode) {
                    state.repeat = None;
                }
            }
            window.handle_input(PlatformInput::KeyUp(KeyUpEvent { keystroke }));
        }
    }

    /// Key repeat, from the rate and delay the compositor sent: a timer per
    /// held key, forgotten when a newer key is pressed or this one released
    fn arm_repeat(&self, keycode: u32) {
        let (handle, generation, (rate, delay)) = {
            let mut state = self.0.borrow_mut();
            state.repeat_generation += 1;
            let generation = state.repeat_generation;
            state.repeat = Some(Repeat {
                keycode,
                generation,
            });
            (state.loop_handle.clone(), generation, state.repeat_info)
        };
        if rate <= 0 {
            return;
        }
        let interval = Duration::from_millis((1000 / rate.max(1)) as u64);
        handle
            .insert_source(
                Timer::from_duration(Duration::from_millis(delay.max(0) as u64)),
                move |_, _, client: &mut MaylandClient| {
                    let still_held = client
                        .0
                        .borrow()
                        .repeat
                        .as_ref()
                        .is_some_and(|r| r.generation == generation && r.keycode == keycode);
                    if !still_held {
                        return TimeoutAction::Drop;
                    }
                    client.key(keycode, true, true);
                    TimeoutAction::ToDuration(interval)
                },
            )
            .log_err();
    }

    fn mouse_moved(&self, window: &Rc<Inner>, position: Point<Pixels>) {
        let (pressed_button, modifiers, capslock) = {
            let state = self.0.borrow();
            (state.pressed_button, state.modifiers, state.capslock)
        };
        window.set_input_state(Some(position), modifiers, capslock);
        window.handle_input(PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button,
            modifiers,
        }));
    }

    fn button(&self, button: u32, pressed: bool, serial: u32) {
        // evdev's numbers, which is what Mayland sends
        let button = match button {
            0x110 => MouseButton::Left,
            0x111 => MouseButton::Right,
            0x112 => MouseButton::Middle,
            0x113 => MouseButton::Navigate(NavigationDirection::Back),
            0x114 => MouseButton::Navigate(NavigationDirection::Forward),
            _ => return,
        };
        let (window, position, modifiers, click_count) = {
            let mut state = self.0.borrow_mut();
            state.serial = serial;
            let position = state.mouse_position;
            if pressed {
                let again = state
                    .clicks
                    .last_click
                    .is_some_and(|at| at.elapsed() < DOUBLE_CLICK_INTERVAL)
                    && state.clicks.last_button == Some(button)
                    && is_within_click_distance(state.clicks.last_location, position);
                state.clicks.count = if again { state.clicks.count + 1 } else { 1 };
                state.clicks.last_click = Some(Instant::now());
                state.clicks.last_button = Some(button);
                state.clicks.last_location = position;
                state.pressed_button = Some(button);
            } else {
                state.pressed_button = None;
            }
            (
                state.pointer_focus.clone(),
                position,
                state.modifiers,
                state.clicks.count,
            )
        };
        let Some(window) = window else {
            return;
        };
        let input = if pressed {
            PlatformInput::MouseDown(MouseDownEvent {
                button,
                position,
                modifiers,
                click_count,
                first_mouse: false,
            })
        } else {
            PlatformInput::MouseUp(MouseUpEvent {
                button,
                position,
                modifiers,
                click_count,
            })
        };
        window.handle_input(input);
    }

    /// One notch is fifteen units, down the page positive; gpui's lines are
    /// up the page positive, three to a notch as on the other backends
    fn axis(&self, axis: u32, value: i32) {
        let (window, position, modifiers) = {
            let state = self.0.borrow();
            (state.pointer_focus.clone(), state.mouse_position, state.modifiers)
        };
        let Some(window) = window else {
            return;
        };
        let lines = -(value as f32 / 15.0) * SCROLL_LINES;
        let horizontal = axis == 1 || modifiers.shift;
        let delta = if horizontal {
            point(lines, 0.0)
        } else {
            point(0.0, lines)
        };
        window.handle_input(PlatformInput::ScrollWheel(ScrollWheelEvent {
            position,
            delta: ScrollDelta::Lines(delta),
            modifiers,
            touch_phase: TouchPhase::Moved,
        }));
    }

    fn write_selection(&self, which: u32, item: ClipboardItem) {
        let Some(text) = item.text() else {
            return;
        };
        let state = self.0.borrow();
        if state.globals.clipboard == 0 || text.is_empty() {
            return;
        }
        // The bytes go in a pool of this process's; the compositor copies
        // them out before `set` returns, so the pool goes straight back
        let Some(mut pool) = Pool::new(text.len()) else {
            log::error!("mayland: no memory to copy {} bytes", text.len());
            return;
        };
        pool.bytes()[..text.len()].copy_from_slice(text.as_bytes());
        let mut conn = state.conn.borrow_mut();
        conn.clipboard_set(
            state.globals.clipboard,
            state.serial,
            which,
            MIME_TEXT,
            pool.handle,
            0,
            text.len() as u32,
        );
        conn.flush();
        drop(conn);
        pool.free();
    }

    /// The selection, asked for and waited on. Everything else that arrives
    /// meanwhile is kept for the event loop, in order
    fn read_selection(&self, which: u32) -> Option<ClipboardItem> {
        let (clipboard, serial, offered) = {
            let state = self.0.borrow();
            (
                state.globals.clipboard,
                state.serial,
                state.offered.get(which as usize).copied().unwrap_or(false),
            )
        };
        if clipboard == 0 || !offered {
            return None;
        }
        {
            let state = self.0.borrow();
            let mut conn = state.conn.borrow_mut();
            conn.clipboard_get(clipboard, serial, which, MIME_TEXT);
            conn.flush();
        }
        let deadline = Instant::now() + CLIPBOARD_WAIT;
        let mut events = empty_events(64);
        loop {
            let fd = self.0.borrow().conn.borrow().fd as RawFd;
            let buffered = self.0.borrow().conn.borrow().buffered();
            if !buffered {
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return None;
                }
                let mut poll = libc::pollfd {
                    fd,
                    events: libc::POLLIN,
                    revents: 0,
                };
                let ready = unsafe { libc::poll(&mut poll, 1, left.as_millis() as i32) };
                if ready <= 0 {
                    return None;
                }
            }
            let n = {
                let state = self.0.borrow();
                let mut conn = state.conn.borrow_mut();
                let n = conn.read_events(&mut events);
                if conn.closed {
                    return None;
                }
                n
            };
            let mut answer = None;
            for event in &events[..n] {
                match *event {
                    Event::ClipboardData {
                        which: got,
                        handle,
                        size,
                    } if got == which && answer.is_none() => {
                        answer = Some(match Pool::adopt(handle, size as usize) {
                            Some(mut pool) => {
                                let text = String::from_utf8_lossy(
                                    &pool.as_slice()[..size as usize],
                                )
                                .into_owned();
                                pool.free();
                                Some(ClipboardItem::new_string(text))
                            }
                            None => None,
                        });
                    }
                    other => self.0.borrow_mut().pending.push(other),
                }
            }
            if let Some(answer) = answer {
                return answer;
            }
        }
    }
}

fn adopt_layout(format: u32, handle: u32, size: u32) -> Option<Pool> {
    if format != protocol::keyboard::KEYMAP_MAY_V2 {
        return None;
    }
    Pool::adopt(handle, size as usize)
}

impl LinuxClient for MaylandClient {
    fn compositor_name(&self) -> &'static str {
        "Mayland"
    }

    fn with_common<R>(&self, f: impl FnOnce(&mut LinuxCommon) -> R) -> R {
        f(&mut self.0.borrow_mut().common)
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(LinuxKeyboardLayout::new(self.layout_code().into()))
    }

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![self.0.borrow().display.clone()]
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.0.borrow().display.clone())
    }

    fn display(&self, id: DisplayId) -> Option<Rc<dyn PlatformDisplay>> {
        let display: Rc<dyn PlatformDisplay> = self.0.borrow().display.clone();
        (display.id() == id).then_some(display)
    }

    #[cfg(feature = "screen-capture")]
    fn screen_capture_sources(
        &self,
    ) -> futures::channel::oneshot::Receiver<anyhow::Result<Vec<Rc<dyn gpui::ScreenCaptureSource>>>>
    {
        let (tx, rx) = futures::channel::oneshot::channel();
        tx.send(Err(anyhow::anyhow!("Mayland has no screen capture")))
            .ok();
        rx
    }

    fn open_window(
        &self,
        _handle: AnyWindowHandle,
        params: WindowParams,
    ) -> anyhow::Result<Box<dyn PlatformWindow>> {
        let (conn, globals, display) = {
            let state = self.0.borrow();
            (
                state.conn.clone(),
                (
                    state.globals.compositor,
                    state.globals.shm,
                    state.globals.xdg,
                ),
                state.display.clone() as Rc<dyn PlatformDisplay>,
            )
        };
        let window = MaylandWindow::new(conn, globals, display, params);
        self.0.borrow_mut().windows.push(window.0.clone());
        Ok(Box::new(window))
    }

    fn set_cursor_style(&self, _style: CursorStyle) {}

    fn open_uri(&self, uri: &str) {
        log::info!("mayland: nothing here opens {uri}");
    }

    fn reveal_path(&self, path: std::path::PathBuf) {
        log::info!("mayland: nothing here reveals {}", path.display());
    }

    fn write_to_primary(&self, item: ClipboardItem) {
        self.write_selection(protocol::clipboard::PRIMARY, item);
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        self.write_selection(protocol::clipboard::CLIPBOARD, item);
    }

    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.read_selection(protocol::clipboard::PRIMARY)
    }

    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.read_selection(protocol::clipboard::CLIPBOARD)
    }

    fn active_window(&self) -> Option<AnyWindowHandle> {
        None
    }

    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        None
    }

    fn run(&self) {
        let mut event_loop = self
            .0
            .borrow_mut()
            .event_loop
            .take()
            .expect("App is already running");
        event_loop.run(None, &mut self.clone(), |_| {}).log_err();
    }
}

