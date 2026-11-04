// Font index
//
// Search path: `/etc/aosh/fonts`, `$XDG_DATA_HOME/fonts`
// (`~/.local/share/fonts`), and every FontLoader file, recursively.
//
// Metadata comes from each file's tables, read once. Outlines load on first
// draw (`Fonts::data`).
//
// `font.family` and a FontLoader's `name` match the `name` table as in Qt's
// font database:
// - typographic family (ID 16), else family (ID 1);
// - typographic subfamily (ID 17), else subfamily (ID 2);
// - from the Windows Unicode record (platform 3, UTF-16BE), else Macintosh
//   Roman (platform 1);
// - US English when several languages exist.
//
// Each TrueType collection face is its own entry.

/// A font's names
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Names {
    pub family: String,
    pub style: String,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes([*b.get(at)?, *b.get(at + 1)?]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(at)?, *b.get(at + 1)?, *b.get(at + 2)?, *b.get(at + 3)?]))
}

/// Offset and length of a table in face `index`
fn table(b: &[u8], index: u32, tag: &[u8; 4]) -> Option<(usize, usize)> {
    // A collection (`ttcf`) points at its fonts
    let base = if b.get(0..4)? == b"ttcf" { u32_at(b, 12 + 4 * index as usize)? as usize } else { 0 };
    let n = u16_at(b, base + 4)? as usize;
    (0..n).find_map(|i| {
        let rec = base + 12 + i * 16;
        if b.get(rec..rec + 4)? != tag {
            return None;
        }
        Some((u32_at(b, rec + 8)? as usize, u32_at(b, rec + 12)? as usize))
    })
}

/// The family and style names of the font in `bytes`, its first face
pub fn names(bytes: &[u8]) -> Option<Names> {
    names_of(bytes, 0)
}

fn names_of(bytes: &[u8], index: u32) -> Option<Names> {
    let [typo_family, family, typo_style, style] = name_strings(bytes, index, [16, 1, 17, 2]);
    let family = typo_family.or(family)?;
    Some(Names { family, style: typo_style.or(style).unwrap_or_else(|| "Regular".into()) })
}

/// The strings of four name IDs, each from its best record
fn name_strings(bytes: &[u8], index: u32, ids: [u16; 4]) -> [Option<String>; 4] {
    name_records(bytes, index, ids).unwrap_or_default()
}

fn name_records(bytes: &[u8], index: u32, ids: [u16; 4]) -> Option<[Option<String>; 4]> {
    let (at, len) = table(bytes, index, b"name")?;
    let t = bytes.get(at..at + len)?;
    let count = u16_at(t, 2)? as usize;
    let strings = u16_at(t, 4)? as usize;
    // Best record per ID; lower rank is better
    let mut best: [(u32, Option<String>); 4] = Default::default();
    for i in 0..count {
        let r = 6 + i * 12;
        let (platform, encoding, language, id) = (u16_at(t, r)?, u16_at(t, r + 2)?, u16_at(t, r + 4)?, u16_at(t, r + 6)?);
        let Some(slot) = ids.iter().position(|&x| x == id) else { continue };
        let (len, off) = (u16_at(t, r + 8)? as usize, u16_at(t, r + 10)? as usize);
        let Some(raw) = t.get(strings + off..strings + off + len) else { continue };
        let (rank, text) = match (platform, encoding) {
            (3, 1) | (3, 10) | (0, _) => {
                let units: Vec<u16> = raw.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                let english = platform == 0 || language == 0x0409;
                (if english { 1 } else { 2 }, String::from_utf16_lossy(&units))
            }
            // Macintosh Roman, English: ASCII in family names
            (1, 0) if language == 0 => (3, raw.iter().map(|&c| if c < 0x80 { c as char } else { '?' }).collect()),
            _ => continue,
        };
        let slot = &mut best[slot];
        if slot.1.is_none() || rank < slot.0 {
            *slot = (rank, Some(text));
        }
    }
    Some(best.map(|(_, s)| s))
}

