// Item-to-window transforms
//
// Parent transform as in QQuickItemPrivate::itemToParentTransform, last written
// applied first:
// - `x` and `y`;
// - the `transform` list (Scale, Rotation, Translate), first entry applied
//   first;
// - `scale` and `rotation` about `transformOrigin` (default center).
//
// Angles are degrees clockwise on screen. Quarter turns are exact, as in Qt.

use crate::object::value::Oid;
use crate::object::Core;

/// `[a c tx; b d ty]`: (x, y) maps to (a x + c y + tx, b x + d y + ty)
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Xf {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub tx: f64,
    pub ty: f64,
}

impl Default for Xf {
    fn default() -> Xf {
        Xf::IDENTITY
    }
}

impl Xf {
    pub const IDENTITY: Xf = Xf { a: 1.0, b: 0.0, c: 0.0, d: 1.0, tx: 0.0, ty: 0.0 };

    pub fn translate(x: f64, y: f64) -> Xf {
        Xf { tx: x, ty: y, ..Xf::IDENTITY }
    }

    pub fn scale(sx: f64, sy: f64) -> Xf {
        Xf { a: sx, d: sy, ..Xf::IDENTITY }
    }

    /// `deg` degrees clockwise on the screen, a quarter turn exact
    pub fn rotate(deg: f64) -> Xf {
        let (s, c) = match deg.rem_euclid(360.0) {
            0.0 => (0.0, 1.0),
            90.0 => (1.0, 0.0),
            180.0 => (0.0, -1.0),
            270.0 => (-1.0, 0.0),
            _ => deg.to_radians().sin_cos(),
        };
        Xf { a: c, b: s, c: -s, d: c, tx: 0.0, ty: 0.0 }
    }

    /// `self · o`: `o` applies first
    pub fn times(&self, o: &Xf) -> Xf {
        Xf {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            tx: self.a * o.tx + self.c * o.ty + self.tx,
            ty: self.b * o.tx + self.d * o.ty + self.ty,
        }
    }

    pub fn map(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.tx, self.b * x + self.d * y + self.ty)
    }

    pub fn inverse(&self) -> Option<Xf> {
        let det = self.a * self.d - self.b * self.c;
        if det.abs() < 1e-12 {
            return None;
        }
        let (a, b, c, d) = (self.d / det, -self.b / det, -self.c / det, self.a / det);
        Some(Xf { a, b, c, d, tx: -(a * self.tx + c * self.ty), ty: -(b * self.tx + d * self.ty) })
    }

    /// Scale of a shape-preserving matrix (translation, rotation, mirror,
    /// uniform scale), or none for stretch or shear. Qt's raster engine draws
    /// thin pens only through the former (`svg/mod.rs`)
    pub fn similarity(&self) -> Option<f64> {
        let same = |x: f64, y: f64| x == y || (x - y).abs() <= 1e-12 * x.abs().max(y.abs());
        let keeps = (same(self.a, self.d) && same(self.b, -self.c)) || (same(self.a, -self.d) && same(self.b, self.c));
        keeps.then(|| (self.a * self.d - self.b * self.c).abs().sqrt())
    }

    /// Pure translation, for the painter's fast paths
    pub fn is_translation(&self) -> bool {
        self.a == 1.0 && self.b == 0.0 && self.c == 0.0 && self.d == 1.0
    }

    /// The box, in whole pixels, a local rectangle covers once moved
    pub fn bounds(&self, x: f64, y: f64, w: f64, h: f64) -> super::paint::Rect {
        let corners = [self.map(x, y), self.map(x + w, y), self.map(x, y + h), self.map(x + w, y + h)];
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (px, py) in corners {
            x0 = x0.min(px);
            y0 = y0.min(py);
            x1 = x1.max(px);
            y1 = y1.max(py);
        }
        super::paint::Rect::covering(x0, y0, x1 - x0, y1 - y0)
    }
}

