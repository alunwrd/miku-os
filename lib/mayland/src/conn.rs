// A Mayland connection for programs
//
// The client half of the protocol: open a socket to the display server, queue
// requests, read events. No allocation; buffers are fixed and live in the
// struct. Uses the same wire.rs and protocol.rs as the kernel.
//
// Requests are queued and sent in batches. Attach, damage, frame, and commit
// describe one frame and reach the server together, since the server acts on
// commit.

#![allow(dead_code)]

use super::protocol::{self, Interface, DISPLAY_ID};
use super::requests::{Requests, Sink};
use super::wire::{self, Arg, Reader};

// Syscall numbers. See kernel/syscall/mod.rs
const SYS_CLOSE:      u64 = 12;
const SYS_SOCKET:     u64 = 56;
const SYS_SEND:       u64 = 58;
const SYS_RECV:       u64 = 59;
const SYS_MAY_POOL:   u64 = 72;
const SYS_MAY_UNPOOL: u64 = 73;
const SYS_MAY_MAP:    u64 = 74;
const SYS_GPU_INFO:   u64 = 94;
const SYS_GPU_SUBMIT: u64 = 95;
const SYS_GPU_BIND:   u64 = 96;
const SYS_GPU_WAIT:   u64 = 97;
const SYS_GPU_PROGRAM: u64 = 98;
const SYS_GPU_ALLOC:  u64 = 121;

const AF_MAYLAND: u64 = 42;
const SOCK_STREAM: u64 = 1;

// Every argument register is `inlateout ... => _`, not `in`. `in` promises the
// register comes back unchanged, but the kernel's entry stub moves the
// arguments one register along (`mov rsi, rdi`, `mov rdx, rsi`, `mov rdi, rax`
// in kernel/syscall/mod.rs), and its epilogue restores rcx, r11, r10, r8, and
// r9 but not rdi, rsi, or rdx. Same form as `lib/libmiku/sys.rs`

#[inline(always)]
unsafe fn sc3(nr: u64, a1: u64, a2: u64, a3: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        inlateout("rdi") a1 => _, inlateout("rsi") a2 => _, inlateout("rdx") a3 => _,
        lateout("rcx") _, lateout("r11") _,
        lateout("r10") _, lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    ret
}

#[inline(always)]
unsafe fn sc4(nr: u64, a1: u64, a2: u64, a3: u64, a4: u64) -> i64 {
    let ret: i64;
    core::arch::asm!(
        "syscall",
        inlateout("rax") nr => ret,
        inlateout("rdi") a1 => _, inlateout("rsi") a2 => _, inlateout("rdx") a3 => _,
        inlateout("r10") a4 => _,
        lateout("rcx") _, lateout("r11") _,
        lateout("r8") _, lateout("r9") _,
        options(nostack)
    );
    ret
}

/// A shared-memory pool: the client's pixels and the physical memory the copy
/// engine reads
pub struct Pool {
    pub handle: u32,
    pub addr: *mut u8,
    pub size: usize,
}

impl Pool {
    /// Asks the kernel for 'size' bytes of contiguous memory, mapped here
    pub fn new(size: usize) -> Option<Pool> {
        let mut out = [0u64; 2];
        let r = unsafe {
            sc3(SYS_MAY_POOL, size as u64, out.as_mut_ptr() as u64, 0)
        };
        if r != 0 {
            return None;
        }
        Some(Pool { handle: out[0] as u32, addr: out[1] as *mut u8, size })
    }

    /// A pool of video memory (`gpu_alloc`, 121): the GPU reads it in place,
    /// and this process writes it write-combining through BAR1. Stores are fast
    /// and loads cross the bus one at a time, so fill it and let the GPU read
    /// it. `gpu_bind`, `blit_to`, and `free` take the handle as for any pool.
    /// The errno on refusal: `ENODEV` with no card or BAR1 window, `EDQUOT`
    /// when programs hold all they may
    pub fn video(size: usize) -> Result<Pool, i64> {
        let mut out = [0u64; 2];
        let r = unsafe {
            sc3(SYS_GPU_ALLOC, size as u64, 0, out.as_mut_ptr() as u64)
        };
        if r != 0 {
            return Err(r as i64);
        }
        Ok(Pool { handle: out[0] as u32, addr: out[1] as *mut u8, size })
    }

    /// Maps a pool the compositor made for this process, as for the keymap.
    /// Freed with `free`, since this process owns it
    pub fn adopt(handle: u32, size: usize) -> Option<Pool> {
        if handle == 0 || size == 0 {
            return None;
        }
        let mut addr = 0u64;
        let r = unsafe {
            sc3(SYS_MAY_MAP, handle as u64, &mut addr as *mut u64 as u64, 0)
        };
        if r != 0 {
            return None;
        }
        Some(Pool { handle, addr: addr as *mut u8, size })
    }

    /// Moves a rectangle of pixels with the GPU's copy engine.
    ///
    /// Both ends are pools this process owns; the kernel resolves every address
    /// from the handle and refuses anything outside, so no privilege is needed.
    /// Returns a fence: the engine reads the source after this returns, so
    /// nothing may rewrite it until `gpu_wait` says the batch is done.
    ///
    /// `None` means no capable GPU, refused arguments, or a submission already
    /// in flight from this process
    pub fn blit_to(
        &self,
        dst: &Pool,
        r: &GpuBlit,
    ) -> Option<u64> {
        let mut raw = [0u8; BLIT_RECT];
        put_blit(&mut raw, self, dst, r);

        let args = [GPU_OP_BLIT, raw.as_ptr() as u64, 1u64, 0, 0, 0];
        let fence = unsafe { sc3(SYS_GPU_SUBMIT, args.as_ptr() as u64, 0, 0) };
        // Errnos come back as small negatives
        if fence < 0 {
            return None;
        }
        Some(fence as u64)
    }

