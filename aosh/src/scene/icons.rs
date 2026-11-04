// Theme icons, following KDE's icon loader (`QIcon::fromTheme` under
// `QT_QPA_PLATFORMTHEME=kde`, checked by `icon_themes_choose_as_kde_chooses`)
//
// - Themes: the one in `kdeglobals` (`[Icons] Theme`, default breeze), its
//   inherited themes depth first, then hicolor. Directories come from
//   `index.theme` with scale 1, under every XDG data `icons` directory.
// - The first theme with the name is used, whatever sizes it has.
// - SVG before PNG: `NAME.svg` in any directory wins over `NAME.png`.
// - Size: the first listed directory matching the size (Fixed, Threshold
//   default 2, or Scalable range). Otherwise the smallest bigger one, then the
//   biggest.
// - Drawn at the requested size: SVG rendered (`svg/`), PNG scaled.
// - Color scheme: in an SVG with an element of id `current-color-scheme`, that
//   element's text is replaced with KDE's style sheet (`.ColorScheme-Text`
//   window foreground, `-Background` its background, `-Highlight` and `-Accent`
//   selection background, `-HighlightedText` its foreground, `-PositiveText`,
//   `-NeutralText`, `-NegativeText` the window's). Other SVGs keep their
//   colors.
// - A name starting with `/` is a file.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::io::kdeglobals::KdeGlobals;
use crate::io::xdg::{kind, Env};

use super::picture::{self, Picture};
use super::svg::xml::{Element, Node};

/// The colours an icon's style sheet gives its classes, `#rrggbb`
#[derive(Clone, Debug, PartialEq)]
pub struct Colours {
    pub text: String,
    pub background: String,
    pub highlight: String,
    pub highlighted_text: String,
    pub positive: String,
    pub neutral: String,
    pub negative: String,
}

impl Colours {
    /// From `kdeglobals`, KDE's own Breeze where it says nothing
    pub fn of(k: &KdeGlobals) -> Colours {
        let get = |g: &str, key: &str, d: &str| k.get(g, key).unwrap_or(d).to_string();
        Colours {
            text: get("Colors:Window", "ForegroundNormal", "#232629"),
            background: get("Colors:Window", "BackgroundNormal", "#eff0f1"),
            highlight: get("Colors:Selection", "BackgroundNormal", "#3daee9"),
            highlighted_text: get("Colors:Selection", "ForegroundNormal", "#ffffff"),
            positive: get("Colors:Window", "ForegroundPositive", "#27ae60"),
            neutral: get("Colors:Window", "ForegroundNeutral", "#f67400"),
            negative: get("Colors:Window", "ForegroundNegative", "#da4453"),
        }
    }