/// `transformOrigin` point of a `w` by `h` item: Item.TopLeft 0 to
/// Item.BottomRight 8, default Item.Center 4
fn origin(which: i64, w: f64, h: f64) -> (f64, f64) {
    match which {
        0 => (0.0, 0.0),
        1 => (w / 2.0, 0.0),
        2 => (w, 0.0),
        3 => (0.0, h / 2.0),
        5 => (w, h / 2.0),
        6 => (0.0, h),
        7 => (w / 2.0, h),
        8 => (w, h),
        _ => (w / 2.0, h / 2.0),
    }
}

/// One of an item's `transform` list
fn step(core: &Core, t: Oid) -> Xf {
    let s = core.0.borrow();
    let Some(o) = s.objs.get(t) else { return Xf::IDENTITY };
    let name = o.def.name;
    drop(s);
    let n = |p: &str| core.num(t, p);
    let about = |m: Xf| {
        let (ox, oy) = (n("origin.x"), n("origin.y"));
        Xf::translate(ox, oy).times(&m).times(&Xf::translate(-ox, -oy))
    };
    match name {
        "Translate" => Xf::translate(n("x"), n("y")),
        "Scale" => about(Xf::scale(n("xScale"), n("yScale"))),
        // Rotation about the screen axis only; x or y axes are perspective and
        // not drawable
        "Rotation" if n("axis.x") == 0.0 && n("axis.y") == 0.0 && n("axis.z") != 0.0 => {
            about(Xf::rotate(n("angle") * n("axis.z").signum()))
        }
        _ => Xf::IDENTITY,
    }
}

/// An item's transform to its parent's coordinates
pub fn to_parent(core: &Core, oid: Oid) -> Xf {
    let mut m = Xf::translate(core.num(oid, "x"), core.num(oid, "y"));
    if let Some(list) = core.peek_named(oid, "transform").and_then(|v| v.as_list()) {
        // The last of the list is the nearest the item's place
        for t in list.iter().rev().filter_map(|v| v.as_obj()) {
            m = m.times(&step(core, t));
        }
    }
    let (scale, rotation) = (core.num(oid, "scale"), core.num(oid, "rotation"));
    if scale != 1.0 || rotation != 0.0 {
        let (ox, oy) = origin(core.num(oid, "transformOrigin") as i64, core.num(oid, "width"), core.num(oid, "height"));
        m = m.times(&Xf::translate(ox, oy)).times(&Xf::scale(scale, scale)).times(&Xf::rotate(rotation)).times(&Xf::translate(-ox, -oy));
    }
    m
}

/// Item-to-window transform through all ancestors, up to and including the
/// window
pub fn to_window(core: &Core, mut oid: Oid) -> Xf {
    let mut m = Xf::IDENTITY;
    for _ in 0..256 {
        let (is_item, is_window, parent) = {
            let s = core.0.borrow();
            let Some(o) = s.objs.get(oid) else { break };
            (crate::types::is_a(o.def, "Item"), o.def.kind == crate::types::Kind::Window, o.parent)
        };
        if !is_item || is_window {
            break;
        }
        m = to_parent(core, oid).times(&m);
        match parent {
            Some(p) => oid = p,
            None => break,
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_compose_as_qt_puts_them() {
        // Quarter turn about the middle of a 10 by 4 item at (100, 50)
        let m = Xf::translate(100.0, 50.0).times(&Xf::translate(5.0, 2.0)).times(&Xf::rotate(90.0)).times(&Xf::translate(-5.0, -2.0));
        assert_eq!(m.map(0.0, 0.0), (107.0, 47.0));
        assert_eq!(m.map(10.0, 0.0), (107.0, 57.0));
        let back = m.inverse().unwrap().map(107.0, 57.0);
        assert!((back.0 - 10.0).abs() < 1e-9 && back.1.abs() < 1e-9);
        assert!(Xf::translate(3.0, 4.0).is_translation() && !Xf::rotate(30.0).is_translation());
    }
}
