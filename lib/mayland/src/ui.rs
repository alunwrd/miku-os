// Widgets
//
// Shared by `#[path]` between the kernel and clients, like `text.rs` and
// `paint.rs`.
//
// Immediate mode: a widget is a function that draws itself and returns whether
// it was activated this frame. Only `Ui` persists: pointer position, held
// button, and keyboard focus. No retained tree, allocation, or `dyn`.
//
// Every control takes an `id`, unique within a screen. Tab and the arrows move
// `focus` along them and Enter activates the focused one, so a screen can be
// driven from a headless test.

use super::paint::{Canvas, Rect};
use super::text;

// the faces `fontgen.rs` built, named

pub const FACE_TITLE: usize = 0;
pub const FACE_HEAD: usize = 1;
pub const FACE_BODY: usize = 2;
pub const FACE_SMALL: usize = 3;
pub const FACE_DISPLAY: usize = 4;

// The palette, sampled from the design in docs/refs/miku-os
//
// `0xAARRGGBB`, each written with its `FF`. On a blended surface a zero top
// byte is invisible. `conf::colour` treats six digits as solid too.

pub const PAGE: u32 = 0xFF1A1414;
pub const CARD: u32 = 0xFF38302E;
pub const CARD_HOVER: u32 = 0xFF413836;
pub const CARD_SELECTED: u32 = 0xFF3D3332;
pub const FIELD: u32 = 0xFF2F2524;
pub const ACCENT: u32 = 0xFFEEA3B4;
pub const ACCENT_DIM: u32 = 0xFFA36967;
pub const TEXT: u32 = 0xFFE5DBDC;
pub const TEXT_MUTED: u32 = 0xFFB9A9A9;
pub const ON_ACCENT: u32 = 0xFF1A1414;
pub const BORDER: u32 = 0xFF56403F;

/// The same color at another alpha, in parts of 255
pub const fn at(colour: u32, alpha: u8) -> u32 {
    (colour & 0x00FF_FFFF) | ((alpha as u32) << 24)
}

/// The alpha of a colour, on its own
pub const fn alpha_of(colour: u32) -> u8 {
    (colour >> 24) as u8
}

/// The same colors as a value. A screen taking a `Palette` can use a config's
/// colors. Named by part, `accent` and never `pink`
#[derive(Clone, Copy)]
pub struct Palette {
    /// What everything is drawn on
    pub page: u32,
    /// A surface standing off the page: a pill, tile, or menu
    pub card: u32,
    /// The one color that is not the page or the text
    pub accent: u32,
    /// On top of `accent`. Separate from `page`, since an accent light enough
    /// to need dark text has a page that is not
    pub on_accent: u32,
    pub text: u32,
    /// Text that is present but not primary
    pub text_muted: u32,
    /// A hairline, and the edge of anything that has one
    pub border: u32,
}

/// The palette the constants above spell out
pub const MIKU: Palette = Palette {
    page: PAGE,
    card: CARD,
    accent: ACCENT,
    on_accent: ON_ACCENT,
    text: TEXT,
    text_muted: TEXT_MUTED,
    border: BORDER,
};

impl Palette {
    /// Takes one `theme` line. True when the key is one of ours. Shared by the
    /// bar, window manager, and launcher.
    ///
    /// Accepts `background` for `page`, `surface` for `card`, and `muted` for
    /// `text_muted`, as other desktops' palettes write them
    pub fn take(&mut self, key: &str, value: &str) -> bool {
        let Some(c) = super::conf::colour(value) else { return false };
        match key {
            "page" | "background" => self.page = c,
            "card" | "surface" => self.card = c,
            "accent" => self.accent = c,
            "on_accent" => self.on_accent = c,
            "text" => self.text = c,
            "text_muted" | "muted" => self.text_muted = c,
            "border" => self.border = c,
            _ => return false,
        }
        true
    }

    /// One color over another at `alpha` parts of 255: "the text color at 15%".
    /// Worked out here so the wallpaper does not show through a pill lifted off
    /// its card. The result has the card's alpha
    pub fn mix(&self, over: u32, alpha: u8) -> u32 {
        at(super::paint::blend(self.card, over, alpha), alpha_of(self.card))
    }
}

/// Mouse button numbers as the protocol carries them
pub const BTN_LEFT: u32 = 0x110;

/// Keys, as the compositor's keycodes, the ones meaning the same in every
/// layout
pub const KEY_ENTER: u32 = 28;
pub const KEY_ESC: u32 = 1;
pub const KEY_SPACE: u32 = 57;
pub const KEY_TAB: u32 = 15;
pub const KEY_UP: u32 = 103;
pub const KEY_DOWN: u32 = 108;
pub const KEY_LEFT: u32 = 105;
pub const KEY_RIGHT: u32 = 106;
pub const KEY_BACKSPACE: u32 = 14;
pub const KEY_DELETE: u32 = 111;
pub const KEY_HOME: u32 = 102;
pub const KEY_END: u32 = 107;

const MAX_KEYS: usize = 16;
/// Characters per frame, for a held key repeating. Overflow drops the newest
const MAX_CHARS: usize = 16;
/// Controls per frame. A screen with an open list is the most: each row is one
const MAX_CONTROLS: usize = 64;

/// Ids a `select` owns: its own and one per open-list item. Selects are spaced
/// at least this far apart, so an open list knows a click belongs to it
pub const SELECT_SPAN: u32 = 64;

