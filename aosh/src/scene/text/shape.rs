// Text shaping with rustybuzz, like Qt's HarfBuzz (`QTextEngine::shapeText`)
//
// Default features (ligatures, kerning, contextual forms), with direction and
// script guessed from the text.
//
// The face is not at the Text's axes; Qt's HarfBuzz font never gets them
// (`_hb_qt_font_create`). It is at the fontconfig-listed named instance
// (`hb_face_set_index`, `hb_font_create`). `instance` holds those coordinates,
// empty for the default.
//
// Starting advances are FreeType's at the axes, plus GPOS kerning at the
// instance. So each glyph carries `base`, and `advance - base` is added to
// FreeType's advance in `mod.rs`.
//
// Glyphs are in font units, so one shaping serves every size

use std::rc::Rc;

/// One glyph of a shaped run, in font units
#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    pub id: u16,
    /// Byte where its cluster starts
    pub cluster: u32,
    pub advance: i32,
    /// Glyph advance before shaping moved it
    pub base: i32,
    pub x_offset: i32,
    pub y_offset: i32,
}

/// Shapes `text` at `instance` with `features` over all of it. None if the face
/// does not parse
pub fn shape(data: &[u8], index: u32, instance: &[([u8; 4], f32)], features: &[([u8; 4], u32)], text: &str) -> Option<Rc<Vec<Glyph>>> {
    let mut face = rustybuzz::Face::from_slice(data, index)?;
    for (tag, v) in instance {
        face.set_variation(rustybuzz::ttf_parser::Tag::from_bytes(tag), *v);
    }
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    buf.guess_segment_properties();
    let features: Vec<rustybuzz::Feature> = features.iter()
        .map(|(tag, v)| rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(tag), *v, ..))
        .collect();
    let out = rustybuzz::shape(&face, &features, buf);
    let glyphs = out.glyph_infos().iter().zip(out.glyph_positions())
        .map(|(i, p)| Glyph {
            id: i.glyph_id as u16,
            cluster: i.cluster,
            advance: p.x_advance,
            base: face.glyph_hor_advance(rustybuzz::ttf_parser::GlyphId(i.glyph_id as u16)).unwrap_or(0) as i32,
            x_offset: p.x_offset,
            y_offset: p.y_offset,
        })
        .collect();
    Some(Rc::new(glyphs))
}

/// Whether the face has a glyph for `c`
pub fn covers(face: &rustybuzz::ttf_parser::Face<'_>, c: char) -> bool {
    c.is_control() || face.glyph_index(c).is_some_and(|g| g.0 != 0)
}
