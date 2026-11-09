// The numbers FreeType 2.14.3 gave a C probe for Google Sans Flex's Medium
// instance at 15 px: the face, size and calls every 15 px Text in end4 makes

use super::*;

fn sans() -> Option<Rc<[u8]>> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../personal/home/.local/share/fonts/GoogleSansFlex.ttf");
    std::fs::read(p).ok().map(Into::into)
}

/// opsz, wdth, wght, GRAD, ROND, slnt: the Medium instance
const MEDIUM: [f32; 6] = [18.0, 100.0, 500.0, 0.0, 0.0, 0.0];

#[test]
fn a_glyph_is_what_qt_reads() {
    let Some(data) = sans() else { return };
    let mut f = Face::new(data, 0).expect("Google Sans Flex");
    assert!(f.setup(&MEDIUM, 15 * 64));
    // 'A' is glyph 38
    let hinted = Load { hint: true, ..Load::default() };
    let g = f.glyph(38, hinted).unwrap();
    assert_eq!((g.linear, g.advance, g.bearing_x, g.width), (654, 640, 0, 704));
    let g = f.glyph(38, Load { embolden: true, ..hinted }).unwrap();
    assert_eq!((g.linear, g.advance, g.bearing_x, g.width), (654, 679, 0, 743));
    // 'N', glyph 51, stands a pixel off its origin
    let g = f.glyph(51, hinted).unwrap();
    assert_eq!((g.linear, g.advance, g.bearing_x, g.width), (691, 704, 64, 576));
}

#[test]
fn a_rendered_glyph_has_coverage() {
    let Some(data) = sans() else { return };
    let mut f = Face::new(data, 0).unwrap();
    assert!(f.setup(&MEDIUM, 15 * 64));
    let g = f.glyph(38, Load { hint: true, render: true, ..Load::default() }).unwrap();
    // The cap height on whole pixels: 11 rows, the baseline under the last
    assert_eq!((g.top, g.h), (11, 11));
    assert!(g.w >= 10 && g.coverage.len() == g.w * g.h);
    assert!(g.coverage.iter().any(|&c| c == 255));
}

#[test]
fn no_file_is_no_face() {
    assert!(Face::new(Rc::from(&b"not a font"[..]), 0).is_none());
}

#[test]
fn a_square_path_covers_its_pixels_and_half_of_the_edge_ones() {
    use crate::path::Path;
    let mut p = Path::new();
    p.move_to(2.0, 2.0);
    p.line_to(8.0, 2.0);
    p.line_to(8.0, 7.5);
    p.line_to(2.0, 7.5);
    p.close();
    let cov = p.fill(false, 10, 10);
    assert_eq!(cov[3 * 10 + 3], 255);
    assert_eq!(cov[1 * 10 + 3], 0);
    assert!((cov[7 * 10 + 3] as i32 - 128).abs() <= 1, "half a pixel: {}", cov[7 * 10 + 3]);
}

fn caveat() -> Option<Rc<[u8]>> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf");
    std::fs::read(p).ok().map(Into::into)
}

/// The outline Qt makes a path from: Caveat's 'a' (glyph 217) loaded with
/// the default hinting at the face's units per em, 1000, starts where
/// QRawFont::pathForGlyph at 1000 px starts it - 106.046875 across, 36 up -
/// and goes on with a conic. Unhinted it would start at 107
#[test]
fn an_outline_is_qts() {
    let Some(data) = caveat() else { return };
    let mut f = Face::new(data, 0).unwrap();
    assert!(f.setup(&[], 1000 * 64));
    let o = f.outline(217, true).unwrap();
    let first = o[0][0];
    assert_eq!((first.x, first.y, first.on), (6787, 36 * 64, true));
    assert!(!o[0][1].on && !o[0][1].cubic, "{:?}", o[0][1]);
    assert_eq!(f.outline(217, false).unwrap()[0][0].x, 107 * 64);
}
