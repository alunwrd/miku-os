#![no_std]
#![no_main]

//! maygreet: graphical login screen
//!
//! One surface over everything, holding the keyboard, asking mikudod as `login`
//! does. Runs unprivileged, except for the capability to draw over the screen.
//! It cannot read /etc/shadow or change identity.
//! `mikuauth::client::open_session` is the entire privileged path: a socket
//! write to mikudod.
//!
//! `CAP_MAY_PRIV` follows the seat (kernel/process/session.rs). This program
//! holds it while nobody is logged in, and when a session opens the kernel
//! moves it to the session's leader. A successful login ends this process, and
//! the compositor drops the lock when the connection closes.
//!
//! While up, the compositor's lock disables Ctrl+Alt+Esc, F12, Ctrl+Alt+Tab,
//! and window manager chords.

extern crate alloc;

pub use miku;

pub use libmay::conf;
pub use libmay::conn;
pub use libmay::manifest;
pub use libmay::paint;
pub use libmay::protocol;
pub use libmay::requests;
pub use libmay::text;
pub use libmay::ui;
pub use libmay::wire;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

use conn::{Conn, Event, Pool};
use libmay::conf::{Note, Sink, Where};
use paint::{Canvas, Rect};
use protocol::Interface;
use requests::Requests;
use ui::{ButtonStyle, Palette, Ui};

use mikuauth::client::{self, Ask, Refused};
use mikuauth::db;

/// Widget ids, distinct within a frame (`ui` is immediate mode)
const ID_USER: u32 = 1;
const ID_PASSWORD: u32 = 2;
const ID_ENTER: u32 = 3;
const ID_REVEAL: u32 = 4;
const ID_GEAR: u32 = 5;
/// The gear's menu. One id per row, and `ui::list_row` wants them distinct
const ID_MENU: u32 = 16;

/// Card size in pixels: most of the screen, capped
const CARD_MAX_W: i32 = 1180;
const CARD_MAX_H: i32 = 940;
/// How much of the page is left showing around it
const CARD_INSET: i32 = 24;

/// Portrait width. The file ships at 260 (`GREETER_ART` in build.rs)
const ART_SIDE: i32 = 260;

/// Refused password color. The pill mixes toward it; the ring and padlock use
/// it whole
const DENIED: u32 = 0xFFD9_3B4F;

fn say(msg: &str, detail: &str) {
    miku::print("  maygreet: ");
    miku::print(msg);
    if !detail.is_empty() {
        miku::print(" - ");
        miku::print(detail);
    }
    miku::println("");
}

fn fail(msg: &str) -> ! {
    say(msg, "");
    miku::klog("[maygreet] cannot start");
    miku::exit(1);
}

/// Screen state
struct Greeter {
    ui: Ui,
    palette: Palette,
    /// Field fill color, which `Palette` lacks
    field: u32,
    /// Portrait, decoded once. `None` without a file, drawing no circle
    art: Option<(i32, i32)>,

    /// Login name, prefilled with the first ordinary account
    user: [u8; 33],
    user_len: usize,
    user_caret: usize,

    /// Typed password, wiped after every attempt
    pw: [u8; mikuauth::proto::MAX_PASSWORD],
    pw_len: usize,
    pw_caret: usize,

    /// Text under the fields: a refusal or a daemon message
    message: String,
    /// The last attempt was refused: the password field turns red. Cleared on
    /// typing
    denied: bool,
    /// A login is being checked, since Argon2 takes long enough to press Enter
    /// twice
    busy: bool,
    /// Whether the password is shown (`ui::field_row`)
    reveal: bool,
    /// Whether the gear's menu is open, and what it offers
    menu: bool,
}

impl Greeter {
    fn new(theme: Theme, first_user: &str) -> Greeter {
        let art = load_art(&theme.image);
        let mut g = Greeter {
            ui: Ui::new(),
            palette: theme.p,
            field: theme.field,
            art,
            user: [0; 33],
            user_len: 0,
            user_caret: 0,
            pw: [0; mikuauth::proto::MAX_PASSWORD],
            pw_len: 0,
            pw_caret: 0,
            message: String::new(),
            denied: false,
            busy: false,
            reveal: false,
            menu: false,
        };
        let n = first_user.len().min(g.user.len());
        g.user[..n].copy_from_slice(&first_user.as_bytes()[..n]);
        g.user_len = n;
        g.user_caret = n;
        // The password box starts focused, since the name is usually right
        g.ui.focus = ID_PASSWORD;
        g
    }

