// The four screens
//
// Each is a function that draws itself and returns what the person did. No
// screen keeps state: what has been chosen lives in `Model`, and what is
// pointed at lives in `Ui`, so a screen can be drawn twice or not at all and
// mean the same.
//
// Welcome and Done are pictures with a button. Disk asks twice: choosing a row
// selects a disk, and the button after that confirms it, as `install go` takes
// the size back as an argument. A disk is never chosen by this program.
//
// Every phrase comes from `lang.rs` through `m.lang`, so the language is a
// choice on the first screen.

extern crate alloc;

use super::{lang, tz};
use super::{Model, Screen};
use crate::mayland::paint::{Canvas, Rect};
use crate::mayland::text;
use crate::mayland::ui::{self, ButtonStyle, Icon, Right, Row, Select, Ui};

/// What a frame of a screen decided
pub enum Action {
    /// Start the install on this drive, whose size in MB was read when it was
    /// listed - the same pair `install go` takes, and refuses if they disagree
    Install { drive: usize, mb: u64 },
    Quit,
    /// The header's arrow. Answered by the screen that drew the header, so it
    /// never reaches the loop
    Back,
}

/// The mascot, decoded from the artwork at build time
mod art {
    include!(concat!(env!("OUT_DIR"), "/mascot_image.rs"));
}

// Control ids. One list, so two screens cannot pick the same number
const ID_NEXT: u32 = 10;
const ID_BACK: u32 = 11;
const ID_CLOSE: u32 = 12;
const ID_REFRESH: u32 = 13;
const ID_CANCEL: u32 = 14;
const ID_FAIL_CLOSE: u32 = 17;
const ID_STARTED: u32 = 18;
const ID_DRIVE_0: u32 = 100;
// A field with a list owns `SELECT_SPAN` ids after its own, one per item, so
// these two are that far apart and clear of everything above
const ID_LANG: u32 = 1024;
const ID_TZ: u32 = ID_LANG + ui::SELECT_SPAN;

pub fn draw(c: &mut Canvas, ui: &mut Ui, m: &mut Model) -> Option<Action> {
    c.clear(ui::PAGE);
    // A hairline of the accent around the whole window, which tells a
    // full-screen window from the desktop
    c.stroke_round_rect(Rect::new(0, 0, c.w, c.h), 14, 1, ui::BORDER);

    match m.screen {
        Screen::Welcome => welcome(c, ui, m),
        Screen::Disk => disk(c, ui, m),
        Screen::Progress => progress(c, ui, m),
        Screen::Done => done(c, ui, m),
    }
}

/// Title and body text, laid out from the left margin
fn heading(c: &mut Canvas, x: i32, y: i32, title: &str, lines: &[&str]) -> i32 {
    let mut y = y;
    text::draw_face(c, ui::FACE_TITLE, x, y, title, ui::TEXT);
    y += text::face_line_height(ui::FACE_TITLE) + 6;
    for line in lines {
        text::draw_face(c, ui::FACE_BODY, x, y, line, ui::TEXT_MUTED);
        y += text::face_line_height(ui::FACE_BODY);
    }
    y
}

fn mascot(c: &mut Canvas, cx: i32, y: i32) {
    if art::MASCOT_W == 0 {
        return;
    }
    c.blit(art::MASCOT_PIXELS, art::MASCOT_W, art::MASCOT_H, cx - art::MASCOT_W as i32 / 2, y);
}

/// A section's name and the line under it
fn section(c: &mut Canvas, x: i32, y: i32, title: &str, sub: &str) -> i32 {
    text::draw_face(c, ui::FACE_HEAD, x, y, title, ui::TEXT);
    let mut y = y + text::face_line_height(ui::FACE_HEAD);
    if !sub.is_empty() {
        text::draw_face(c, ui::FACE_SMALL, x, y + 2, sub, ui::TEXT_MUTED);
        y += text::face_line_height(ui::FACE_SMALL) + 2;
    }
    y
}

/// The window's own title bar: a way back on the left, the name of the screen
/// in the middle, and a way out on the right. Returns what was pressed
fn header(c: &mut Canvas, ui_: &mut Ui, title: &str) -> Option<Action> {
    let h = 64;
    text::draw_face_centred(
        c, ui::FACE_HEAD, 0, (h - text::face_line_height(ui::FACE_HEAD)) / 2, c.w, title, ui::TEXT);
    ui::divider(c, 1, h, c.w - 2);

    let back = ui::icon_button(ui_, c, ID_BACK, Rect::new(16, (h - 40) / 2, 40, 40), Icon::Back, false);
    let close = ui::icon_button(
        ui_, c, ID_CLOSE, Rect::new(c.w - 56, (h - 40) / 2, 40, 40), Icon::Close, true);
    if close {
        return Some(Action::Quit);
    }
    if back {
        return Some(Action::Back);
    }
    None
}