    pub fn bytes(&mut self) -> &mut [u8] {
        unsafe { core::slice::from_raw_parts_mut(self.addr, self.size) }
    }

    /// The pool's contents, read-only, as for a keymap
    pub fn as_slice(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.addr, self.size) }
    }

    pub fn free(&mut self) {
        if self.handle != 0 {
            unsafe { sc3(SYS_MAY_UNPOOL, self.handle as u64, 0, 0) };
            self.handle = 0;
        }
    }
}

/// One decoded event. Copy with no borrowed data, so it outlives the read
/// buffer
#[derive(Clone, Copy)]
pub enum Event {
    /// Fatal: the server objected and the connection is over
    Error { object: u32, code: u32, msg: [u8; 64], msg_len: usize },
    DeleteId { id: u32 },
    /// The interface is resolved here, so a program matches on the enum
    Global { name: u32, iface: Option<Interface>, version: u32 },
    /// A global has gone
    GlobalRemove { name: u32 },
    Done { id: u32, data: u32 },
    ShmFormat { format: u32 },
    OutputMode { width: i32, height: i32, refresh: i32 },
    OutputScale { factor: i32 },
    /// The output's name, `DP-3`: version 2, and constant
    OutputName { name: [u8; 32], name_len: usize },
    /// End of a burst of output events: what arrived since is one state
    OutputDone,

    // may_output_manager: seen by the one client holding `may_wm`
    /// One connector, as the display layer has it. `current` and `preferred`
    /// index its modes, -1 for none; flags and adaptive-sync are
    /// `protocol::output_manager`'s
    OutHead {
        name: [u8; 32], name_len: usize,
        description: [u8; 32], description_len: usize,
        flags: u32, current: i32, preferred: i32,
        adaptive_sync: u32, max_bpc: u32, bpc: u32,
    },
    /// One of a head's modes, by its index in that head's list
    OutMode { head: [u8; 32], head_len: usize, index: u32, width: i32, height: i32, refresh: i32 },
    /// Everything since the last one is the whole state; configure against this
    /// serial
    OutDone { serial: u32 },
    OutFinished,
    // may_output_configuration: `config` is the configuration's object
    OutConfigSucceeded { config: u32 },
    /// A focus grab is over: a press beside its surfaces, or another's grab
    FocusGrabCleared { grab: u32 },
    /// Refused, in check's words
    OutConfigFailed { config: u32, reason: [u8; 64], reason_len: usize },
    /// Made against a state that has changed: configure again after the next
    /// `OutDone`
    OutConfigCancelled { config: u32 },
    /// Which of a pointer and a keyboard the seat has
    SeatCapabilities { caps: u32 },
    BufferRelease { id: u32 },
    /// The compositor proposes a size; zero means the client chooses.
    /// 'activated' says this window has the keyboard, the only way a client can
    /// learn that. `toplevel` says which window is being resized
    ToplevelConfigure { toplevel: u32, width: i32, height: i32, activated: bool },
    /// Acknowledge this serial, then commit a buffer drawn for that size.
    /// `xdg_surface` is the one it is for; acknowledging it on another is a
    /// protocol error
    XdgConfigure { xdg_surface: u32, serial: u32 },
    /// The layout, in a pool this client can map. `format` says what is in it:
    /// `KEYMAP_MAY_V2` is the table `keymap::Table` reads. A client ignores a
    /// format it does not recognize
    Keymap { format: u32, handle: u32, size: u32 },
    KeyboardEnter { surface: u32 },
    KeyboardLeave { surface: u32 },
    /// `serial` is the compositor's number for this keystroke, quoted back to
    /// take the pointer for a menu or set the selection
    Key { keycode: u32, pressed: bool, serial: u32 },
    /// Which modifiers are held and which layout is in use: the group, an index
    /// into the keymap's. Sent before keys typed under them
    Modifiers { mask: u32, group: u32 },
    /// How a held key repeats: `rate` per second, after `delay` milliseconds
    RepeatInfo { rate: i32, delay: i32 },
    /// The cursor arrived over this surface, at these surface-local pixels.
    /// Separate from the keyboard's enter: one window can be pointed at while
    /// another is typed at
    PointerEnter { surface: u32, x: i32, y: i32 },
    PointerLeave { surface: u32 },
    PointerMotion { x: i32, y: i32 },
    PointerButton { button: u32, pressed: bool, serial: u32 },
    /// One notch is fifteen units, down the page positive
    PointerAxis { axis: u32, value: i32 },
    /// The compositor asks this toplevel to go away
    Close { toplevel: u32 },

    // may_popup: where the compositor put a menu and when it stopped being open
    /// Where the popup landed, relative to the surface it hangs off. Sent
    /// before the `XdgConfigure` whose serial acknowledges it
    PopupConfigure { x: i32, y: i32, width: i32, height: i32 },
    /// The menu is dismissed by a click outside or its parent going away.
    /// Destroy the popup; it is sent once
    PopupDone,
    /// A `reposition` with this token has been applied
    PopupRepositioned { token: u32 },

    // may_clipboard
    /// There is something in this selection, offered in this type
    ClipboardOffer { which: u32, mime_len: usize, mime: [u8; 64] },
    /// Answer to `clipboard_get`: adopt `handle`, read `size` bytes, free it
    ClipboardData { which: u32, handle: u32, size: u32 },
    /// This selection's content is gone
    ClipboardLost { which: u32 },

