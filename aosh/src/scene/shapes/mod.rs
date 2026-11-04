// Shape pictures: ShapePaths filled and stroked
//
// Each ShapePath's elements (lines, curves, center arcs, polylines) become a
// Canvas path in item pixels (`canvas/shape.rs`), then the lines and quadratics
// Qt's curve renderer uses (`quads.rs`), covered per the Shape's renderer
// (`cover.rs`): fill, then stroke, in declaration order. A Shape is not clipped
// to its box, so the picture spans what the paths reach, on whole item pixels.
// Rebuilt when its `print` changes (`shapes_are_qts`).

mod cover;
mod quads;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use crate::object::value::Oid;
use crate::object::Core;
use crate::scene::canvas::shape::{arc_segs, Shape as Path};
use crate::scene::canvas::stroke::{self, Cap, Join, Pen};
use crate::scene::colour;
use crate::scene::paint::{Image, Painter, Rect};
use crate::scene::xf::Xf;

#[derive(Default)]
pub struct Shapes {
    made: HashMap<Oid, (u64, Picture)>,
}

/// What a Shape's paths cover, and where its corner is in the item
pub struct Picture {
    pub image: Image,
    pub at: (i32, i32),
}

impl Shapes {
    /// Picture of Shape `oid`, rebuilt when its inputs changed
    pub fn picture(&mut self, core: &Core, oid: Oid) -> Option<&Picture> {
        let paths = paths(core, oid);
        let curve = curve(core, oid);
        let print = print(&paths, curve);
        if self.made.get(&oid).is_none_or(|(p, _)| *p != print) {
            let pic = extent(&paths).map(|(x0, y0, x1, y1)| Picture { image: paint(&paths, curve, (x0, y0), x1 - x0, y1 - y0), at: (x0, y0) });
            match pic {
                Some(pic) => self.made.insert(oid, (print, pic)),
                None => self.made.remove(&oid),
            };
        }
        self.made.get(&oid).map(|(_, pic)| pic)
    }

    /// What it is made of, to tell a new picture from the last one
    pub fn print(&self, core: &Core, oid: Oid) -> u64 {
        print(&paths(core, oid), curve(core, oid))
    }

    /// Forgets the pictures of Shapes that are gone
    pub fn prune(&mut self, core: &Core) {
        self.made.retain(|&o, _| core.0.borrow().objs.is_live(o));
    }
}

/// Implicit size as in Qt: to the right and bottom of what its paths cover from
/// the item's corner, with a visible pen's half width times √2 outward
pub fn implicit(core: &Core, oid: Oid) -> (f64, f64) {
    paths(core, oid).iter().fold((0.0, 0.0), |(w, h), one| {
        let pad = if one.pen().is_some() { one.width / 2.0 * std::f64::consts::SQRT_2 } else { 0.0 };
        points(one).fold((w, h), |(w, h), (x, y)| (f64::max(w, x + pad), f64::max(h, y + pad)))
    })
}

/// Whole item pixels a Shape's paths may cover, including pen reach and the
/// smoothing pixel. `None` when empty
pub fn reach(core: &Core, oid: Oid) -> Option<(f64, f64, f64, f64)> {
    extent(&paths(core, oid)).map(|(x0, y0, x1, y1)| (x0 as f64, y0 as f64, (x1 - x0) as f64, (y1 - y0) as f64))
}

fn extent(paths: &[One]) -> Option<(i32, i32, i32, i32)> {
    let mut b: Option<(f64, f64, f64, f64)> = None;
    for one in paths {
        let pad = 1.0 + one.pen().map_or(0.0, |p| p.width / 2.0 * p.miter_limit.max(std::f64::consts::SQRT_2));
        for (x, y) in points(one) {
            let (l, t, r, bt) = b.unwrap_or((x, y, x, y));
            b = Some((l.min(x - pad), t.min(y - pad), r.max(x + pad), bt.max(y + pad)));
        }
    }
    let (l, t, r, bt) = b?;
    let (x0, y0, x1, y1) = (l.floor() as i32, t.floor() as i32, r.ceil() as i32, bt.ceil() as i32);
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1, y1))
}