/// What a frame needs to remember from the last one
pub struct Ui {
    /// What this screen is drawn in. Set once at the top of a frame; defaults
    /// to the desktop's dark palette
    pub palette: Palette,
    /// Which control has the keyboard. 0 means none yet; the first drawn takes
    /// it
    pub focus: u32,
    /// Pointer position in surface pixels
    pub px: i32,
    pub py: i32,
    /// Held now. Only for drawing a control as pushed; a click is not read from
    /// this
    down: bool,
    /// Where the button went down and up since the last frame. Latched edges,
    /// since frames draw only on change and a press and release can land
    /// between two
    press_at: Option<(i32, i32)>,
    release_at: Option<(i32, i32)>,
    /// Which control the press landed on
    pressed: u32,
    /// Keys since the last frame, consumed as widgets ask
    keys: [u32; MAX_KEYS],
    n_keys: usize,
    /// Characters since the last frame, kept apart from keys. A keycode means
    /// the same under every layout; a character needs the layout, handed to the
    /// client over the protocol (`Event::Keymap`). The client decodes and
    /// pushes the answer
    chars: [char; MAX_CHARS],
    n_chars: usize,
    /// Ids drawn this frame in order, for Tab, and where each was. The
    /// rectangles serve the following frame, so `hit` can answer whether a
    /// pointer position would change anything without a repaint
    order: [u32; MAX_CONTROLS],
    rects: [Rect; MAX_CONTROLS],
    n_order: usize,
    /// Which `select` has its list open, or 0. An open list is modal, so a
    /// click choosing an item does not also press what is under it
    open: u32,
    /// Set when the focus should move at the end of the frame
    step: i32,
    /// A control reported itself activated this frame. That frame cannot show
    /// the result, so one more is owed
    acted: bool,
}

impl Ui {
    pub const fn new() -> Self {
        Self {
            palette: MIKU,
            focus: 0,
            px: -1,
            py: -1,
            down: false,
            press_at: None,
            release_at: None,
            pressed: 0,
            keys: [0; MAX_KEYS],
            n_keys: 0,
            chars: ['\0'; MAX_CHARS],
            n_chars: 0,
            order: [0; MAX_CONTROLS],
            rects: [Rect::new(0, 0, 0, 0); MAX_CONTROLS],
            n_order: 0,
            open: 0,
            step: 0,
            acted: false,
        }
    }

    /// Where the pointer is now. True when the frame would change: only on
    /// entering or leaving a control. The cursor is the compositor's
    pub fn pointer_moved(&mut self, x: i32, y: i32) -> bool {
        let before = self.hit(self.px, self.py);
        self.px = x;
        self.py = y;
        self.hit(x, y) != before
    }

    /// Which control is at a point, from where they were drawn last frame.
    /// Topmost wins
    pub fn hit(&self, x: i32, y: i32) -> u32 {
        let mut found = 0;
        for i in 0..self.n_order {
            if self.rects[i].contains(x, y) {
                found = self.order[i];
            }
        }
        found
    }

    /// The button went down or came up at the current position. Motion precedes
    /// the button in a batch, so the position held is where it was pressed
    pub fn pointer_button(&mut self, button: u32, pressed: bool) {
        if button != BTN_LEFT {
            return;
        }
        self.down = pressed;
        if pressed {
            self.press_at = Some((self.px, self.py));
        } else {
            self.release_at = Some((self.px, self.py));
        }
    }

    /// A key going down. Releases are dropped
    pub fn key(&mut self, keycode: u32, pressed: bool) {
        if pressed && self.n_keys < MAX_KEYS {
            self.keys[self.n_keys] = keycode;
            self.n_keys += 1;
        }
    }

    /// Called at the top of a frame, before any widget
    pub fn begin(&mut self) {
        self.n_order = 0;
        self.step = 0;
        self.acted = false;
        // Tab and the vertical arrows belong to the frame; the rest are left
        // for widgets
        let mut kept = [0u32; MAX_KEYS];
        let mut n = 0;
        for i in 0..self.n_keys {
            match self.keys[i] {
                KEY_TAB | KEY_DOWN => self.step += 1,
                KEY_UP => self.step -= 1,
                k => {
                    kept[n] = k;
                    n += 1;
                }
            }
        }
        self.keys = kept;
        self.n_keys = n;
    }

    /// Called at the bottom of a frame, after every widget. Returns whether the
    /// frame is out of date: focus moves here, after controls drew with the old
    /// one
    pub fn end(&mut self) -> bool {
        let was = self.focus;
        // A control no longer drawn cannot be focused, as on a screen change.
        // Without this the keyboard has nothing to act on until Tab
        let known = self.order[..self.n_order].contains(&self.focus);
        if !known && self.n_order > 0 {
            self.focus = self.order[0];
        }

        if self.step != 0 && self.n_order > 0 {
            let at = self
                .order
                .iter()
                .take(self.n_order)
                .position(|&id| id == self.focus)
                .unwrap_or(0) as i32;
            let n = self.n_order as i32;
            let next = (at + self.step).rem_euclid(n);
            self.focus = self.order[next as usize];
        }
        // The edges were offered to every control this frame, so they are
        // spent. The release ends its press
        if self.release_at.is_some() {
            self.pressed = 0;
        }
        self.press_at = None;
        self.release_at = None;
        self.n_keys = 0;
        self.n_chars = 0;
        self.focus != was || self.acted
    }

    /// Puts an open list away. The keyboard returns to the field, since the
    /// list's rows are gone from this frame
    fn close_list(&mut self, field: u32) {
        self.open = 0;
        self.focus = field;
        self.acted = true;
    }

    /// A character the client decoded from a key. Pushed beside `key`: a
    /// Backspace is a keycode with no character, a letter is both
    pub fn ch(&mut self, c: char) {
        // Control characters are not text; a field must not hold a byte nothing
        // can draw
        if (c as u32) < 0x20 || c as u32 == 0x7f {
            return;
        }
        if self.n_chars < MAX_CHARS {
            self.chars[self.n_chars] = c;
            self.n_chars += 1;
        }
    }