    fn user_name(&self) -> &str {
        core::str::from_utf8(&self.user[..self.user_len]).unwrap_or("")
    }

    /// Forget the password, on every path
    fn wipe(&mut self) {
        mikuauth::ct::wipe(&mut self.pw);
        self.pw_len = 0;
        self.pw_caret = 0;
    }
}

/// Colors and picture from the desktop's config. `theme` first, with `greeter`
/// over it, so the login screen can differ from the desktop
struct Theme {
    p: Palette,
    field: u32,
    image: String,
}

impl Sink for Theme {
    fn key(&mut self, _at: Where, section: &str, key: &str, value: &str) {
        if section != "theme" && section != "greeter" {
            return;
        }
        // Keys `Palette` has no field for, checked first
        match key {
            "field" => { if let Some(c) = conf::colour(value) { self.field = c; } return; }
            "image" => { self.image = value.to_string(); return; }
            _ => {}
        }
        self.p.take(key, value);
    }
    fn note(&mut self, _at: Where, _note: Note, _text: &str) {}
}

fn read_theme() -> Theme {
    let mut files = manifest::DiskFiles;
    let m = manifest::Manifest::build(&mut files, &manifest::LAYERS);
    let mut theme = Theme {
        p: ui::MIKU,
        field: ui::FIELD,
        image: String::new(),
    };
    let vars = conf::Vars::new();
    for &file in m.roots() {
        conf::parse_into(m.text(file), &mut theme, &vars, file);
    }
    theme
}

/// Portrait, decoded once into a static buffer. QOI has straight alpha, so
/// `blit_over`. The circle is cut at build time by `lib/buildgen/qoigen.rs`
static mut ART: [u8; (ART_SIDE * ART_SIDE * 4) as usize] = [0; (ART_SIDE * ART_SIDE * 4) as usize];

fn load_art(path: &str) -> Option<(i32, i32)> {
    if path.is_empty() {
        return None;
    }
    let (ptr, size) = miku::read_file(path)?;
    let file = unsafe { core::slice::from_raw_parts(ptr as *const u8, size) };
    let (w, h) = libmay::qoi::size(file)?;
    if w == 0 || h == 0 || w > ART_SIDE as u32 || h > ART_SIDE as u32 {
        return None;
    }
    let n = (w * h * 4) as usize;
    let rows = unsafe {
        let buf = &mut *core::ptr::addr_of_mut!(ART);
        libmay::qoi::decode(file, &mut buf[..n])
    };
    (rows >= h as usize).then_some((w as i32, h as i32))
}

/// First ordinary account from /etc/passwd
fn first_account() -> String {
    let text = miku::read_file_string(mikuauth::PASSWD_PATH).unwrap_or_default();
    for (_, line) in db::Lines::new(&text) {
        let Ok(p) = db::Passwd::parse(line) else { continue };
        // Uid 1000 and up, excluding root and service accounts
        if p.uid >= 1000 && p.uid != mikuauth::NOBODY_UID {
            return p.name.to_string();
        }
    }
    String::new()
}

/// `Ask` for a greeter: the password is already typed. One connection carries
/// one attempt, and `None` the second time ends it
struct Typed(Option<Vec<u8>>);

impl Ask for Typed {
    fn password(&mut self, _user: &str) -> Option<Vec<u8>> {
        self.0.take()
    }
}