/// Path points with curves flattened
fn points(one: &One) -> impl Iterator<Item = (f64, f64)> {
    build(&one.path, (0.0, 0.0)).subs.into_iter().flat_map(|s| s.polyline())
}

/// One ShapePath's values: path, fill, and stroke
#[derive(Debug, PartialEq)]
struct One {
    path: Vec<El>,
    fill: u32,
    winding: bool,
    stroke: u32,
    width: f64,
    cap: i64,
    join: i64,
    miter: f64,
}

#[derive(Debug, PartialEq)]
enum El {
    Move(f64, f64),
    Line(f64, f64),
    Quad(f64, f64, f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    /// Center, radii, start and sweep in degrees, and whether it starts a
    /// subpath
    Arc(f64, f64, f64, f64, f64, f64, bool),
    Poly(Vec<(f64, f64)>),
}

impl One {
    /// Its pen, when it strokes
    fn pen(&self) -> Option<Pen> {
        if self.stroke >> 24 == 0 || self.width <= 0.0 {
            return None;
        }
        let cap = match self.cap { 32 => Cap::Round, 0 => Cap::Butt, _ => Cap::Square };
        let join = match self.join { 128 => Join::Round, 0 => Join::Miter, _ => Join::Bevel };
        Some(Pen { width: self.width, cap, join, miter_limit: self.miter })
    }
}

fn paths(core: &Core, oid: Oid) -> Vec<One> {
    let kids = |o: Oid| core.0.borrow().objs.get(o).map(|x| x.children.clone()).unwrap_or_default();
    let ty = |o: Oid| core.0.borrow().objs.get(o).map(|x| x.def.name).unwrap_or("");
    kids(oid).into_iter().filter(|&p| ty(p) == "ShapePath").map(|p| {
        let mut path = vec![El::Move(core.num(p, "startX"), core.num(p, "startY"))];
        for e in kids(p) {
            let n = |k: &str| core.num(e, k);
            path.push(match ty(e) {
                "PathLine" => El::Line(n("x"), n("y")),
                "PathMove" => El::Move(n("x"), n("y")),
                "PathQuad" => El::Quad(n("controlX"), n("controlY"), n("x"), n("y")),
                "PathCubic" => El::Cubic(n("control1X"), n("control1Y"), n("control2X"), n("control2Y"), n("x"), n("y")),
                "PathAngleArc" => El::Arc(n("centerX"), n("centerY"), n("radiusX"), n("radiusY"),
                                          n("startAngle"), n("sweepAngle"), core.flag(e, "moveToStart")),
                "PathPolyline" => El::Poly(core.text(e, "__points").split_whitespace().filter_map(|xy| {
                    let (x, y) = xy.split_once(',')?;
                    Some((x.parse().ok()?, y.parse().ok()?))
                }).collect()),
                _ => continue,
            });
        }
        One {
            path,
            fill: colour(&core.text(p, "fillColor")),
            winding: core.num(p, "fillRule") as i64 == 1,
            stroke: colour(&core.text(p, "strokeColor")),
            width: core.num(p, "strokeWidth"),
            cap: core.num(p, "capStyle") as i64,
            join: core.num(p, "joinStyle") as i64,
            miter: core.num(p, "miterLimit"),
        }
    }).collect()
}

/// Whether the Shape uses `Shape.CurveRenderer`; otherwise the geometry
/// renderer, Qt's default
fn curve(core: &Core, oid: Oid) -> bool {
    core.num(oid, "preferredRendererType") as i64 == 4
}

fn print(paths: &[One], curve: bool) -> u64 {
    let mut s = std::collections::hash_map::DefaultHasher::new();
    format!("{paths:?}").hash(&mut s);
    curve.hash(&mut s);
    s.finish()
}

/// The elements as the Canvas's path, in the item's pixels less `at`
fn build(els: &[El], at: (f64, f64)) -> Path {
    let mut p = Path::default();
    let xf = Xf::translate(-at.0, -at.1);
    let m = |x: f64, y: f64| xf.map(x, y);
    for e in els {
        match *e {
            El::Move(x, y) => p.move_to(m(x, y)),
            El::Line(x, y) => p.line_to(m(x, y)),
            El::Quad(cx, cy, x, y) => {
                let (c, to) = (m(cx, cy), m(x, y));
                let from = p.current().unwrap_or(c);
                let c1 = (from.0 + 2.0 / 3.0 * (c.0 - from.0), from.1 + 2.0 / 3.0 * (c.1 - from.1));
                let c2 = (to.0 + 2.0 / 3.0 * (c.0 - to.0), to.1 + 2.0 / 3.0 * (c.1 - to.1));
                p.cubic_to(c1, c2, to);
            }
            El::Cubic(c1x, c1y, c2x, c2y, x, y) => p.cubic_to(m(c1x, c1y), m(c2x, c2y), m(x, y)),
            El::Arc(cx, cy, rx, ry, start, sweep, own) => {
                let a0 = start.to_radians();
                let sweep = sweep.to_radians().clamp(-std::f64::consts::TAU, std::f64::consts::TAU);
                // Exact start at circle quarters, as in Qt: 23, not
                // 23.000000000000004
                let (cos, sin) = turn(start);
                let first = m(cx + rx * cos, cy + ry * sin);
                if own || p.current().is_none() {
                    p.move_to(first);
                } else {
                    p.line_to(first);
                }
                if sweep != 0.0 && rx > 0.0 && ry > 0.0 {
                    if let Some(s) = p.subs.last_mut() {
                        arc_segs(&mut s.segs, &xf, (cx, cy), (rx, ry), a0, sweep);
                    }
                }
            }
            El::Poly(ref pts) => {
                for (k, &(x, y)) in pts.iter().enumerate() {
                    if k == 0 { p.move_to(m(x, y)) } else { p.line_to(m(x, y)) }
                }
            }
        }
    }
    p
}

/// The cosine and sine of `degrees`, exact on the quarters
fn turn(degrees: f64) -> (f64, f64) {
    match degrees.rem_euclid(360.0) {
        0.0 => (1.0, 0.0),
        90.0 => (0.0, 1.0),
        180.0 => (-1.0, 0.0),
        270.0 => (0.0, -1.0),
        _ => (degrees.to_radians().cos(), degrees.to_radians().sin()),
    }
}

/// Fills then strokes every path over the last, on a `w` by `h` picture with
/// its corner at `at` in the item
fn paint(paths: &[One], curve: bool, at: (i32, i32), w: i32, h: i32) -> Image {
    let mut img = Image::new(w, h);
    let all = Rect::new(0, 0, w, h);
    let (uw, uh) = (w as usize, h as usize);
    for one in paths {
        let path = build(&one.path, (at.0 as f64, at.1 as f64));
        let runs = if curve { quads::runs(&path.subs) } else { quads::lines(&path.subs) };
        if runs.is_empty() {
            continue;
        }
        if one.fill >> 24 != 0 {
            let cov = cover::fill(&runs, one.winding, curve, uw, uh);
            Painter::new(&mut img, all).mask(0, 0, &cov, uw, uh, 0, one.fill, 1.0);
        }
        if let Some(pen) = one.pen() {
            let cov = if curve {
                cover::stroke(&runs, &pen, uw, uh)
            } else {
                // The pen's outline, filled without smoothing
                let outline = stroke::outline(&path, &pen);
                let pieces: Vec<_> = quads::lines(&outline.subs).into_iter().flat_map(|r| r.pieces).collect();
                cover::solid(&pieces, uw, uh)
            };
            Painter::new(&mut img, all).mask(0, 0, &cov, uw, uh, 0, one.stroke, 1.0);
        }
    }
    img
}