    /// Takes the pending focus movement instead of moving focus. `begin` folds
    /// Tab, Down, and Up into one number. A launcher has one field and a list,
    /// so the arrows drive the list and Tab stays on the field
    pub fn take_step(&mut self) -> i32 {
        core::mem::replace(&mut self.step, 0)
    }

    /// Takes one key if waiting, so two controls cannot act on it
    pub fn take_key(&mut self, code: u32) -> bool {
        for i in 0..self.n_keys {
            if self.keys[i] == code {
                self.keys[i] = 0;
                return true;
            }
        }
        false
    }

    /// Register a control and say how it should be drawn this frame
    fn visit(&mut self, id: u32, r: Rect) -> State {
        self.visit_if(id, r, true)
    }

    /// The same for a control shown but unusable: drawn and not registered, so
    /// Tab passes it and no click reaches it
    fn visit_if(&mut self, id: u32, r: Rect, enabled: bool) -> State {
        self.visit_keyed(id, r, enabled, true)
    }

    /// The same for a control Enter and Space do not activate: a text field.
    /// Enter means what the surrounding screen decided, so it stays available
    /// to the screen's `take_key(KEY_ENTER)`
    fn visit_keyed(&mut self, id: u32, r: Rect, enabled: bool, by_key: bool) -> State {
        // A control behind an open list is drawn and answers nothing, like a
        // disabled one
        let behind_list = self.open != 0 && !(id >= self.open && id < self.open + SELECT_SPAN);
        if !enabled || behind_list {
            return State {
                hovered: false,
                held: false,
                focused: false,
                activated: false,
                enabled: false,
            };
        }
        if self.n_order < self.order.len() {
            self.order[self.n_order] = id;
            self.rects[self.n_order] = r;
            self.n_order += 1;
        }
        if self.focus == 0 {
            self.focus = id;
        }

        let over = r.contains(self.px, self.py);
        if let Some((x, y)) = self.press_at {
            if r.contains(x, y) {
                self.pressed = id;
                self.focus = id;
            }
        }

        // A click is the release over the control the press landed on. A press
        // that wanders off and lets go is cancelled
        let clicked = matches!(self.release_at, Some((x, y)) if r.contains(x, y))
                      && self.pressed == id;
        let by_key = by_key && self.focus == id
                     && (self.take_key(KEY_ENTER) || self.take_key(KEY_SPACE));
        self.acted |= clicked || by_key;

        State {
            hovered: over,
            held: self.pressed == id && self.down && over,
            focused: self.focus == id,
            activated: clicked || by_key,
            enabled: true,
        }
    }
}

/// How one control stands this frame
#[derive(Clone, Copy)]
pub struct State {
    pub hovered: bool,
    pub held: bool,
    pub focused: bool,
    pub activated: bool,
    pub enabled: bool,
}

// Layout

/// Hands out rectangles down a column
pub struct Column {
    pub x: i32,
    pub y: i32,
    pub w: i32,
}

impl Column {
    pub fn new(r: Rect) -> Self {
        Self { x: r.x, y: r.y, w: r.w }
    }

    /// The next `h` pixels, then move down by `h + gap`
    pub fn row(&mut self, h: i32, gap: i32) -> Rect {
        let r = Rect::new(self.x, self.y, self.w, h);
        self.y += h + gap;
        r
    }

    pub fn skip(&mut self, h: i32) {
        self.y += h;
    }
}

/// Hands out rectangles from both ends of a row, as a bar needs. Keeps two
/// pens; `rest` is the gap between them, where a window title is ellipsized
pub struct Strip {
    pub left: i32,
    pub right: i32,
    pub y: i32,
    pub h: i32,
}

impl Strip {
    pub fn new(r: Rect) -> Self {
        Self { left: r.x, right: r.right(), y: r.y, h: r.h }
    }

    /// The next `w` pixels from the left, then move along by `w + gap`
    pub fn take_left(&mut self, w: i32, gap: i32) -> Rect {
        let r = Rect::new(self.left, self.y, w, self.h);
        self.left += w + gap;
        r
    }

    /// The next `w` pixels from the right, moving inwards
    pub fn take_right(&mut self, w: i32, gap: i32) -> Rect {
        self.right -= w;
        let r = Rect::new(self.right, self.y, w, self.h);
        self.right -= gap;
        r
    }

    /// Everything the two ends have not claimed. Empty when they have met
    pub fn rest(&self) -> Rect {
        Rect::new(self.left, self.y, (self.right - self.left).max(0), self.h)
    }
}

// The widgets

/// A line of text; nothing focuses or clicks it
pub fn label(c: &mut Canvas, face: usize, x: i32, y: i32, s: &str, colour: u32) {
    text::draw_face(c, face, x, y, s, colour);
}

/// Registers a control this file does not draw, since `visit_keyed` is private.
/// Used by the login screen's button, a color ramp
pub fn hot(ui: &mut Ui, id: u32, r: Rect) -> State {
    ui.visit(id, r)
}

