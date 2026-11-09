// Rasterizes the system font at build time
//
// Included by build scripts with `#[path]`, so the kernel and the userspace
// apps get the same glyphs from the same file at the same size.
//
// Output is a table of coverage bytes: `fontdue` antialiases, and the drawing
// side blends 0..255 per pixel. See `text.rs`.

use std::path::Path;

/// Cell size. The advance is stored per glyph but stays at least this, so the
/// console can index by column
pub const CHAR_WIDTH: usize = 9;
pub const CHAR_HEIGHT: usize = 16;

/// Characters the console can print. Sorted, and the lookup relies on it, with
/// the same shape and rule as `UI_RANGES`
const CONSOLE_RANGES: [(u32, u32); 5] = [
    (0x20, 0x7e),   // printable ASCII
    (0xab, 0xab),   // «
    (0xbb, 0xbb),   // »
    (0x401, 0x401), // Ё
    (0x410, 0x44f), // А-я
];

/// And ё, outside the rest of the alphabet
const CONSOLE_EXTRA: [(u32, u32); 1] = [(0x451, 0x451)];

/// Every range the console draws, in order
fn console_ranges() -> Vec<(u32, u32)> {
    let mut v: Vec<(u32, u32)> = CONSOLE_RANGES.to_vec();
    v.extend_from_slice(&CONSOLE_EXTRA);
    v.sort_unstable();
    v
}

// The interface faces
//
// A second table beside the console's, which is one face at one size centered
// in a fixed cell. An interface needs a heading bigger than its body text, and
// a proportional face keeps each glyph's bearings. The console keeps indexing
// `GLYPHS` by column

/// One face: a weight and a pixel size. `ui.rs` names its text styles by these
/// indices, so adding one here means adding one there
pub const UI_FACE_COUNT: usize = 5;
pub const UI_FACES: [(&str, bool, f32); UI_FACE_COUNT] = [
    ("Title", true, 26.0),
    ("Head", true, 15.0),
    ("Body", false, 15.0),
    ("Small", false, 13.0),
    // The login screen's greeting, big enough to read while standing
    ("Display", true, 34.0),
];

/// Characters an interface can print: ASCII, then Cyrillic. Sorted, and the
/// lookup relies on it
const UI_RANGES: [(u32, u32); 8] = [
    (0x20, 0x7e),     // printable ASCII
    (0xab, 0xab),     // «
    (0xb1, 0xb1),     // ±, as a zone with no offset writes it
    (0xbb, 0xbb),     // »
    (0x401, 0x401),   // Ё
    (0x410, 0x44f),   // А-я
    (0x451, 0x451),   // ё
    (0x2022, 0x2022), // •, which separates the parts of a disk's description
];

/// Writes `font_data.rs` into `out_dir`, rasterized from `ttf`. A missing font
/// gives an empty table, so a checkout without the asset builds and shows a
/// blank screen
pub fn generate(ttf: &Path, out_dir: &Path) {
    let dest = out_dir.join("font_data.rs");
    let Ok(bytes) = std::fs::read(ttf) else {
        println!("cargo:warning={} not found, using an empty font", ttf.display());
        std::fs::write(dest, table(&[], &[])).unwrap();
        return;
    };
    let font = fontdue::Font::from_bytes(
        bytes,
        fontdue::FontSettings { scale: CHAR_HEIGHT as f32, ..Default::default() },
    )
    .expect("the font file is not a font");

    let ranges = console_ranges();
    let mut glyphs = Vec::new();
    for (a, b) in &ranges {
        for cp in *a..=*b {
            glyphs.push(rasterise(&font, char::from_u32(cp).unwrap()));
        }
    }
    std::fs::write(dest, table(&glyphs, &ranges)).unwrap();
    println!("cargo:rerun-if-changed={}", ttf.display());
}

