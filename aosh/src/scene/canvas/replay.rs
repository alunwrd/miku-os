// Replays context calls into a Canvas's picture
//
// State follows the HTML canvas: styles, line width, caps, joins, dash, global
// alpha, and matrix, saved and restored together; the path is not saved. Fills
// and strokes draw antialiased coverage in a color or gradient times global
// alpha. Text and images are not implemented.

use super::shape::Shape;
use super::stroke::{self, Cap, Join, Pen};
use super::thin;
use crate::scene::paint::{Image, Painter, Rect};
use crate::scene::xf::Xf;

/// One argument of a call, as the context put it on its list
#[derive(Clone, Debug)]
pub enum Arg {
    Num(f64),
    Str(String),
    List(Vec<f64>),
    Gradient(Gradient),
}

impl Arg {
    fn num(&self) -> f64 {
        match self {
            Arg::Num(n) => *n,
            Arg::Str(s) => s.trim().parse().unwrap_or(f64::NAN),
            _ => f64::NAN,
        }
    }
}

/// A call: name and arguments
pub type Op = (String, Vec<Arg>);

/// Gradient at the time it became a style: a linear one's line, and stops as
/// straight `0xAARRGGBB`, in order
#[derive(Clone, Debug)]
pub struct Gradient {
    pub kind: String,
    pub args: Vec<f64>,
    pub stops: Vec<(f64, u32)>,
}

/// Fill or stroke paint: straight `0xAARRGGBB`, or a gradient in `Xf`'s source
/// coordinates
#[derive(Clone, Debug)]
pub enum Paint {
    Colour(u32),
    Gradient(Gradient, Xf),
}

#[derive(Clone, Debug)]
struct State {
    xf: Xf,
    fill: Paint,
    stroke: Paint,
    width: f64,
    cap: Cap,
    join: Join,
    miter: f64,
    alpha: f64,
    dash: Vec<f64>,
    dash_offset: f64,
    even_odd: bool,
}

impl Default for State {
    fn default() -> State {
        State {
            xf: Xf::IDENTITY,
            fill: Paint::Colour(0xFF00_0000),
            stroke: Paint::Colour(0xFF00_0000),
            width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter: 10.0,
            alpha: 1.0,
            dash: Vec::new(),
            dash_offset: 0.0,
            even_odd: false,
        }
    }
}

/// Context state and path, kept between paints
#[derive(Default)]
pub struct Replay {
    state: State,
    saved: Vec<State>,
    path: Shape,
}

/// Parses a canvas color: Qt forms (`#rgb`, `#rrggbb`, `#aarrggbb`, a name) or
/// CSS `rgb()`, `rgba()`, `hsl()`, `hsla()`. Returns straight `0xAARRGGBB`
pub fn colour(s: &str) -> Option<u32> {
    let t = s.trim();
    let n = crate::object::value::normalise_colour(t);
    if let Some(h) = n.strip_prefix('#').filter(|h| h.len() == 8) {
        return u32::from_str_radix(h, 16).ok();
    }
    let lower = t.to_ascii_lowercase();
    let (name, rest) = lower.split_once('(')?;
    let inner = rest.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    let num = |p: &str| -> Option<f64> {
        match p.strip_suffix('%') {
            Some(v) => v.trim().parse::<f64>().ok().map(|v| v / 100.0),
            None => p.parse().ok(),
        }
    };
    let alpha = |i: usize| parts.get(i).and_then(|p| num(p)).unwrap_or(1.0).clamp(0.0, 1.0);
    let byte = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    let (r, g, b, a) = match name.trim() {
        "rgb" | "rgba" if parts.len() >= 3 => {
            let ch = |p: &str| -> Option<f64> {
                if p.ends_with('%') { num(p) } else { p.parse::<f64>().ok().map(|v| v / 255.0) }
            };
            (ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, alpha(3))
        }
        "hsl" | "hsla" if parts.len() >= 3 => {
            let h = parts[0].parse::<f64>().ok()?.rem_euclid(360.0) / 360.0;
            let (s, l) = (num(parts[1])?.clamp(0.0, 1.0), num(parts[2])?.clamp(0.0, 1.0));
            let q = if l < 0.5 { l * (1.0 + s) } else { l + s - l * s };
            let p = 2.0 * l - q;
            let hue = |t: f64| {
                let t = t.rem_euclid(1.0);
                if t < 1.0 / 6.0 { p + (q - p) * 6.0 * t } else if t < 0.5 { q } else if t < 2.0 / 3.0 { p + (q - p) * (2.0 / 3.0 - t) * 6.0 } else { p }
            };
            (hue(h + 1.0 / 3.0), hue(h), hue(h - 1.0 / 3.0), alpha(3))
        }
        _ => return None,
    };
    Some(byte(a) << 24 | byte(r) << 16 | byte(g) << 8 | byte(b))
}