#[no_mangle]
pub extern "C" fn _start_main() -> ! {
    if !miku::cred::has_cap(miku::cred::CAP_MAY_PRIV) {
        // Logs a clearer error than the compositor's missing global
        say("started without may_priv", "the compositor will not give us the overlay");
    }

    let mut g = Greeter::new(read_theme(), &first_account());

    let mut c = match Conn::open() {
        Some(c) => c,
        None => fail("cannot reach the display server"),
    };
    let (registry, globals) = c.handshake();
    if globals.failed {
        fail("the server refused the handshake");
    }
    if !globals.has(Interface::Shm) || !globals.has(Interface::Compositor) {
        fail("the registry is missing a global we need");
    }
    if !globals.has(Interface::Layer) {
        fail("this compositor has no may_layer - a greeter is one surface in it");
    }

    let shm = c.bind(registry, globals.name(Interface::Shm), Interface::Shm, 1);
    let comp = c.bind(registry, globals.name(Interface::Compositor), Interface::Compositor, 1);
    let layer = c.bind(registry, globals.name(Interface::Layer), Interface::Layer, 1);
    if globals.has(Interface::Seat) {
        let seat = c.bind(registry, globals.name(Interface::Seat), Interface::Seat, 1);
        c.get_pointer(seat);
        c.get_keyboard(seat);
    }

    let mut s = Surface::new();
    s.open(&mut c, comp, layer);
    c.flush();

    let mut evs = [Event::Other { object: 0, opcode: 0 }; 32];
    let mut keymap_pool: Option<Pool> = None;
    let mut mods = 0u32;
    // Active keyboard layout
    let mut group = 0u32;
    let mut pointer_at = (0i32, 0i32);

    loop {
        let n = c.read_events(&mut evs);
        if n == 0 && c.closed {
            break;
        }
        let mut died = false;

        for e in &evs[..n] {
            match *e {
                Event::BufferRelease { id } => {
                    for b in s.buffers.iter_mut() {
                        if b.0 == id {
                            b.1 = true;
                        }
                    }
                }
                Event::Done { id, .. } => {
                    if s.frame_cb == id {
                        s.armed = true;
                    }
                }
                Event::LayerConfigure { object, serial, width, height } => {
                    c.layer_ack_configure(object, serial);
                    if s.ls == object {
                        let want = (if width > 0 { width } else { s.w },
                                    if height > 0 { height } else { s.h });
                        s.up = true;
                        if want != (s.w, s.h) || s.pool.is_none() {
                            s.pending = Some(want);
                        }
                        s.armed = true;
                        s.dirty = true;
                    }
                }
                Event::LayerClosed { object } => {
                    if s.ls == object {
                        // The compositor took the screen back
                        say("the compositor closed our surface", "");
                        died = true;
                    }
                }

                Event::PointerEnter { x, y, .. } | Event::PointerMotion { x, y } => {
                    pointer_at = (x, y);
                    if g.ui.pointer_moved(x, y) {
                        s.dirty = true;
                    }
                }
                Event::PointerButton { button, pressed, .. } => {
                    g.ui.pointer_button(button, pressed);
                    let _ = pointer_at;
                    s.dirty = true;
                }

                Event::Keymap { format, handle, size } => {
                    if format == protocol::keyboard::KEYMAP_MAY_V2 {
                        keymap_pool = Pool::adopt(handle, size as usize);
                    }
                }
                Event::Modifiers { mask, group: g } => { mods = mask; group = g; }
                Event::Key { keycode, pressed, .. } => {
                    if pressed {
                        g.ui.key(keycode, true);
                        let ch = match keymap_pool.as_ref() {
                            Some(p) => libmay::keymap::Table::new(p.as_slice())
                                .and_then(|t| t.to_char_in(keycode as u16, mods, group)),
                            None => libmay::keymap::to_char(keycode as u16, mods),
                        };
                        if let Some(ch) = ch {
                            g.ui.ch(ch);
                        }
                        s.dirty = true;
                    }
                }

                Event::Error { msg, msg_len, .. } => {
                    let m = core::str::from_utf8(&msg[..msg_len]).unwrap_or("");
                    say("the server refused something", m);
                    died = true;
                }
                _ => {}
            }
        }
        if died {
            break;
        }

        if let Some((w, h)) = s.pending {
            if s.armed {
                s.pending = None;
                s.w = w;
                s.h = h;
                s.cut(&mut c, shm);
            }
        }

        if draw(&mut c, &mut s, &mut g) {
            // Enter or the button. The socket exchange happens between frames,
            // leaving the screen as is
            if attempt(&mut g) {
                break;
            }
            s.dirty = true;
        }
        c.flush();
    }

    g.wipe();
    s.close(&mut c);
    c.flush();
    c.close();
    miku::exit(0);
}

