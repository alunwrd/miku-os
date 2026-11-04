// Text lines, following QQuickText
//
// - `\n` always breaks.
// - `wrapMode` breaks lines wider than a set width: `WordWrap` (1) at Unicode
//   breaks, `WrapAnywhere` (3) between any characters, `Wrap` (4) at a word,
//   else anywhere.
// - `maximumLineCount` limits lines.
// - `elide` cuts the last line when too wide: `ElideLeft` (0), `ElideRight`
//   (1), `ElideMiddle` (2), `ElideNone` (3).
// - Line height is `ascent + descent`, times `lineHeight` (ProportionalHeight,
//   0) or `lineHeight` pixels (FixedHeight, 1).
//
// The widest unwrapped line is the implicit width; the lines at the item's
// width give the implicit height.

use super::{Font, Fonts, Metrics};

#[derive(Clone, Copy, Debug)]
pub struct Opts {
    /// The item's width, when it has one to break and cut at
    pub width: Option<f32>,
    pub wrap: i64,
    pub elide: i64,
    pub max_lines: usize,
    pub line_height: f32,
    pub line_height_fixed: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { width: None, wrap: 0, elide: 3, max_lines: usize::MAX, line_height: 1.0, line_height_fixed: false }
    }
}

pub struct Line {
    pub text: String,
    pub width: f32,
}

pub struct Laid {
    pub lines: Vec<Line>,
    /// Widest unbroken line: the implicit width
    pub natural: f32,
    /// Widest laid-out line: `contentWidth`
    pub content: f32,
    /// Total height: the implicit height before rounding up
    pub height: f32,
    /// One line's step
    pub step: f32,
    pub metrics: Metrics,
    /// Lines were omitted or one was cut
    pub truncated: bool,
}

const ELLIPSIS: &str = "\u{2026}";

pub fn lay(fonts: &mut Fonts, f: &Font, text: &str, o: &Opts) -> Laid {
    let metrics = fonts.metrics(f);
    let step = if o.line_height_fixed { o.line_height } else { metrics.height() * o.line_height };
    let paragraphs: Vec<&str> = text.split('\n').collect();
    let natural = paragraphs.iter().map(|p| fonts.width(f, p)).fold(0.0f32, f32::max);
    let mut lines: Vec<String> = Vec::new();
    for p in &paragraphs {
        match o.width {
            Some(w) if o.wrap != 0 && fonts.width(f, p) > w => lines.extend(wrap(fonts, f, p, w, o.wrap)),
            _ => lines.push(p.to_string()),
        }
    }
    let mut truncated = false;
    if lines.len() > o.max_lines.max(1) {
        lines.truncate(o.max_lines.max(1));
        truncated = true;
    }
    if let (Some(w), Some(last)) = (o.width, lines.last_mut()) {
        if o.elide != 3 && (truncated || fonts.width(f, last) > w) {
            let cut = elide(fonts, f, last, w, o.elide, truncated);
            truncated |= cut != *last;
            *last = cut;
        }
    }
    let lines: Vec<Line> = lines.into_iter().map(|t| {
        let width = fonts.width(f, &t);
        Line { text: t, width }
    }).collect();
    let content = lines.iter().map(|l| l.width).fold(0.0f32, f32::max);
    let height = step * lines.len() as f32;
    Laid { lines, natural, content, height, step, metrics, truncated }
}

/// One paragraph broken to lines no wider than `w`
fn wrap(fonts: &mut Fonts, f: &Font, text: &str, w: f32, mode: i64) -> Vec<String> {
    let anywhere = |fonts: &mut Fonts, s: &str| -> Vec<String> {
        let mut out = Vec::new();
        let mut cur = String::new();
        for c in s.chars() {
            let mut next = cur.clone();
            next.push(c);
            if !cur.is_empty() && fonts.width(f, &next) > w {
                out.push(std::mem::take(&mut cur));
                cur.push(c);
            } else {
                cur = next;
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    if mode == 3 {
        return anywhere(fonts, text);
    }
    // Where Unicode allows a break, and the pieces between
    let mut words: Vec<&str> = Vec::new();
    let mut at = 0;
    for (i, _) in unicode_linebreak::linebreaks(text) {
        if i > at {
            words.push(&text[at..i]);
        }
        at = i;
    }
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    for word in words {
        let next = format!("{cur}{word}");
        if cur.is_empty() || fonts.width(f, next.trim_end()) <= w {
            cur = next;
        } else {
            out.push(cur.trim_end().to_string());
            cur = word.to_string();
        }
        // Word wider than the line: `Wrap` breaks it anywhere
        if mode == 4 && fonts.width(f, cur.trim_end()) > w {
            let mut pieces = anywhere(fonts, &cur);
            cur = pieces.pop().unwrap_or_default();
            out.extend(pieces);
        }
    }
    if !cur.is_empty() || out.is_empty() {
        out.push(cur.trim_end().to_string());
    }
    out
}

/// Elides `text` to `w` per `mode`. `forced`: later lines were omitted, so it
/// gets an ellipsis even if it fits
pub fn elide(fonts: &mut Fonts, f: &Font, text: &str, w: f32, mode: i64, forced: bool) -> String {
    if !forced && fonts.width(f, text) <= w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let fits = |fonts: &mut Fonts, s: &str| fonts.width(f, s) <= w;
    match mode {
        // ElideLeft: keeps the end
        0 => {
            for n in (0..chars.len()).rev() {
                let s = format!("{ELLIPSIS}{}", chars[chars.len() - n..].iter().collect::<String>());
                if fits(fonts, &s) {
                    return s;
                }
            }
            ELLIPSIS.to_string()
        }
        // ElideMiddle: keeps equal parts of both ends
        2 => {
            for n in (0..chars.len()).rev() {
                let head = n.div_ceil(2);
                let tail = n / 2;
                let s = format!("{}{ELLIPSIS}{}", chars[..head].iter().collect::<String>(),
                    chars[chars.len() - tail..].iter().collect::<String>());
                if fits(fonts, &s) {
                    return s;
                }
            }
            ELLIPSIS.to_string()
        }
        // ElideRight
        _ => {
            for n in (0..=chars.len()).rev() {
                let s = format!("{}{ELLIPSIS}", chars[..n].iter().collect::<String>().trim_end());
                if fits(fonts, &s) {
                    return s;
                }
            }
            ELLIPSIS.to_string()
        }
    }
}