impl Replay {
    /// Back to a new context's state, the path empty
    pub fn reset(&mut self) {
        *self = Replay::default();
    }

    /// `ops` into `img`
    pub fn play(&mut self, ops: &[Op], img: &mut Image) {
        for (name, args) in ops {
            self.one(name, args, img);
        }
    }

    fn style(&self, a: Option<&Arg>) -> Option<Paint> {
        match a? {
            Arg::Str(s) => colour(s).map(Paint::Colour),
            Arg::Gradient(g) => Some(Paint::Gradient(g.clone(), self.state.xf)),
            _ => None,
        }
    }

    fn one(&mut self, name: &str, a: &[Arg], img: &mut Image) {
        let n = |i: usize| a.get(i).map(Arg::num).unwrap_or(f64::NAN);
        let ok = |k: usize| (0..k).all(|i| n(i).is_finite());
        let st = &mut self.state;
        match name {
            "fillStyle" => {
                if let Some(s) = self.style(a.first()) {
                    self.state.fill = s;
                }
            }
            "strokeStyle" => {
                if let Some(s) = self.style(a.first()) {
                    self.state.stroke = s;
                }
            }
            "lineWidth" if n(0).is_finite() && n(0) > 0.0 => st.width = n(0),
            "miterLimit" if n(0).is_finite() && n(0) > 0.0 => st.miter = n(0),
            "globalAlpha" if (0.0..=1.0).contains(&n(0)) => st.alpha = n(0),
            "lineDashOffset" if n(0).is_finite() => st.dash_offset = n(0),
            "fillRule" => st.even_odd = n(0) as i64 == 0,
            "lineCap" => {
                if let Some(Arg::Str(s)) = a.first() {
                    st.cap = match s.as_str() { "round" => Cap::Round, "square" => Cap::Square, "butt" => Cap::Butt, _ => st.cap };
                }
            }
            "lineJoin" => {
                if let Some(Arg::Str(s)) = a.first() {
                    st.join = match s.as_str() { "round" => Join::Round, "bevel" => Join::Bevel, "miter" => Join::Miter, _ => st.join };
                }
            }
            "setLineDash" => {
                if let Some(Arg::List(d)) = a.first() {
                    if d.iter().all(|v| v.is_finite() && *v >= 0.0) {
                        // An odd list is the list twice, as the canvas has it
                        let mut d = d.clone();
                        if d.len() % 2 == 1 {
                            d.extend(d.clone());
                        }
                        st.dash = d;
                    }
                }
            }
            "save" => self.saved.push(self.state.clone()),
            "restore" => {
                if let Some(s) = self.saved.pop() {
                    self.state = s;
                }
            }
            "reset" => {
                self.reset();
                img.clear(img.bounds());
            }
            "scale" if ok(2) => st.xf = st.xf.times(&Xf::scale(n(0), n(1))),
            "rotate" if ok(1) => st.xf = st.xf.times(&Xf::rotate(n(0).to_degrees())),
            "translate" if ok(2) => st.xf = st.xf.times(&Xf::translate(n(0), n(1))),
            "transform" if ok(6) => st.xf = st.xf.times(&Xf { a: n(0), b: n(1), c: n(2), d: n(3), tx: n(4), ty: n(5) }),
            "setTransform" if ok(6) => st.xf = Xf { a: n(0), b: n(1), c: n(2), d: n(3), tx: n(4), ty: n(5) },
            "resetTransform" => st.xf = Xf::IDENTITY,
            "beginPath" => self.path.clear(),
            "closePath" => self.path.close(),
            "moveTo" if ok(2) => self.path.move_to(st.xf.map(n(0), n(1))),
            "lineTo" if ok(2) => self.path.line_to(st.xf.map(n(0), n(1))),
            "bezierCurveTo" if ok(6) => {
                let xf = st.xf;
                self.path.cubic_to(xf.map(n(0), n(1)), xf.map(n(2), n(3)), xf.map(n(4), n(5)));
            }
            "quadraticCurveTo" if ok(4) => {
                // Quadratic as a cubic: each handle two thirds of the way from
                // an end to the control point
                let xf = st.xf;
                let from = self.path.current().and_then(|c| xf.inverse().map(|i| i.map(c.0, c.1))).unwrap_or((n(0), n(1)));
                let (q, to) = ((n(0), n(1)), (n(2), n(3)));
                let c1 = (from.0 + 2.0 / 3.0 * (q.0 - from.0), from.1 + 2.0 / 3.0 * (q.1 - from.1));
                let c2 = (to.0 + 2.0 / 3.0 * (q.0 - to.0), to.1 + 2.0 / 3.0 * (q.1 - to.1));
                self.path.cubic_to(xf.map(c1.0, c1.1), xf.map(c2.0, c2.1), xf.map(to.0, to.1));
            }
            "arc" if ok(5) => {
                let acw = matches!(a.get(5), Some(Arg::Num(v)) if *v != 0.0) || matches!(a.get(5), Some(Arg::Str(s)) if s == "true");
                let xf = st.xf;
                self.path.arc(&xf, n(0), n(1), n(2).max(0.0), n(3), n(4), acw);
            }
            "arcTo" if ok(5) => {
                let xf = st.xf;
                let inv = xf.inverse().unwrap_or(Xf::IDENTITY);
                self.path.arc_to(&xf, &inv, (n(0), n(1)), (n(2), n(3)), n(4));
            }
            "rect" if ok(4) => {
                let xf = st.xf;
                rect(&mut self.path, &xf, n(0), n(1), n(2), n(3));
            }
            // Qt extension: rectangle with corner radii `xr` by `yr`
            "roundedRect" if ok(6) => {
                let xf = st.xf;
                let (x, y, w, h, rx, ry) = (n(0), n(1), n(2), n(3), n(4).min(n(2) / 2.0), n(5).min(n(3) / 2.0));
                let pi = std::f64::consts::PI;
                self.path.move_to(xf.map(x + rx, y));
                self.path.ellipse_arc(&xf, x + w - rx, y + ry, rx, ry, -pi / 2.0, 0.0, false);
                self.path.ellipse_arc(&xf, x + w - rx, y + h - ry, rx, ry, 0.0, pi / 2.0, false);
                self.path.ellipse_arc(&xf, x + rx, y + h - ry, rx, ry, pi / 2.0, pi, false);
                self.path.ellipse_arc(&xf, x + rx, y + ry, rx, ry, pi, pi * 1.5, false);
                self.path.close();
            }
            // Qt extension: ellipse inside `x, y, w, h`
            "ellipse" if ok(4) => {
                let xf = st.xf;
                let (rx, ry) = (n(2) / 2.0, n(3) / 2.0);
                self.path.move_to(xf.map(n(0) + n(2), n(1) + ry));
                self.path.ellipse_arc(&xf, n(0) + rx, n(1) + ry, rx, ry, 0.0, std::f64::consts::TAU, false);
                self.path.close();
            }
            "fill" => {
                let path = self.path.clone();
                self.fill(&path, img);
            }
            "stroke" => {
                let path = self.path.clone();
                self.stroke(&path, img);
            }
            "fillRect" if ok(4) => {
                let mut p = Shape::default();
                rect(&mut p, &st.xf.clone(), n(0), n(1), n(2), n(3));
                self.fill(&p, img);
            }
            "strokeRect" if ok(4) => {
                let mut p = Shape::default();
                rect(&mut p, &st.xf.clone(), n(0), n(1), n(2), n(3));
                self.stroke(&p, img);
            }
            "clearRect" if ok(4) => {
                let mut p = Shape::default();
                rect(&mut p, &st.xf.clone(), n(0), n(1), n(2), n(3));
                let (w, h) = (img.w as usize, img.h as usize);
                let cov = p.ft().fill(false, w, h);
                for (d, c) in img.px.iter_mut().zip(&cov) {
                    if *c == 255 {
                        *d = 0;
                    } else if *c > 0 {
                        let keep = 255 - *c as u32;
                        let ch = |s: u32| ((((*d >> s) & 0xff) * keep + 127) / 255) << s;
                        *d = ch(24) | ch(16) | ch(8) | ch(0);
                    }
                }
            }
            _ => {}
        }
    }