/// A rounded panel, the thing everything else sits on
pub fn card(c: &mut Canvas, r: Rect, colour: u32) {
    c.fill_round_rect(r, 12, colour);
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ButtonStyle {
    /// Filled, for the one action the screen is about
    Primary,
    /// Outlined, for the way back
    Secondary,
}

/// Returns true on the frame it was activated
pub fn button(
    ui: &mut Ui,
    c: &mut Canvas,
    id: u32,
    r: Rect,
    text_: &str,
    style: ButtonStyle,
    arrow: bool,
) -> bool {
    button_icon(ui, c, id, r, text_, style, arrow, Icon::None)
}

/// A button with a drawing before its label, measured and centered together
pub fn button_icon(
    ui: &mut Ui,
    c: &mut Canvas,
    id: u32,
    r: Rect,
    text_: &str,
    style: ButtonStyle,
    arrow: bool,
    icon: Icon,
) -> bool {
    let p = ui.palette;
    let st = ui.visit(id, r);

    let (bg, fg) = match style {
        ButtonStyle::Primary => {
            let base = if st.held { p.mix(p.accent, 200) } else { p.accent };
            (base, p.on_accent)
        }
        ButtonStyle::Secondary => {
            let base = if st.hovered { p.mix(p.text, 20) } else { p.mix(p.text, 10) };
            (base, p.text)
        }
    };
    c.fill_round_rect(r, 10, bg);
    if st.focused {
        c.stroke_round_rect(r.inset(-2), 12, 2, p.accent);
    }

    // Label, icon, and arrow are centered as one block
    let tw = text::face_width(FACE_HEAD, text_);
    let icon_w = if icon == Icon::None { 0 } else { 28 };
    let arrow_w = if arrow { 26 } else { 0 };
    let mut x = r.x + (r.w - tw - arrow_w - icon_w) / 2;
    let ty = r.y + (r.h - text::face_line_height(FACE_HEAD)) / 2;
    if icon != Icon::None {
        draw_icon(c, icon, Rect::new(x, r.y, 24, r.h), fg);
        x += icon_w;
    }
    text::draw_face(c, FACE_HEAD, x, ty, text_, fg);
    if arrow {
        arrow_right(c, x + tw + 10, r.y + r.h / 2, fg);
    }

    st.activated
}

/// What a row of a list says. An empty string draws nothing, so one row serves
/// a disk, a network, or a setting
pub struct Row<'r> {
    pub title: &'r str,
    pub subtitle: &'r str,
    /// What stands at the right-hand end of it
    pub right: Right<'r>,
    pub selected: bool,
    pub icon: Icon,
    /// Drawn and unusable: dimmed, skipped by the keyboard, unreachable by
    /// clicks
    pub enabled: bool,
}

impl<'r> Row<'r> {
    /// A plain row: a title, everything else off
    pub const fn new(title: &'r str) -> Self {
        Self {
            title,
            subtitle: "",
            right: Right::None,
            selected: false,
            icon: Icon::None,
            enabled: true,
        }
    }
}

/// A row's right-hand end: a disk states a number, a row leading elsewhere
/// shows where, a setting shows its switch
pub enum Right<'r> {
    None,
    /// Two right-aligned lines: "Free" over "812.4 GB" in the design
    Pair { top: &'r str, bottom: &'r str },
    /// Points onwards, for a row that opens something
    Arrow,
    /// A switch, drawn but owned by the caller
    Switch(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    None,
    Disk,
    Gear,
    Lock,
    Refresh,
    Close,
    Back,
    ChevronDown,
    /// A magnifier, for the one thing on this desktop that searches
    Search,
    /// A head and shoulders, for the field somebody's name goes in
    Person,
    /// An eye and the same eye struck through: "show what I typed" and "stop
    /// showing it"
    Eye,
    EyeOff,
}

/// Toward the page color, for what is shown but not offered
fn dim(colour: u32) -> u32 {
    super::paint::blend(PAGE, colour, 120)
}

/// A selectable row. Returns true when chosen this frame. Rows are flat inside
/// the caller's `card` and rise only when hovered or chosen, so a list reads as
/// one
pub fn list_row(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect, row: &Row) -> bool {
    let p = ui.palette;
    let st = ui.visit_if(id, r, row.enabled);

    if row.selected {
        c.fill_round_rect(r, 10, p.mix(p.accent, 40));
        c.stroke_round_rect(r, 10, 2, p.accent);
    } else if st.hovered {
        c.fill_round_rect(r, 10, p.mix(p.text, 16));
    }
    if st.focused && !row.selected {
        c.stroke_round_rect(r, 10, 1, p.accent);
    }

    let (fg, fg_sub) = if row.enabled {
        (p.text, p.text_muted)
    } else {
        (p.mix(p.text, 120), p.mix(p.text_muted, 120))
    };

    let mut x = r.x + 14;
    if row.icon != Icon::None {
        // The tile is the accent itself with the drawing cut out dark, as in
        // the design
        let box_ = Rect::new(x, r.y + (r.h - 44) / 2, 44, 44);
        let (tile, glyph) = if row.enabled {
            (p.accent, p.on_accent)
        } else {
            (p.mix(p.accent, 120), p.mix(p.text, 120))
        };
        c.fill_round_rect(box_, 12, tile);
        draw_icon(c, row.icon, box_, glyph);
        x += 60;
    }

    // Two lines on the left and the right side, each vertically centered as a
    // block
    let lh_title = text::face_line_height(FACE_HEAD);
    let lh_sub = text::face_line_height(FACE_SMALL);
    let has_sub = !row.subtitle.is_empty();
    let block = lh_title + if has_sub { lh_sub } else { 0 };
    let top = r.y + (r.h - block) / 2;

    // The right end is measured first, since the left stops where it begins
    let mut right = r.right() - 16;
    if row.selected {
        let cx = right - 14;
        let cy = r.y + r.h / 2;
        c.fill_circle(cx, cy, 14, p.accent);
        tick(c, cx, cy, p.on_accent);
        right -= 42;
    }

    let limit = match &row.right {
        Right::None => right,
        Right::Pair { top: t, bottom: b } => {
            let wt = text::face_width(FACE_SMALL, t);
            let wb = text::face_width(FACE_HEAD, b);
            if !t.is_empty() {
                text::draw_face(c, FACE_SMALL, right - wt, top, t, fg_sub);
            }
            if !b.is_empty() {
                text::draw_face(c, FACE_HEAD, right - wb, top + lh_sub, b, fg);
            }
            right - wt.max(wb) - 12
        }
        Right::Arrow => {
            arrow_right(c, right - 16, r.y + r.h / 2, fg_sub);
            right - 32
        }
        Right::Switch(on) => {
            // Drawn by the row, not `toggle`: the whole row is the control
            let track = Rect::new(right - 52, r.y + (r.h - 26) / 2, 52, 26);
            switch_face(c, track, *on, row.enabled, st.focused);
            right - 68
        }
    };

    text::draw_face_within(c, FACE_HEAD, x, top, limit, row.title, fg);
    if has_sub {
        text::draw_face_within(c, FACE_SMALL, x, top + lh_title, limit, row.subtitle, fg_sub);
    }

    st.activated
}

