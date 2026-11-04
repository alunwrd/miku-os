// Exact Qt easing curves ported 1:1 from `qeasingcurve.cpp` (Penner's equations
// with Qt's specific constants, not Bézier approximations).
// Numeric IDs match `QEasingCurve::Type` exposed by the prelude's `Easing`
// object
use std::f64::consts::{FRAC_PI_2, PI};

// Custom Bézier spline defined by the file's control points (`bezier`)
pub const BEZIER_SPLINE: i64 = 45;

pub struct Params {
    pub amplitude: f64,
    pub period: f64,
    pub overshoot: f64,
}

/// Progress `t` (0..1) through curve `kind`
pub fn ease(kind: i64, t: f64, p: &Params) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let (a, per, s) = (p.amplitude, p.period, p.overshoot);
    match kind {
        1 => in_quad(t),
        2 => out_quad(t),
        3 => in_out_quad(t),
        4 => out_in(t, out_quad, in_quad),
        5 => in_cubic(t),
        6 => out_cubic(t),
        7 => in_out_cubic(t),
        8 => out_in(t, out_cubic, in_cubic),
        9 => in_quart(t),
        10 => out_quart(t),
        11 => in_out_quart(t),
        12 => out_in(t, out_quart, in_quart),
        13 => in_quint(t),
        14 => out_quint(t),
        15 => in_out_quint(t),
        16 => out_in(t, out_quint, in_quint),
        17 => in_sine(t),
        18 => out_sine(t),
        19 => -0.5 * ((PI * t).cos() - 1.0),
        20 => out_in(t, out_sine, in_sine),
        21 => in_expo(t),
        22 => out_expo(t),
        23 => in_out_expo(t),
        24 => out_in(t, out_expo, in_expo),
        25 => in_circ(t),
        26 => out_circ(t),
        27 => in_out_circ(t),
        28 => out_in(t, out_circ, in_circ),
        29 => in_elastic_h(t, 0.0, 1.0, 1.0, a, per),
        30 => out_elastic_h(t, 1.0, a, per),
        31 => in_out_elastic(t, a, per),
        32 => {
            if t < 0.5 {
                out_elastic_h(t * 2.0, 0.5, a, per)
            } else {
                in_elastic_h(2.0 * t - 1.0, 0.5, 0.5, 1.0, a, per)
            }
        }
        33 => in_back(t, s),
        34 => out_back(t, s),
        35 => in_out_back(t, s),
        36 => {
            if t < 0.5 {
                out_back(2.0 * t, s) / 2.0
            } else {
                in_back(2.0 * t - 1.0, s) / 2.0 + 0.5
            }
        }
        37 => in_bounce(t, a),
        38 => out_bounce_h(t, 1.0, a),
        39 => {
            if t < 0.5 {
                in_bounce(2.0 * t, a) / 2.0
            } else if t == 1.0 {
                1.0
            } else {
                out_bounce_h(2.0 * t - 1.0, 1.0, a) / 2.0 + 0.5
            }
        }
        40 => {
            if t < 0.5 {
                out_bounce_h(t * 2.0, 0.5, a)
            } else {
                1.0 - out_bounce_h(2.0 - 2.0 * t, 0.5, a)
            }
        }
        41 => {
            let mix = smooth_mix(t);
            sin_progress(t) * mix + t * (1.0 - mix)
        }
        42 => {
            let mix = smooth_mix(1.0 - t);
            sin_progress(t) * mix + t * (1.0 - mix)
        }
        43 => ((t * PI * 2.0 - FRAC_PI_2).sin() + 1.0) / 2.0,
        44 => ((t * PI * 2.0 - FRAC_PI_2).cos() + 1.0) / 2.0,
        // Linear and types without explicit formulas (TCBSpline, Custom)
        // BezierSpline uses control points provided by the file (`bezier`)
        _ => t,
    }
}

