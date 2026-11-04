// Lines a pixel wide or thinner, walked as in Qt's raster engine
// (`canvases_are_qts`, case 5)
//
// - Along the longer axis one pixel at a time, each column getting one pixel of
//   ink. End columns get what the line covers in 64ths, with ends truncated to
//   64ths: 1.33 to 10.7 gives column 1 43/64, column 10 44/64.
// - Ink splits between the two pixels across the line by distance: down a
//   column at 30.2, pixel 29 gets 0.3 and pixel 30 gets 0.7.
// - Position is 16.16 fixed point, stepped by the truncated slope, with the
//   split in 256ths.
//
// Curves flatten as fills do. Each piece is walked separately and overlapping
// ink adds as separate draws (`joins.svg`). A non-butt cap extends half a pixel
// past each end of an open subpath.

use super::shape::Shape;
use super::stroke::Cap;

type P = (f64, f64);

/// A 16.16 fixed point number, cut towards nought
fn fixed(v: f64) -> i64 {
    (v * 65536.0) as i64
}

/// Coverage per pixel, 0 to 255, of the thin line along `path` in a `w` by `h`
/// mask, at `opacity` 256ths
pub fn coverage(path: &Shape, cap: Cap, opacity: u32, w: usize, h: usize) -> Vec<u8> {
    let mut cov = vec![0u8; w * h];
    for s in &path.subs {
        let mut pts = s.polyline();
        pts.dedup();
        if pts.len() < 2 {
            continue;
        }
        if cap != Cap::Butt && !s.closed {
            let n = pts.len();
            pts[0] = past(pts[1], pts[0]);
            pts[n - 1] = past(pts[n - 2], pts[n - 1]);
        }
        for p in pts.windows(2) {
            walk(&mut cov, w, h, opacity, p[0], p[1]);
        }
    }
    cov
}

/// `b` moved half a pixel on, the way from `a`
fn past(a: P, b: P) -> P {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l = (dx * dx + dy * dy).sqrt();
    if l == 0.0 { b } else { (b.0 + dx / l * 0.5, b.1 + dy / l * 0.5) }
}

/// The line from `a` to `b` walked into `cov`, `opacity` 256ths as strong
fn walk(cov: &mut [u8], w: usize, h: usize, opacity: u32, a: P, b: P) {
    // Qt walks a line between its ends cut to 64ths of a pixel
    let cut = |p: P| ((p.0 * 64.0).floor() / 64.0, (p.1 * 64.0).floor() / 64.0);
    let (a, b) = (cut(a), cut(b));
    // Along the longer axis; for y the axes swap for the walk
    let along_x = (b.0 - a.0).abs() >= (b.1 - a.1).abs();
    let (a, b) = if along_x { (a, b) } else { ((a.1, a.0), (b.1, b.0)) };
    let (a, b) = if b.0 < a.0 { (b, a) } else { (a, b) };
    let run = b.0 - a.0;
    if run <= 0.0 {
        return;
    }
    // Columns spanned, ends in 64ths, and coverage of the first and last
    let (fa, fb) = ((a.0 * 64.0).floor() as i64, (b.0 * 64.0).floor() as i64);
    let (first, last) = (fa >> 6, fb >> 6);
    let share = |col: i64| -> u32 {
        match (col == first, col == last) {
            (true, true) => (fb - fa) as u32,
            (true, false) => (64 - (fa & 63)) as u32,
            (false, true) => (fb & 63) as u32,
            (false, false) => 64,
        }
    };
    let slope = fixed((b.1 - a.1) / run);
    // Position across at the first column's middle, relative to pixel middles
    let mut at = fixed(a.1 - 0.5) + (slope * fixed(first as f64 + 0.5 - a.0) >> 16);
    for col in first..=last {
        let row = at >> 16;
        let below = ((at & 0xFFFF) >> 8) as u32;
        let part = share(col);
        for (r, ink) in [(row, (255 - below) * part >> 6), (row + 1, below * part >> 6)] {
            let ink = (ink * opacity >> 8) as u8;
            let (x, y) = if along_x { (col, r) } else { (r, col) };
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && ink > 0 {
                let c = &mut cov[y as usize * w + x as usize];
                // Two draws of the same ink
                *c = (*c as u32 + ink as u32 - (*c as u32 * ink as u32 + 127) / 255) as u8;
            }
        }
        at += slope;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End columns get covered ink in 64ths, positions truncated to 64ths
    /// (QPainter reference values)
    #[test]
    fn a_lines_ends_cover_what_it_covers_of_their_pixels() {
        let row = |x0: f64, x1: f64, y: f64, cap: Cap, at: usize| {
            let mut s = Shape::default();
            s.move_to((x0, y));
            s.line_to((x1, y));
            let cov = coverage(&s, cap, 256, 16, 12);
            cov[at * 16..at * 16 + 13].to_vec()
        };
        assert_eq!(row(1.33, 10.7, 5.5, Cap::Butt, 5), [0, 171, 255, 255, 255, 255, 255, 255, 255, 255, 175, 0, 0]);
        assert_eq!(row(1.0, 10.0, 5.5, Cap::Butt, 5), [0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0]);
        assert_eq!(row(1.5, 10.5, 5.5, Cap::Butt, 5), [0, 127, 255, 255, 255, 255, 255, 255, 255, 255, 127, 0, 0]);
        assert_eq!(row(1.9, 10.1, 5.5, Cap::Butt, 5), [0, 27, 255, 255, 255, 255, 255, 255, 255, 255, 23, 0, 0]);
        assert_eq!(row(1.33, 10.7, 5.2, Cap::Butt, 4), [0, 53, 79, 79, 79, 79, 79, 79, 79, 79, 54, 0, 0]);
        assert_eq!(row(1.33, 10.7, 5.2, Cap::Butt, 5), [0, 118, 176, 176, 176, 176, 176, 176, 176, 176, 121, 0, 0]);
        assert_eq!(row(1.33, 10.7, 5.5, Cap::Square, 5), [43, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 47, 0]);
    }

    /// Slope 0.4 from (5, 30): one pixel of ink per column, split as in Qt
    #[test]
    fn a_slanted_line_gives_each_column_one_pixel_of_ink() {
        let mut s = Shape::default();
        s.move_to((5.0, 30.0));
        s.line_to((55.0, 50.0));
        let cov = coverage(&s, Cap::Butt, 256, 60, 60);
        let at = |x: usize, y: usize| cov[y * 60 + x];
        assert_eq!((at(5, 29), at(5, 30)), (76, 179));
        assert_eq!((at(6, 30), at(6, 31)), (230, 25));
        assert_eq!((at(7, 30), at(7, 31)), (128, 127));
        assert_eq!(at(4, 29) as u32 + at(4, 30) as u32, 0);
        assert_eq!((at(54, 49), at(54, 50)), (179, 76));
        assert_eq!(at(55, 50), 0);
    }
}