// Picking one of a few things
//
// Two calls, since an open list is drawn over the screen: `select` places the
// closed field, and `select_list` is called after everything else. The caller
// keeps the value, as with `toggle`

/// What a field is choosing between
pub struct Select<'s> {
    pub items: &'s [&'s str],
    /// A muted line at the right of each open-list item: the offset beside a
    /// time zone. Empty when unneeded
    pub notes: &'s [&'s str],
    /// Which one is chosen now
    pub current: usize,
}

const ITEM_H: i32 = 34;

/// The closed field: what is chosen now, and a chevron
pub fn select(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect, sel: &Select) -> bool {
    let st = ui.visit(id, r);
    let open = ui.open == id;

    let bg = if open || st.hovered || st.held { CARD_HOVER } else { FIELD };
    c.fill_round_rect(r, 10, bg);
    c.stroke_round_rect(r, 10, if st.focused { 2 } else { 1 },
                        if st.focused || open { ACCENT } else { BORDER });

    let label = sel.items.get(sel.current).copied().unwrap_or("");
    let ty = r.y + (r.h - text::face_line_height(FACE_BODY)) / 2;
    text::draw_face_within(c, FACE_BODY, r.x + 16, ty, r.right() - 44, label, TEXT);
    // Points the way the list will go
    chevron(c, r.right() - 26, r.y + r.h / 2, !open, TEXT_MUTED);

    if st.activated {
        ui.open = if open { 0 } else { id };
    }
    st.activated
}

/// The open list, drawn last. Returns the item chosen this frame
pub fn select_list(ui: &mut Ui, c: &mut Canvas, id: u32, field: Rect, sel: &Select) -> Option<usize> {
    if ui.open != id || sel.items.is_empty() {
        return None;
    }
    // Esc puts it away without choosing
    if ui.take_key(KEY_ESC) {
        ui.close_list(id);
        return None;
    }

    // Below the field if it fits, above if not, and never taller than the room
    let gap = 6;
    let below = c.h - 10 - (field.bottom() + gap);
    let above = field.y - gap - 10;
    let down = below >= above;
    let room = if down { below } else { above };
    let n = sel.items.len() as i32;
    let visible = ((room - 8) / ITEM_H).clamp(1, n);
    let h = visible * ITEM_H + 8;
    let y = if down { field.bottom() + gap } else { field.y - gap - h };
    let list = Rect::new(field.x, y, field.w, h);

    // Which items are shown when they do not all fit, with the chosen one in
    // the middle
    let first = (sel.current as i32 - visible / 2).clamp(0, (n - visible).max(0));

    c.fill_round_rect(list, 12, CARD);
    c.stroke_round_rect(list, 12, 1, BORDER);

    let mut chosen = None;
    for slot in 0..visible {
        let i = (first + slot) as usize;
        let r = Rect::new(list.x + 4, list.y + 4 + slot * ITEM_H, list.w - 8, ITEM_H);
        let st = ui.visit(id + 1 + i as u32, r);
        if st.hovered || st.focused {
            c.fill_round_rect(r.inset(1), 8, CARD_HOVER);
        }
        let is_current = i == sel.current;
        let ty = r.y + (r.h - text::face_line_height(FACE_BODY)) / 2;
        let mut limit = r.right() - 14;
        if is_current {
            tick(c, r.right() - 20, r.y + r.h / 2, ACCENT);
            limit -= 22;
        }
        if let Some(note) = sel.notes.get(i) {
            if !note.is_empty() {
                let nw = text::face_width(FACE_SMALL, note);
                let ny = r.y + (r.h - text::face_line_height(FACE_SMALL)) / 2;
                text::draw_face(c, FACE_SMALL, limit - nw, ny, note, TEXT_MUTED);
                limit -= nw + 14;
            }
        }
        let colour = if is_current { ACCENT } else { TEXT };
        text::draw_face_within(c, FACE_BODY, r.x + 14, ty, limit, sel.items[i], colour);
        if st.activated {
            chosen = Some(i);
        }
    }

    if chosen.is_some() {
        ui.close_list(id);
    } else if let Some((px, py)) = ui.press_at {
        // A press elsewhere puts the list away; `visit_if` keeps anything
        // behind from answering it
        if !list.contains(px, py) && !field.contains(px, py) {
            ui.close_list(id);
        }
    }
    chosen
}

/// Whether a list is up, for a screen acting on a key of its own
pub fn list_open(ui: &Ui) -> bool {
    ui.open != 0
}

/// A line of text to type into. `secret` below is the same control with dots.
///
/// Owns nothing: the text is the caller's buffer and length. `buf` is UTF-8 and
/// `len` how much is used. Returns whether the text changed, which a launcher
/// filters on.
///
/// Characters come from `Ui::ch`, decoded by the client through the
/// compositor's keymap. Backspace and Delete step by a character, not a byte,
/// as Cyrillic is two bytes in UTF-8.
///
/// Left, Right, Home, and End move the caret. No selection or clipboard
pub fn field(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect,
             buf: &mut [u8], len: &mut usize, caret: &mut usize,
             placeholder: &str, p: &Palette) -> bool
{
    input(ui, c, id, r, buf, len, caret, placeholder, p, None)
}

