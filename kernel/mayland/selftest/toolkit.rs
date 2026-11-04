// The widget kit and the curves
//
// Both live in `libmay` so a client and the compositor share them, and both are
// silent when wrong. A curve that disagrees between the two makes a launcher
// decelerate differently from the window behind it. A text field stepping by a
// byte and not a character leaves half a letter in the buffer, and every later
// draw is a string that will not decode.
//
// Both are checked by arithmetic, with no screen, client or compositor.

use super::*;

use libmay::ease;
use libmay::paint::{Canvas, Rect};
use libmay::ui::{self, Ui};

/// The curves are the same on both sides, and they do what their names say
pub(super) fn test_ease_curves(t: &mut Runner) {
    let expo = ease::out_expo();
    let back = ease::out_back();

    // The ends are exact. An animation finishing at 0.998 would leave every
    // window a pixel short
    let ends = expo.y_at(0.0) == 0.0 && expo.y_at(1.0) == 1.0
        && back.y_at(0.0) == 0.0 && back.y_at(1.0) == 1.0
        // And past the ends: a frame that arrives late asks for a fraction
        // greater than one
        && expo.y_at(1.5) == 1.0 && expo.y_at(-0.5) == 0.0;

    // Decelerating means most of the distance is covered in the first half. A
    // wrong control point would break this silently
    let early = expo.y_at(0.25);
    let decelerates = early > 0.5;

    // `out_back` overshoots: somewhere before the end it is past its target
    let mut peak = 0.0f32;
    let mut i = 0;
    while i <= 100 {
        let y = back.y_at(i as f32 / 100.0);
        if y > peak { peak = y; }
        i += 1;
    }
    let overshoots = peak > 1.01;

    // The compositor reaches the same code by a different name. This checks
    // they stay one implementation
    let same = {
        let mine = ease::out_expo();
        let theirs: crate::mayland::shell::anim::Curve = ease::out_expo();
        (0..=10).all(|k| {
            let x = k as f32 / 10.0;
            (mine.y_at(x) - theirs.y_at(x)).abs() < 1e-6
        })
    };

    t.check("toolkit: the easing curves are exact at their ends and shaped as named",
            ends && decelerates && overshoots && same,
            format!("expo(0.25) {:.3}, back peaks at {:.3}, ends {}, one implementation {}",
                    early, peak, ends, same));
}

/// Drives one frame of a screen that is only a field
///
/// Two frames on the first call, because focus is decided in `end`: the frame
/// that draws a control registers it, and it cannot have the keyboard until the
/// next frame
fn typed(ui: &mut Ui, px: &mut [u8], buf: &mut [u8], len: &mut usize, caret: &mut usize) {
    let mut c = Canvas::new(px, 200, 200, 40);
    ui.begin();
    let _ = ui.take_step();
    ui::field(ui, &mut c, 1, Rect::new(0, 0, 200, 40), buf, len, caret, "", &ui::MIKU);
    ui.end();
}

/// A field inserts, deletes by whole characters, and moves its caret
pub(super) fn test_ui_field(t: &mut Runner) {
    let mut px = alloc::vec![0u8; 200 * 40 * 4];
    let mut ui = Ui::new();
    let mut buf = [0u8; 64];
    let mut len = 0usize;
    let mut caret = 0usize;

    // One frame to register the control, so the next one has the keyboard
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);

    // Types a word with two-byte letters and a one-byte letter at the end.
    // Cyrillic, which the shipped app database uses and a byte-wise field gets
    // wrong.
    //
    // The last letter is a Latin `p`, written as an escape because the Cyrillic
    // `р` looks the same
    const WORD: &str = "Те\u{70}";
    for ch in WORD.chars() {
        ui.ch(ch);
    }
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let inserted = core::str::from_utf8(&buf[..len]) == Ok(WORD) && len == 5;

    // Backspace takes the whole of the last character and not one byte of it
    ui.key(ui::KEY_BACKSPACE, true);
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let one_char = core::str::from_utf8(&buf[..len]) == Ok("Те") && len == 4;

    // Now the multi-byte one. Five bytes went in, one byte came off with the
    // `p`, and this takes the `е`, which is two
    ui.key(ui::KEY_BACKSPACE, true);
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let whole_letter = core::str::from_utf8(&buf[..len]) == Ok("Т") && len == 2;

    // Home, then a character: it lands at the front rather than the end
    ui.key(ui::KEY_HOME, true);
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let home = caret == 0;
    ui.ch('x');
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let inserted_at_caret = core::str::from_utf8(&buf[..len]) == Ok("xТ");

    // End, then Delete: nothing to delete past the end, and nothing breaks
    ui.key(ui::KEY_END, true);
    ui.key(ui::KEY_DELETE, true);
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let past_end = core::str::from_utf8(&buf[..len]) == Ok("xТ") && caret == len;

    t.check("toolkit: a field steps by characters and not by bytes",
            inserted && one_char && whole_letter && home && inserted_at_caret && past_end,
            format!("typed {}, one {}, whole letter {}, home {}, at caret {}, past end {} - \
                     buffer {:?}",
                    inserted, one_char, whole_letter, home, inserted_at_caret, past_end,
                    core::str::from_utf8(&buf[..len])));
}

/// A field refuses what it cannot hold, rather than half of it
pub(super) fn test_ui_field_full(t: &mut Runner) {
    let mut px = alloc::vec![0u8; 200 * 40 * 4];
    let mut ui = Ui::new();
    // Five bytes: room for two Cyrillic letters and one byte spare, so the
    // third has to be refused whole
    let mut buf = [0u8; 5];
    let mut len = 0usize;
    let mut caret = 0usize;
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);

    for ch in "абв".chars() {
        ui.ch(ch);
    }
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);

    // Two letters in, the third refused, and what is there still decodes: a
    // field writing one byte of the third would hold an unreadable string
    let ok = len == 4 && core::str::from_utf8(&buf[..len]) == Ok("аб");

    // A control character is not text. A client decoding Ctrl+C hands one over,
    // and a field inserting it would hold a byte nothing can draw
    ui.ch('\u{3}');
    typed(&mut ui, &mut px, &mut buf, &mut len, &mut caret);
    let refused_control = len == 4;

    t.check("toolkit: a full field refuses a whole character, and control codes are not text",
            ok && refused_control,
            format!("{} bytes, {:?}, control refused {}",
                    len, core::str::from_utf8(&buf[..len]), refused_control));
}
