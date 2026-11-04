// Text selection and its popup menu
//
// Selection: a range of cells, marked by dragging and drawn inverted. Two grid
// positions only; text is read from the grid on copy.
//
// Menu: a popup surface holding the pointer while open, able to extend past the
// window.
//
// Copy goes out through `may_clipboard`. Paste returns through `may_console`,
// since text must arrive as input.

use super::conn::{Conn, Pool};
use super::grid::{self, Grid};
use super::protocol;
use super::requests::Requests;
use super::text;

/// The menu's items, in the order they are drawn
pub const ITEMS: &[&str] = &["Copy", "Paste"];
const ITEM_H: i32 = 22;
const MENU_W: i32 = 120;
const MENU_PAD: i32 = 4;

const MENU_BG: u32 = 0x0022_1E28;
const MENU_FG: u32 = 0x00E4_DEF4;
const MENU_HI: u32 = 0x0043_3B57;
const MENU_DIM: u32 = 0x006A_6280;

/// Copy limit; a full 256 by 96 screen fits
const MAX_COPY: usize = 32 * 1024;

/// Drag start and current position in cells
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub from: (usize, usize),
    pub to: (usize, usize),
}

impl Span {
    /// The two ends in reading order, whichever way the drag went
    pub fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        let a = (self.from.1, self.from.0);
        let b = (self.to.1, self.to.0);
        if a <= b { (self.from, self.to) } else { (self.to, self.from) }
    }

    /// Whether this cell is selected, in reading order rather than as a
    /// rectangle
    pub fn covers(&self, col: usize, row: usize) -> bool {
        let (a, b) = self.ordered();
        let here = (row, col);
        (a.1, a.0) <= here && here <= (b.1, b.0)
    }

    pub fn is_empty(&self) -> bool { self.from == self.to }
}

/// Pointer state for this terminal
pub struct Menu {
    /// Where the cursor is inside the terminal, in pixels
    pub at: (i32, i32),
    /// Surface the pointer is in, since motion carries none
    over: u32,
    /// The selection, and whether a drag is still adding to it
    pub span: Option<Span>,
    dragging: bool,
    /// Last input serial, required for grabs and copies
    pub serial: u32,

    /// The menu's own surface, when it is open
    open: Option<Popup>,
    /// Item under the pointer, for the highlight
    hot: Option<usize>,
    /// Created once and kept
    positioner: u32,
    /// The pool a copy is staged through, made on the first copy
    copy_pool: Option<Pool>,
}

struct Popup {
    surface: u32,
    xdg_surface: u32,
    popup: u32,
    pool: Pool,
    pool_obj: u32,
    buffer: u32,
    w: i32,
    h: i32,
    /// Popup position relative to this window, to map pointer positions to
    /// items
    committed: bool,
}

impl Menu {
    pub const fn new() -> Self {
        Menu {
            at: (0, 0),
            over: 0,
            span: None,
            dragging: false,
            serial: 0,
            open: None,
            hot: None,
            positioner: 0,
            copy_pool: None,
        }
    }

    pub fn is_open(&self) -> bool { self.open.is_some() }

    /// The menu's surface
    pub fn surface(&self) -> u32 {
        self.open.as_ref().map(|p| p.surface).unwrap_or(0)
    }

    /// Whether the pointer is in the open menu
    pub fn pointer_in_menu(&self) -> bool {
        self.over != 0 && self.over == self.surface()
    }

    /// The pointer entered one of this program's surfaces
    pub fn entered(&mut self, surface: u32, x: i32, y: i32) {
        self.over = surface;
        if surface != self.surface() {
            self.at = (x, y);
        }
    }

    pub fn left(&mut self, surface: u32) {
        if self.over == surface {
            self.over = 0;
        }
        // A drag that leaves the window ends, since the release goes elsewhere
        if surface != self.surface() {
            self.dragging = false;
        }
    }

    /// Cell under a pixel position, clamped so a drag past the edge reaches the
    /// end
    pub fn cell_at(g: &Grid, x: i32, y: i32) -> (usize, usize) {
        let col = ((x - grid::PAD).max(0) / grid::CELL_W) as usize;
        let row = ((y - grid::PAD).max(0) / grid::LINE_H) as usize;
        (col.min(g.cols.saturating_sub(1)), row.min(g.rows.saturating_sub(1)))
    }

    // Selection

    /// Left button down: starts a selection
    pub fn begin_drag(&mut self, g: &Grid, x: i32, y: i32) {
        let cell = Self::cell_at(g, x, y);
        self.span = Some(Span { from: cell, to: cell });
        self.dragging = true;
    }

    /// Pointer moved. Returns whether a redraw is needed
    pub fn drag_to(&mut self, g: &Grid, x: i32, y: i32) -> bool {
        self.at = (x, y);
        if !self.dragging {
            return false;
        }
        let cell = Self::cell_at(g, x, y);
        match self.span.as_mut() {
            Some(s) if s.to != cell => { s.to = cell; true }
            _ => false,
        }
    }