/// The same field showing dots. One mask character per text character, not
/// byte, so a Cyrillic password does not reveal which letters are non-ASCII.
///
/// The rest is `field`: editing, caret, UTF-8 boundaries.
///
/// Hides the text from the room, not the machine: the bytes are in the caller's
/// buffer, and the caller wipes them after sending. `maygreet` does so on every
/// path
pub fn secret(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect,
              buf: &mut [u8], len: &mut usize, caret: &mut usize,
              placeholder: &str, p: &Palette) -> bool
{
    input(ui, c, id, r, buf, len, caret, placeholder, p, Some(MASK))
}

/// A field inside a filled pill with a glyph at its left, as the login screen
/// uses. Here because `visit_keyed` is private to this file, so the pill and
/// field can be one control.
///
/// `mask` is the caller's: the eye beside a password box is an ordinary
/// `icon_button`, and the caller passes `false` on the next frame to reveal.
/// `pad_right` is room for that eye
pub fn field_row(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect,
                 buf: &mut [u8], len: &mut usize, caret: &mut usize,
                 placeholder: &str, p: &Palette,
                 fill: u32, icon: Icon, mask: bool, pad_right: i32) -> bool
{
    // A pill: radius half the height. Drawn before the field, so caret and text
    // land on top
    c.fill_round_rect(r, r.h / 2, fill);
    let gap = r.h / 2;
    let glyph = Rect::new(r.x + gap - 8, r.y + (r.h - 20) / 2, 20, 20);
    draw_icon(c, icon, glyph, p.text_muted);
    let inner = Rect::new(glyph.right() + 10, r.y,
                          (r.w - (glyph.right() - r.x) - 10 - pad_right).max(1), r.h);
    let changed = input(ui, c, id, inner, buf, len, caret, placeholder, p,
                        if mask { Some(MASK) } else { None });
    // The focus ring goes round the pill, after `input` has drawn over this
    // edge
    if ui.focus == id {
        c.stroke_round_rect(r, r.h / 2, 2, p.accent);
    }
    changed
}

/// A hidden character. `fontgen.rs` builds U+2022
const MASK: char = '\u{2022}';

/// The shared body. `mask` changes what is drawn, not what is stored, so a
/// caller can send the same bytes
fn input(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect,
         buf: &mut [u8], len: &mut usize, caret: &mut usize,
         placeholder: &str, p: &Palette, mask: Option<char>) -> bool
{
    // Not `visit`: Enter belongs to the screen, and Space is a typed character.
    // See `visit_keyed`
    let st = ui.visit_keyed(id, r, true, false);
    // A click puts the keyboard here, with the caret at the end
    if st.activated {
        *caret = *len;
    }
    let mut changed = false;
    if st.focused {
        changed = edit(ui, buf, len, caret);
    }

    // The text or the placeholder, at the same baseline
    let text = core::str::from_utf8(&buf[..*len]).unwrap_or("");
    let baseline = r.y + (r.h - text::face_line_height(FACE_BODY)) / 2;
    if text.is_empty() && !placeholder.is_empty() {
        text::draw_face(c, FACE_BODY, r.x, baseline, placeholder, p.text_muted);
    } else if let Some(m) = mask {
        draw_mask(c, r.x, baseline, m, text.chars().count(), p.text);
    } else {
        text::draw_face(c, FACE_BODY, r.x, baseline, text, p.text);
    }

    // The caret, a solid bar at the end of the text. Not blinking: a blink is a
    // repaint on a timer, copying the whole box twice a second
    if st.focused {
        let upto = core::str::from_utf8(&buf[..*caret]).unwrap_or(text);
        let x = match mask {
            Some(m) => r.x + mask_width(m, upto.chars().count()),
            None => r.x + text::face_width(FACE_BODY, upto),
        };
        // As tall as the line, not the row
        let tall = text::face_line_height(FACE_BODY);
        c.fill_rect(Rect::new(x, baseline - 1, 2, tall + 2), p.accent);
    }
    changed
}

/// `n` mask characters, drawn one at a time with no heap copy holding the
/// password's length
fn draw_mask(c: &mut Canvas, x: i32, y: i32, m: char, n: usize, colour: u32) {
    let mut enc = [0u8; 4];
    let one: &str = m.encode_utf8(&mut enc);
    let advance = text::face_width(FACE_BODY, one);
    for i in 0..n {
        text::draw_face(c, FACE_BODY, x + advance * i as i32, y, one, colour);
    }
}

/// How wide `n` of them are
fn mask_width(m: char, n: usize) -> i32 {
    let mut enc = [0u8; 4];
    text::face_width(FACE_BODY, m.encode_utf8(&mut enc)) * n as i32
}