    // may_wm: only the client holding the window manager's channel sees these;
    // others are refused at bind
    /// One window. The title is copied, as there is no allocator here
    DeskWindow {
        client: u32, surface: u32,
        title: [u8; 32], title_len: usize,
        /// What the program calls itself, which a window rule matches. Copied
        /// too
        app_id: [u8; 32], app_id_len: usize,
        x: i32, y: i32, width: i32, height: i32,
        flags: u32, workspace: u32,
    },
    DeskWindowsDone { serial: u32 },
    DeskOpened { client: u32, surface: u32 },
    DeskClosed { client: u32, surface: u32 },
    /// 0,0 when nothing has the keyboard
    DeskFocused { client: u32, surface: u32 },
    WmChord { id: u32 },
    DeskWorkspace { index: u32 },
    /// A word sent with `desk_signal`, carried and not interpreted. A key
    /// binding in the window manager says `launcher`, and the shell holding a
    /// layer of that name shows it
    DeskSignalled { name: [u8; 32], name_len: usize },
    /// Current keyboard layout for a bar: group `group` of `groups`, by code
    /// ("ru") and name ("Russian")
    DeskKeyboardLayout {
        group: u32, groups: u32,
        code: [u8; 32], code_len: usize,
        name: [u8; 32], name_len: usize,
    },
    /// Result of `desk_capture` `serial`: the window's `width` by `height`
    /// pixels in the pool, rows `width * 4` apart, when `copied`. Otherwise a
    /// size means a bigger pool would fit, and no size means nothing to copy
    DeskCaptured { serial: u32, copied: bool, width: i32, height: i32, format: u32 },
    /// The rectangle a layout may use, and the screen it sits in
    DeskWorkArea {
        x: i32, y: i32, width: i32, height: i32,
        screen_w: i32, screen_h: i32,
    },

    // may_console: one client holds it, others are refused at bind
    /// Where the character grid lives. Map it with `Pool::adopt(handle, size)`
    /// and read `cols * rows` cells of `console::CELL_BYTES` each. Sent again
    /// on reallocation, so the old mapping is dropped on arrival
    ConsoleGrid { handle: u32, size: u32, cols: u32, rows: u32 },
    /// Rows `row0..row1` changed, and the caret is here
    ConsoleDamage { row0: u32, row1: u32, cursor_col: u32, cursor_row: u32 },
    /// The terminal this client was drawing has gone: its master exited or it
    /// was hung up. The pool stays mapped and stops changing; a client normally
    /// closes its window
    ConsoleGone,

    // may_layer_surface: the same configure handshake as a window, draw for
    // this size, acknowledge this serial, then commit
    /// `object` is the layer surface being configured; one process may hold
    /// several
    LayerConfigure { object: u32, serial: u32, width: i32, height: i32 },
    /// The compositor took the surface away. The client's destroy is still owed
    LayerClosed { object: u32 },

    Other { object: u32, opcode: u16 },
}

/// Live objects one program can name at once: the size of the local table
/// mapping ids to interfaces for event decoding. Ids return on `delete_id`, so
/// this is occupancy, not a lifetime total
const MAX_IDS: usize = 64;
const OUT_CAP: usize = 2048;
const IN_CAP: usize = 8192;

pub struct Conn {
    pub fd: i64,
    next_id: u32,
    kinds: [(u32, Interface); MAX_IDS],
    n_kinds: usize,
    out: [u8; OUT_CAP],
    out_len: usize,
    inbuf: [u8; IN_CAP],
    in_len: usize,
    /// Set when the connection is unusable: end of stream or no object ids
    /// left. A read that found nothing within the socket's timeout leaves this
    /// clear
    pub closed: bool,
    /// Version this program bound `may_wm` at. The server shapes a `window`
    /// event by the listener's version, and version 1 has no app id
    desk_version: u32,
    /// Why, for the program to print
    pub death: &'static str,
}

impl Conn {
    pub fn open() -> Option<Conn> {
        let fd = unsafe { sc3(SYS_SOCKET, AF_MAYLAND, SOCK_STREAM, 0) };
        if fd < 0 {
            return None;
        }
        let mut c = Conn {
            fd,
            next_id: 2, // 1 is the display, which exists before any request
            kinds: [(0, Interface::Display); MAX_IDS],
            n_kinds: 0,
            out: [0; OUT_CAP],
            out_len: 0,
            inbuf: [0; IN_CAP],
            in_len: 0,
            closed: false,
            desk_version: 0,
            death: "the server closed the connection",
        };
        let _ = c.remember(DISPLAY_ID, Interface::Display);
        Some(c)
    }

    pub fn close(&mut self) {
        if self.fd >= 0 {
            unsafe { sc3(SYS_CLOSE, self.fd as u64, 0, 0) };
            self.fd = -1;
        }
    }

    /// False when the table is full. An id it does not know decodes as
    /// `Event::Other`, so its events stop arriving
    #[must_use]
    fn remember(&mut self, id: u32, iface: Interface) -> bool {
        if self.n_kinds >= MAX_IDS {
            return false;
        }
        self.kinds[self.n_kinds] = (id, iface);
        self.n_kinds += 1;
        true
    }

    fn forget(&mut self, id: u32) {
        let mut i = 0;
        while i < self.n_kinds {
            if self.kinds[i].0 == id {
                self.kinds[i] = self.kinds[self.n_kinds - 1];
                self.n_kinds -= 1;
                return;
            }
            i += 1;
        }
    }