    /// Left button up. A click, or a drag over only blank cells, selects
    /// nothing
    pub fn end_drag(&mut self, g: &Grid) {
        self.dragging = false;
        let empty = match self.span {
            None => return,
            Some(s) => s.is_empty() || !Self::holds_anything(g, s),
        };
        if empty {
            self.span = None;
        }
    }

    /// Whether a span covers a character, read from the grid now
    fn holds_anything(g: &Grid, span: Span) -> bool {
        let (a, b) = span.ordered();
        for row in a.1..=b.1 {
            let from = if row == a.1 { a.0 } else { 0 };
            let to = if row == b.1 { b.0 } else { g.cols.saturating_sub(1) };
            for col in from..=to {
                match g.char_at(col, row) {
                    Some(ch) if ch != ' ' && ch != '\0' => return true,
                    _ => {}
                }
            }
        }
        false
    }

    /// Writes selected text to the staging pool and returns its length. Read at
    /// copy time
    fn write_selection(&mut self, g: &Grid) -> usize {
        let Some(span) = self.span else { return 0 };
        let (a, b) = span.ordered();

        // Upper bound from the selection's rectangle: rows times columns plus
        // newlines
        let want = (((b.1 - a.1 + 1) * (g.cols + 1)) + 1).min(MAX_COPY).max(64);
        let big_enough = self.copy_pool.as_ref().is_some_and(|p| p.size >= want);
        if !big_enough {
            if let Some(old) = self.copy_pool.as_mut() { old.free(); }
            self.copy_pool = Pool::new(want);
        }
        let Some(pool) = self.copy_pool.as_mut() else { return 0 };
        let cap = pool.size.min(MAX_COPY);
        let out = pool.bytes();
        let mut n = 0;

        let mut row = a.1;
        while row <= b.1 && row < g.rows {
            let first = if row == a.1 { a.0 } else { 0 };
            let last = if row == b.1 { b.0 } else { g.cols.saturating_sub(1) };
            let line_start = n;
            let mut col = first;
            while col <= last && col < g.cols && n < cap {
                let ch = g.char_at(col, row).unwrap_or(' ');
                // As UTF-8
                let mut enc = [0u8; 4];
                let bytes = ch.encode_utf8(&mut enc).as_bytes();
                // Stops rather than cutting a character
                if n + bytes.len() > cap { break; }
                out[n..n + bytes.len()].copy_from_slice(bytes);
                n += bytes.len();
                col += 1;
            }
            // Trims trailing padding, except on the last row
            if row < b.1 {
                while n > line_start && out[n - 1] == b' ' { n -= 1; }
                if n < cap { out[n] = b'\n'; n += 1; }
            }
            row += 1;
        }
        n
    }

    // Menu

    /// Opens the menu at the pointer, hanging off this window. Only the
    /// positioner persists, since a surface's role is set once
    pub fn open(&mut self, c: &mut Conn, shm: u32, xdg: u32, comp: u32,
                parent_xdg: u32, seat: u32) {
        if self.open.is_some() || self.serial == 0 {
            return;
        }
        let h = ITEMS.len() as i32 * ITEM_H + 2 * MENU_PAD;
        let w = MENU_W;

        if self.positioner == 0 {
            self.positioner = c.create_positioner(xdg);
        }
        c.positioner_set_size(self.positioner, w, h);
        // One-pixel anchor at the cursor
        c.positioner_set_anchor_rect(self.positioner, self.at.0, self.at.1, 1, 1);
        c.positioner_set_anchor(self.positioner, protocol::positioner::anchor::BOTTOM_RIGHT);
        c.positioner_set_gravity(self.positioner, protocol::positioner::gravity::BOTTOM_RIGHT);
        // Flips left near the right edge and slides up near the bottom
        c.positioner_set_constraint(self.positioner,
            protocol::positioner::adjust::FLIP_X | protocol::positioner::adjust::FLIP_Y
            | protocol::positioner::adjust::SLIDE_X | protocol::positioner::adjust::SLIDE_Y);

        let bytes = (w * h * 4) as usize;
        let Some(pool) = Pool::new(bytes) else { return };
        let pool_obj = c.create_pool(shm, pool.handle, bytes as i32);
        let buffer = c.create_buffer(pool_obj, 0, w, h, w * 4,
                                     protocol::shm::FORMAT_XRGB8888);
        let surface = c.create_surface(comp);
        let xdg_surface = c.get_xdg_surface(xdg, surface);
        let popup = c.get_popup(xdg_surface, parent_xdg, self.positioner);
        // Grabbed immediately, so a click away closes it
        c.popup_grab(popup, seat, self.serial);
        c.flush();

        self.open = Some(Popup {
            surface, xdg_surface, popup, pool, pool_obj, buffer, w, h,
            committed: false,
        });
        self.hot = None;
    }

