// SVG rendering, matching QtSvg painted into a transparent picture by
// QPainter's raster engine (`tests/qtref/svg.py`, `svgs_are_qts`). Draws with
// the Canvas engine (`canvas/replay.rs`).
//
// Supported:
// - `path`, `rect` (also rounded), `circle`, `ellipse`, `line`, `polyline`,
//   `polygon`, inside `g`, `svg`, `a`, `switch`, and via `use`; `defs`,
//   `symbol`, `clipPath`, `mask`, and editor elements are not drawn;
// - `transform` (matrix, translate, scale, rotate, skewX, skewY);
// - fill and stroke in a color, `currentColor`, or linear or radial gradient,
//   in user space or the bounding box, with `gradientTransform` and `href`
//   inheritance;
// - `fill-rule`, `fill-opacity`, `stroke-opacity`, `opacity`, stroke width,
//   caps, joins, miter limit, and dashes;
// - style sheets, `style`, and presentation attributes (`css.rs`);
//   `display: none` and `visibility: hidden`.
//
// The viewBox fills the picture, stretched if shapes differ.

pub mod css;
pub mod path;
pub mod xml;

use std::collections::HashMap;

use crate::scene::canvas::replay::{self, Gradient, Paint};
use crate::scene::canvas::stroke::{self, Cap, Join, Pen};
use crate::scene::paint::{Image, Painter};
use crate::scene::xf::Xf;

use css::Sheet;
use path::Cmd;
use xml::Element;

/// Draws a parsed document at `w` by `h`, premultiplied; `None` if it is not
/// SVG
pub fn render_tree(root: &Element, w: usize, h: usize) -> Option<Image> {
    if root.name != "svg" || w == 0 || h == 0 {
        return None;
    }
    let mut ids = HashMap::new();
    index(root, &mut ids);
    let doc = Doc { sheet: Sheet::of(root), ids };
    let (vx, vy, vw, vh) = view_box(root)?;
    let xf = Xf::scale(w as f64 / vw, h as f64 / vh).times(&Xf::translate(-vx, -vy));
    let mut img = Image::new(w as i32, h as i32);
    let style = doc.style_of(root, &Style::default());
    for c in root.elements() {
        doc.draw(c, &style, &xf, 1.0, &mut img, 0);
    }
    Some(img)
}

/// The viewBox, or the document's size from the origin when it has none
fn view_box(root: &Element) -> Option<(f64, f64, f64, f64)> {
    if let Some(v) = root.attr("viewBox") {
        let n = path::numbers(v);
        if n.len() == 4 && n[2] > 0.0 && n[3] > 0.0 {
            return Some((n[0], n[1], n[2], n[3]));
        }
    }
    let len = |n: &str| root.attr(n).filter(|v| !v.trim().ends_with('%')).and_then(length);
    let (w, h) = (len("width").unwrap_or(100.0), len("height").unwrap_or(100.0));
    (w > 0.0 && h > 0.0).then_some((0.0, 0.0, w, h))
}

fn index<'a>(el: &'a Element, ids: &mut HashMap<String, &'a Element>) {
    if let Some(id) = el.attr("id") {
        ids.entry(id.to_string()).or_insert(el);
    }
    for c in el.elements() {
        index(c, ids);
    }
}

/// What a fill or a stroke is said to be painted with
#[derive(Clone, Debug, PartialEq)]
enum Spec {
    Colour(u32),
    Current,
    /// A gradient by id, and what to paint when there is none
    Url(String, Option<Box<Spec>>),
}

/// Inherited properties and `color` at an element
#[derive(Clone, Debug)]
struct Style {
    fill: Option<Spec>,
    fill_opacity: f64,
    even_odd: bool,
    stroke: Option<Spec>,
    stroke_width: f64,
    stroke_opacity: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    dash: Vec<f64>,
    dash_offset: f64,
    color: u32,
    visible: bool,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            fill: Some(Spec::Colour(0xFF00_0000)),
            fill_opacity: 1.0,
            even_odd: false,
            stroke: None,
            stroke_width: 1.0,
            stroke_opacity: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 4.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            color: 0xFF00_0000,
            visible: true,
        }
    }
}