    fn kind_of(&self, id: u32) -> Option<Interface> {
        let mut i = 0;
        while i < self.n_kinds {
            if self.kinds[i].0 == id {
                return Some(self.kinds[i].1);
            }
            i += 1;
        }
        None
    }

    fn alloc(&mut self, iface: Interface) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        if !self.remember(id, iface) {
            self.closed = true;
            self.death = "this program ran out of object ids";
        }
        id
    }

    fn queue(&mut self, sender: u32, opcode: u16, args: &[Arg]) {
        // A full queue: flush and continue, since dropping the tail would
        // desynchronize the stream
        if self.out_len + wire::message_len(args) > OUT_CAP {
            self.flush();
        }
        match wire::encode(&mut self.out[self.out_len..], sender, opcode, args) {
            Ok(n) => self.out_len += n,
            // An unencodable request never reaches the server, and `alloc` has
            // probably spent an id on it, so the program would wait forever.
            // This must not be silent
            Err(_) => {
                self.closed = true;
                self.death = "a request would not fit in one message";
            }
        }
    }

    /// Send everything queued. False means the connection is gone
    pub fn flush(&mut self) -> bool {
        if self.out_len == 0 {
            return true;
        }
        let n = unsafe {
            sc4(SYS_SEND, self.fd as u64, self.out.as_ptr() as u64, self.out_len as u64, 0)
        };
        // Checked against what was queued, before emptying the queue, and marks
        // the connection dead on failure
        let sent = n >= 0 && n as usize == self.out_len;
        self.out_len = 0;
        if !sent {
            self.closed = true;
            self.death = "the connection would not take a request";
        }
        sent
    }

    // Requests are in `requests.rs`, also compiled into the kernel's client
    // half. This connection says where the bytes go and where ids come from:
    // the `Sink` implementation below

    // events

    /// Whether a whole event is in hand, so `read_events` would return it
    /// without reading the socket. For programs waiting in `poll`: a batch
    /// bigger than the caller's `out` leaves the rest here, where `poll` cannot
    /// see it
    pub fn buffered(&self) -> bool {
        matches!(wire::peek_header(&self.inbuf[..self.in_len]), Ok(Some(_)))
    }

    /// Reads what the server has, decoding into 'out'. Blocks until at least
    /// one event arrives. Returns the count; a partial message at the end stays
    /// for the next call
    pub fn read_events(&mut self, out: &mut [Event]) -> usize {
        // Only when nothing whole is in hand, so buffered events do not wait
        // behind a blocking `recv`
        let buffered = matches!(
            wire::peek_header(&self.inbuf[..self.in_len]), Ok(Some(_)));
        if !buffered && self.in_len < IN_CAP {
            let n = unsafe {
                sc4(SYS_RECV, self.fd as u64,
                    self.inbuf.as_mut_ptr() as u64 + self.in_len as u64,
                    (IN_CAP - self.in_len) as u64, 0)
            };
            if n > 0 {
                self.in_len += n as usize;
            } else if n == 0 {
                self.closed = true;
            }
        }

        let mut count = 0;
        let mut consumed = 0;
        while count < out.len() {
            let header = match wire::peek_header(&self.inbuf[consumed..self.in_len]) {
                Ok(Some(h)) => h,
                // Not a whole message yet, the ordinary state of a stream
                Ok(None) => break,
                // An unreadable header cannot be resynchronized. Nothing would
                // be consumed, the buffer would fill to IN_CAP, and a client
                // would spin on a return of 0
                Err(_) => {
                    self.closed = true;
                    self.death = "the event stream is malformed";
                    break;
                }
            };
            let body_start = consumed + wire::HEADER_LEN;
            let body_end = consumed + header.size;
            out[count] = decode_one(
                self.kind_of(header.sender), header.sender, header.opcode,
                &self.inbuf[body_start..body_end], self.desk_version,
            );
            // The server frees ids when done; forgetting them keeps the table
            // from filling
            if let Event::DeleteId { id } = out[count] {
                self.forget(id);
            }
            consumed = body_end;
            count += 1;
        }
        if consumed > 0 {
            self.inbuf.copy_within(consumed..self.in_len, 0);
            self.in_len -= consumed;
        }
        count
    }
}

/// What the registry advertised, from the handshake every client runs
#[derive(Clone, Copy)]
pub struct Globals {
    found: [(Interface, u32, u32); MAX_GLOBALS],
    n: usize,
    /// The handshake ended in an error or end of stream instead of `done`.
    /// Nothing can be bound
    pub failed: bool,
}

/// More than the protocol has interfaces a client can bind
const MAX_GLOBALS: usize = 16;

impl Globals {
    const EMPTY: Globals = Globals {
        found: [(Interface::Display, 0, 0); MAX_GLOBALS],
        n: 0,
        failed: false,
    };

    fn push(&mut self, iface: Interface, name: u32, version: u32) {
        if self.n < MAX_GLOBALS {
            self.found[self.n] = (iface, name, version);
            self.n += 1;
        }
    }

    /// Registry name to bind this interface by, or 0 if the server has none.
    /// Zero is never a real name
    pub fn name(&self, iface: Interface) -> u32 {
        let mut i = 0;
        while i < self.n {
            if self.found[i].0 == iface {
                return self.found[i].1;
            }
            i += 1;
        }
        0
    }

    pub fn has(&self, iface: Interface) -> bool {
        self.name(iface) != 0
    }