/// Rasterizes the interface faces into `out_dir`. `regular` and `bold` are
/// proportional; a missing one falls back to `mono`, the console's font, at the
/// same sizes
pub fn generate_ui(regular: &Path, bold: &Path, mono: &Path, out_dir: &Path) {
    let load = |p: &Path| -> Option<Vec<u8>> { std::fs::read(p).ok() };

    let mono_bytes = load(mono);
    let reg_bytes = load(regular).or_else(|| {
        println!("cargo:warning={} not found, UI text falls back to the mono face",
                 regular.display());
        mono_bytes.clone()
    });
    let bold_bytes = load(bold).or(reg_bytes.clone());

    let chars: Vec<char> = UI_RANGES
        .iter()
        .flat_map(|(a, b)| (*a..=*b).filter_map(char::from_u32))
        .collect();

    let mut pixels: Vec<u8> = Vec::new();
    let mut glyphs: Vec<String> = Vec::new();
    let mut faces: Vec<String> = Vec::new();

    for (_, is_bold, px) in UI_FACES {
        let src = if is_bold { &bold_bytes } else { &reg_bytes };
        let Some(bytes) = src.clone() else {
            // No font at all. An empty face, so the drawing side has one shape
            // to handle
            faces.push(format!(
                "    UiFace {{ line_height: {}, ascent: {}, first: 0, count: 0 }},",
                px.round() as u16, (px * 0.8).round() as u16
            ));
            continue;
        };
        let font = fontdue::Font::from_bytes(
            bytes,
            fontdue::FontSettings { scale: px, ..Default::default() },
        )
        .expect("the UI font file is not a font");

        let metrics = font.horizontal_line_metrics(px);
        let ascent = metrics.map(|m| m.ascent.ceil() as u16).unwrap_or((px * 0.8) as u16);
        let line_height = metrics
            .map(|m| (m.ascent - m.descent + m.line_gap).ceil() as u16)
            .unwrap_or((px * 1.3) as u16);

        let first = glyphs.len();
        for &ch in &chars {
            let (m, bitmap) = font.rasterize(ch, px);
            let off = pixels.len();
            pixels.extend_from_slice(&bitmap);
            glyphs.push(format!(
                "    UiGlyph {{ off: {off}, w: {}, h: {}, left: {}, top: {}, advance: {} }},",
                m.width.min(255),
                m.height.min(255),
                (m.xmin).clamp(-128, 127),
                // Distance from the baseline up to the glyph's top row, which
                // the drawing side subtracts
                (m.ymin + m.height as i32).clamp(-128, 127),
                m.advance_width.round().clamp(0.0, 255.0) as u8,
            ));
        }
        faces.push(format!(
            "    UiFace {{ line_height: {line_height}, ascent: {ascent}, first: {first}, count: {} }},",
            chars.len()
        ));
    }

    std::fs::write(out_dir.join("ui_font_pixels.bin"), &pixels).unwrap();
    std::fs::write(out_dir.join("ui_font_data.rs"), ui_table(&glyphs, &faces, &chars)).unwrap();

    for p in [regular, bold, mono] {
        println!("cargo:rerun-if-changed={}", p.display());
    }
}

/// The generated interface-font source
fn ui_table(glyphs: &[String], faces: &[String], chars: &[char]) -> String {
    let mut s = String::new();
    s.push_str(
        "// Generated by fontgen.rs. One flat coverage blob, one glyph table\n\
         // indexed per face, and the character ranges that map a `char` to a\n\
         // slot inside a face\n\n\
         #[derive(Clone, Copy)]\n\
         pub struct UiGlyph {\n\
         \x20   pub off: usize,\n\
         \x20   pub w: u8,\n\
         \x20   pub h: u8,\n\
         \x20   /// Left side bearing: how far right of the pen the ink starts\n\
         \x20   pub left: i8,\n\
         \x20   /// How far above the baseline the glyph's top row sits\n\
         \x20   pub top: i8,\n\
         \x20   pub advance: u8,\n\
         }\n\n\
         #[derive(Clone, Copy)]\n\
         pub struct UiFace {\n\
         \x20   pub line_height: u16,\n\
         \x20   pub ascent: u16,\n\
         \x20   first: usize,\n\
         \x20   count: usize,\n\
         }\n\n",
    );
    s.push_str(&format!(
        "pub static UI_PIXELS: &[u8] = include_bytes!(concat!(env!(\"OUT_DIR\"), \
         \"/ui_font_pixels.bin\"));\n\n\
         pub const UI_FACE_COUNT: usize = {};\n\n",
        faces.len()
    ));

    s.push_str(&format!("pub static UI_GLYPHS: [UiGlyph; {}] = [\n", glyphs.len()));
    for g in glyphs {
        s.push_str(g);
        s.push('\n');
    }
    s.push_str("];\n\n");

    s.push_str(&format!("pub static UI_FACES: [UiFace; {}] = [\n", faces.len()));
    for f in faces {
        s.push_str(f);
        s.push('\n');
    }
    s.push_str("];\n\n");

    // (first char, last char, slot of the first char). Sorted, so a lookup
    // walks four entries instead of a 1,100-entry hole between ASCII and
    // Cyrillic
    let mut ranges = String::new();
    let mut slot = 0usize;
    for (a, b) in UI_RANGES {
        ranges.push_str(&format!("    ({a}, {b}, {slot}),\n"));
        slot += (b - a + 1) as usize;
    }
    debug_assert_eq!(slot, chars.len());
    s.push_str(&format!(
        "static UI_CHAR_RANGES: [(u32, u32, usize); {}] = [\n{ranges}];\n\n",
        UI_RANGES.len()
    ));

    s.push_str(
        "/// The glyph for a character in one face, or `None` for a character\n\
         /// this font was not built with - which the caller draws as nothing\n\
         pub fn ui_glyph(face: usize, c: char) -> Option<&'static UiGlyph> {\n\
         \x20   let f = UI_FACES.get(face)?;\n\
         \x20   if f.count == 0 {\n\
         \x20       return None;\n\
         \x20   }\n\
         \x20   let cp = c as u32;\n\
         \x20   let slot = UI_CHAR_RANGES\n\
         \x20       .iter()\n\
         \x20       .find(|(a, b, _)| cp >= *a && cp <= *b)\n\
         \x20       .map(|(a, _, s)| s + (cp - a) as usize)?;\n\
         \x20   UI_GLYPHS.get(f.first + slot)\n\
         }\n\n\
         /// Whether any interface face has glyphs at all\n\
         pub fn ui_available() -> bool {\n\
         \x20   UI_FACES.iter().any(|f| f.count > 0)\n\
         }\n",
    );
    s
}

