// Glyph rendering as Qt uses FreeType (`qfontengine_ft.cpp`, `qfontengine.cpp`,
// `qsgdefaultglyphnode_p.cpp`)
//
// - Hinting: default-preferring text is hinted lightly (`platform.rs`),
//   no-hinting text is not, and neither is a size over 64 px, drawn from
//   outlines (`QT_MAX_CACHED_GLYPH_SIZE`).
// - Size: `qRound(pixelSize * 64)`.
// - Position: the pen floors to a pixel with the remainder rendered in quarters
//   (`subPixelPositionForX`); the baseline rounds.
// - Right bearing (`glyph_metrics_t::rightBearing`): hinted advance minus the
//   whole-pixel box, added to a line's width if the last glyph overhangs

use super::Font;

/// Quarters of a pixel the pen can stand at
pub const SUBPIXELS: i64 = 4;

/// Hint style (`QFontEngine::HintStyle`)
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Hint {
    None,
    #[default]
    Light,
}

impl Hint {
    /// `font.hintingPreference`: PreferDefaultHinting (0) and
    /// PreferVerticalHinting (2) are light, PreferNoHinting (1) none.
    /// PreferFullHinting (3) is also light, since FreeType's full hinting is
    /// not built
    pub fn from_preference(p: i64) -> Hint {
        if p == 1 { Hint::None } else { Hint::Light }
    }
}

/// Glyph coverage and position: `left` from the pen's whole pixel, `top` above
/// the baseline
pub struct Mask {
    /// What its copy in video memory is kept under (`gpu::serial`)
    pub serial: u64,
    pub w: usize,
    pub h: usize,
    pub left: i32,
    pub top: i32,
    pub data: Vec<u8>,
}

/// What Qt measures of a glyph, 26.6
#[derive(Clone, Copy, Debug)]
pub struct Info {
    /// The advance it lays out with
    pub linear: i64,
    /// The hinted advance and box, emboldened if the font is
    pub advance: i64,
    pub bearing_x: i64,
    pub width: i64,
}

impl Info {
    /// `xoff - x - width` in whole pixels: box floored and ceiled, advance
    /// rounded (`loadGlyph`, `boundingBox`)
    pub fn right_bearing(&self) -> i64 {
        let left = self.bearing_x & -64;
        let right = (self.bearing_x + self.width + 63) & -64;
        let xoff = (self.advance + 32) >> 6;
        xoff - (left >> 6) - ((right - left) >> 6)
    }
}

/// The size Qt sets FreeType to, 26.6
pub fn size26(px: f32) -> i64 {
    (px as f64 * 64.0 + 0.5).floor() as i64
}

/// Whether FreeType hints the glyphs of `font`
pub fn hinted(font: &Font) -> bool {
    font.hint == Hint::Light && size26(font.px) <= 64 * 64
}

/// How a glyph of `font` is loaded, `sub` quarters of a pixel right
pub fn load(font: &Font, render: bool, sub: i64) -> freetype::Load {
    freetype::Load { hint: hinted(font), embolden: font.bold, render, subx: sub * 64 / SUBPIXELS }
}

/// Quarter-pixel offset for pen `v` in 64ths. Qt adds 1/64 before flooring, so
/// a pen 1/64 short of a pixel moves to the next
pub fn sub_for(v: i64) -> i64 {
    if v == 0 {
        0
    } else {
        (((v & 63) + 1) * SUBPIXELS) >> 6
    }
}

/// The coverage FreeType rendered, as a mask
pub fn mask(g: freetype::Glyph) -> Option<Mask> {
    (g.w > 0 && g.h > 0).then(|| Mask { serial: crate::scene::gpu::serial(), w: g.w, h: g.h, left: g.left, top: g.top, data: g.coverage })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qts_quarters() {
        assert_eq!(sub_for(0), 0);
        assert_eq!(sub_for(64), 0);
        assert_eq!(sub_for(15), 1);
        assert_eq!(sub_for(14), 0);
        assert_eq!(sub_for(32 + 64), 2);
        // A 64th short of a pixel is the next pixel's own start
        assert_eq!(sub_for(63), 4);
    }

    #[test]
    fn the_right_bearing_is_whole_pixels() {
        // Medium 'A' at 15 px: 10 px advance and 11 px box, so the line is a
        // pixel wider
        let a = Info { linear: 654, advance: 640, bearing_x: 0, width: 704 };
        assert_eq!(a.right_bearing(), -1);
        // Emboldened: 11 and 12
        let b = Info { linear: 654, advance: 679, bearing_x: 0, width: 743 };
        assert_eq!(b.right_bearing(), -1);
        let zero = Info { linear: 624, advance: 640, bearing_x: 0, width: 640 };
        assert_eq!(zero.right_bearing(), 0);
    }
}