/// Try the login. True when it worked and this program is finished
fn attempt(g: &mut Greeter) -> bool {
    let user = g.user_name().to_string();
    if user.is_empty() {
        g.message = "who are you?".to_string();
        return false;
    }

    g.busy = true;
    let mut ask = Typed(Some(g.pw[..g.pw_len].to_vec()));
    let result = client::open_session(&user, client::GRAPHICAL, client::SEAT0, &mut ask);
    // Wipes both copies; `Typed` gives its `Vec` away on the first prompt, so
    // anything left is from a path where the daemon never asked
    if let Some(mut leftover) = ask.0.take() {
        mikuauth::ct::wipe(&mut leftover);
    }
    g.wipe();
    g.busy = false;

    match result {
        Ok(id) => {
            miku::klog("[maygreet] session opened; the seat is no longer ours");
            let _ = id;
            true
        }
        // Wrong password or unknown account, answered identically by the
        // daemon: the field turns red and regains the keyboard
        Err(Refused::Denied(_)) => {
            g.denied = true;
            g.message.clear();
            g.ui.focus = ID_PASSWORD;
            false
        }
        // Other errors are the machine's and are shown as text
        Err(Refused::NoDaemon) => {
            g.message = "the login service is not running".to_string();
            false
        }
        Err(why) => {
            g.message = why.message().to_string();
            false
        }
    }
}

/// Draw a frame if one is owed. Returns whether the login should be tried
fn draw(c: &mut Conn, s: &mut Surface, g: &mut Greeter) -> bool {
    if !s.up || !s.armed {
        return false;
    }
    let free = s.buffers.iter().position(|b| b.1);
    let (Some(slot), Some(_)) = (free, s.pool.as_ref()) else { return false };
    if !s.dirty {
        // Unchanged: one callback, no attach, no damage
        s.frame_cb = c.frame(s.surface);
        c.commit(s.surface);
        s.armed = false;
        return false;
    }

    let (w, h, bytes, stride) = (s.w, s.h, s.bytes, s.stride as usize);
    let from = slot * bytes;
    let (go, owed) = {
        let px = &mut s.pool.as_mut().unwrap().bytes()[from..from + bytes];
        let mut canvas = Canvas::new(px, stride / 4, w, h);
        paint_screen(&mut canvas, g)
    };

    c.attach(s.surface, s.buffers[slot].0, 0, 0);
    c.damage(s.surface, 0, 0, w, h);
    s.frame_cb = c.frame(s.surface);
    c.commit(s.surface);
    s.buffers[slot].1 = false;
    s.armed = false;
    // `Ui::end` reports when one more frame is owed, since an activated control
    // has not shown its result yet
    s.dirty = owed;
    go
}