/// The editing. Returns whether the text changed
fn edit(ui: &mut Ui, buf: &mut [u8], len: &mut usize, caret: &mut usize) -> bool {
    let mut changed = false;

    if ui.take_key(KEY_HOME) {
        *caret = 0;
    }
    if ui.take_key(KEY_END) {
        *caret = *len;
    }
    if ui.take_key(KEY_LEFT) {
        *caret = prev_boundary(&buf[..*len], *caret);
    }
    if ui.take_key(KEY_RIGHT) {
        *caret = next_boundary(&buf[..*len], *caret);
    }
    if ui.take_key(KEY_BACKSPACE) && *caret > 0 {
        let from = prev_boundary(&buf[..*len], *caret);
        buf.copy_within(*caret..*len, from);
        *len -= *caret - from;
        *caret = from;
        changed = true;
    }
    if ui.take_key(KEY_DELETE) && *caret < *len {
        let to = next_boundary(&buf[..*len], *caret);
        buf.copy_within(to..*len, *caret);
        *len -= to - *caret;
        changed = true;
    }

    // Typed characters last, so a backspace and its replacement land in pressed
    // order
    for i in 0..ui.n_chars {
        let ch = ui.chars[i];
        let mut enc = [0u8; 4];
        let bytes = ch.encode_utf8(&mut enc).as_bytes();
        if *len + bytes.len() > buf.len() {
            break;
        }
        buf.copy_within(*caret..*len, *caret + bytes.len());
        buf[*caret..*caret + bytes.len()].copy_from_slice(bytes);
        *len += bytes.len();
        *caret += bytes.len();
        changed = true;
    }
    ui.n_chars = 0;
    changed
}

/// The byte before `at` that starts a character, or 0. UTF-8 continuation bytes
/// are `10xxxxxx`
fn prev_boundary(s: &[u8], at: usize) -> usize {
    let mut i = at.min(s.len());
    while i > 0 {
        i -= 1;
        if s[i] & 0xC0 != 0x80 {
            return i;
        }
    }
    0
}

/// The byte after `at` that starts a character, or the end
fn next_boundary(s: &[u8], at: usize) -> usize {
    let mut i = (at + 1).min(s.len());
    while i < s.len() && s[i] & 0xC0 == 0x80 {
        i += 1;
    }
    i
}

/// A line between two rows of the same card
pub fn divider(c: &mut Canvas, x: i32, y: i32, w: i32) {
    c.fill_rect(Rect::new(x, y, w, 1), 0x2E2624);
}

/// A round or square button with a drawing and no words. Takes a palette, since
/// the login screen is light
pub fn icon_button(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect,
                   icon: Icon, round: bool) -> bool {
    let p = ui.palette;
    let st = ui.visit(id, r);
    let radius = if round { r.h / 2 } else { 12 };
    let fill = if st.held {
        p.mix(p.accent, 90)
    } else if st.hovered {
        p.mix(p.text, 20)
    } else {
        p.mix(p.text, 10)
    };
    c.fill_round_rect(r, radius, fill);
    c.stroke_round_rect(r, radius, if st.focused { 2 } else { 1 },
                        if st.focused { p.accent } else { p.border });
    // Inset: edge to edge, the glyph would touch the ring, and its stroke
    // scales with its box
    draw_icon(c, icon, r.inset(r.h / 6), p.text);
    st.activated
}

/// A switch. Returns true on the frame it was flipped; the caller owns the
/// value
pub fn toggle(ui: &mut Ui, c: &mut Canvas, id: u32, r: Rect, on: bool) -> bool {
    let st = ui.visit(id, r);
    switch_face(c, Rect::new(r.x, r.y + (r.h - 26) / 2, 52, 26), on, true, st.focused);
    st.activated
}

/// A switch's look, used by `toggle` and by a row ending in a switch
fn switch_face(c: &mut Canvas, track: Rect, on: bool, enabled: bool, focused: bool) {
    let (fill, knob) = match (on, enabled) {
        (true, true) => (ACCENT, ON_ACCENT),
        (false, true) => (FIELD, 0xC7BBBB),
        (true, false) => (dim(ACCENT), dim(TEXT)),
        (false, false) => (dim(FIELD), dim(0xC7BBBB)),
    };
    c.fill_round_rect(track, track.h / 2, fill);
    if focused {
        c.stroke_round_rect(track.inset(-2), track.h / 2 + 2, 2, ACCENT);
    }
    let knob_x = if on { track.right() - 23 } else { track.x + 3 };
    c.fill_circle(knob_x + 10, track.y + track.h / 2, 10, knob);
}

/// A bar filling left to right. `done` and `total` keep the caller from
/// dividing by zero
pub fn progress(c: &mut Canvas, r: Rect, done: u64, total: u64) {
    c.fill_round_rect(r, r.h / 2, FIELD);
    if total == 0 {
        return;
    }
    let w = ((done.min(total) as u128 * r.w as u128) / total as u128) as i32;
    if w > 1 {
        c.fill_round_rect(Rect::new(r.x, r.y, w.max(r.h), r.h), r.h / 2, ACCENT);
    }
}

// The small drawings
//
// Vectors, drawn at any tile size. All are strokes of one weight with round
// ends, using `stroke`, `arc`, and `fill_poly` from `paint.rs`

/// Weight of every icon, from its box size, so a 40-pixel button and a 44-pixel
/// tile match
fn weight(r: Rect) -> i32 {
    (r.w.min(r.h) / 12).clamp(2, 3)
}

/// A point on a circle about (cx, cy), in degrees from east, clockwise, as
/// `paint` measures
fn on_circle(cx: i32, cy: i32, rad: i32, deg: i32) -> (i32, i32) {
    let (s, c_) = super::paint::sin_cos_deg(deg);
    (cx + scale(c_, rad), cy + scale(s, rad))
}

/// A 1024ths fraction of a length, rounded so both ends of a shape move equally
fn scale(fraction: i32, len: i32) -> i32 {
    let v = fraction * len;
    if v >= 0 { (v + 512) / 1024 } else { -((-v + 512) / 1024) }
}

/// The arrow ending a row or button, from its left edge and middle: the drawing
/// of `Icon::Back`, placed by a point on a line of text
pub const ARROW: i32 = 16;

pub fn arrow_right(c: &mut Canvas, x: i32, y: i32, colour: u32) {
    art(c, "arrow-right", Rect::new(x, y - ARROW / 2, ARROW, ARROW), colour);
}