    /// KDE's style sheet
    fn sheet(&self) -> String {
        [
            ("Text", &self.text),
            ("Background", &self.background),
            ("Highlight", &self.highlight),
            ("HighlightedText", &self.highlighted_text),
            ("PositiveText", &self.positive),
            ("NeutralText", &self.neutral),
            ("NegativeText", &self.negative),
            ("Accent", &self.highlight),
        ].iter().map(|(n, c)| format!(".ColorScheme-{n} {{ color:{c}; }}\n")).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Type {
    Fixed,
    Scalable { min: u32, max: u32 },
    Threshold(u32),
}

#[derive(Clone, Debug)]
struct Dir {
    path: String,
    size: u32,
    ty: Type,
}

impl Dir {
    /// Distance from `size`: zero when equal, positive when bigger
    fn delta(&self, size: u32) -> i64 {
        let s = size as i64;
        match self.ty {
            Type::Fixed => self.size as i64 - s,
            Type::Threshold(t) if (self.size as i64 - s).abs() <= t as i64 => 0,
            Type::Threshold(_) => self.size as i64 - s,
            Type::Scalable { min, .. } if s < min as i64 => min as i64 - s,
            Type::Scalable { max, .. } if s > max as i64 => max as i64 - s,
            Type::Scalable { .. } => 0,
        }
    }
}

#[derive(Clone, Debug)]
struct Theme {
    name: String,
    /// The `icons` directories that have it
    bases: Vec<PathBuf>,
    dirs: Vec<Dir>,
    inherits: Vec<String>,
}

pub struct Icons {
    chain: Vec<Theme>,
    colours: Colours,
}

impl Icons {
    /// Process-wide instance using the environment's theme, read once
    pub fn global() -> &'static Icons {
        static ICONS: OnceLock<Icons> = OnceLock::new();
        ICONS.get_or_init(|| {
            let env = Env::process();
            let k = KdeGlobals::read(&env);
            let mut bases: Vec<PathBuf> = env.locations(kind::GENERIC_DATA).into_iter().map(|d| d.join("icons")).collect();
            if let Some(home) = (env.var)("HOME").filter(|h| h.starts_with('/')) {
                bases.insert(1.min(bases.len()), Path::new(&home).join(".icons"));
            }
            Icons::new(&bases, k.get("Icons", "Theme").unwrap_or("breeze"), Colours::of(&k))
        })
    }

    /// The theme `name` and what it inherits, from `bases`
    pub fn new(bases: &[PathBuf], name: &str, colours: Colours) -> Icons {
        let mut chain: Vec<Theme> = Vec::new();
        let first = load(bases, name).or_else(|| load(bases, "breeze"));
        if let Some(t) = first {
            add(bases, t, &mut chain);
        }
        if !chain.iter().any(|t| t.name == "hicolor") {
            if let Some(t) = load(bases, "hicolor") {
                chain.push(t);
            }
        }
        Icons { chain, colours }
    }

    /// The file `name` is drawn from at `size`
    pub fn find(&self, name: &str, size: u32) -> Option<PathBuf> {
        if name.is_empty() {
            return None;
        }
        if name.starts_with('/') {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        for theme in &self.chain {
            for ext in ["svg", "png"] {
                let file = format!("{name}.{ext}");
                let mut best: Option<(i64, PathBuf)> = None;
                for d in &theme.dirs {
                    let Some(p) = theme.bases.iter().map(|b| b.join(&theme.name).join(&d.path).join(&file)).find(|p| p.is_file()) else {
                        continue;
                    };
                    let delta = d.delta(size);
                    if delta == 0 {
                        return Some(p);
                    }
                    // Bigger before smaller, nearer first
                    let rank = |d: i64| (d < 0, d.abs());
                    if best.as_ref().is_none_or(|(b, _)| rank(delta) < rank(*b)) {
                        best = Some((delta, p));
                    }
                }
                if let Some((_, p)) = best {
                    return Some(p);
                }
            }
        }
        None
    }

    /// Whether the theme has `name`: quickshell's `iconPath(name, true)`
    pub fn has(&self, name: &str) -> bool {
        self.find(name, 32).is_some()
    }

    /// `name` drawn `size` by `size`, premultiplied
    pub fn picture(&self, name: &str, size: u32) -> Option<Picture> {
        if size == 0 {
            return None;
        }
        let path = self.find(name, size)?;
        let bytes = std::fs::read(&path).ok()?;
        let s = size as usize;
        if path.extension().is_some_and(|e| e == "svg") {
            let text = String::from_utf8_lossy(&bytes);
            let mut root = super::svg::xml::parse(&text)?;
            recolour(&mut root, &self.colours.sheet());
            let img = super::svg::render_tree(&root, s, s)?;
            return Some(Picture::new(s, s, img.px));
        }
        Some(picture::scaled(picture::decode(&bytes)?, s, s))
    }
}

/// Theme `name` from the first base with its `index.theme`, with directories
/// from every base
fn load(bases: &[PathBuf], name: &str) -> Option<Theme> {
    let index = bases.iter().map(|b| b.join(name).join("index.theme")).find(|p| p.is_file())?;
    let text = std::fs::read_to_string(index).ok()?;
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut group = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(g) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            group = g.to_string();
        } else if let Some((k, v)) = line.split_once('=') {
            groups.entry(group.clone()).or_default().insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    let head = groups.get("Icon Theme")?;
    let list = |k: &str| head.get(k).map(|v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()).unwrap_or_else(Vec::new);
    let mut dirs = Vec::new();
    for path in list("Directories") {
        let Some(g) = groups.get(&path) else { continue };
        let num = |k: &str| g.get(k).and_then(|v| v.parse::<u32>().ok());
        if num("Scale").unwrap_or(1) != 1 {
            continue;
        }
        let Some(size) = num("Size") else { continue };
        let ty = match g.get("Type").map(String::as_str) {
            Some("Fixed") => Type::Fixed,
            Some("Scalable") => Type::Scalable { min: num("MinSize").unwrap_or(size), max: num("MaxSize").unwrap_or(size) },
            _ => Type::Threshold(num("Threshold").unwrap_or(2)),
        };
        dirs.push(Dir { path, size, ty });
    }
    let have: Vec<PathBuf> = bases.iter().filter(|b| b.join(name).is_dir()).cloned().collect();
    Some(Theme { name: name.to_string(), bases: have, dirs, inherits: list("Inherits") })
}

/// `t`, then what it inherits, each followed by what that inherits
fn add(bases: &[PathBuf], t: Theme, chain: &mut Vec<Theme>) {
    if chain.iter().any(|c| c.name == t.name) || chain.len() > 32 {
        return;
    }
    let inherits = t.inherits.clone();
    chain.push(t);
    for name in inherits {
        if !chain.iter().any(|c| c.name == name) {
            if let Some(p) = load(bases, &name) {
                add(bases, p, chain);
            }
        }
    }
}

/// Replaces the text of the `current-color-scheme` element with KDE's style
/// sheet
fn recolour(el: &mut Element, sheet: &str) -> bool {
    if el.attr("id") == Some("current-color-scheme") {
        el.children = vec![Node::Text(sheet.to_string())];
        return true;
    }
    el.children.iter_mut().any(|c| match c {
        Node::Element(e) => recolour(e, sheet),
        Node::Text(_) => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(path: &Path, colour: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10"><rect width="10" height="10" fill="{colour}"/></svg>"#)).unwrap();
    }

    /// Two themes laid out like breeze-dark's applications and hicolor, with
    /// reference output from KDE's loader for sizes 1 to 130
    #[test]
    fn icon_themes_choose_as_kde_chooses() {
        let base = std::env::temp_dir().join(format!("aosh-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let fx = base.join("fx");
        std::fs::create_dir_all(&fx).unwrap();
        std::fs::write(fx.join("index.theme"), "[Icon Theme]\nName=fx\nInherits=fxhi\n\
            Directories=apps/16,apps/22,apps/32,apps/48,apps/16@2x,apps/22@2x\n\n\
            [apps/16]\nSize=16\nType=Fixed\n[apps/22]\nSize=22\nType=Fixed\n[apps/32]\nSize=32\nType=Fixed\n\
            [apps/48]\nSize=48\nType=Scalable\nMinSize=48\nMaxSize=256\n\
            [apps/16@2x]\nSize=16\nScale=2\nType=Fixed\n[apps/22@2x]\nSize=22\nScale=2\nType=Fixed\n").unwrap();
        for (d, c) in [("16", "#ff0000"), ("22", "#00ff00"), ("32", "#0000ff"), ("48", "#ffff00"), ("16@2x", "#ff00ff"),
                       ("22@2x", "#00ffff"), ("64", "#808080")] {
            square(&fx.join(format!("apps/{d}/sq.svg")), c);
        }
        let hi = base.join("fxhi");
        std::fs::create_dir_all(&hi).unwrap();
        std::fs::write(hi.join("index.theme"), "[Icon Theme]\nName=fxhi\n\
            Directories=16x16/apps,22x22/apps,48x48/apps,scalable/apps\n\n\
            [16x16/apps]\nSize=16\nType=Threshold\n[22x22/apps]\nSize=22\nType=Threshold\n\
            [48x48/apps]\nSize=48\nType=Threshold\n[scalable/apps]\nSize=128\nMinSize=1\nMaxSize=256\nType=Scalable\n").unwrap();
        for (d, c) in [("16x16", "#ff0000"), ("22x22", "#00ff00"), ("48x48", "#ffff00"), ("scalable", "#0000ff")] {
            square(&hi.join(format!("{d}/apps/hq.svg")), c);
        }
        square(&hi.join("48x48/apps/only.svg"), "#123456");
        // In both themes: the first has it at 16 only and is used at 48 as well
        square(&fx.join("apps/16/two.svg"), "#ff0000");
        square(&hi.join("48x48/apps/two.svg"), "#ffff00");
        // An SVG anywhere beats a PNG of the exact size
        square(&fx.join("apps/48/both.svg"), "#ffff00");
        std::fs::copy(fx.join("apps/16/sq.svg"), fx.join("apps/16/both.png")).unwrap();

        let icons = Icons::new(&[base.clone()], "fx", Colours::of(&KdeGlobals::default()));
        let from = |name: &str, s: u32| -> String {
            let p = icons.find(name, s).unwrap_or_else(|| panic!("{name} at {s}: nothing"));
            p.strip_prefix(&base).unwrap().parent().unwrap().display().to_string()
        };
        let runs = |name: &str| -> String {
            let mut out: Vec<String> = Vec::new();
            let mut run: Option<(u32, u32, String)> = None;
            for s in 1..=130 {
                let d = from(name, s);
                match &mut run {
                    Some((_, end, at)) if *at == d => *end = s,
                    _ => {
                        if let Some((a, b, at)) = run.take() {
                            out.push(format!("{a}-{b}:{at}"));
                        }
                        run = Some((s, s, d));
                    }
                }
            }
            let (a, b, at) = run.unwrap();
            out.push(format!("{a}-{b}:{at}"));
            out.join(" ")
        };
        assert_eq!(runs("sq"), "1-16:fx/apps/16 17-22:fx/apps/22 23-32:fx/apps/32 33-130:fx/apps/48");
        assert_eq!(runs("hq"), "1-13:fxhi/scalable/apps 14-18:fxhi/16x16/apps 19-19:fxhi/scalable/apps \
            20-24:fxhi/22x22/apps 25-45:fxhi/scalable/apps 46-50:fxhi/48x48/apps 51-130:fxhi/scalable/apps");
        assert_eq!(from("only", 22), "fxhi/48x48/apps");
        assert_eq!(from("two", 48), "fx/apps/16");
        assert_eq!(icons.find("both", 16).unwrap().strip_prefix(&base).unwrap().display().to_string(), "fx/apps/48/both.svg");
        assert!(!icons.has("nothing") && !icons.has("sq-extra") && icons.has("sq"));
        let _ = std::fs::remove_dir_all(&base);
    }

    /// Scheme classes get KDE's colors only when the file has the replaced
    /// element; others stay black
    #[test]
    fn an_icons_style_sheet_is_the_colour_schemes() {
        let mut k = KdeGlobals::default();
        k.add("[Colors:Window]\nForegroundNormal=#c1c7ce\nBackgroundNormal=#1c2024\nForegroundPositive=#00e479\n\
               ForegroundNeutral=#fcb38a\nForegroundNegative=#ffb3b4\nForegroundActive=#8fc9fc\n\
               [Colors:Selection]\nBackgroundNormal=#90caf3\nForegroundNormal=#00344c\n");
        let classes = ["Text", "Background", "Highlight", "HighlightedText", "PositiveText", "NeutralText", "NegativeText",
                       "ActiveText", "Complement", "Contrast", "Accent"];
        let body: String = classes.iter().enumerate()
            .map(|(i, n)| format!(r#"<rect x="{i}" y="0" width="1" height="20" style="fill:currentColor" class="ColorScheme-{n}"/>"#))
            .collect();
        let base = std::env::temp_dir().join(format!("aosh-recolour-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let t = base.join("t");
        std::fs::create_dir_all(t.join("apps/22")).unwrap();
        std::fs::write(t.join("index.theme"), "[Icon Theme]\nDirectories=apps/22\n[apps/22]\nSize=22\nType=Fixed\n").unwrap();
        std::fs::write(t.join("apps/22/cls.svg"), format!(r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><style id="current-color-scheme">.ColorScheme-Text {{ color:#000001; }}</style>{body}</svg>"#)).unwrap();
        std::fs::write(t.join("apps/22/plain.svg"), r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 20"><rect width="20" height="20" style="fill:currentColor" class="ColorScheme-Text"/></svg>"#).unwrap();
        let icons = Icons::new(&[base.clone()], "t", Colours::of(&k));
        let p = icons.picture("cls", 20).unwrap();
        let got: Vec<String> = (0..classes.len()).map(|i| format!("#{:06x}", p.px[10 * 20 + i] & 0xffffff)).collect();
        assert_eq!(got, ["#c1c7ce", "#1c2024", "#90caf3", "#00344c", "#00e479", "#fcb38a", "#ffb3b4", "#000000",
                         "#000000", "#000000", "#90caf3"]);
        assert_eq!(icons.picture("plain", 20).unwrap().px[210] & 0xffffff, 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