/// Named instance of a variable face (`fvar`): name and axis values. Qt with a
/// weight and no `variableAxes` draws the nearest one, using all its axis
/// values
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    pub name: String,
    pub coords: Vec<f32>,
}

/// `fvar`'s named instances, coordinates in the order of the axes
fn instances(bytes: &[u8], index: u32) -> Vec<Instance> {
    let read = || -> Option<Vec<Instance>> {
        let (at, len) = table(bytes, index, b"fvar")?;
        let t = bytes.get(at..at + len)?;
        let (axes_at, axis_count, axis_size) = (u16_at(t, 4)? as usize, u16_at(t, 8)? as usize, u16_at(t, 10)? as usize);
        let (count, size) = (u16_at(t, 12)? as usize, u16_at(t, 14)? as usize);
        let first = axes_at + axis_count * axis_size;
        let mut out = Vec::new();
        for i in 0..count {
            let r = first + i * size;
            let name_id = u16_at(t, r)?;
            let coords = (0..axis_count)
                .map(|k| u32_at(t, r + 4 + 4 * k).map(|v| v as i32 as f32 / 65536.0))
                .collect::<Option<Vec<f32>>>()?;
            let [name, ..] = name_strings(bytes, index, [name_id, 0xffff, 0xffff, 0xffff]);
            out.push(Instance { name: name.unwrap_or_default(), coords });
        }
        Some(out)
    };
    read().unwrap_or_default()
}

/// Variation axis: tag and value range
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub tag: [u8; 4],
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// Face line metrics in font units, as Qt reads them, ignoring `MVAR`
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Lines {
    pub upm: u16,
    /// `hhea`: ascender, descender (negative), line gap
    pub hhea: [i16; 3],
    /// OS/2, when it is long enough for Qt to read (78 bytes)
    pub os2: Option<Os2>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Os2 {
    pub fs_selection: u16,
    /// `sTypoAscender`, `sTypoDescender` (negative), `sTypoLineGap`
    pub typo: [i16; 3],
    /// `usWinAscent`, `usWinDescent` (positive)
    pub win: [u16; 2],
}

/// The table a face's lines come from, read raw
fn lines(bytes: &[u8], index: u32) -> Lines {
    let mut l = Lines::default();
    if let Some((at, _)) = table(bytes, index, b"head") {
        l.upm = u16_at(bytes, at + 18).unwrap_or(0);
    }
    if let Some((at, _)) = table(bytes, index, b"hhea") {
        let i = |o| u16_at(bytes, at + o).map(|v| v as i16).unwrap_or(0);
        l.hhea = [i(4), i(6), i(8)];
    }
    if let Some((at, len)) = table(bytes, index, b"OS/2") {
        if len >= 78 {
            let u = |o| u16_at(bytes, at + o).unwrap_or(0);
            l.os2 = Some(Os2 {
                fs_selection: u(62),
                typo: [u(68) as i16, u(70) as i16, u(72) as i16],
                win: [u(74), u(76)],
            });
        }
    }
    l
}

/// FreeType's bold flag (`FT_STYLE_FLAG_BOLD`): OS/2 `fsSelection` bit 5, else
/// `head` `macStyle` bit 0. Qt synthesizes bold only without it
fn bold_flag(bytes: &[u8], index: u32) -> bool {
    match table(bytes, index, b"OS/2") {
        Some((at, len)) if len >= 64 => u16_at(bytes, at + 62).is_some_and(|f| f & 32 != 0),
        _ => table(bytes, index, b"head").and_then(|(at, _)| u16_at(bytes, at + 44)).is_some_and(|m| m & 1 != 0),
    }
}

/// One face, as the index knows it
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: std::path::PathBuf,
    /// Its face in a collection, 0 otherwise
    pub index: u32,
    pub family: String,
    pub style: String,
    /// OS/2's `usWeightClass`, 100 to 900
    pub weight: f32,
    pub italic: bool,
    /// FreeType's bold flag (`bold_flag`)
    pub bold: bool,
    /// `fvar` axes in face order; a variable font's weight is `wght`
    pub axes: Vec<Axis>,
    /// Its named instances, if it is variable
    pub instances: Vec<Instance>,
    pub monospace: bool,
    pub lines: Lines,
    /// From a FontLoader: its weight is that of the instance it is drawn at
    /// (`Fonts::resolve`)
    pub loaded: bool,
}

