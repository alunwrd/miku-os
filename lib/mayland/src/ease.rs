// Easing: one cubic bezier, evaluated the same way on both sides of the
// protocol.
//
// `shell::anim` owns the classes, their curves, and durations. This file turns
// a fraction of a duration into a fraction of a distance.
//
// Hyprland's form: two control points, with endpoints at (0,0) and (1,1),
// written `bezier = name, x0, y0, x1, y1`.

/// Number of cached samples per curve, as in hyprutils. Generated once and
/// interpolated during lookup
const BAKED: usize = 255;

#[derive(Clone, Copy)]
pub struct Curve {
    /// Two control points. The endpoints are always (0,0) and (1,1).
    p1: (f32, f32),
    p2: (f32, f32),
    /// Precomputed curve samples.
    xs: [f32; BAKED],
    ys: [f32; BAKED],
    set: bool,
}

impl Curve {
    pub const LINEAR: Curve = Curve {
        p1: (0.0, 0.0),
        p2: (1.0, 1.0),
        xs: [0.0; BAKED],
        ys: [0.0; BAKED],
        set: false,
    };

    /// Bake a curve from its two control points
    pub fn new(p1: (f32, f32), p2: (f32, f32)) -> Curve {
        let mut c = Curve { p1, p2, xs: [0.0; BAKED], ys: [0.0; BAKED], set: true };
        for i in 0..BAKED {
            // Skip t = 0 since the starting point is always (0,0)
            let t = (i + 1) as f32 / BAKED as f32;
            c.xs[i] = c.at(t, c.p1.0, c.p2.0);
            c.ys[i] = c.at(t, c.p1.1, c.p2.1);
        }
        c
    }

    /// One coordinate of the cubic at t, with the endpoints fixed at 0 and 1
    fn at(&self, t: f32, a: f32, b: f32) -> f32 {
        let u = 1.0 - t;
        // Endpoint terms omitted since P0 = 0 and P3 = 1
        3.0 * t * u * u * a + 3.0 * t * t * u * b + t * t * t
    }

    /// How far along the animation is at fraction `x` of its duration
    pub fn y_at(&self, x: f32) -> f32 {
        if !self.set {
            // Unset curves default to linear interpolation.
            return x.clamp(0.0, 1.0);
        }
        if x <= 0.0 {
            return 0.0;
        }
        if x >= 1.0 {
            return 1.0;
        }
        // X samples are sorted, so binary search finds the surrounding points
        // and interpolation gives the value
        let (mut lo, mut hi) = (0usize, BAKED - 1);
        while lo < hi {
            let mid = (lo + hi) / 2;
            if self.xs[mid] < x {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == 0 {
            // Between the origin and the first baked point
            let (x1, y1) = (self.xs[0], self.ys[0]);
            if x1 <= 0.0 {
                return y1;
            }
            return y1 * (x / x1);
        }
        let (x0, y0) = (self.xs[lo - 1], self.ys[lo - 1]);
        let (x1, y1) = (self.xs[lo], self.ys[lo]);
        let span = x1 - x0;
        if span <= 0.0 {
            return y1;
        }
        y0 + (y1 - y0) * ((x - x0) / span)
    }
}

// Curves a config need not name. Constructors, for use by any program with no
// table to initialize

/// Fast away, long settle: `easeOutExpo` as Qt draws it, used by the reference
/// shell for panels and rows
pub fn out_expo() -> Curve {
    Curve::new((0.16, 1.0), (0.3, 1.0))
}

/// Decelerating with an overshoot: `easeOutBack` with Qt's overshoot of 1.5.
/// The first control point's y is above 1, so a reader must handle a value
/// above the target
pub fn out_back() -> Curve {
    Curve::new((0.34, 1.56), (0.64, 1.0))
}

/// Accelerating away, for something leaving
pub fn in_expo() -> Curve {
    Curve::new((0.64, 0.0), (0.78, 0.0))
}