/// Draws the screen. Returns whether to try the login and whether another frame
/// is owed
fn paint_screen(c: &mut Canvas, g: &mut Greeter) -> (bool, bool) {
    let p = g.palette;
    c.clear(p.page);

    let (w, h) = (c.w, c.h);
    let card = Rect::new(
        (w - CARD_MAX_W.min(w - CARD_INSET * 2)) / 2,
        (h - CARD_MAX_H.min(h - CARD_INSET * 2)) / 2,
        CARD_MAX_W.min(w - CARD_INSET * 2),
        CARD_MAX_H.min(h - CARD_INSET * 2),
    );

    // Set before `begin`, since controls take their colors as they draw
    g.ui.palette = p;
    g.ui.begin();
    // A low-weight edge instead of a shadow, which would need floats and a
    // scratch buffer
    c.fill_round_rect(card, 18, p.card);
    c.stroke_round_rect(card, 18, 1, p.border);

    // The gear in the card's corner, outside the column
    let gear = Rect::new(card.right() - 34 - 24, card.y + 24, 34, 34);
    if ui::icon_button(&mut g.ui, c, ID_GEAR, gear, ui::Icon::Gear, true) {
        g.menu = !g.menu;
    }

    // One column, and everything below is a row taken off it
    let side = (card.w - 96).min(560);
    let mut col = ui::Column::new(Rect::new(
        card.x + (card.w - side) / 2, card.y + 72, side, card.h - 96));

    // Portrait on its own row, which closes up when absent
    if let Some((aw, ah)) = g.art {
        let box_ = col.row(ah.min(ART_SIDE), 24);
        // SAFETY: single-threaded and the only reader; `load_art` fills it once
        // before the first frame
        let art = unsafe { &*core::ptr::addr_of!(ART) };
        c.blit_over(art, aw as usize, ah as usize,
                    box_.x + (box_.w - aw) / 2, box_.y, aw, ah);
    }

    let title = col.row(text::face_line_height(ui::FACE_DISPLAY), 6);
    text::draw_face_centred(c, ui::FACE_DISPLAY, title.x, title.y, title.w, "Welcome!", p.text);
    let sub = col.row(text::face_line_height(ui::FACE_BODY), 28);
    text::draw_face_centred(c, ui::FACE_BODY, sub.x, sub.y, sub.w,
                            "Please sign in to continue", p.text_muted);

    if g.menu {
        let go = paint_menu(c, g, &mut col);
        return (go, g.ui.end());
    }

    let name_box = col.row(52, 14);
    ui::field_row(&mut g.ui, c, ID_USER, name_box,
                  &mut g.user, &mut g.user_len, &mut g.user_caret,
                  "Username", &p, g.field, ui::Icon::Person, false, 0);

    let pw_box = col.row(52, 14);
    // A refusal copies the palette with two colors replaced: `field_row` draws
    // its ring in `accent` and padlock in `text_muted`
    let (pw_pal, pw_fill) = if g.denied {
        let mut pal = p;
        pal.accent = DENIED;
        pal.text_muted = DENIED;
        (pal, p.mix(DENIED, 36))
    } else {
        (p, g.field)
    };
    // Field width excludes the eye button
    if ui::field_row(&mut g.ui, c, ID_PASSWORD, pw_box,
                     &mut g.pw, &mut g.pw_len, &mut g.pw_caret,
                     "Password", &pw_pal, pw_fill, ui::Icon::Lock, !g.reveal, 44) {
        // Typing clears the refusal
        g.denied = false;
    }
    let eye = Rect::new(pw_box.right() - 44, pw_box.y + (pw_box.h - 32) / 2, 32, 32);
    let eye_icon = if g.reveal { ui::Icon::EyeOff } else { ui::Icon::Eye };
    if ui::icon_button(&mut g.ui, c, ID_REVEAL, eye, eye_icon, true) {
        g.reveal = !g.reveal;
    }

    // Message row, always reserved so the button below does not move
    let msg = col.row(text::face_line_height(ui::FACE_SMALL), 8);
    if g.busy {
        text::draw_face_centred(c, ui::FACE_SMALL, msg.x, msg.y, msg.w,
                                "checking...", p.text_muted);
    } else if !g.message.is_empty() {
        text::draw_face_centred(c, ui::FACE_SMALL, msg.x, msg.y, msg.w,
                                &g.message, p.text_muted);
    }

    // Sign-in button, the same width as the fields. `button_icon` centers label
    // and arrow
    let button = col.row(52, 0);
    let clicked = ui::button(&mut g.ui, c, ID_ENTER, button,
                             "Sign in", ButtonStyle::Secondary, true);

    // Enter anywhere tries the login. `ui::visit` takes the key when the button
    // has focus, `take_key` when a field does (`by_key: false`)
    let entered = g.ui.take_key(ui::KEY_ENTER);
    let go = clicked || entered;

    (go, g.ui.end())
}

/// Gear menu: machine actions and desktop choice, drawn in place of the fields
fn paint_menu(c: &mut Canvas, g: &mut Greeter, col: &mut ui::Column) -> bool {
    for (i, (label, detail)) in MENU.iter().enumerate() {
        let r = col.row(56, 10);
        let row = ui::Row { subtitle: detail, right: ui::Right::Arrow, ..ui::Row::new(label) };
        if ui::list_row(&mut g.ui, c, ID_MENU + i as u32, r, &row) {
            let action = if i == 0 {
                mikuauth::proto::power_action::OFF
            } else {
                mikuauth::proto::power_action::REBOOT
            };
            // Granted without a password, since nobody is logged in
            let mut silent = client::Silent;
            match client::power(action, "", &mut silent) {
                Ok(()) => g.message = "stopping...".to_string(),
                Err(Refused::NoDaemon) =>
                    g.message = "the login service is not running".to_string(),
                Err(why) => g.message = why.message().to_string(),
            }
            g.menu = false;
        }
    }
    let back = col.row(52, 0);
    if ui::button(&mut g.ui, c, ID_MENU + MENU.len() as u32, back,
                  "Back", ButtonStyle::Secondary, false) {
        g.menu = false;
    }
    false
}