fn welcome(c: &mut Canvas, ui_: &mut Ui, m: &mut Model) -> Option<Action> {
    let s = m.lang.s();
    let margin = 56;
    let top = heading(c, margin, 48, s.welcome_title, &[s.welcome_1, s.welcome_2]);

    // The two questions the first screen asks, as a label beside a field each.
    // Laid out from the bottom, so the button stays put when the window is
    // short
    let btn_h = 48;
    let btn_y = c.h - 40 - btn_h;
    let field_h = 46;
    let gap = 14;
    let rows_top = btn_y - 36 - (field_h * 2 + gap);

    let names = tz::names();
    let notes = tz::notes();
    // From `Lang::ALL`, so a third language is still one const in `lang.rs`
    let langs = lang::Lang::ALL.map(|l| l.own_name());
    let lang_sel = Select {
        items: &langs,
        notes: &[],
        current: lang::Lang::ALL.iter().position(|l| *l == m.lang).unwrap_or(0),
    };
    let tz_sel = Select { items: &names, notes: &notes, current: m.tz };

    // One column for the labels and one for the fields, both measured from the
    // longest label, so the fields line up in any language
    let label_w = text::face_width(ui::FACE_HEAD, s.language)
        .max(text::face_width(ui::FACE_HEAD, s.time))
        + 28;
    let group_w = (c.w - margin * 2).min(560);
    let group_x = (c.w - group_w) / 2;
    let field_x = group_x + label_w;
    let field_w = group_w - label_w;

    let lang_r = Rect::new(field_x, rows_top, field_w, field_h);
    let tz_r = Rect::new(field_x, rows_top + field_h + gap, field_w, field_h);
    for (r, label) in [(lang_r, s.language), (tz_r, s.time)] {
        let ly = r.y + (r.h - text::face_line_height(ui::FACE_HEAD)) / 2;
        text::draw_face(c, ui::FACE_HEAD, group_x, ly, label, ui::TEXT);
    }
    ui::select(ui_, c, ID_LANG, lang_r, &lang_sel);
    ui::select(ui_, c, ID_TZ, tz_r, &tz_sel);

    // The mascot fills what is left between the words and the fields, so a
    // short window loses picture rather than controls
    let space = rows_top - top;
    if space > 80 {
        mascot(c, c.w / 2, top + (space - art::MASCOT_H as i32) / 2);
    }

    let bw = (text::face_width(ui::FACE_HEAD, s.next) + 110).max(180);
    let btn = Rect::new(c.w / 2 - bw / 2, btn_y, bw, btn_h);
    let go = ui::button(ui_, c, ID_NEXT, btn, s.next, ButtonStyle::Primary, true);

    // Both lists are drawn after everything else, which puts them in front
    if let Some(i) = ui::select_list(ui_, c, ID_LANG, lang_r, &lang_sel) {
        m.lang = lang::Lang::ALL[i.min(lang::Lang::ALL.len() - 1)];
        // The zone follows the language until somebody says otherwise
        if !m.tz_chosen {
            m.tz = tz::default_for(m.lang);
        }
    }
    if let Some(i) = ui::select_list(ui_, c, ID_TZ, tz_r, &tz_sel) {
        m.tz = i;
        m.tz_chosen = true;
    }

    if go {
        crate::kcore::clock::set_tz_offset(tz::ZONES[m.tz].minutes);
        // Read here rather than at startup: a disk may have been plugged in
        // since, and this is the last moment before it matters
        m.drives = crate::shell::commands::install_cmds::drives();
        m.screen = Screen::Disk;
    }
    None
}