/// A family style in Qt's font database, as fontconfig lists a file: the face
/// with its `name` style and OS/2 weight, then each named instance whose
/// coordinates differ from the face's
#[derive(Debug, Clone, PartialEq)]
pub struct Style {
    pub name: String,
    pub weight: f32,
    pub italic: bool,
    /// Instance coordinates in face axis order; axis defaults for the face
    /// itself
    pub coords: Vec<f32>,
    /// Index in `fvar`'s named instances; None for the face
    pub instance: Option<usize>,
}

impl Entry {
    pub fn axis(&self, tag: &[u8; 4]) -> Option<Axis> {
        self.axes.iter().copied().find(|a| &a.tag == tag)
    }

    pub fn styles(&self) -> Vec<Style> {
        let defaults: Vec<f32> = self.axes.iter().map(|a| a.default).collect();
        let mut out = vec![Style { name: self.style.clone(), weight: self.weight, italic: self.italic, coords: defaults.clone(), instance: None }];
        let wght = self.axes.iter().position(|a| &a.tag == b"wght");
        for (n, i) in self.instances.iter().enumerate() {
            if i.coords == defaults || i.coords.len() != defaults.len() {
                continue;
            }
            let at = |tag: &[u8; 4]| self.axes.iter().position(|a| &a.tag == tag).map(|k| i.coords[k]);
            out.push(Style {
                name: i.name.clone(),
                weight: wght.map(|k| i.coords[k]).unwrap_or(self.weight),
                italic: i.name.contains("Italic") || i.name.contains("Oblique")
                    || at(b"ital").is_some_and(|v| v >= 0.5) || at(b"slnt").is_some_and(|v| v != 0.0),
                coords: i.coords.clone(),
                instance: Some(n),
            });
        }
        out
    }
}

/// Faces in a file; none if it is not a font
pub fn entries(path: &std::path::Path, bytes: &[u8]) -> Vec<Entry> {
    let n = rustybuzz::ttf_parser::fonts_in_collection(bytes).unwrap_or(1);
    let mut out = Vec::new();
    for index in 0..n {
        let Ok(face) = rustybuzz::ttf_parser::Face::parse(bytes, index) else { continue };
        let Some(names) = names_of(bytes, index) else { continue };
        let axes = face.variation_axes().into_iter()
            .map(|a| Axis { tag: a.tag.0.to_be_bytes(), min: a.min_value, default: a.def_value, max: a.max_value })
            .collect();
        out.push(Entry {
            path: path.to_path_buf(),
            index,
            weight: face.weight().to_number() as f32,
            italic: face.is_italic(),
            bold: bold_flag(bytes, index),
            monospace: face.is_monospaced(),
            family: names.family,
            style: names.style,
            axes,
            instances: instances(bytes, index),
            lines: lines(bytes, index),
            loaded: false,
        });
    }
    out
}