struct Doc<'a> {
    sheet: Sheet,
    ids: HashMap<String, &'a Element>,
}

/// `use` recursion limit
const MAX_DEPTH: u32 = 16;

impl<'a> Doc<'a> {
    /// `el`'s style, from its parent's and what the cascade gives it
    fn style_of(&self, el: &Element, parent: &Style) -> Style {
        let mut s = parent.clone();
        for (k, v) in self.sheet.cascade(el) {
            if v == "inherit" {
                continue;
            }
            match k.as_str() {
                "fill" => s.fill = paint(&v),
                "stroke" => s.stroke = paint(&v),
                "fill-opacity" => s.fill_opacity = fraction(&v).unwrap_or(s.fill_opacity),
                "stroke-opacity" => s.stroke_opacity = fraction(&v).unwrap_or(s.stroke_opacity),
                "fill-rule" => s.even_odd = v == "evenodd",
                "stroke-width" => s.stroke_width = length(&v).filter(|w| *w >= 0.0).unwrap_or(s.stroke_width),
                "stroke-linecap" => s.cap = match v.as_str() { "round" => Cap::Round, "square" => Cap::Square, "butt" => Cap::Butt, _ => s.cap },
                "stroke-linejoin" => s.join = match v.as_str() { "round" => Join::Round, "bevel" => Join::Bevel, "miter" => Join::Miter, _ => s.join },
                "stroke-miterlimit" => s.miter = v.parse().ok().filter(|m: &f64| *m >= 1.0).unwrap_or(s.miter),
                "stroke-dasharray" => {
                    let d = if v == "none" { Vec::new() } else { lengths(&v) };
                    s.dash = if d.iter().any(|x| *x < 0.0) || d.iter().sum::<f64>() <= 0.0 {
                        Vec::new()
                    } else if d.len() % 2 == 1 {
                        d.iter().chain(d.iter()).copied().collect()
                    } else {
                        d
                    };
                }
                "stroke-dashoffset" => s.dash_offset = length(&v).unwrap_or(s.dash_offset),
                "color" => {
                    if let Some(c) = colour(&v) {
                        s.color = c;
                    }
                }
                "visibility" => s.visible = v == "visible",
                _ => {}
            }
        }
        s
    }

