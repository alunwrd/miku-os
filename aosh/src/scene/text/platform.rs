// Application font, from the platform
//
// A Text's font is `QGuiApplication::font()` with file settings on top
// (`QFont::resolve`). With `QT_QPA_PLATFORMTHEME=kde`, the font comes from
// `kdeglobals` `[General] font`, a `QFont::toString()`:
//
//     font=Google Sans Flex,11,-1,5,500,0,0,0,0,0,0,0,0,0,0,1,Medium
//
// The style name takes priority over weight when picking a face (`bestStyle`),
// so a Text setting only a weight remains Medium.
//
// `kdeglobals` is read as in KConfig (`io/kdeglobals.rs`). Without it: the
// default face at 13 px.
//
// Hint style is `hintslight`, so default-hinted text is hinted lightly.

#[derive(Clone, Debug, PartialEq)]
pub struct AppFont {
    /// Empty for the default face
    pub family: String,
    /// Points when given in points; otherwise `pixel_size`
    pub point_size: f32,
    pub pixel_size: f32,
    /// 100 to 900
    pub weight: f32,
    pub italic: bool,
    pub style: String,
}

impl Default for AppFont {
    fn default() -> AppFont {
        AppFont { family: String::new(), point_size: -1.0, pixel_size: 13.0, weight: 400.0, italic: false, style: String::new() }
    }
}

impl AppFont {
    /// The application's font, from `kdeglobals`, else aosh's own
    pub fn read() -> AppFont {
        let k = crate::io::kdeglobals::KdeGlobals::read(&crate::io::xdg::Env::process());
        k.get("General", "font").and_then(AppFont::parse).unwrap_or_default()
    }

    /// Parses `QFont::toString()` as `QFont::fromString` does: family, point
    /// size, and in longer forms pixel size, style hint, weight (OpenType from
    /// 16 fields, Qt 5's 0 to 99 below), style, and style name last in the 11-
    /// and 17-field forms. None if Qt would refuse it
    pub fn parse(s: &str) -> Option<AppFont> {
        let l: Vec<&str> = s.trim().split(',').collect();
        let count = l.len();
        if l[0].is_empty() || (count > 2 && count < 10) {
            return None;
        }
        let mut f = AppFont { family: l[0].to_string(), point_size: 12.0, pixel_size: -1.0, ..AppFont::default() };
        let num = |i: usize| l.get(i).and_then(|v| v.trim().parse::<f64>().ok()).unwrap_or(0.0);
        if count > 1 && num(1) > 0.0 {
            f.point_size = num(1) as f32;
        }
        if count >= 10 {
            if num(2) > 0.0 {
                f.pixel_size = num(2) as f32;
                f.point_size = -1.0;
            }
            f.weight = if count >= 16 { num(4) as f32 } else { legacy_weight(num(4) as i32) };
            f.italic = num(5) as i32 == 1;
            if count == 11 || count == 17 {
                f.style = l[count - 1].to_string();
            }
        }
        Some(f)
    }

    /// Size in pixels from points at 96 dpi, to a hundredth
    /// (`QFontPrivate::engineForScript`)
    pub fn px(&self) -> f32 {
        if self.point_size > 0.0 {
            points_to_px(self.point_size)
        } else {
            self.pixel_size
        }
    }
}

/// Points to pixels at 96 dpi, to a hundredth
pub fn points_to_px(pt: f32) -> f32 {
    ((pt as f64 * 96.0 / 72.0 * 100.0 + 0.5).floor() / 100.0) as f32
}

/// Qt 5 weight (0 to 99) as Qt 6's OpenType weight
/// (`qt_legacyToOpenTypeWeight`)
fn legacy_weight(w: i32) -> f32 {
    const MAP: [(i32, f32); 9] =
        [(0, 100.0), (12, 200.0), (25, 300.0), (50, 400.0), (57, 500.0), (63, 600.0), (75, 700.0), (81, 800.0), (87, 900.0)];
    let w = w.clamp(0, 99);
    let mut best = (i32::MAX, 400.0);
    for (legacy, open) in MAP {
        let d = (w - legacy).abs();
        if d < best.0 {
            best = (d, open);
        } else {
            break;
        }
    }
    best.1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_arch_boxs_font_reads_as_qt_reads_it() {
        let f = AppFont::parse("Google Sans Flex,11,-1,5,500,0,0,0,0,0,0,0,0,0,0,1,Medium").unwrap();
        assert_eq!((f.family.as_str(), f.point_size, f.weight, f.italic, f.style.as_str()), ("Google Sans Flex", 11.0, 500.0, false, "Medium"));
        // 11 pt at 96 dpi, to a hundredth
        assert_eq!(f.px(), 14.67);
        // Qt 5 form: legacy weight, no style name
        let f = AppFont::parse("Noto Sans,10,-1,5,75,0,0,0,0,0").unwrap();
        assert_eq!((f.weight, f.style.as_str()), (700.0, ""));
        // Pixel size overrides point size
        assert_eq!(AppFont::parse("Sans,10,16,5,50,0,0,0,0,0").unwrap().px(), 16.0);
        assert_eq!(AppFont::parse("Sans,10,16"), None);
    }
}