/// Every font file at or under `dir`, in name order
pub fn files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut here: Vec<std::path::PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).collect();
    here.sort();
    for p in here {
        if p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
            continue;
        }
        if p.is_dir() {
            files(&p, out);
        } else if p.extension().is_some_and(|e| {
            let e = e.to_string_lossy().to_ascii_lowercase();
            e == "ttf" || e == "otf" || e == "ttc" || e == "otc"
        }) {
            out.push(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A font of nothing but a `name` table, written the way a font is
    fn font(records: &[(u16, u16, u16, u16, &[u8])]) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut recs = Vec::new();
        for &(p, e, l, id, s) in records {
            for v in [p, e, l, id, s.len() as u16, strings.len() as u16] {
                recs.extend_from_slice(&v.to_be_bytes());
            }
            strings.extend_from_slice(s);
        }
        let mut name = Vec::new();
        for v in [0u16, records.len() as u16, (6 + recs.len()) as u16] {
            name.extend_from_slice(&v.to_be_bytes());
        }
        name.extend(recs);
        name.extend(strings);
        let mut f = Vec::new();
        f.extend_from_slice(&0x0001_0000u32.to_be_bytes());
        f.extend_from_slice(&1u16.to_be_bytes());
        f.extend_from_slice(&[0; 6]);
        f.extend_from_slice(b"name");
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&28u32.to_be_bytes());
        f.extend_from_slice(&(name.len() as u32).to_be_bytes());
        f.extend(name);
        f
    }

    fn utf16(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect()
    }

    #[test]
    fn the_typographic_family_wins() {
        let (a, b, c) = (utf16("Google Sans Flex Medium"), utf16("Google Sans Flex"), utf16("Medium"));
        let f = font(&[(3, 1, 0x409, 1, &a), (3, 1, 0x409, 16, &b), (3, 1, 0x409, 17, &c), (1, 0, 0, 1, b"Mac Name")]);
        assert_eq!(names(&f), Some(Names { family: "Google Sans Flex".into(), style: "Medium".into() }));
        // Mac record only, with Regular as the default style
        let f = font(&[(1, 0, 0, 1, b"Caveat")]);
        assert_eq!(names(&f), Some(Names { family: "Caveat".into(), style: "Regular".into() }));
        assert_eq!(names(b"not a font"), None);
    }

    /// Google Sans Flex, if the tree has its copy: a variable face with a
    /// weight axis
    #[test]
    fn a_variable_face_says_its_axes() {
        let p = std::path::PathBuf::from("../personal/home/.local/share/fonts/GoogleSansFlex.ttf");
        let Ok(bytes) = std::fs::read(&p) else { return };
        let e = entries(&p, &bytes);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].family, "Google Sans Flex");
        let w = e[0].axis(b"wght").expect("a weight axis");
        assert!(w.min <= 400.0 && w.max >= 700.0, "{w:?}");
        eprintln!("axes {:?}", e[0].axes.iter().map(|a| (String::from_utf8_lossy(&a.tag).to_string(), a.min, a.default, a.max)).collect::<Vec<_>>());
        for i in &e[0].instances {
            eprintln!("{} {:?}", i.name, i.coords);
        }
        let bold = e[0].instances.iter().find(|i| i.name == "Bold").expect("a Bold instance");
        let wi = e[0].axes.iter().position(|a| &a.tag == b"wght").unwrap();
        assert_eq!(bold.coords[wi], 700.0);
        // Styles as fontconfig lists them: the face as "Regular", then every
        // instance except the one at its coordinates, so "Medium" is the fifth,
        // at wght 500
        let styles = e[0].styles();
        assert_eq!((styles[0].name.as_str(), styles[0].instance), ("Regular", None));
        assert_eq!(styles.iter().filter(|s| s.name == "Regular").count(), 1);
        let medium = styles.iter().find(|s| s.name == "Medium").expect("a Medium style");
        assert_eq!((medium.weight, medium.instance, medium.italic), (500.0, Some(4), false));
        assert!(styles.iter().find(|s| s.name == "Bold Italic").is_some_and(|s| s.italic));
    }

    /// The fonts in the tree, when they are there
    #[test]
    fn the_shipped_fonts_say_their_families() {
        let dir = std::path::Path::new("../etc/aosh/negi/assets/fonts");
        let Ok(bytes) = std::fs::read(dir.join("MaterialSymbolsRounded.ttf")) else { return };
        assert_eq!(names(&bytes).map(|n| n.family).as_deref(), Some("Material Symbols Rounded"));
        if let Ok(bytes) = std::fs::read(dir.join("handwriting/Caveat.ttf")) {
            assert_eq!(names(&bytes).map(|n| n.family).as_deref(), Some("Caveat"));
        }
    }
}