    /// Acknowledge the configure and put the pixels up
    pub fn configure(&mut self, c: &mut Conn, serial: u32) {
        let Some(p) = self.open.as_mut() else { return };
        c.ack_configure(p.xdg_surface, serial);
        p.committed = false;
        self.paint(c);
    }

    /// Draw the menu as it stands
    pub fn paint(&mut self, c: &mut Conn) {
        let hot = self.hot;
        let Some(p) = self.open.as_mut() else { return };
        {
            let stride = p.w as usize;
            let (w, h) = (p.w, p.h);
            let px = p.pool.bytes();
            let mut cv = text::canvas_of(px, stride);
            text::fill_on(&mut cv, 0, 0, w, h, MENU_BG);
            let mut i = 0;
            while i < ITEMS.len() {
                let y = MENU_PAD + i as i32 * ITEM_H;
                if hot == Some(i) {
                    text::fill_on(&mut cv, MENU_PAD, y, w - 2 * MENU_PAD, ITEM_H, MENU_HI);
                }
                let colour = if hot == Some(i) { MENU_FG } else { MENU_DIM };
                let bg = if hot == Some(i) { MENU_HI } else { MENU_BG };
                let mut pen = MENU_PAD + 8;
                for ch in ITEMS[i].chars() {
                    pen += text::draw_char_on(&mut cv, pen, y + 3, ch, colour, Some(bg));
                }
                i += 1;
            }
        }
        c.attach(p.surface, p.buffer, 0, 0);
        c.damage(p.surface, 0, 0, p.w, p.h);
        c.commit(p.surface);
        c.flush();
        p.committed = true;
    }

    /// Pointer moved in the menu. Returns whether the highlight moved
    pub fn hover(&mut self, y: i32) -> bool {
        let item = if y < MENU_PAD { None } else {
            let i = ((y - MENU_PAD) / ITEM_H) as usize;
            if i < ITEMS.len() { Some(i) } else { None }
        };
        if item == self.hot {
            return false;
        }
        self.hot = item;
        true
    }

    /// Hovered item, which is also the item a click activates
    pub fn hot_item(&self) -> Option<usize> { self.hot }

    /// Whether the menu is waiting for its first configure, separating its
    /// serial from the window's
    pub fn awaiting_configure(&self) -> bool {
        self.open.as_ref().is_some_and(|p| !p.committed)
    }

    /// Half-open, clamped rows the selection touches
    pub fn touched_rows(&self, g: &Grid) -> (usize, usize) {
        match self.span {
            Some(s) => {
                let (a, b) = s.ordered();
                (a.1.min(g.rows), (b.1 + 1).min(g.rows))
            }
            None => (0, 0),
        }
    }

    /// Closes the menu, innermost first: popup, xdg_surface, surface
    pub fn close(&mut self, c: &mut Conn) {
        let Some(mut p) = self.open.take() else { return };
        c.destroy_popup(p.popup);
        c.destroy_xdg_surface(p.xdg_surface);
        c.destroy_surface(p.surface);
        c.destroy_buffer(p.buffer);
        c.destroy_pool(p.pool_obj);
        c.flush();
        p.pool.free();
        self.hot = None;
    }

    // Item actions

    /// Copies the selection to the clipboard through a kept staging pool
    pub fn copy(&mut self, c: &mut Conn, g: &Grid, clip: u32) -> bool {
        if clip == 0 || self.serial == 0 {
            return false;
        }
        let need = self.write_selection(g);
        if need == 0 {
            return false;
        }
        let Some(handle) = self.copy_pool.as_ref().map(|p| p.handle) else { return false };
        c.clipboard_set(clip, self.serial, protocol::clipboard::CLIPBOARD,
                        "text/plain", handle, 0, need as u32);
        c.flush();
        true
    }

    /// Ask for the clipboard. The answer arrives as an event
    pub fn paste(&mut self, c: &mut Conn, clip: u32) -> bool {
        if clip == 0 || self.serial == 0 {
            return false;
        }
        c.clipboard_get(clip, self.serial, protocol::clipboard::CLIPBOARD, "text/plain");
        c.flush();
        true
    }
}

/// Clipboard response: maps the pool, passes the text to the console as input,
/// frees it
pub fn deliver(c: &mut Conn, console: u32, handle: u32, size: u32) {
    if handle == 0 || size == 0 {
        return;
    }
    let Some(mut pool) = Pool::adopt(handle, size as usize) else { return };
    {
        let bytes = pool.as_slice();
        if let Ok(s) = core::str::from_utf8(bytes) {
            c.console_paste(console, s);
            c.flush();
        }
    }
    // Freed after reading; a client is limited to eight pools
    pool.free();
}

/// Whether a pointer button is the one that opens the menu
pub fn opens_menu(button: u32) -> bool {
    // evdev numbering; 0x111 is the right button
    button == 0x111
}

/// And the one that selects
pub const BTN_LEFT: u32 = 0x110;
