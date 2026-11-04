// Drawing
//
// The text the client half and the console half both go through. A glyph drawn
// by a client and one drawn by the console come from the same code and must
// land on the same pixels.

use super::*;

/// Drawing a letter into a buffer, which is what a client needs to show words
pub(super) fn test_text(t: &mut Runner) {
    use crate::mayland::text;

    const W: usize = 64;
    const H: usize = 20;
    let mut px = alloc::vec![0u8; W * H * 4];

    let pen = text::draw_text(&mut px, W, 2, 2, "Hi", 0x00ff_ffff, Some(0));
    let ink = px.chunks_exact(4).filter(|p| p != &[0, 0, 0, 0]).count();

    // Advanced by two cells, with ink in the buffer. A font that failed to
    // build produces no glyphs, and this reports it
    let drew = ink > 0 && pen >= 2 + 2 * text::CELL_W as i32;
    t.check("text: a string draws ink and advances by its width", drew,
            format!("{} pixels inked, pen at {} from {}, width {}",
                    ink, pen, 2, text::text_width("Hi")));

    // A string entirely outside puts nothing anywhere: asking for it neither
    // draws nor faults
    let mut edges = alloc::vec![0u8; W * H * 4];
    text::draw_text(&mut edges, W, -400, 2, "off to the left", 0x00ff_ffff, None);
    text::draw_text(&mut edges, W, 2, H as i32 + 4, "below the bottom", 0x00ff_ffff, None);
    text::draw_text(&mut edges, W, 2, -(text::CELL_H as i32) - 4, "above the top",
                    0x00ff_ffff, None);
    let stray = edges.chunks_exact(4).filter(|p| p != &[0, 0, 0, 0]).count();

    // One that starts outside and walks back in draws only the part inside: the
    // buffer is untouched to the left of where it arrives
    let mut half = alloc::vec![0u8; W * H * 4];
    text::draw_text(&mut half, W, -(text::CELL_W as i32) * 2, 2, "MM", 0x00ff_ffff, None);
    let clipped_away = half.chunks_exact(4).all(|p| p == [0, 0, 0, 0]);

    t.check("text: drawing off the edges clips instead of corrupting",
            stray == 0 && clipped_away,
            format!("{} pixels from strings entirely outside, two cells left of zero drew {}",
                    stray, if clipped_away { "nothing" } else { "something" }));
}
