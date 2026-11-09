//! Paths filled by FreeType: a Canvas's
//!
//! A path is subpaths of straight lines, in pixels, with curves already
//! flattened by the caller (aosh flattens as Qt does). Filling it is FreeType's
//! smooth rasteriser, the anti-aliasing one its glyphs use: each pixel's
//! coverage, 0 to 255, by the path's area under the non-zero or even-odd rule.
//! A line along a path is filled as its outline, which aosh makes
//! (`scene/canvas/stroke.rs`).

use std::ffi::{c_int, c_long, c_void};

extern "C" {
    fn aft_path_fill(lib: *mut c_void, points: *const [c_long; 2], tags: *const u8, n_points: c_int, ends: *const u16,
                     n_ends: c_int, even_odd: c_int, w: c_int, h: c_int, cov: *mut u8) -> c_int;
}

const ON: u8 = 1;

/// The most points one outline of FreeType's can have
const MOST: usize = 0xFFFF;

/// Subpaths of lines, in pixels
#[derive(Clone, Debug, Default)]
pub struct Path {
    points: Vec<[c_long; 2]>,
    tags: Vec<u8>,
    ends: Vec<u16>,
    /// Where the subpath being drawn starts, if one is
    start: Option<usize>,
}

fn fixed(v: f64) -> c_long {
    (v * 64.0).round() as c_long
}

impl Path {
    pub fn new() -> Path {
        Path::default()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty() && self.start.is_none()
    }

    /// A new subpath at `(x, y)`, the last one ended
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.end(false);
        self.start = Some(self.points.len());
        self.points.push([fixed(x), fixed(y)]);
        self.tags.push(ON);
    }

    pub fn line_to(&mut self, x: f64, y: f64) {
        if self.start.is_none() {
            self.move_to(x, y);
            return;
        }
        self.points.push([fixed(x), fixed(y)]);
        self.tags.push(ON);
    }

    /// The subpath being drawn, closed
    pub fn close(&mut self) {
        self.end(true);
    }

    /// Ends the subpath being drawn. One point alone is no subpath; a closed
    /// one does not repeat its first point at its end
    fn end(&mut self, closed: bool) {
        let Some(start) = self.start.take() else { return };
        let mut n = self.points.len();
        if closed && n - start > 1 && self.points[n - 1] == self.points[start] {
            self.points.pop();
            self.tags.pop();
            n -= 1;
        }
        if n - start < 2 || n > MOST {
            self.points.truncate(start);
            self.tags.truncate(start);
            return;
        }
        self.ends.push((n - 1) as u16);
    }

    /// The finished subpaths, the one being drawn ended
    fn finished(&self) -> Path {
        let mut p = self.clone();
        p.end(false);
        p
    }

    /// Each pixel's coverage, 0 to 255, of a mask `w` by `h` with rows
    /// going down
    pub fn fill(&self, even_odd: bool, w: usize, h: usize) -> Vec<u8> {
        let p = self.finished();
        let mut cov = vec![0u8; w * h];
        let Some(lib) = crate::library() else { return cov };
        if p.ends.is_empty() || w == 0 || h == 0 {
            return cov;
        }
        unsafe {
            aft_path_fill(lib, p.points.as_ptr(), p.tags.as_ptr(), p.points.len() as c_int, p.ends.as_ptr(),
                          p.ends.len() as c_int, even_odd as c_int, w as c_int, h as c_int, cov.as_mut_ptr());
        }
        cov
    }
}