/// `Easing.BezierSpline` (45): cubic segments via `easing.bezierCurve` starting
/// at (0, 0)
/// and ending at (1, 1), parsed like QEasingCurve's BezierEase:
/// `[c1x, c1y, c2x, c2y, endX, endY, ...]`.
/// Animation progress maps to x; the eased result is y.
/// Used across all end4 animations (e.g., M3 `expressiveDefaultSpatial`:
/// `[0.38, 1.21, 0.22, 1.0, 1, 1]`, which overshoots)
pub fn bezier(points: &[f64], t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if points.len() < 6 {
        return t;
    }
    let cubic = |a: f64, b: f64, c: f64, d: f64, s: f64| {
        let u = 1.0 - s;
        u * u * u * a + 3.0 * u * u * s * b + 3.0 * u * s * s * c + s * s * s * d
    };
    let segs: Vec<&[f64]> = points.chunks_exact(6).collect();
    let mut start = (0.0, 0.0);
    for (k, seg) in segs.iter().enumerate() {
        let end = (seg[4], seg[5]);
        if t <= end.0 || k == segs.len() - 1 {
            // x rises monotonically with the parameter: bisects for the
            // parameter at x = t
            let (mut lo, mut hi) = (0.0f64, 1.0f64);
            for _ in 0..48 {
                let mid = (lo + hi) / 2.0;
                if cubic(start.0, seg[0], seg[2], end.0, mid) < t {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            return cubic(start.1, seg[1], seg[3], end.1, (lo + hi) / 2.0);
        }
        start = end;
    }
    t
}

fn out_in(t: f64, out: fn(f64) -> f64, inn: fn(f64) -> f64) -> f64 {
    if t < 0.5 {
        out(2.0 * t) / 2.0
    } else {
        inn(2.0 * t - 1.0) / 2.0 + 0.5
    }
}

fn in_quad(t: f64) -> f64 {
    t * t
}

fn out_quad(t: f64) -> f64 {
    -t * (t - 2.0)
}

fn in_out_quad(t: f64) -> f64 {
    let t = t * 2.0;
    if t < 1.0 {
        t * t / 2.0
    } else {
        let t = t - 1.0;
        -0.5 * (t * (t - 2.0) - 1.0)
    }
}

fn in_cubic(t: f64) -> f64 {
    t * t * t
}

fn out_cubic(t: f64) -> f64 {
    let t = t - 1.0;
    t * t * t + 1.0
}

fn in_out_cubic(t: f64) -> f64 {
    let t = t * 2.0;
    if t < 1.0 {
        0.5 * t * t * t
    } else {
        let t = t - 2.0;
        0.5 * (t * t * t + 2.0)
    }
}

fn in_quart(t: f64) -> f64 {
    t * t * t * t
}

fn out_quart(t: f64) -> f64 {
    let t = t - 1.0;
    -(t * t * t * t - 1.0)
}

fn in_out_quart(t: f64) -> f64 {
    let t = t * 2.0;
    if t < 1.0 {
        0.5 * t * t * t * t
    } else {
        let t = t - 2.0;
        -0.5 * (t * t * t * t - 2.0)
    }
}

fn in_quint(t: f64) -> f64 {
    t * t * t * t * t
}

fn out_quint(t: f64) -> f64 {
    let t = t - 1.0;
    t * t * t * t * t + 1.0
}

fn in_out_quint(t: f64) -> f64 {
    let t = t * 2.0;
    if t < 1.0 {
        0.5 * t * t * t * t * t
    } else {
        let t = t - 2.0;
        0.5 * (t * t * t * t * t + 2.0)
    }
}

fn in_sine(t: f64) -> f64 {
    if t == 1.0 {
        1.0
    } else {
        -(t * FRAC_PI_2).cos() + 1.0
    }
}

fn out_sine(t: f64) -> f64 {
    (t * FRAC_PI_2).sin()
}

fn in_expo(t: f64) -> f64 {
    if t == 0.0 || t == 1.0 {
        t
    } else {
        2f64.powf(10.0 * (t - 1.0)) - 0.001
    }
}

fn out_expo(t: f64) -> f64 {
    if t == 1.0 {
        1.0
    } else {
        1.001 * (-(2f64.powf(-10.0 * t)) + 1.0)
    }
}

fn in_out_expo(t: f64) -> f64 {
    if t == 0.0 {
        return 0.0;
    }
    if t == 1.0 {
        return 1.0;
    }
    let t = t * 2.0;
    if t < 1.0 {
        0.5 * 2f64.powf(10.0 * (t - 1.0)) - 0.0005
    } else {
        0.5 * 1.0005 * (-(2f64.powf(-10.0 * (t - 1.0))) + 2.0)
    }
}

fn in_circ(t: f64) -> f64 {
    -((1.0 - t * t).sqrt() - 1.0)
}

fn out_circ(t: f64) -> f64 {
    let t = t - 1.0;
    (1.0 - t * t).sqrt()
}

fn in_out_circ(t: f64) -> f64 {
    let t = t * 2.0;
    if t < 1.0 {
        -0.5 * ((1.0 - t * t).sqrt() - 1.0)
    } else {
        let t = t - 2.0;
        0.5 * ((1.0 - t * t).sqrt() + 1.0)
    }
}

/// Qt's `easeInElastic_helper(t, b, c, d, a, p)`
fn in_elastic_h(t: f64, b: f64, c: f64, d: f64, a: f64, p: f64) -> f64 {
    if t == 0.0 {
        return b;
    }
    let mut t_adj = t / d;
    if t_adj == 1.0 {
        return b + c;
    }
    let (a, s) = if a < c.abs() {
        (c, p / 4.0)
    } else {
        (a, p / (2.0 * PI) * (c / a).asin())
    };
    t_adj -= 1.0;
    -(a * 2f64.powf(10.0 * t_adj) * ((t_adj * d - s) * (2.0 * PI) / p).sin()) + b
}

/// Port of Qt's `easeOutElastic_helper` with fixed defaults: `b = 0` (start)
/// and `d = 1` (duration)
fn out_elastic_h(t: f64, c: f64, a: f64, p: f64) -> f64 {
    if t == 0.0 {
        return 0.0;
    }
    if t == 1.0 {
        return c;
    }
    let (a, s) = if a < c {
        (c, p / 4.0)
    } else {
        (a, p / (2.0 * PI) * (c / a).asin())
    };
    a * 2f64.powf(-10.0 * t) * ((t - s) * (2.0 * PI) / p).sin() + c
}

fn in_out_elastic(t: f64, a: f64, p: f64) -> f64 {
    if t == 0.0 {
        return 0.0;
    }
    let t = t * 2.0;
    if t == 2.0 {
        return 1.0;
    }
    let (a, s) = if a < 1.0 {
        (1.0, p / 4.0)
    } else {
        (a, p / (2.0 * PI) * (1.0 / a).asin())
    };
    if t < 1.0 {
        return -0.5 * (a * 2f64.powf(10.0 * (t - 1.0)) * ((t - 1.0 - s) * (2.0 * PI) / p).sin());
    }
    a * 2f64.powf(-10.0 * (t - 1.0)) * ((t - 1.0 - s) * (2.0 * PI) / p).sin() * 0.5 + 1.0
}

fn in_back(t: f64, s: f64) -> f64 {
    t * t * ((s + 1.0) * t - s)
}

fn out_back(t: f64, s: f64) -> f64 {
    let t = t - 1.0;
    t * t * ((s + 1.0) * t + s) + 1.0
}

fn in_out_back(t: f64, s: f64) -> f64 {
    let t = t * 2.0;
    let s = s * 1.525;
    if t < 1.0 {
        0.5 * (t * t * ((s + 1.0) * t - s))
    } else {
        let t = t - 2.0;
        0.5 * (t * t * ((s + 1.0) * t + s) + 2.0)
    }
}

/// Qt's `easeOutBounce_helper(t, c, a)`
fn out_bounce_h(t: f64, c: f64, a: f64) -> f64 {
    if t == 1.0 {
        return c;
    }
    if t < 4.0 / 11.0 {
        c * (7.5625 * t * t)
    } else if t < 8.0 / 11.0 {
        let t = t - 6.0 / 11.0;
        -a * (1.0 - (7.5625 * t * t + 0.75)) + c
    } else if t < 10.0 / 11.0 {
        let t = t - 9.0 / 11.0;
        -a * (1.0 - (7.5625 * t * t + 0.9375)) + c
    } else {
        let t = t - 21.0 / 22.0;
        -a * (1.0 - (7.5625 * t * t + 0.984375)) + c
    }
}

fn in_bounce(t: f64, a: f64) -> f64 {
    1.0 - out_bounce_h(1.0 - t, 1.0, a)
}

fn sin_progress(t: f64) -> f64 {
    ((t * PI) - FRAC_PI_2).sin() / 2.0 + 0.5
}

fn smooth_mix(t: f64) -> f64 {
    (1.0 - t * 2.0 + 0.3).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const P: Params = Params {
        amplitude: 1.0,
        period: 0.3,
        overshoot: 1.70158,
    };

    fn at(kind: i64, t: f64) -> f64 {
        ease(kind, t, &P)
    }

    /// All easing curves except SineCurve and CosineCurve (periodic waves, not
    /// 0-to-1 transitions)
    #[test]
    fn every_curve_starts_at_nought_and_ends_at_one() {
        for kind in 0..=42 {
            assert!(at(kind, 0.0).abs() < 1e-9, "{kind} at 0: {}", at(kind, 0.0));
            assert!(
                (at(kind, 1.0) - 1.0).abs() < 1e-9,
                "{kind} at 1: {}",
                at(kind, 1.0)
            );
        }
    }

    /// Reference QEasingCurve outputs at t = 0.25 and 0.5 (derived from exact
    /// Qt formulas)
    #[test]
    fn the_numbers_are_qts() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        assert!(close(at(2, 0.5), 0.75), "OutQuad");
        assert!(close(at(3, 0.25), 0.125), "InOutQuad");
        assert!(close(at(6, 0.5), 0.875), "OutCubic");
        assert!(close(at(5, 0.5), 0.125), "InCubic");
        assert!(close(at(10, 0.5), 0.9375), "OutQuart");
        assert!(close(at(18, 0.5), (PI / 4.0).sin()), "OutSine");
        assert!(
            close(at(22, 0.5), 1.001 * (1.0 - 2f64.powf(-5.0))),
            "OutExpo"
        );
        assert!(close(at(21, 0.5), 2f64.powf(-5.0) - 0.001), "InExpo");
        assert!(close(at(26, 0.5), (0.75f64).sqrt()), "OutCirc");
        assert!(close(at(34, 0.5), 1.0876975), "OutBack");
        assert!(close(at(38, 0.5), 0.765625), "OutBounce");
        assert!(close(at(37, 0.5), 1.0 - 0.765625), "InBounce");
        // OutElastic overshoots one before it settles
        assert!(at(30, 0.1) > 1.0, "OutElastic {}", at(30, 0.1));
        // OutBack goes past the end and comes back
        assert!(at(34, 0.8) > 1.0);
    }

    #[test]
    fn a_bezier_spline_is_its_curve() {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
        // (0,0) (1/3,1/3) (2/3,2/3) (1,1) is the straight line
        let line = [1.0 / 3.0, 1.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 1.0, 1.0];
        for t in [0.0, 0.1, 0.5, 0.9, 1.0] {
            assert!(close(bezier(&line, t), t), "{t}: {}", bezier(&line, t));
        }
        // CSS `ease` (cubic-bezier(0.25, 0.1, 0.25, 1)): expects y ≈ 0.8024 at
        // x = 0.5
        let ease = [0.25, 0.1, 0.25, 1.0, 1.0, 1.0];
        assert!(
            (bezier(&ease, 0.5) - 0.8024).abs() < 1e-3,
            "{}",
            bezier(&ease, 0.5)
        );
        // kata expressiveDefaultSpatial: overshoots before settling at 1.0
        let spatial = [0.38, 1.21, 0.22, 1.00, 1.0, 1.0];
        assert!((0..100).any(|i| bezier(&spatial, i as f64 / 100.0) > 1.0));
        assert!(close(bezier(&spatial, 1.0), 1.0) && close(bezier(&spatial, 0.0), 0.0));
        // Two cubic segments: Material Design `emphasized`, split at (5/24,
        // 0.82)
        let emph = [
            0.05,
            0.0,
            2.0 / 15.0,
            0.06,
            1.0 / 6.0,
            0.4,
            5.0 / 24.0,
            0.82,
            0.25,
            1.0,
            1.0,
            1.0,
        ];
        assert!(
            close(bezier(&emph, 1.0 / 6.0), 0.4),
            "{}",
            bezier(&emph, 1.0 / 6.0)
        );
        assert!(bezier(&emph, 0.5) > 0.82 && bezier(&emph, 0.5) <= 1.0);
    }
}