fn disk(c: &mut Canvas, ui_: &mut Ui, m: &mut Model) -> Option<Action> {
    let s = m.lang.s();
    let margin = 24;
    let w = c.w - margin * 2;

    if let Some(a) = header(c, ui_, s.disk_title) {
        match a {
            Action::Back => {
                m.screen = Screen::Welcome;
                m.confirming = false;
                return None;
            }
            other => return Some(other),
        }
    }

    // The footer is measured from the bottom and the content fills what is
    // left, so a short window loses list rows rather than its buttons
    let foot_h = 76;
    let foot_y = c.h - foot_h;
    ui::divider(c, 1, foot_y, c.w - 2);

    let mut y = section(c, margin, 84, s.available, s.available_sub) + 12;

    // The list runs down to the footer, less the line that names what is about
    // to be erased once there is one
    let warn_h = if m.selected.is_some() { 30 } else { 0 };
    let list = Rect::new(margin, y, w, (foot_y - 14 - warn_h - y).max(0));
    if list.h > 20 {
        ui::card(c, list, ui::CARD);
        y = list.y + 6;
        if m.drives.is_empty() {
            text::draw_face(c, ui::FACE_BODY, list.x + 20, list.y + 22, s.no_disks, ui::TEXT_MUTED);
        }
        for i in 0..m.drives.len() {
            let rh = 64;
            if y + rh > list.bottom() - 6 {
                break;
            }
            if i > 0 {
                ui::divider(c, list.x + 16, y, list.w - 32);
            }
            let r = Rect::new(list.x + 6, y + 1, list.w - 12, rh - 2);
            y += rh;

            let d = &m.drives[i];
            let name = if d.name().is_empty() {
                alloc::format!("Drive {}", d.drive)
            } else {
                alloc::string::String::from(d.name())
            };
            let size = lang::size(m.lang, d.mb());
            let what = if d.is_source {
                alloc::format!("{} {} {}", d.bus(), size, s.the_medium)
            } else {
                lang::disk_subtitle(m.lang, d.bus(), &size, d.partitions, d.has_gpt)
            };
            let free = lang::size(m.lang, d.free_mb());
            let row = Row {
                title: &name,
                subtitle: &what,
                right: Right::Pair { top: s.free, bottom: &free },
                selected: m.selected == Some(i),
                icon: Icon::Disk,
                // The medium is listed and cannot be chosen: installing onto
                // the disk doing the installing is prevented outright
                enabled: !d.is_source,
            };
            if ui::list_row(ui_, c, ID_DRIVE_0 + i as u32, r, &row) {
                m.selected = Some(i);
                m.confirming = false;
            }
        }
    }

    // The line naming what is about to be destroyed, only after a disk has been
    // picked: "the selected disk" could not be checked
    if let Some(i) = m.selected {
        if let Some(d) = m.drives.get(i) {
            let size = lang::size(m.lang, d.mb());
            let msg = if m.confirming {
                lang::erase_warning(m.lang, d.drive, &size, s.install)
            } else {
                lang::selected_line(m.lang, d.drive, &size)
            };
            let colour = if m.confirming { 0xFF6B6B } else { ui::TEXT_MUTED };
            text::draw_face(c, ui::FACE_SMALL, margin, foot_y - 24, &msg, colour);
        }
    }

    // The footer: the list can be read again on the left, and the two answers
    // are on the right
    let by = foot_y + (foot_h - 44) / 2;
    let rw = text::face_width(ui::FACE_HEAD, s.refresh) + 76;
    if ui::button_icon(ui_, c, ID_REFRESH, Rect::new(margin, by, rw, 44), s.refresh,
                       ButtonStyle::Secondary, false, Icon::Refresh)
    {
        m.drives = crate::shell::commands::install_cmds::drives();
        m.selected = None;
        m.confirming = false;
    }

    let word = if m.confirming { s.install } else { s.next };
    let nw = (text::face_width(ui::FACE_HEAD, word) + 80).max(140);
    let cw = (text::face_width(ui::FACE_HEAD, s.cancel) + 48).max(120);
    let next_r = Rect::new(c.w - margin - nw, by, nw, 44);
    let cancel_r = Rect::new(next_r.x - 12 - cw, by, cw, 44);

    if ui::button(ui_, c, ID_CANCEL, cancel_r, s.cancel, ButtonStyle::Secondary, false) {
        return Some(Action::Quit);
    }
    if ui::button(ui_, c, ID_NEXT, next_r, word, ButtonStyle::Primary, !m.confirming)
        && m.selected.is_some()
    {
        if m.confirming {
            if let Some(d) = m.selected.and_then(|i| m.drives.get(i)) {
                return Some(Action::Install { drive: d.drive, mb: d.mb() });
            }
        } else {
            m.confirming = true;
        }
    }
    None
}

fn progress(c: &mut Canvas, ui_: &mut Ui, m: &mut Model) -> Option<Action> {
    use crate::shell::commands::install_cmds::progress as p;
    let st = p::get();
    let s = m.lang.s();

    let margin = 56;
    heading(c, margin, 48, s.installing, &[lang::step_name(m.lang, st.step)]);
    mascot(c, c.w / 2, 150);

    let bar = Rect::new(margin, c.h - 150, c.w - margin * 2, 14);
    ui::progress(c, bar, st.bytes_done, st.bytes_total);

    let counted = if st.bytes_total > 0 {
        lang::copied_line(m.lang, st.bytes_done / (1024 * 1024), st.bytes_total / (1024 * 1024))
    } else {
        alloc::string::String::from(s.preparing)
    };
    text::draw_face(c, ui::FACE_SMALL, margin, c.h - 126, &counted, ui::TEXT_MUTED);

    match st.step {
        p::Step::Done => m.screen = Screen::Done,
        p::Step::Failed => {
            text::draw_face(c, ui::FACE_BODY, margin, c.h - 100, st.error, 0xFF6B6B);
            let btn = Rect::new(c.w / 2 - 90, c.h - 76, 180, 46);
            if ui::button(ui_, c, ID_FAIL_CLOSE, btn, s.close, ButtonStyle::Secondary, false) {
                return Some(Action::Quit);
            }
        }
        _ => {}
    }
    None
}

fn done(c: &mut Canvas, ui_: &mut Ui, m: &mut Model) -> Option<Action> {
    let s = m.lang.s();
    let margin = 56;
    heading(c, margin, 44, s.done_title, &[s.done_1, s.done_2, "", s.done_3]);
    mascot(c, c.w / 2, 170);

    let bw = (text::face_width(ui::FACE_HEAD, s.get_started) + 90).max(200);
    let btn = Rect::new(c.w / 2 - bw / 2, c.h - 100, bw, 48);
    if ui::button(ui_, c, ID_STARTED, btn, s.get_started, ButtonStyle::Primary, true) {
        return Some(Action::Quit);
    }
    None
}