pub fn arrow_left(c: &mut Canvas, x: i32, y: i32, colour: u32) {
    art(c, "arrow-left", Rect::new(x, y - ARROW / 2, ARROW, ARROW), colour);
}

fn tick(c: &mut Canvas, cx: i32, cy: i32, colour: u32) {
    c.stroke(cx - 5, cy, cx - 1, cy + 4, 2, colour);
    c.stroke(cx - 1, cy + 4, cx + 6, cy - 5, 2, colour);
}

/// The two strokes of a chevron, pointing down when `down` and up otherwise
fn chevron(c: &mut Canvas, cx: i32, cy: i32, down: bool, colour: u32) {
    let d = if down { 1 } else { -1 };
    c.stroke(cx - 5, cy - 2 * d, cx, cy + 3 * d, 2, colour);
    c.stroke(cx, cy + 3 * d, cx + 5, cy - 2 * d, 2, colour);
}

/// One drawing, centred in a rectangle
pub fn draw_icon(c: &mut Canvas, icon: Icon, r: Rect, colour: u32) {
    // Each is a file in `assets/icons-lucide`, flattened by `icongen.rs`. A new
    // picture is an SVG there and a name here
    let name = match icon {
        Icon::None => return,
        Icon::Disk => "hard-drive",
        Icon::Gear => "settings",
        Icon::Lock => "lock",
        Icon::Refresh => "refresh-cw",
        Icon::Close => "x",
        Icon::Back => "arrow-left",
        Icon::ChevronDown => "chevron-down",
        Icon::Search => "search",
        Icon::Person => "user",
        Icon::Eye => "eye",
        Icon::EyeOff => {
            icon_eye(c, r, colour, true);
            return;
        }
    };
    art(c, name, r, colour);
}


/// Draws one of the generated icons, scaled into `r` and centered. The
/// polylines come from `icongen.rs`; `Canvas::stroke`'s round ends are the
/// paths' `stroke-linecap="round"` and also make round joins.
///
/// Width is the path's `stroke-width` scaled like the coordinates, at least one
fn draw_art(c: &mut Canvas, art: &super::icons::IconArt, r: Rect, colour: u32) {
    use super::icons::{ICON_STROKE, ICON_UNITS};
    const Q: i32 = 64;
    let side = r.w.min(r.h);
    if side <= 0 {
        return;
    }
    // Sub-pixel throughout; rounded points would give a staircase, and a
    // rounded width only spidery or heavy
    let (ox, oy) = ((r.x + (r.w - side) / 2) * Q, (r.y + (r.h - side) / 2) * Q);
    let map = |v: i16, o: i32| o + (v as i32 * side * Q) / ICON_UNITS;
    let half = ((ICON_STROKE * side * Q) / ICON_UNITS / 2).max(Q / 2);

    // On the stack, as the kernel links this with no allocator. The size is
    // what the build measured across all icons
    let mut pts = [(0i32, 0i32); super::icons::ICON_MAX_POINTS];
    for stroke in art.strokes {
        let n = stroke.len().min(pts.len());
        for (i, &(x, y)) in stroke[..n].iter().enumerate() {
            pts[i] = (map(x, ox), map(y, oy));
        }
        c.stroke_path(&pts[..n], half, colour);
    }
}

/// The same, by the name of the file the icon came from
fn art(c: &mut Canvas, name: &str, r: Rect, colour: u32) {
    if let Some(a) = super::icons::icon_art(name) {
        draw_art(c, a, r, colour);
    }
}

/// The eye, struck through to mean "stop showing it". One glyph with a stroke,
/// as lucide's `eye-off` has a different eye
fn icon_eye(c: &mut Canvas, r: Rect, colour: u32, struck: bool) {
    art(c, "eye", r, colour);
    if struck {
        let side = r.w.min(r.h);
        let (ox, oy) = (r.x + (r.w - side) / 2, r.y + (r.h - side) / 2);
        let (a, b) = (side / 6, side - side / 6);
        c.stroke(ox + a, oy + b, ox + b, oy + a, weight(r), colour);
    }
}

fn icon_refresh(c: &mut Canvas, r: Rect, colour: u32) {
    let (cx, cy) = (r.x + r.w / 2, r.y + r.h / 2);
    let w = weight(r);
    let rad = (r.w.min(r.h) / 2 - w - 2).clamp(6, 10);
    // Almost a whole ring, open at the top right, with the head continuing
    // clockwise from where it stops, showing the direction
    let (start, sweep) = (340, 300);
    c.arc(cx, cy, rad + w / 2, rad - (w + 1) / 2, start, sweep, colour);

    let end = start + sweep;
    let (s, co) = super::paint::sin_cos_deg(end);
    let head = on_circle(cx, cy, rad, end);
    // Outward and along the direction: the head is a triangle across the band
    let (ox, oy) = (scale(co, w + 2), scale(s, w + 2));
    let (ax, ay) = (scale(-s, w + 5), scale(co, w + 5));
    c.fill_tri(
        (head.0 + ox, head.1 + oy),
        (head.0 + ax, head.1 + ay),
        (head.0 - ox, head.1 - oy),
        colour,
    );
}

fn icon_disk(c: &mut Canvas, r: Rect, colour: u32) {
    let w = weight(r);
    let side = (r.w.min(r.h) * 5 / 8).max(16);
    let body = r.centred(side, side);
    c.stroke_round_rect(body, side / 4, w, colour);
    // A platter and spindle. The ring clears the spindle by more than its
    // thickness, to stay distinct at drawn size
    let (cx, cy) = (body.x + body.w / 2, body.y + body.h / 2);
    c.stroke_circle(cx, cy, side * 5 / 16, w, colour);
    c.fill_circle(cx, cy, (side / 14).max(2), colour);
}