    fn fill(&self, path: &Shape, img: &mut Image) {
        fill(img, path, self.state.even_odd, &self.state.fill, self.state.alpha);
    }

    /// Strokes the path at the matrix-scaled width. Dash lengths are in line
    /// widths, as in Qt's pen
    fn stroke(&self, path: &Shape, img: &mut Image) {
        let st = &self.state;
        let k = (st.xf.a * st.xf.d - st.xf.b * st.xf.c).abs().sqrt();
        let width = st.width * k;
        let pen = Pen { width, cap: st.cap, join: st.join, miter_limit: st.miter };
        let dash: Vec<f64> = st.dash.iter().map(|v| v * width).collect();
        stroke(img, path, &pen, &dash, st.dash_offset * width, &st.stroke, st.alpha);
    }
}

/// Fills `path` with the even-odd or nonzero rule, in `paint` at `alpha`. Used
/// by Canvas `fill` and SVG
pub fn fill(img: &mut Image, path: &Shape, even_odd: bool, paint: &Paint, alpha: f64) {
    let (w, h) = (img.w as usize, img.h as usize);
    let cov = path.ft().fill(even_odd, w, h);
    put(img, &cov, paint, alpha);
}

/// Strokes `path` with `pen`, width and dashes in picture scale: walked at a
/// pixel or less, otherwise its outline filled
pub fn stroke(img: &mut Image, path: &Shape, pen: &Pen, dash: &[f64], dash_offset: f64, paint: &Paint, alpha: f64) {
    if pen.width <= 0.0 {
        return;
    }
    let (w, h) = (img.w as usize, img.h as usize);
    let dashed;
    let path = if dash.iter().sum::<f64>() > 0.0 {
        dashed = path.dashed(dash, dash_offset);
        &dashed
    } else {
        path
    };
    if pen.width <= 1.0 {
        // A thin line is one pixel wide at an opacity equal to its width, in
        // 256ths (`svgs_are_qts`)
        let opacity = (256.0 * pen.width).floor().clamp(0.0, 256.0) as u32;
        put(img, &thin::coverage(path, pen.cap, opacity, w, h), paint, alpha);
    } else {
        put(img, &stroke::outline(path, pen).ft().fill(false, w, h), paint, alpha);
    }
}

