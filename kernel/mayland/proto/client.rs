// Client side of the protocol.
//
// Handles request encoding, event decoding and id allocation. Transport is
// abstracted so the protocol can be tested without a userspace client library.
#![allow(dead_code)]

extern crate alloc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::mayland::protocol::{self, Interface, DISPLAY_ID};
use crate::mayland::outq::Writer;
use crate::mayland::wire::{self, Arg, Reader};

/// Decoded protocol event.
#[derive(Clone, Debug)]
pub enum Event {
    /// Connection error reported by the server.
    Error { object: u32, code: u32, message: String },
    /// Released object id that can be reused by the client.
    DeleteId { id: u32 },
    Global { name: u32, interface: String, version: u32 },
    GlobalRemove { name: u32 },
    /// Sync or frame callback.
    Done { id: u32, data: u32 },
    ShmFormat { format: u32 },
    SeatCapabilities { caps: u32 },
    SeatName { name: String },
    OutputMode { flags: u32, width: i32, height: i32, refresh: i32 },
    OutputDone,
    OutputName { name: String },
    OutputDescription { description: String },
    /// `may_output_manager`: one connector, then its modes, then `done`
    OutHead {
        name: String, description: String, flags: u32, current: i32, preferred: i32,
        adaptive_sync: u32, max_bpc: u32, bpc: u32,
    },
    OutMode { head: String, index: u32, width: i32, height: i32, refresh: i32 },
    OutDone { serial: u32 },
    OutFinished,
    /// `may_output_configuration`'s answer, `config` being the object
    OutConfigSucceeded { config: u32 },
    /// A focus grab is over: a press beside its surfaces, or another's grab
    FocusGrabCleared { grab: u32 },
    OutConfigFailed { config: u32, reason: String },
    OutConfigCancelled { config: u32 },
    KeyboardKeymap { format: u32, handle: u32, size: u32 },
    KeyboardEnter { surface: u32 },
    KeyboardLeave { surface: u32 },
    /// `serial` is the compositor's number for this keystroke, quoted back by
    /// two requests: taking the pointer for a menu, and setting the selection
    Key { keycode: u32, pressed: bool, serial: u32 },
    /// Modifier state and the layout group, sent before related key events.
    Modifiers { mask: u32, group: u32 },
    /// Pointer focus is independent from keyboard focus.
    PointerEnter { surface: u32, x: i32, y: i32 },
    PointerLeave { surface: u32 },
    PointerMotion { x: i32, y: i32 },
    PointerButton { button: u32, pressed: bool, serial: u32 },
    /// Axis value uses 15 units per scroll step.
    PointerAxis { axis: u32, value: i32 },
    BufferRelease { id: u32 },
    /// Width/height of zero means the client chooses the size.
    ToplevelConfigure { width: i32, height: i32, activated: bool },
    /// Close request. The client may reject it.
    ToplevelClose,
    /// Client must acknowledge before committing the buffer.
    XdgConfigure { serial: u32 },
    /// Window state as reported by the window manager.
    DeskWindow {
        client: u32, surface: u32, title: String,
        /// Application identifier used for matching rules.
        app_id: String,
        x: i32, y: i32, width: i32, height: i32,
        flags: u32, workspace: u32,
    },
    /// End of window list for this serial.
    DeskWindowsDone { serial: u32 },
    DeskOpened { client: u32, surface: u32 },
    DeskClosed { client: u32, surface: u32 },
    /// 0,0 when nothing has the keyboard
    DeskFocused { client: u32, surface: u32 },
    WmChord { id: u32 },
    DeskWorkspace { index: u32 },
    /// The keyboard layout in use: group `group` of `groups`, by code and name
    DeskKeyboardLayout { group: u32, groups: u32, code: String, name: String },
    /// Available layout area and screen size.
    DeskWorkArea {
        x: i32, y: i32, width: i32, height: i32,
        screen_w: i32, screen_h: i32,
    },
    /// What a `desk_capture` came to: copied, or the size it would need, or
    /// nothing to copy
    DeskCaptured { serial: u32, copied: bool, width: i32, height: i32, format: u32 },
    /// Where the console's character grid lives, and how big it is
    ConsoleGrid { handle: u32, size: u32, cols: u32, rows: u32 },
    /// Changed rows and current cursor position.
    ConsoleDamage { row0: u32, row1: u32, cursor_col: u32, cursor_row: u32 },
    /// The terminal this client was drawing has gone
    ConsoleGone,
    /// Layer surface size request.
    LayerConfigure { serial: u32, width: i32, height: i32 },
    LayerClosed,
    /// Where the compositor put a popup, relative to what it hangs off
    PopupConfigure { x: i32, y: i32, width: i32, height: i32 },
    /// The menu is dismissed. Sent once
    PopupDone,
    PopupRepositioned { token: u32 },
    /// There is something in this selection, offered in this type
    ClipboardOffer { which: u32, mime: String },
    /// The answer to a get: adopt the handle, read `size` bytes, free it
    ClipboardData { which: u32, handle: u32, size: u32, mime: String },
    ClipboardLost { which: u32 },
    /// Unknown events can be ignored by clients.
    Other { object: u32, opcode: u16 },
}