/// Gear menu's machine actions. Session choice can use `/etc/mayland/session.d`
static MENU: [(&str, &str); 2] = [
    ("Power off", "stop the machine"),
    ("Restart", "stop it and start it again"),
];

/// The surface and its memory
struct Surface {
    surface: u32,
    ls: u32,
    up: bool,
    w: i32,
    h: i32,
    stride: i32,
    bytes: usize,
    pool: Option<Pool>,
    pool_obj: u32,
    buffers: [(u32, bool); 2],
    frame_cb: u32,
    armed: bool,
    dirty: bool,
    pending: Option<(i32, i32)>,
}

impl Surface {
    fn new() -> Surface {
        Surface {
            surface: 0, ls: 0, up: false,
            w: 0, h: 0, stride: 0, bytes: 0,
            pool: None, pool_obj: 0,
            buffers: [(0, true); 2],
            frame_cb: 0, armed: false, dirty: true,
            pending: None,
        }
    }

    /// Requests the screen: overlay band, all four anchors with zero size for
    /// the whole screen, exclusive -1 so the work area stays, and exclusive
    /// keyboard, which the compositor's lock is keyed to
    fn open(&mut self, c: &mut Conn, comp: u32, layer: u32) {
        use protocol::layer_surface as ls;
        self.surface = c.create_surface(comp);
        self.ls = c.get_layer_surface(layer, self.surface, protocol::layer::OVERLAY);
        c.layer_set_anchor(self.ls,
            ls::ANCHOR_TOP | ls::ANCHOR_BOTTOM | ls::ANCHOR_LEFT | ls::ANCHOR_RIGHT);
        c.layer_set_size(self.ls, 0, 0);
        c.layer_set_exclusive(self.ls, -1);
        c.layer_set_keyboard(self.ls, ls::KEYBOARD_EXCLUSIVE);
    }

    fn cut(&mut self, c: &mut Conn, shm: u32) {
        self.drop_buffers(c);
        if self.w <= 0 || self.h <= 0 {
            return;
        }
        self.stride = self.w * 4;
        self.bytes = (self.stride * self.h) as usize;
        let Some(p) = Pool::new(self.bytes * 2) else {
            say("the kernel would not give us a pool", "");
            return;
        };
        self.pool_obj = c.create_pool(shm, p.handle, (self.bytes * 2) as i32);
        for i in 0..2 {
            self.buffers[i] = (
                // XRGB: nothing shows through, and `compose::translucent` would
                // put an ARGB surface on the blend path
                c.create_buffer(self.pool_obj, (i * self.bytes) as i32, self.w, self.h,
                                self.stride, protocol::shm::FORMAT_XRGB8888),
                true,
            );
        }
        self.pool = Some(p);
        self.dirty = true;
    }

    fn drop_buffers(&mut self, c: &mut Conn) {
        for b in self.buffers.iter_mut() {
            if b.0 != 0 {
                c.destroy_buffer(b.0);
            }
            *b = (0, true);
        }
        if self.pool_obj != 0 {
            c.destroy_pool(self.pool_obj);
            self.pool_obj = 0;
        }
        if let Some(mut p) = self.pool.take() {
            // The compositor must see the pool destroyed before the memory is
            // freed
            c.flush();
            p.free();
        }
    }

    fn close(&mut self, c: &mut Conn) {
        if self.ls != 0 {
            c.destroy_layer_surface(self.ls);
        }
        if self.surface != 0 {
            c.destroy_surface(self.surface);
        }
        self.ls = 0;
        self.surface = 0;
        self.up = false;
        self.drop_buffers(c);
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    miku::klog("maygreet: panic");
    miku::exit(1);
}