/// The rectangle `x, y, w, h` through `xf`, as a closed subpath
fn rect(p: &mut Shape, xf: &Xf, x: f64, y: f64, w: f64, h: f64) {
    p.move_to(xf.map(x, y));
    p.line_to(xf.map(x + w, y));
    p.line_to(xf.map(x + w, y + h));
    p.line_to(xf.map(x, y + h));
    p.close();
}

/// Coverage in a style over the picture
fn put(img: &mut Image, cov: &[u8], style: &Paint, alpha: f64) {
    let (w, h) = (img.w as usize, img.h as usize);
    let all = Rect::new(0, 0, img.w, img.h);
    match style {
        Paint::Colour(c) => Painter::new(img, all).mask(0, 0, cov, w, h, 0, *c, alpha as f32),
        Paint::Gradient(g, xf) => {
            let inv = xf.inverse().unwrap_or(Xf::IDENTITY);
            Painter::new(img, all).mask_shaded(0, 0, cov, w, h, alpha as f32, |x, y| {
                let (ux, uy) = inv.map(x as f64 + 0.5, y as f64 + 0.5);
                gradient_at(g, ux, uy)
            });
        }
    }
}

/// Gradient color at a point in context coordinates: along a linear line, out
/// from a radial's first circle, around a conical center; padded past the ends
fn gradient_at(g: &Gradient, x: f64, y: f64) -> u32 {
    if g.stops.is_empty() {
        return 0;
    }
    let a = |i: usize| g.args.get(i).copied().unwrap_or(0.0);
    let t = match g.kind.as_str() {
        "radial" => {
            let (dx, dy) = (x - a(3), y - a(4));
            let r = (dx * dx + dy * dy).sqrt();
            if a(5) - a(2) == 0.0 { 0.0 } else { (r - a(2)) / (a(5) - a(2)) }
        }
        "conical" => {
            let ang = (-(y - a(1))).atan2(x - a(0)).to_degrees() - a(2);
            ang.rem_euclid(360.0) / 360.0
        }
        _ => {
            let (dx, dy) = (a(2) - a(0), a(3) - a(1));
            let l = dx * dx + dy * dy;
            if l == 0.0 { 0.0 } else { ((x - a(0)) * dx + (y - a(1)) * dy) / l }
        }
    };
    let t = t.clamp(0.0, 1.0);
    let s = &g.stops;
    if t <= s[0].0 {
        return s[0].1;
    }
    for w in s.windows(2) {
        let ((p0, c0), (p1, c1)) = (w[0], w[1]);
        if t <= p1 {
            let f = if p1 > p0 { (t - p0) / (p1 - p0) } else { 1.0 };
            let mix = |sh: u32| {
                let (u, v) = (((c0 >> sh) & 0xff) as f64, ((c1 >> sh) & 0xff) as f64);
                ((u + (v - u) * f).round() as u32) << sh
            };
            return mix(24) | mix(16) | mix(8) | mix(0);
        }
    }
    s[s.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn css_and_qt_colours_read_alike() {
        assert_eq!(colour("#3060c0"), Some(0xFF3060C0));
        assert_eq!(colour("#6040a080"), Some(0x6040A080), "Qt's #aarrggbb");
        assert_eq!(colour("rgba(255, 0, 0, 0.5)"), Some(0x80FF0000));
        assert_eq!(colour("rgb(0,128,255)"), Some(0xFF0080FF));
        assert_eq!(colour("red"), Some(0xFFFF0000));
        assert_eq!(colour("hsl(120, 100%, 50%)"), Some(0xFF00FF00));
        assert_eq!(colour("nonsense"), None);
    }

    #[test]
    fn a_filled_rect_is_its_colour_and_clear_rect_takes_it_away() {
        let mut img = Image::new(10, 10);
        let mut r = Replay::default();
        let op = |n: &str, a: &[f64]| (n.to_string(), a.iter().map(|v| Arg::Num(*v)).collect::<Vec<_>>());
        r.play(&[("fillStyle".into(), vec![Arg::Str("#ff0000".into())]), op("fillRect", &[2.0, 2.0, 6.0, 6.0])], &mut img);
        assert_eq!(img.px[5 * 10 + 5], 0xFFFF0000);
        assert_eq!(img.px[1 * 10 + 5], 0);
        r.play(&[op("clearRect", &[0.0, 0.0, 10.0, 10.0])], &mut img);
        assert!(img.px.iter().all(|p| *p == 0));
    }
}
