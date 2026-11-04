// Text compared to Qt's numbers
//
// `aosh/tests/qtref/make.sh text` renders `fixtures/text.qml` with `qml6` and
// stores results in `target/qtref/text.json`. Skipped without the file

use std::path::Path;

use super::layout::{lay, Opts};
use super::*;
use crate::scene::paint::{Image, Rect};

fn fonts() -> Fonts {
    let f = Fonts::load(&[Path::new("../assets")]);
    assert!(!f.is_empty(), "the repository's fonts are in assets/");
    f
}

fn req(family: &str, px: f32, weight: f32) -> Request {
    Request { family: family.into(), px, weight, ..Request::default() }
}

/// Application font `qml6` ran with for the references
const ARCH_FONT: &str = "Google Sans Flex,11,-1,5,500,0,0,0,0,0,0,0,0,0,0,1,Medium";

#[test]
fn faces_are_found_and_picked() {
    let mut f = fonts();
    let regular = f.resolve(&req("", 15.0, 400.0)).unwrap();
    let bold = f.resolve(&req("", 15.0, 700.0)).unwrap();
    assert_ne!(regular.face, bold.face, "the interface family has a bold face");
    let mono = f.resolve(&req("JetBrains Mono", 15.0, 400.0)).unwrap();
    assert!(f.family(mono.face).to_lowercase().contains("mono"), "{}", f.family(mono.face));
    // A family there is none of is the interface face
    let other = f.resolve(&req("No Such Family", 15.0, 400.0)).unwrap();
    assert_eq!(other.face, regular.face);
}

#[test]
fn width_grows_with_size_and_elide_fits() {
    let mut f = fonts();
    let small = f.resolve(&req("", 12.0, 400.0)).unwrap();
    let big = f.resolve(&req("", 24.0, 400.0)).unwrap();
    let (a, b) = (f.width(&small, "workspace"), f.width(&big, "workspace"));
    assert!(b > a * 1.9 && b < a * 2.1, "{a} {b}");
    let font = f.resolve(&req("", 15.0, 400.0)).unwrap();
    let cut = layout::elide(&mut f, &font, "a rather long window title indeed", 80.0, 1, false);
    assert!(cut.ends_with('…'));
    assert!(f.width(&font, &cut) <= 80.0);
    // Wrapped at the width, by words
    let l = lay(&mut f, &font, "one two three four five six", &Opts { width: Some(60.0), wrap: 1, ..Opts::default() });
    assert!(l.lines.len() > 2 && l.lines.iter().all(|x| x.width <= 60.0 || !x.text.contains(' ')));
    assert_eq!(l.height, l.step * l.lines.len() as f32);
}

#[test]
fn text_puts_ink_on_the_page() {
    let mut f = fonts();
    let font = f.resolve(&req("", 15.0, 400.0)).unwrap();
    let mut img = Image::new(60, 20);
    let mut p = Painter::new(&mut img, Rect::new(0, 0, 60, 20));
    let base = f.metrics(&font).ascent;
    f.draw(&mut p, &font, &crate::scene::xf::Xf::IDENTITY, 0.0, base, "Miku", 0xFFFFFFFF, 1.0);
    assert!(img.px.iter().any(|&px| px >> 24 > 200));
}

/// Fixture fonts: Google Sans Flex installed for the user, two by FontLoader,
/// and the application font
pub(crate) fn reference_fonts() -> Option<Fonts> {
    let mut f = fonts();
    let sans = Path::new("../personal/home/.local/share/fonts/GoogleSansFlex.ttf");
    if !sans.exists() {
        return None;
    }
    f.add_file(sans, false);
    f.add_file(Path::new("../etc/aosh/negi/assets/fonts/MaterialSymbolsRounded.ttf"), true);
    f.add_file(Path::new("../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf"), true);
    f.set_app(AppFont::parse(ARCH_FONT).unwrap());
    Some(f)
}

#[test]
fn metrics_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/text.json") else { return };
    let Some(mut fonts) = reference_fonts() else { return };
    let v: serde_like::Json = serde_like::parse(&json);
    let rows = v.get("rows").array();
    assert!(!rows.is_empty());
    let mut worst = 0.0f32;
    let mut wrong = Vec::new();
    for r in rows {
        let axes: Vec<([u8; 4], f32)> = r.get("axes").object().into_iter()
            .filter_map(|(k, v)| k.as_bytes().try_into().ok().map(|t: [u8; 4]| (t, v.num() as f32)))
            .collect();
        let family = r.get("family").str();
        // Family, size, weight, and axes from the fixture; style name from the
        // application. Material Symbols prefers no hinting
        let rq = Request {
            hint: if family.starts_with("Material") { Hint::None } else { Hint::Light },
            family, px: r.get("px").num() as f32, weight: r.get("weight").num() as f32,
            style: fonts.app().style.clone(), axes, ..Request::default()
        };
        let font = fonts.resolve(&rq).unwrap();
        let text = r.get("text").str();
        let m = fonts.metrics(&font);
        let l = lay(&mut fonts, &font, &text, &Opts::default());
        let q = |k: &str| r.get(k).num() as f32;
        let line = format!("{} {}px w{} {:?}: ascent {:.3}/{:.3} descent {:.3}/{:.3} height {}/{} width {:.2}/{:.2}",
            rq.family, rq.px, rq.weight, text, m.ascent, q("ascent"), m.descent, q("descent"),
            l.height.ceil(), q("implicitHeight"), l.natural, q("implicitWidth"));
        eprintln!("{line}");
        let dw = (l.natural - q("implicitWidth")).abs();
        worst = worst.max(dw);
        // Line and width exact, to Qt's 1/64 px
        if m.ascent != q("ascent") || m.descent != q("descent")
            || l.height.ceil() != q("implicitHeight") || dw > 1e-4
        {
            wrong.push(line);
        }
    }
    assert!(wrong.is_empty(), "not Qt's:\n{}", wrong.join("\n"));
    eprintln!("widest width difference from Qt: {worst:.3} px");
}