    /// One element: cascade's `display`, `opacity`, and `transform`, then its
    /// drawing
    fn draw(&self, el: &Element, parent: &Style, xf: &Xf, alpha: f64, img: &mut Image, depth: u32) {
        if depth > MAX_DEPTH {
            return;
        }
        let name = el.name.as_str();
        if matches!(name, "defs" | "symbol" | "clipPath" | "mask" | "linearGradient" | "radialGradient" | "pattern"
            | "style" | "title" | "desc" | "metadata" | "marker" | "filter" | "text") || name.contains(':')
        {
            return;
        }
        let props = self.sheet.cascade(el);
        let last = |k: &str| props.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        if last("display") == Some("none") {
            return;
        }
        let style = self.style_of(el, parent);
        let own = last("opacity").and_then(fraction).unwrap_or(1.0);
        let mut xf = *xf;
        if let Some(t) = el.attr("transform") {
            xf = xf.times(&transform(t));
        }
        let num = |n: &str| el.attr(n).and_then(length).unwrap_or(0.0);
        // A container with opacity draws separately and composites as one, as
        // in QtSvg
        let container = matches!(name, "g" | "a" | "switch" | "svg" | "use");
        if container && own < 1.0 {
            let mut layer = Image::new(img.w, img.h);
            self.contents(el, &style, &xf, &mut layer, depth);
            let all = img.bounds();
            Painter::new(img, all).layer(&layer, (alpha * own) as f32);
            return;
        }
        let alpha = alpha * own;
        match name {
            "g" | "a" | "switch" | "svg" | "use" => self.contents(el, &style, &xf, img, depth),
            _ => {
                let cmds = match name {
                    "path" => path::parse(el.attr("d").unwrap_or("")),
                    "rect" => {
                        let (w, h) = (num("width"), num("height"));
                        if w <= 0.0 || h <= 0.0 {
                            return;
                        }
                        let (rx, ry) = (el.attr("rx").and_then(length), el.attr("ry").and_then(length));
                        let (rx, ry) = match (rx, ry) {
                            (Some(x), Some(y)) => (x, y),
                            (Some(x), None) => (x, x),
                            (None, Some(y)) => (y, y),
                            (None, None) => (0.0, 0.0),
                        };
                        path::rect(num("x"), num("y"), w, h, rx.clamp(0.0, w / 2.0), ry.clamp(0.0, h / 2.0))
                    }
                    "circle" => {
                        let r = num("r");
                        if r <= 0.0 {
                            return;
                        }
                        path::ellipse(num("cx"), num("cy"), r, r)
                    }
                    "ellipse" => {
                        let (rx, ry) = (num("rx"), num("ry"));
                        if rx <= 0.0 || ry <= 0.0 {
                            return;
                        }
                        path::ellipse(num("cx"), num("cy"), rx, ry)
                    }
                    "line" => vec![Cmd::Move((num("x1"), num("y1"))), Cmd::Line((num("x2"), num("y2")))],
                    "polyline" => path::poly(el.attr("points").unwrap_or(""), false),
                    "polygon" => path::poly(el.attr("points").unwrap_or(""), true),
                    _ => return,
                };
                if cmds.is_empty() || !style.visible {
                    return;
                }
                self.shape(&cmds, &style, &xf, alpha, img);
            }
        }
    }

    /// Container contents: children, or the target of a `use`
    fn contents(&self, el: &Element, style: &Style, xf: &Xf, img: &mut Image, depth: u32) {
        let num = |n: &str| el.attr(n).and_then(length).unwrap_or(0.0);
        if el.name == "use" {
            let href = el.attr("xlink:href").or_else(|| el.attr("href")).unwrap_or("");
            if let Some(target) = href.strip_prefix('#').and_then(|id| self.ids.get(id)) {
                let xf = xf.times(&Xf::translate(num("x"), num("y")));
                self.draw(target, style, &xf, 1.0, img, depth + 1);
            }
            return;
        }
        let xf = if el.name == "svg" { xf.times(&Xf::translate(num("x"), num("y"))) } else { *xf };
        for c in el.elements() {
            self.draw(c, style, &xf, 1.0, img, depth + 1);
        }
    }

    /// An outline filled, then stroked
    fn shape(&self, cmds: &[Cmd], style: &Style, xf: &Xf, alpha: f64, img: &mut Image) {
        // In document units, mapped to the picture as Qt maps a path through a
        // matrix (`Shape::mapped`)
        let own = path::shape(cmds, &Xf::IDENTITY);
        let bbox = || path::bounds(cmds);
        if let Some(p) = style.fill.as_ref().and_then(|s| self.paint(s, style.color, xf, &bbox)) {
            replay::fill(img, &own.mapped(xf), style.even_odd, &p, alpha * style.fill_opacity);
        }
        if let Some(p) = style.stroke.as_ref().and_then(|s| self.paint(s, style.color, xf, &bbox)) {
            let alpha = alpha * style.stroke_opacity;
            let pen = Pen { width: style.stroke_width, cap: style.cap, join: style.join, miter_limit: style.miter };
            match xf.similarity() {
                // A shape-preserving matrix scales the line width; walked at a
                // pixel or less
                Some(k) => {
                    let pen = Pen { width: pen.width * k, ..pen };
                    let dash: Vec<f64> = style.dash.iter().map(|d| d * k).collect();
                    replay::stroke(img, &path::shape(cmds, xf), &pen, &dash, style.dash_offset * k, &p, alpha);
                }
                // A stretching or shearing matrix: the outline is built in
                // document units and mapped, however thin
                None => {
                    let own = if style.dash.iter().sum::<f64>() > 0.0 { own.dashed(&style.dash, style.dash_offset) } else { own };
                    replay::fill(img, &stroke::outline(&own, &pen).mapped(xf), false, &p, alpha);
                }
            }
        }
    }