    /// Version the server speaks of this interface, or 0. Bind the lower of
    /// this and the client's own
    pub fn version(&self, iface: Interface) -> u32 {
        let mut i = 0;
        while i < self.n {
            if self.found[i].0 == iface {
                return self.found[i].2;
            }
            i += 1;
        }
        0
    }
}

impl Conn {
    /// Asks the registry what there is and waits for all of it. Returns the
    /// registry object, since binding needs it. `done` ends this, and reading
    /// to it leaves nothing in the input buffer
    pub fn handshake(&mut self) -> (u32, Globals) {
        let registry = self.get_registry();
        self.sync();
        self.flush();

        let mut out = Globals::EMPTY;
        let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
        loop {
            let n = self.read_events(&mut evs);
            if n == 0 && self.closed {
                out.failed = true;
                return (registry, out);
            }
            for e in &evs[..n] {
                match *e {
                    Event::Global { name, iface: Some(i), version } => out.push(i, name, version),
                    Event::Done { .. } => return (registry, out),
                    Event::Error { .. } => {
                        out.failed = true;
                        return (registry, out);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Where this connection's bytes go and where its ids come from. `queue`
/// decides what a full buffer means, `alloc` what running out of ids means
impl Sink for Conn {
    fn put(&mut self, sender: u32, opcode: u16, args: &[Arg]) {
        self.queue(sender, opcode, args);
    }

    fn alloc_id(&mut self, iface: Interface) -> u32 {
        self.alloc(iface)
    }

    fn note_version(&mut self, iface: Interface, version: u32) {
        // Only `may_desktop` has two versions, differing in `app_id` on the
        // window event. Recorded at bind, since the bytes alone cannot tell
        if iface == Interface::Desktop {
            self.desk_version = version;
        }
    }
}

/// A configure's states array is a list of u32 words. An unknown state is
/// ignored
fn has_state(states: &[u8], want: u32) -> bool {
    let mut i = 0;
    while i + 4 <= states.len() {
        if u32::from_le_bytes([states[i], states[i + 1], states[i + 2], states[i + 3]])
            == want
        {
            return true;
        }
        i += 4;
    }
    false
}

/// The next string, copied into an array an event can carry. Copied since an
/// `Event` outlives the read buffer, truncated since a long title is still a
/// window
fn copy_string(r: &mut Reader) -> ([u8; 32], usize) {
    let mut out = [0u8; 32];
    let mut len = 0;
    if let Ok(Some(s)) = r.string() {
        let b = s.as_bytes();
        len = if b.len() < 32 { b.len() } else { 31 };
        out[..len].copy_from_slice(&b[..len]);
    }
    (out, len)
}

fn decode_one(kind: Option<Interface>, sender: u32, opcode: u16, body: &[u8],
              desk_version: u32) -> Event
{
    let mut r = Reader::new(body);
    let unknown = Event::Other { object: sender, opcode };

    match kind {
        Some(Interface::Display) => match opcode {
            protocol::display::ERROR => {
                let object = r.object().unwrap_or(0);
                let code = r.uint().unwrap_or(0);
                let mut msg = [0u8; 64];
                let mut msg_len = 0;
                if let Ok(Some(s)) = r.string() {
                    let b = s.as_bytes();
                    msg_len = if b.len() < 64 { b.len() } else { 63 };
                    msg[..msg_len].copy_from_slice(&b[..msg_len]);
                }
                Event::Error { object, code, msg, msg_len }
            }
            protocol::display::DELETE_ID => Event::DeleteId { id: r.uint().unwrap_or(0) },
            _ => unknown,
        },
        Some(Interface::Registry) => match opcode {
            protocol::registry::GLOBAL => {
                let name = r.uint().unwrap_or(0);
                let iface = r.string().ok().flatten().and_then(Interface::from_name);
                let version = r.uint().unwrap_or(0);
                Event::Global { name, iface, version }
            }
            protocol::registry::GLOBAL_REMOVE =>
                Event::GlobalRemove { name: r.uint().unwrap_or(0) },
            _ => unknown,
        },
        Some(Interface::Seat) if opcode == protocol::seat::CAPABILITIES => {
            Event::SeatCapabilities { caps: r.uint().unwrap_or(0) }
        }
        Some(Interface::Callback) if opcode == protocol::callback::DONE => {
            Event::Done { id: sender, data: r.uint().unwrap_or(0) }
        }
        Some(Interface::Shm) if opcode == protocol::shm::FORMAT => {
            Event::ShmFormat { format: r.uint().unwrap_or(0) }
        }
        Some(Interface::Output) => match opcode {
            protocol::output::MODE => {
                let _flags = r.uint().unwrap_or(0);
                Event::OutputMode {
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    refresh: r.int().unwrap_or(0),
                }
            }
            protocol::output::SCALE => Event::OutputScale { factor: r.int().unwrap_or(1) },
            protocol::output::NAME => {
                let (name, name_len) = copy_string(&mut r);
                Event::OutputName { name, name_len }
            }
            protocol::output::DONE => Event::OutputDone,
            _ => unknown,
        },
        Some(Interface::OutputManager) => match opcode {
            protocol::output_manager::HEAD => {
                let (name, name_len) = copy_string(&mut r);
                let (description, description_len) = copy_string(&mut r);
                Event::OutHead {
                    name, name_len, description, description_len,
                    flags: r.uint().unwrap_or(0),
                    current: r.int().unwrap_or(-1),
                    preferred: r.int().unwrap_or(-1),
                    adaptive_sync: r.uint().unwrap_or(0),
                    max_bpc: r.uint().unwrap_or(0),
                    bpc: r.uint().unwrap_or(0),
                }
            }
            protocol::output_manager::MODE => {
                let (head, head_len) = copy_string(&mut r);
                Event::OutMode {
                    head, head_len,
                    index: r.uint().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    refresh: r.int().unwrap_or(0),
                }
            }
            protocol::output_manager::DONE => Event::OutDone { serial: r.uint().unwrap_or(0) },
            protocol::output_manager::FINISHED => Event::OutFinished,
            _ => unknown,
        },
        Some(Interface::FocusGrab) if opcode == protocol::focus_grab::CLEARED => {
            Event::FocusGrabCleared { grab: sender }
        }
        Some(Interface::OutputConfiguration) => match opcode {
            protocol::output_configuration::SUCCEEDED => Event::OutConfigSucceeded { config: sender },
            protocol::output_configuration::FAILED => {
                let mut reason = [0u8; 64];
                let mut reason_len = 0;
                if let Ok(Some(m)) = r.string() {
                    let b = m.as_bytes();
                    reason_len = if b.len() < 64 { b.len() } else { 63 };
                    reason[..reason_len].copy_from_slice(&b[..reason_len]);
                }
                Event::OutConfigFailed { config: sender, reason, reason_len }
            }
            protocol::output_configuration::CANCELLED => Event::OutConfigCancelled { config: sender },
            _ => unknown,
        },
        Some(Interface::Buffer) if opcode == protocol::buffer::RELEASE => {
            Event::BufferRelease { id: sender }
        }
        Some(Interface::XdgSurface) if opcode == protocol::xdg_surface::CONFIGURE => {
            Event::XdgConfigure { xdg_surface: sender, serial: r.uint().unwrap_or(0) }
        }
        Some(Interface::Toplevel) => match opcode {
            protocol::toplevel::CONFIGURE => {
                let width = r.int().unwrap_or(0);
                let height = r.int().unwrap_or(0);
                let states = r.array().unwrap_or(&[]);
                Event::ToplevelConfigure {
                    toplevel: sender, width, height,
                    activated: has_state(states, protocol::toplevel::STATE_ACTIVATED),
                }
            }
            protocol::toplevel::CLOSE => Event::Close { toplevel: sender },
            _ => unknown,
        },
        Some(Interface::Desktop) => match opcode {
            protocol::desktop::WINDOW => {
                let client = r.uint().unwrap_or(0);
                let surface = r.uint().unwrap_or(0);
                let (title, title_len) = copy_string(&mut r);
                // Version 1 has no app id; reading one would take the x
                // coordinate as a string's length
                let (app_id, app_id_len) = if desk_version >= 2 {
                    copy_string(&mut r)
                } else {
                    ([0u8; 32], 0)
                };
                Event::DeskWindow {
                    client, surface, title, title_len, app_id, app_id_len,
                    x: r.int().unwrap_or(0),
                    y: r.int().unwrap_or(0),
                    width: r.int().unwrap_or(0),
                    height: r.int().unwrap_or(0),
                    flags: r.uint().unwrap_or(0),
                    workspace: r.uint().unwrap_or(0),
                }
            }
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
            protocol::desktop::SIGNALLED => {
                let (name, name_len) = copy_string(&mut r);
                Event::DeskSignalled { name, name_len }
            }
            protocol::desktop::KEYBOARD_LAYOUT => {
                let group = r.uint().unwrap_or(0);
                let groups = r.uint().unwrap_or(0);
                let (code, code_len) = copy_string(&mut r);
                let (name, name_len) = copy_string(&mut r);
                Event::DeskKeyboardLayout { group, groups, code, code_len, name, name_len }
            }
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
        // One event, sent to whoever bound the chord, always the client holding
        // this interface
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
            protocol::clipboard::OFFER => {
                let which = r.uint().unwrap_or(0);
                let mut mime = [0u8; 64];
                let mut mime_len = 0;
                if let Ok(Some(m)) = r.string() {
                    let b = m.as_bytes();
                    mime_len = if b.len() < 64 { b.len() } else { 63 };
                    mime[..mime_len].copy_from_slice(&b[..mime_len]);
                }
                Event::ClipboardOffer { which, mime_len, mime }
            }
            protocol::clipboard::DATA => Event::ClipboardData {
                which: r.uint().unwrap_or(0),
                handle: r.uint().unwrap_or(0),
                size: r.uint().unwrap_or(0),
            },
            protocol::clipboard::LOST =>
                Event::ClipboardLost { which: r.uint().unwrap_or(0) },
            _ => unknown,
        },
        Some(Interface::LayerSurface) => match opcode {
            protocol::layer_surface::CONFIGURE => Event::LayerConfigure {
                object: sender,
                serial: r.uint().unwrap_or(0),
                width: r.int().unwrap_or(0),
                height: r.int().unwrap_or(0),
            },
            protocol::layer_surface::CLOSED => Event::LayerClosed { object: sender },
            _ => unknown,
        },
        Some(Interface::Keyboard) => match opcode {
            protocol::keyboard::KEYMAP => Event::Keymap {
                format: r.uint().unwrap_or(0),
                handle: r.uint().unwrap_or(0),
                size: r.uint().unwrap_or(0),
            },
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
                // The compositor fills depressed and group; latched and locked
                // are in the message
                let mask = r.uint().unwrap_or(0);
                let _latched = r.uint().unwrap_or(0);
                let _locked = r.uint().unwrap_or(0);
                Event::Modifiers { mask, group: r.uint().unwrap_or(0) }
            }
            protocol::keyboard::REPEAT_INFO => Event::RepeatInfo {
                rate: r.int().unwrap_or(0),
                delay: r.int().unwrap_or(0),
            },
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
        _ => unknown,
    }
}

/// `gpu_submit`'s operations
pub const GPU_OP_BLIT: u64 = 1;
pub const GPU_OP_DISPATCH: u64 = 2;
/// Dispatches one after another, one fence (`gpu_chain`)
pub const GPU_OP_CHAIN: u64 = 3;
/// The longest chain the kernel takes
pub const MAX_CHAIN: usize = 16;

/// What a dispatch parameter is
pub const PARAM_U32: u32 = 0;
pub const PARAM_U64: u32 = 1;
/// A buffer, named by a pool handle and offset. The kernel resolves it from a
/// binding, so a dispatch needs no privilege
pub const PARAM_BUFFER: u32 = 2;
/// A buffer and its bytes from the offset: the kernel puts the size after the
/// address, and kernel programs stay inside it
pub const PARAM_BUFFER_SIZED: u32 = 3;

/// One argument to a dispatch
#[derive(Clone, Copy)]
pub struct GpuParam {
    pub kind: u32,
    pub handle: u32,
    pub value: u64,
}

impl GpuParam {
    pub fn u32(v: u32) -> Self { GpuParam { kind: PARAM_U32, handle: 0, value: v as u64 } }
    pub fn u64(v: u64) -> Self { GpuParam { kind: PARAM_U64, handle: 0, value: v } }
    /// `offset` bytes into the pool `handle`, which must be bound
    pub fn buffer(handle: u32, offset: u64) -> Self {
        GpuParam { kind: PARAM_BUFFER, handle, value: offset }
    }
    /// The same with its size: two program parameters, a pointer and a 64-bit
    /// byte count
    pub fn sized(handle: u32, offset: u64) -> Self {
        GpuParam { kind: PARAM_BUFFER_SIZED, handle, value: offset }
    }
}

/// One launch of a chain (`gpu_chain`)
pub struct GpuLaunch<'a> {
    pub program: u32,
    pub grid: (u32, u32, u32),
    pub block: (u32, u32, u32),
    pub params: &'a [GpuParam],
}

/// One rectangle for `Pool::blit_to`. `pitch_*` are bytes per row and at least
/// `line_len`; overlapping rows are not supported in any useful way
pub struct GpuBlit {
    pub src_off: u64,
    pub dst_off: u64,
    pub pitch_in: u32,
    pub pitch_out: u32,
    pub line_len: u32,
    pub lines: u32,
}

/// Waits for a fence from `Pool::blit_to`. False means the budget ran out or
/// the work is lost, see `gpu_wait_code`
pub fn gpu_wait(fence: u64, timeout_us: u64) -> bool {
    gpu_wait_code(fence, timeout_us) == 0
}

/// `gpu_wait` with the kernel's answer: 0 when finished, `-ETIMEDOUT` (-110)
/// when the budget ran out, `-EIO` (-5) when its channel died first. Then the
/// buffers hold what the engine reached and the work must be resubmitted
/// (`gpu_errno` names it)
pub fn gpu_wait_code(fence: u64, timeout_us: u64) -> i64 {
    unsafe { sc3(SYS_GPU_WAIT, fence, timeout_us, 0) }
}

/// Loads a compiled GPU program and returns the index a dispatch names.
///
/// `cubin` is `nvcc -cubin` output holding one kernel. `regs` and `shared` are
/// its register and shared-memory usage, from `cuobjdump -res-usage`.
///
/// Needs `CAP_GPU`: this runs the caller's machine code, unlike the rest of
/// this range.
///
/// Negative is `-errno`: `EPERM` without the capability, `ENOEXEC` for an
/// unreadable cubin or a program that will not fit, `EINVAL` for a cubin
/// without exactly one kernel
pub fn gpu_program(cubin: &[u8], regs: u32, shared: u32) -> i64 {
    let mut args = [0u8; 24];
    args[0..8].copy_from_slice(&(cubin.as_ptr() as u64).to_le_bytes());
    args[8..16].copy_from_slice(&(cubin.len() as u64).to_le_bytes());
    args[16..20].copy_from_slice(&regs.to_le_bytes());
    args[20..24].copy_from_slice(&shared.to_le_bytes());
    unsafe { sc3(SYS_GPU_PROGRAM, args.as_ptr() as u64, 0, 0) }
}

/// What this machine's GPU offers, as lines of text. Empty when nothing can
/// execute a submission
pub fn gpu_info(buf: &mut [u8]) -> usize {
    let n = unsafe { sc3(SYS_GPU_INFO, buf.as_mut_ptr() as u64, buf.len() as u64, 0) };
    if n < 0 { 0 } else { n as usize }
}

/// Makes a pool reachable by the GPU's MMU. Returns its GPU address, valid only
/// as a `PARAM_BUFFER` and not mapped in this process. Binding twice costs no
/// second slot
pub fn gpu_bind(pool: &Pool) -> Option<u64> {
    let mut out = [0u64; 2];
    let r = unsafe {
        sc3(SYS_GPU_BIND, pool.handle as u64, 0, out.as_mut_ptr() as u64)
    };
    if r != 0 { None } else { Some(out[0]) }
}

/// Runs a program `gpu_info` listed over buffers this process has bound.
/// Returns the fence or the negative errno, so a refusal keeps its reason
pub fn gpu_dispatch(
    program: u32,
    grid: (u32, u32, u32),
    block: (u32, u32, u32),
    params: &[GpuParam],
) -> i64 {
    gpu_chain(&[GpuLaunch { program, grid, block, params }])
}

/// Runs programs one after another, each finished before the next starts, as
/// one submission with one fence: `GPU_OP_CHAIN`, at most `MAX_CHAIN`. Returns
/// as `gpu_dispatch` does
pub fn gpu_chain(launches: &[GpuLaunch<'_>]) -> i64 {
    if launches.is_empty() || launches.len() > MAX_CHAIN {
        return -22;
    }
    let mut raw = [[0u8; 16 * 16]; MAX_CHAIN];
    let mut blocks = [[0u8; 40]; MAX_CHAIN];
    for (k, l) in launches.iter().enumerate() {
        for (i, p) in l.params.iter().enumerate().take(16) {
            let o = i * 16;
            raw[k][o..o + 4].copy_from_slice(&p.kind.to_le_bytes());
            raw[k][o + 4..o + 8].copy_from_slice(&p.handle.to_le_bytes());
            raw[k][o + 8..o + 16].copy_from_slice(&p.value.to_le_bytes());
        }
        let d = &mut blocks[k];
        d[0..4].copy_from_slice(&l.program.to_le_bytes());
        d[4..8].copy_from_slice(&(l.params.len().min(16) as u32).to_le_bytes());
        for (i, v) in [l.grid.0, l.grid.1, l.grid.2, l.block.0, l.block.1, l.block.2].iter().enumerate() {
            d[8 + i * 4..12 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        d[32..40].copy_from_slice(&(raw[k].as_ptr() as u64).to_le_bytes());
    }
    let (op, count) = if launches.len() == 1 { (GPU_OP_DISPATCH, 0) } else { (GPU_OP_CHAIN, launches.len() as u64) };
    let args = [op, blocks.as_ptr() as u64, count, 0, 0, 0];
    unsafe { sc3(SYS_GPU_SUBMIT, args.as_ptr() as u64, 0, 0) }
}

/// The same for a blit, so a refusal can be reported
pub fn gpu_blit(src: &Pool, dst: &Pool, r: &GpuBlit) -> i64 {
    gpu_blits(&[(src, dst, r)])
}

/// Bytes of one rectangle as `gpu_submit` reads it
const BLIT_RECT: usize = 48;

/// Most rectangles `gpu_blits` sends at once. The kernel takes more
/// (`max_rects` in `gpu_info`); this fits on the stack here
pub const MAX_BLITS: usize = 16;

/// One rectangle in `gpu_submit`'s layout: handles, then offsets, pitches, and
/// size
fn put_blit(raw: &mut [u8], src: &Pool, dst: &Pool, r: &GpuBlit) {
    raw[0..4].copy_from_slice(&src.handle.to_le_bytes());
    raw[4..8].copy_from_slice(&dst.handle.to_le_bytes());
    raw[8..16].copy_from_slice(&r.src_off.to_le_bytes());
    raw[16..24].copy_from_slice(&r.dst_off.to_le_bytes());
    raw[24..28].copy_from_slice(&r.pitch_in.to_le_bytes());
    raw[28..32].copy_from_slice(&r.pitch_out.to_le_bytes());
    raw[32..36].copy_from_slice(&r.line_len.to_le_bytes());
    raw[36..40].copy_from_slice(&r.lines.to_le_bytes());
}

/// Several rectangles in one submission under one fence. Each names its two
/// pools. One submission copies between one pair of memories, since the engine
/// is told once per batch where it reads and writes. Rectangles with different
/// pairs, or more than `MAX_BLITS`, are `EINVAL`
pub fn gpu_blits(rects: &[(&Pool, &Pool, &GpuBlit)]) -> i64 {
    if rects.is_empty() || rects.len() > MAX_BLITS {
        return -22;
    }
    let mut raw = [0u8; BLIT_RECT * MAX_BLITS];
    for (i, (src, dst, r)) in rects.iter().enumerate() {
        put_blit(&mut raw[i * BLIT_RECT..(i + 1) * BLIT_RECT], src, dst, r);
    }
    let args = [GPU_OP_BLIT, raw.as_ptr() as u64, rects.len() as u64, 0, 0, 0];
    unsafe { sc3(SYS_GPU_SUBMIT, args.as_ptr() as u64, 0, 0) }
}

/// Bind a pool, returning its GPU address or the raw errno
pub fn gpu_bind_raw(pool: &Pool) -> i64 {
    let mut out = [0u64; 2];
    let r = unsafe {
        sc3(SYS_GPU_BIND, pool.handle as u64, 0, out.as_mut_ptr() as u64)
    };
    if r != 0 { r } else { out[0] as i64 }
}

/// What an errno this interface returns means, in words
pub fn gpu_errno(code: i64) -> &'static str {
    match code {
        -1 => "EPERM - loading a GPU program needs CAP_GPU, and this process does not hold it",
        -5 => "EIO - the channel is dead or has no ring, or died with this work on it",
        -110 => "ETIMEDOUT - the work did not finish in time",
        -11 => "EAGAIN - a submission is already in flight",
        -13 => "EACCES - that pool is not this process's",
        -14 => "EFAULT - the kernel could not read an argument",
        -19 => "ENODEV - no GPU, no channel, or no programs loaded",
        -22 => "EINVAL - an argument was rejected",
        -24 => "EMFILE - too many pools bound",
        -8 => "ENOEXEC - the launch descriptor could not be built",
        -16 => "EBUSY - the compute channel would not take the work",
        -38 => "ENOSYS - this kernel does not have that call",
        -12 => "ENOMEM - no room for it",
        -122 => "EDQUOT - programs hold all the video memory they may",
        _ => "unknown",
    }
}