/// One glyph, centred horizontally in its cell and sitting on the baseline
fn rasterise(font: &fontdue::Font, ch: char) -> ([u8; CHAR_WIDTH * CHAR_HEIGHT], u8) {
    let (m, bitmap) = font.rasterize(ch, CHAR_HEIGHT as f32);
    let mut cell = [0u8; CHAR_WIDTH * CHAR_HEIGHT];

    let x_off = if m.width < CHAR_WIDTH { (CHAR_WIDTH - m.width) / 2 } else { 0 };
    // 13 of the 16 rows are above the baseline; `ymin` is how far the glyph
    // hangs below it
    let y_off = if m.height < CHAR_HEIGHT {
        let baseline = 13i32;
        (baseline - m.ymin as i32 - m.height as i32).max(0) as usize
    } else {
        0
    };

    for row in 0..m.height.min(CHAR_HEIGHT) {
        for col in 0..m.width.min(CHAR_WIDTH) {
            let (y, x) = (row + y_off, col + x_off);
            if y < CHAR_HEIGHT && x < CHAR_WIDTH {
                cell[y * CHAR_WIDTH + x] = bitmap[row * m.width + col];
            }
        }
    }
    (cell, (m.advance_width.round() as u8).max(CHAR_WIDTH as u8))
}

/// Generated source, the same shape with or without glyphs
fn table(glyphs: &[([u8; CHAR_WIDTH * CHAR_HEIGHT], u8)], ranges: &[(u32, u32)]) -> String {
    let mut s = String::new();
    s.push_str(&format!("pub const CHAR_WIDTH: usize = {CHAR_WIDTH};\n"));
    s.push_str(&format!("pub const CHAR_HEIGHT: usize = {CHAR_HEIGHT};\n"));
    s.push_str(&format!("pub const GLYPH_COUNT: usize = {};\n\n", glyphs.len()));

    // Codepoints in the table and where each range starts, in the
    // `(start, end, slot)` shape of `ui_glyph`
    let mut slot = 0usize;
    let mut rows = String::new();
    for (a, b) in ranges {
        rows.push_str(&format!("    ({a:#x}, {b:#x}, {slot}),\n"));
        slot += (b - a + 1) as usize;
    }
    s.push_str(&format!(
        "static CHAR_RANGES: [(u32, u32, usize); {}] = [\n{rows}];\n\n",
        ranges.len()
    ));
    s.push_str(
        "pub static GLYPHS: [([u8; CHAR_WIDTH * CHAR_HEIGHT], u8); GLYPH_COUNT] = [\n",
    );
    for (cell, advance) in glyphs {
        s.push_str(&format!("    ({cell:?}, {advance}),\n"));
    }
    s.push_str("];\n\n");
    s.push_str(
        "/// The glyph for a character, and how far to move on after it. `None` for\n\
         /// a character this font was not built with, which the caller draws as\n\
         /// nothing\n\
         #[inline]\n\
         pub fn get_glyph(c: char) -> Option<&'static ([u8; CHAR_WIDTH * CHAR_HEIGHT], u8)> {\n\
         \x20   if GLYPH_COUNT == 0 {\n\
         \x20       return None;\n\
         \x20   }\n\
         \x20   let cp = c as u32;\n\
         \x20   let slot = CHAR_RANGES\n\
         \x20       .iter()\n\
         \x20       .find(|(a, b, _)| cp >= *a && cp <= *b)\n\
         \x20       .map(|(a, _, s)| s + (cp - a) as usize)?;\n\
         \x20   GLYPHS.get(slot)\n\
         }\n",
    );
    s
}