/// Check if a configure state list contains a value.
fn has_state(states: &[u8], want: u32) -> bool {
    states.chunks_exact(4)
        .any(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) == want)
}

pub struct ClientConn {
    /// Next id to allocate. Client ids start at 2 because the display uses 1.
    next_id: u32,
    out: Writer,
    /// Interface assigned to each created object id, used for event decoding.
    kinds: Vec<(u32, Interface)>,
}

impl ClientConn {
    pub fn new() -> Self {
        let mut kinds = Vec::new();
        kinds.push((DISPLAY_ID, Interface::Display));
        ClientConn { next_id: 2, out: Writer::new(), kinds }
    }

    fn alloc(&mut self, iface: Interface) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.kinds.push((id, iface));
        id
    }

    fn kind_of(&self, id: u32) -> Option<Interface> {
        self.kinds.iter().find(|(i, _)| *i == id).map(|(_, k)| *k)
    }

    /// Take pending request bytes for the transport.
    pub fn take(&mut self) -> Vec<u8> { self.out.take() }

    pub fn pending(&self) -> usize { self.out.len() }

    // Requests are defined in `requests.rs`, shared with the userspace client.
    // This side handles transport and object ids.
    //
    // Events
    /// Decode complete events from a byte stream. Incomplete data is left for
    /// the next read.
    pub fn decode(&self, mut bytes: &[u8]) -> (Vec<Event>, usize) {
        let mut out = Vec::new();
        let mut consumed = 0;
        loop {
            let header = match wire::peek_header(bytes) {
                Ok(Some(h)) => h,
                _ => break,
            };
            let body = &bytes[wire::HEADER_LEN..header.size];
            out.push(self.decode_one(header.sender, header.opcode, body));
            bytes = &bytes[header.size..];
            consumed += header.size;
        }
        (out, consumed)
    }

    fn decode_one(&self, sender: u32, opcode: u16, body: &[u8]) -> Event {
        let mut r = Reader::new(body);
        let unknown = Event::Other { object: sender, opcode };

        match self.kind_of(sender) {
            Some(Interface::Display) => match opcode {
                protocol::display::ERROR => {
                    let object = r.object().unwrap_or(0);
                    let code = r.uint().unwrap_or(0);
                    let message = r.string().ok().flatten().unwrap_or("").into();
                    Event::Error { object, code, message }
                }
                protocol::display::DELETE_ID => {
                    Event::DeleteId { id: r.uint().unwrap_or(0) }
                }
                _ => unknown,
            },
            Some(Interface::Registry) => match opcode {
                protocol::registry::GLOBAL => {
                    let name = r.uint().unwrap_or(0);
                    let interface = r.string().ok().flatten().unwrap_or("").into();
                    let version = r.uint().unwrap_or(0);
                    Event::Global { name, interface, version }
                }
                protocol::registry::GLOBAL_REMOVE => {
                    Event::GlobalRemove { name: r.uint().unwrap_or(0) }
                }
                _ => unknown,
            },
            Some(Interface::Callback) if opcode == protocol::callback::DONE => {
                Event::Done { id: sender, data: r.uint().unwrap_or(0) }
            }
            Some(Interface::Shm) if opcode == protocol::shm::FORMAT => {
                Event::ShmFormat { format: r.uint().unwrap_or(0) }
            }
            Some(Interface::Seat) => match opcode {
                protocol::seat::CAPABILITIES => {
                    Event::SeatCapabilities { caps: r.uint().unwrap_or(0) }
                }
                protocol::seat::NAME => {
                    Event::SeatName { name: r.string().ok().flatten().unwrap_or("").into() }
                }
                _ => unknown,
            },
            Some(Interface::Output) => match opcode {
                protocol::output::MODE => Event::OutputMode {
                    flags:   r.uint().unwrap_or(0),
                    width:   r.int().unwrap_or(0),
                    height:  r.int().unwrap_or(0),
                    refresh: r.int().unwrap_or(0),
                },
                protocol::output::DONE => Event::OutputDone,
                protocol::output::NAME => Event::OutputName {
                    name: r.string().ok().flatten().unwrap_or("").into(),
                },
                protocol::output::DESCRIPTION => Event::OutputDescription {
                    description: r.string().ok().flatten().unwrap_or("").into(),
                },
                _ => unknown,
            },
            Some(Interface::OutputManager) => match opcode {
                protocol::output_manager::HEAD => Event::OutHead {
                    name: r.string().ok().flatten().unwrap_or("").into(),
                    description: r.string().ok().flatten().unwrap_or("").into(),
                    flags: r.uint().unwrap_or(0),
                    current: r.int().unwrap_or(-1),
                    preferred: r.int().unwrap_or(-1),
                    adaptive_sync: r.uint().unwrap_or(0),
                    max_bpc: r.uint().unwrap_or(0),
                    bpc: r.uint().unwrap_or(0),
                },
                protocol::output_manager::MODE => Event::OutMode {
                    head: r.string().ok().flatten().unwrap_or("").into(),
                    index: r.uint().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    refresh: r.int().unwrap_or(0),
                },
                protocol::output_manager::DONE => Event::OutDone { serial: r.uint().unwrap_or(0) },
                protocol::output_manager::FINISHED => Event::OutFinished,
                _ => unknown,
            },
            Some(Interface::FocusGrab) if opcode == protocol::focus_grab::CLEARED => {
            Event::FocusGrabCleared { grab: sender }
        }
        Some(Interface::OutputConfiguration) => match opcode {
                protocol::output_configuration::SUCCEEDED => Event::OutConfigSucceeded { config: sender },
                protocol::output_configuration::FAILED => Event::OutConfigFailed {
                    config: sender,
                    reason: r.string().ok().flatten().unwrap_or("").into(),
                },
                protocol::output_configuration::CANCELLED => Event::OutConfigCancelled { config: sender },
                _ => unknown,
            },
            Some(Interface::Keyboard) if opcode == protocol::keyboard::KEYMAP => {
                Event::KeyboardKeymap {
                    format: r.uint().unwrap_or(0),
                    handle: r.uint().unwrap_or(0),
                    size:   r.uint().unwrap_or(0),
                }
            }
            Some(Interface::Buffer) if opcode == protocol::buffer::RELEASE => {
                Event::BufferRelease { id: sender }
            }
            Some(Interface::Keyboard) => match opcode {
                protocol::keyboard::ENTER => {
                    let _serial = r.uint().unwrap_or(0);
                    Event::KeyboardEnter { surface: r.object().unwrap_or(0) }
                }
                protocol::keyboard::LEAVE => {
                    let _serial = r.uint().unwrap_or(0);
                    Event::KeyboardLeave { surface: r.object().unwrap_or(0) }
                }
                protocol::keyboard::KEY => {
                    let serial = r.uint().unwrap_or(0);
                    let _time = r.uint().unwrap_or(0);
                    let keycode = r.uint().unwrap_or(0);
                    let state = r.uint().unwrap_or(0);
                    Event::Key {
                        keycode,
                        pressed: state == protocol::keyboard::KEY_PRESSED,
                        serial,
                    }
                }
                protocol::keyboard::MODIFIERS => {
                    let _serial = r.uint().unwrap_or(0);
                    // Depressed and group are what the compositor fills;
                    // latched and locked are in the message
                    let mask = r.uint().unwrap_or(0);
                    let _latched = r.uint().unwrap_or(0);
                    let _locked = r.uint().unwrap_or(0);
                    Event::Modifiers { mask, group: r.uint().unwrap_or(0) }
                }
                _ => unknown,
            },
            Some(Interface::Pointer) => match opcode {
                protocol::pointer::ENTER => {
                    let _serial = r.uint().unwrap_or(0);
                    let surface = r.object().unwrap_or(0);
                    let x = r.fixed().unwrap_or(0);
                    let y = r.fixed().unwrap_or(0);
                    Event::PointerEnter {
                        surface,
                        x: wire::fixed_to_int(x),
                        y: wire::fixed_to_int(y),
                    }
                }
                protocol::pointer::LEAVE => {
                    let _serial = r.uint().unwrap_or(0);
                    Event::PointerLeave { surface: r.object().unwrap_or(0) }
                }
                protocol::pointer::MOTION => {
                    let _time = r.uint().unwrap_or(0);
                    let x = r.fixed().unwrap_or(0);
                    let y = r.fixed().unwrap_or(0);
                    Event::PointerMotion { x: wire::fixed_to_int(x), y: wire::fixed_to_int(y) }
                }
                protocol::pointer::BUTTON => {
                    let serial = r.uint().unwrap_or(0);
                    let _time = r.uint().unwrap_or(0);
                    let button = r.uint().unwrap_or(0);
                    let state = r.uint().unwrap_or(0);
                    Event::PointerButton {
                        button,
                        pressed: state == protocol::pointer::BUTTON_PRESSED,
                        serial,
                    }
                }
                protocol::pointer::AXIS => {
                    let _time = r.uint().unwrap_or(0);
                    let axis = r.uint().unwrap_or(0);
                    Event::PointerAxis { axis, value: wire::fixed_to_int(r.fixed().unwrap_or(0)) }
                }
                _ => unknown,
            },
            Some(Interface::XdgSurface) if opcode == protocol::xdg_surface::CONFIGURE => {
                Event::XdgConfigure { serial: r.uint().unwrap_or(0) }
            }
            Some(Interface::Toplevel) if opcode == protocol::toplevel::CLOSE => {
                Event::ToplevelClose
            }
            Some(Interface::Toplevel) if opcode == protocol::toplevel::CONFIGURE => {
                let width = r.int().unwrap_or(0);
                let height = r.int().unwrap_or(0);
                let states = r.array().unwrap_or(&[]);
                Event::ToplevelConfigure {
                    width, height,
                    activated: has_state(states, protocol::toplevel::STATE_ACTIVATED),
                }
            }
            Some(Interface::Desktop) => match opcode {
                // The in-kernel half always binds the newest version, so the
                // app id is always there. `conn.rs` has the version test, as a
                // ring-3 program can be older than the compositor
                protocol::desktop::WINDOW => Event::DeskWindow {
                    client: r.uint().unwrap_or(0),
                    surface: r.uint().unwrap_or(0),
                    title: r.string().ok().flatten().unwrap_or("").into(),
                    app_id: r.string().ok().flatten().unwrap_or("").into(),
                    x: r.int().unwrap_or(0),
                    y: r.int().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    flags: r.uint().unwrap_or(0),
                    workspace: r.uint().unwrap_or(0),
                },
                protocol::desktop::WINDOWS_DONE =>
                    Event::DeskWindowsDone { serial: r.uint().unwrap_or(0) },
                protocol::desktop::OPENED => Event::DeskOpened {
                    client: r.uint().unwrap_or(0), surface: r.uint().unwrap_or(0),
                },
                protocol::desktop::CLOSED => Event::DeskClosed {
                    client: r.uint().unwrap_or(0), surface: r.uint().unwrap_or(0),
                },
                protocol::desktop::FOCUSED => Event::DeskFocused {
                    client: r.uint().unwrap_or(0), surface: r.uint().unwrap_or(0),
                },
                protocol::desktop::WORKSPACE_CHANGED =>
                    Event::DeskWorkspace { index: r.uint().unwrap_or(0) },
                protocol::desktop::KEYBOARD_LAYOUT => Event::DeskKeyboardLayout {
                    group: r.uint().unwrap_or(0),
                    groups: r.uint().unwrap_or(0),
                    code: r.string().ok().flatten().unwrap_or("").into(),
                    name: r.string().ok().flatten().unwrap_or("").into(),
                },
                protocol::desktop::CAPTURED => Event::DeskCaptured {
                    serial: r.uint().unwrap_or(0),
                    copied: r.uint().unwrap_or(0) != 0,
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    format: r.uint().unwrap_or(0),
                },
                protocol::desktop::WORK_AREA => Event::DeskWorkArea {
                    x: r.int().unwrap_or(0),
                    y: r.int().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    screen_w: r.int().unwrap_or(0),
                    screen_h: r.int().unwrap_or(0),
                },
                _ => unknown,
            },
            Some(Interface::Wm) => match opcode {
                protocol::wm::CHORD => Event::WmChord { id: r.uint().unwrap_or(0) },
                _ => unknown,
            },
            Some(Interface::Console) => match opcode {
                protocol::console::GRID => Event::ConsoleGrid {
                    handle: r.uint().unwrap_or(0),
                    size: r.uint().unwrap_or(0),
                    cols: r.uint().unwrap_or(0),
                    rows: r.uint().unwrap_or(0),
                },
                protocol::console::DAMAGE => Event::ConsoleDamage {
                    row0: r.uint().unwrap_or(0),
                    row1: r.uint().unwrap_or(0),
                    cursor_col: r.uint().unwrap_or(0),
                    cursor_row: r.uint().unwrap_or(0),
                },
                protocol::console::GONE => Event::ConsoleGone,
                _ => unknown,
            },
            Some(Interface::LayerSurface) => match opcode {
                protocol::layer_surface::CONFIGURE => Event::LayerConfigure {
                    serial: r.uint().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                },
                protocol::layer_surface::CLOSED => Event::LayerClosed,
                _ => unknown,
            },
            Some(Interface::Popup) => match opcode {
                protocol::popup::CONFIGURE => Event::PopupConfigure {
                    x: r.int().unwrap_or(0),
                    y: r.int().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                },
                protocol::popup::POPUP_DONE => Event::PopupDone,
                protocol::popup::REPOSITIONED =>
                    Event::PopupRepositioned { token: r.uint().unwrap_or(0) },
                _ => unknown,
            },
            Some(Interface::Clipboard) => match opcode {
                protocol::clipboard::OFFER => Event::ClipboardOffer {
                    which: r.uint().unwrap_or(0),
                    mime: r.string().ok().flatten().unwrap_or("").into(),
                },
                protocol::clipboard::DATA => Event::ClipboardData {
                    which: r.uint().unwrap_or(0),
                    handle: r.uint().unwrap_or(0),
                    size: r.uint().unwrap_or(0),
                    mime: r.string().ok().flatten().unwrap_or("").into(),
                },
                protocol::clipboard::LOST =>
                    Event::ClipboardLost { which: r.uint().unwrap_or(0) },
                _ => unknown,
            },
            _ => unknown,
        }
    }
}

/// Request output and object id allocation for the client connection.
impl crate::mayland::requests::Sink for ClientConn {
    fn put(&mut self, sender: u32, opcode: u16, args: &[Arg]) {
        let _ = self.out.message(sender, opcode, args);
    }

    fn alloc_id(&mut self, iface: Interface) -> u32 {
        self.alloc(iface)
    }
}