    /// Paint for `spec`, for a shape through `xf` with box `bbox`
    fn paint(&self, spec: &Spec, color: u32, xf: &Xf, bbox: &dyn Fn() -> Option<(f64, f64, f64, f64)>) -> Option<Paint> {
        match spec {
            Spec::Colour(c) => Some(Paint::Colour(*c)),
            Spec::Current => Some(Paint::Colour(color)),
            Spec::Url(id, fallback) => match self.ids.get(id.as_str()) {
                Some(g) if matches!(g.name.as_str(), "linearGradient" | "radialGradient") => self.gradient(g, xf, bbox),
                _ => fallback.as_ref().and_then(|f| self.paint(f, color, xf, bbox)),
            },
        }
    }

    /// Gradient element with its `href` chain resolved, as a canvas gradient in
    /// `Xf`'s source coordinates
    fn gradient(&self, g: &Element, xf: &Xf, bbox: &dyn Fn() -> Option<(f64, f64, f64, f64)>) -> Option<Paint> {
        let mut chain = vec![g];
        while let Some(next) = chain.last().and_then(|e| e.attr("xlink:href").or_else(|| e.attr("href")))
            .and_then(|h| h.strip_prefix('#')).and_then(|id| self.ids.get(id))
        {
            if chain.len() > 8 || chain.iter().any(|e| std::ptr::eq(*e, *next)) {
                break;
            }
            chain.push(next);
        }
        let attr = |n: &str| chain.iter().find_map(|e| e.attr(n));
        let user = attr("gradientUnits") == Some("userSpaceOnUse");
        let coord = |n: &str, dflt: f64| attr(n).and_then(|v| {
            let v = v.trim();
            match v.strip_suffix('%') {
                Some(p) => p.trim().parse::<f64>().ok().map(|p| p / 100.0),
                None => length(v),
            }
        }).unwrap_or(dflt);
        let stops_of = chain.iter().find(|e| e.elements().any(|c| c.name == "stop"))?;
        let mut stops = Vec::new();
        let mut at = 0.0f64;
        for s in stops_of.elements().filter(|c| c.name == "stop") {
            let props = self.sheet.cascade(s);
            let last = |k: &str| props.iter().rev().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let offset = s.attr("offset").and_then(fraction).unwrap_or(0.0).clamp(0.0, 1.0).max(at);
            at = offset;
            let c = last("stop-color").and_then(|v| colour(&v)).unwrap_or(0xFF00_0000);
            let o = last("stop-opacity").and_then(|v| fraction(&v)).unwrap_or(1.0);
            let a = (((c >> 24) as f64) * o).round() as u32;
            stops.push((offset, (a << 24) | (c & 0x00FF_FFFF)));
        }
        if stops.is_empty() {
            return None;
        }
        let mut space = *xf;
        if !user {
            let (x0, y0, x1, y1) = bbox()?;
            if x1 <= x0 || y1 <= y0 {
                return None;
            }
            space = space.times(&Xf::translate(x0, y0)).times(&Xf::scale(x1 - x0, y1 - y0));
        }
        if let Some(t) = attr("gradientTransform") {
            space = space.times(&transform(t));
        }
        let grad = if g.name == "radialGradient" {
            let (cx, cy, r) = (coord("cx", 0.5), coord("cy", 0.5), coord("r", 0.5));
            let (fx, fy) = (coord("fx", cx), coord("fy", cy));
            Gradient { kind: "radial".into(), args: vec![fx, fy, 0.0, cx, cy, r], stops }
        } else {
            let args = vec![coord("x1", 0.0), coord("y1", 0.0), coord("x2", 1.0), coord("y2", 0.0)];
            Gradient { kind: "linear".into(), args, stops }
        };
        Some(Paint::Gradient(grad, space))
    }
}