/// Single characters and short runs at the background's weights and axes:
/// `fixtures/advances.qml` widths to 1/64
#[test]
fn advances_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/advances.json") else { return };
    let Some(mut fonts) = reference_fonts() else { return };
    let mut wrong = Vec::new();
    for r in serde_like::parse(&json).get("rows").array() {
        let axes: Vec<([u8; 4], f32)> = r.get("axes").object().into_iter()
            .filter_map(|(k, v)| k.as_bytes().try_into().ok().map(|t: [u8; 4]| (t, v.num() as f32)))
            .collect();
        let rq = Request {
            family: "Google Sans Flex".into(), px: r.get("px").num() as f32, weight: r.get("weight").num() as f32,
            style: fonts.app().style.clone(), axes, ..Request::default()
        };
        let font = fonts.resolve(&rq).unwrap();
        let text = r.get("text").str();
        let (ours, qt) = (fonts.width(&font, &text), r.get("width").num() as f32);
        if (ours - qt).abs() > 1e-4 {
            wrong.push(format!("{}px w{} {:?} {:?}: {ours:.6} / {qt:.6}", rq.px, rq.weight, rq.axes, text));
        }
    }
    assert!(wrong.is_empty(), "not Qt's:\n{}", wrong.join("\n"));
}

/// The fixture's rows as `metrics_are_qts` asks for them
fn reference_request(fonts: &Fonts, r: &serde_like::Json) -> Request {
    let axes: Vec<([u8; 4], f32)> = r.get("axes").object().into_iter()
        .filter_map(|(k, v)| k.as_bytes().try_into().ok().map(|t: [u8; 4]| (t, v.num() as f32)))
        .collect();
    let family = r.get("family").str();
    Request {
        hint: if family.starts_with("Material") { Hint::None } else { Hint::Light },
        family, px: r.get("px").num() as f32, weight: r.get("weight").num() as f32,
        style: fonts.app().style.clone(), axes, ..Request::default()
    }
}

/// Qt's picture against aosh's, row by row, as black text on white, with the
/// difference over inked pixels. Both are written to
/// `target/qtref/text-aosh.png`
#[test]
fn pixels_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/text.json") else { return };
    let Ok(qt) = image::open("../target/qtref/text.png") else { return };
    let Some(mut fonts) = reference_fonts() else { return };
    let qt = qt.to_rgba8();
    let (w, h) = (qt.width() as i32, qt.height() as i32);
    let mut img = Image::new(w, h);
    img.px.iter_mut().for_each(|p| *p = 0xFFFF_FFFF);
    let rows = serde_like::parse(&json).get("rows").array();
    let mut bands = Vec::new();
    {
        let mut p = Painter::new(&mut img, Rect::new(0, 0, w, h));
        for r in &rows {
            let font = fonts.resolve(&reference_request(&fonts, r)).unwrap();
            let y = 4.0 + r.get("y").num() as f32;
            let m = fonts.metrics(&font);
            let text = r.get("text").str();
            let style = crate::scene::paint::TextStyle::from_qml(r.get("style").num() as i64);
            // Qt writes opaque `#rrggbb`; the scene keeps `#aarrggbb`
            let style_colour = crate::scene::colour(&r.get("styleColor").str().replacen('#', "#ff", 1));
            fonts.draw_styled(&mut p, &font, &crate::scene::xf::Xf::translate(4.0, y as f64), 0.0, m.ascent, &text, 0xFF00_0000, style, style_colour, 1.0);
            bands.push((text, r.get("px").num(), y as i32, (y + r.get("implicitHeight").num() as f32) as i32));
        }
    }
    // All channels, since styles are colored
    let ours = |x: i32, y: i32| -> [i32; 3] {
        let c = img.px[(y * w + x) as usize];
        [(c >> 16 & 0xFF) as i32, (c >> 8 & 0xFF) as i32, (c & 0xFF) as i32]
    };
    let theirs = |x: i32, y: i32| -> [i32; 3] {
        let c = qt.get_pixel(x as u32, y as u32).0;
        [c[0] as i32, c[1] as i32, c[2] as i32]
    };
    let mut worst = 0.0f64;
    let mut report = Vec::new();
    for (text, px, top, bottom) in &bands {
        let (mut sum, mut ink, mut max) = (0i64, 0i64, 0i32);
        for y in *top..(*bottom).min(h) {
            for x in 0..w {
                let (a, b) = (ours(x, y), theirs(x, y));
                if a.iter().chain(&b).any(|&v| v < 255) {
                    let d = (0..3).map(|k| (a[k] - b[k]).abs()).max().unwrap_or(0);
                    sum += d as i64;
                    ink += 1;
                    max = max.max(d);
                }
            }
        }
        let mean = sum as f64 / ink.max(1) as f64;
        worst = worst.max(mean);
        report.push(format!("{px:>3}px {text:<28} mean {mean:6.2} max {max:3} over {ink} px"));
    }
    eprintln!("{}", report.join("\n"));
    let mut both = image::RgbaImage::from_pixel(w as u32, (2 * h + 4) as u32, image::Rgba([255, 0, 0, 255]));
    for y in 0..h {
        for x in 0..w {
            both.put_pixel(x as u32, y as u32, *qt.get_pixel(x as u32, y as u32));
            let v = ours(x, y);
            both.put_pixel(x as u32, (y + h + 4) as u32, image::Rgba([v[0] as u8, v[1] as u8, v[2] as u8, 255]));
        }
    }
    let _ = both.save("../target/qtref/text-aosh.png");
    // Within a stray 1/255 in styled blends
    assert!(worst < 0.25, "aosh's text is not Qt's, by {worst:.2}/255 on a row:\n{}", report.join("\n"));
}