/// A fill or a stroke as written
fn paint(v: &str) -> Option<Spec> {
    let v = v.trim();
    if v == "none" {
        return None;
    }
    if v == "currentColor" {
        return Some(Spec::Current);
    }
    if let Some(rest) = v.strip_prefix("url(") {
        let (inner, after) = rest.split_once(')')?;
        let id = inner.trim().trim_matches(|c| c == '"' || c == '\'').trim_start_matches('#').to_string();
        let fallback = after.trim();
        let fallback = (!fallback.is_empty()).then(|| paint(fallback)).flatten().map(Box::new);
        return Some(Spec::Url(id, fallback));
    }
    // A colour nobody can read is black, as QtSvg reads it
    Some(Spec::Colour(colour(v).unwrap_or(0xFF00_0000)))
}

/// Color: `#rgb`, `#rrggbb`, `rgb()`, a name
fn colour(v: &str) -> Option<u32> {
    let v = v.trim();
    if v.starts_with('#') && !matches!(v.len(), 4 | 7) {
        return None;
    }
    replay::colour(v)
}

/// A number, or a percentage as a fraction, kept between 0 and 1
fn fraction(v: &str) -> Option<f64> {
    let v = v.trim();
    let n = match v.strip_suffix('%') {
        Some(p) => p.trim().parse::<f64>().ok()? / 100.0,
        None => v.parse::<f64>().ok()?,
    };
    Some(n.clamp(0.0, 1.0))
}

/// Length in user units: a bare number or `px`, `pt`, `mm`, and others
fn length(v: &str) -> Option<f64> {
    let v = v.trim();
    let split = v.find(|c: char| c.is_ascii_alphabetic() || c == '%').unwrap_or(v.len());
    let (n, unit) = v.split_at(split);
    let n: f64 = n.trim().parse().ok()?;
    Some(n * match unit {
        "" | "px" => 1.0,
        "pt" => 1.25,
        "pc" => 15.0,
        "mm" => 3.543307,
        "cm" => 35.43307,
        "in" => 90.0,
        _ => return None,
    })
}

/// Lengths separated by commas and spaces, a dash array's
fn lengths(v: &str) -> Vec<f64> {
    v.split(|c: char| c == ',' || c.is_whitespace()).filter(|s| !s.is_empty()).filter_map(length).collect()
}

/// `transform` attribute; the leftmost applies last
fn transform(v: &str) -> Xf {
    let mut xf = Xf::IDENTITY;
    let mut rest = v;
    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let Some(close) = rest[open..].find(')') else { break };
        let n = path::numbers(&rest[open + 1..open + close]);
        rest = &rest[open + close + 1..];
        let a = |i: usize| n.get(i).copied().unwrap_or(0.0);
        let t = match (name, n.len()) {
            ("matrix", 6) => Xf { a: a(0), b: a(1), c: a(2), d: a(3), tx: a(4), ty: a(5) },
            ("translate", 1) => Xf::translate(a(0), 0.0),
            ("translate", 2) => Xf::translate(a(0), a(1)),
            ("scale", 1) => Xf::scale(a(0), a(0)),
            ("scale", 2) => Xf::scale(a(0), a(1)),
            ("rotate", 1) => Xf::rotate(a(0)),
            ("rotate", 3) => Xf::translate(a(1), a(2)).times(&Xf::rotate(a(0))).times(&Xf::translate(-a(1), -a(2))),
            ("skewX", 1) => Xf { a: 1.0, b: 0.0, c: a(0).to_radians().tan(), d: 1.0, tx: 0.0, ty: 0.0 },
            ("skewY", 1) => Xf { a: 1.0, b: a(0).to_radians().tan(), c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 },
            _ => Xf::IDENTITY,
        };
        xf = xf.times(&t);
    }
    xf
}

#[cfg(test)]
mod tests;