/// Minimal JSON reader for Qt's numbers
mod serde_like {
    #[derive(Debug, Clone)]
    pub enum Json {
        Null,
        Bool(bool),
        Num(f64),
        Str(String),
        Arr(Vec<Json>),
        Obj(Vec<(String, Json)>),
    }

    impl Json {
        pub fn get(&self, k: &str) -> Json {
            match self {
                Json::Obj(v) => v.iter().find(|(n, _)| n == k).map(|(_, x)| x.clone()).unwrap_or(Json::Null),
                _ => Json::Null,
            }
        }
        pub fn array(&self) -> Vec<Json> {
            match self { Json::Arr(v) => v.clone(), _ => Vec::new() }
        }
        pub fn object(&self) -> Vec<(String, Json)> {
            match self { Json::Obj(v) => v.clone(), _ => Vec::new() }
        }
        pub fn num(&self) -> f64 {
            match self { Json::Num(n) => *n, Json::Bool(b) => *b as i32 as f64, _ => f64::NAN }
        }
        pub fn str(&self) -> String {
            match self { Json::Str(s) => s.clone(), _ => String::new() }
        }
    }

    pub fn parse(s: &str) -> Json {
        let b: Vec<char> = s.chars().collect();
        let mut i = 0;
        value(&b, &mut i)
    }

    fn ws(b: &[char], i: &mut usize) {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1;
        }
    }

    fn value(b: &[char], i: &mut usize) -> Json {
        ws(b, i);
        match b.get(*i) {
            Some('{') => {
                *i += 1;
                let mut out = Vec::new();
                loop {
                    ws(b, i);
                    if b.get(*i) == Some(&'}') {
                        *i += 1;
                        break;
                    }
                    let Json::Str(k) = value(b, i) else { break };
                    ws(b, i);
                    *i += 1; // :
                    let v = value(b, i);
                    out.push((k, v));
                    ws(b, i);
                    if b.get(*i) == Some(&',') {
                        *i += 1;
                    }
                }
                Json::Obj(out)
            }
            Some('[') => {
                *i += 1;
                let mut out = Vec::new();
                loop {
                    ws(b, i);
                    if b.get(*i) == Some(&']') {
                        *i += 1;
                        break;
                    }
                    out.push(value(b, i));
                    ws(b, i);
                    if b.get(*i) == Some(&',') {
                        *i += 1;
                    }
                }
                Json::Arr(out)
            }
            Some('"') => {
                *i += 1;
                let mut s = String::new();
                while let Some(&c) = b.get(*i) {
                    *i += 1;
                    match c {
                        '"' => break,
                        '\\' => {
                            let e = b.get(*i).copied().unwrap_or('\\');
                            *i += 1;
                            match e {
                                'n' => s.push('\n'),
                                't' => s.push('\t'),
                                'u' => {
                                    let h: String = b[*i..*i + 4].iter().collect();
                                    *i += 4;
                                    if let Some(c) = u32::from_str_radix(&h, 16).ok().and_then(char::from_u32) {
                                        s.push(c);
                                    }
                                }
                                other => s.push(other),
                            }
                        }
                        c => s.push(c),
                    }
                }
                Json::Str(s)
            }
            Some('t') => { *i += 4; Json::Bool(true) }
            Some('f') => { *i += 5; Json::Bool(false) }
            Some('n') => { *i += 4; Json::Null }
            _ => {
                let start = *i;
                while *i < b.len() && (b[*i].is_ascii_digit() || "+-.eE".contains(b[*i])) {
                    *i += 1;
                }
                Json::Num(b[start..*i].iter().collect::<String>().parse().unwrap_or(f64::NAN))
            }
        }
    }
}
