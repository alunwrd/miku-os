//! FreeType, asked what Qt asks it: a glyph's coverage and its outline. Also a
//! Canvas's paths (`path.rs`)
//!
//! A face made from bytes the caller keeps, set to a variable font's
//! coordinates and a size, and a glyph loaded from it the way `QFontEngineFT`
//! loads one: `src/shim.c` has the calls and where in Qt each comes from.
//! Everything is 26.6 fixed point, as FreeType and Qt's `QFixed` keep it.
//!
//! One FreeType library a thread: FreeType's objects are not shared between
//! threads, and a `Face` is neither `Send` nor `Sync`.

use std::ffi::{c_int, c_long, c_uint, c_void};
use std::rc::Rc;

// The HarfBuzz the autohinter calls, by its C names: `hb.rs`
pub mod hb;
// Paths filled: a Canvas's
pub mod path;

#[repr(C)]
struct RawGlyph {
    linear: c_long,
    advance: c_long,
    bearing_x: c_long,
    width: c_long,
    left: c_int,
    top: c_int,
    w: c_uint,
    h: c_uint,
    pitch: c_int,
    buffer: *const u8,
}

extern "C" {
    fn aft_library_new(out: *mut *mut c_void) -> c_int;
    fn aft_library_done(lib: *mut c_void);
    fn aft_face_new(lib: *mut c_void, data: *const u8, len: c_long, index: c_long, out: *mut *mut c_void) -> c_int;
    fn aft_face_done(face: *mut c_void);
    fn aft_face_setup(face: *mut c_void, coords: *const c_long, n: c_uint, size: c_long) -> c_int;
    fn aft_face_size(face: *mut c_void, size: c_long) -> c_int;
    fn aft_glyph_load(face: *mut c_void, gid: c_uint, flags: c_int, subx: c_long, g: *mut RawGlyph) -> c_int;
    fn aft_glyph_outline(face: *mut c_void, gid: c_uint, hinted: c_int, out: *mut c_long, cap: c_long) -> c_long;
}

const HINT: c_int = 1;
const EMBOLDEN: c_int = 2;
const RENDER: c_int = 4;

struct Library(*mut c_void);

impl Drop for Library {
    fn drop(&mut self) {
        unsafe { aft_library_done(self.0) }
    }
}

thread_local! {
    static LIBRARY: Option<Rc<Library>> = {
        let mut lib = std::ptr::null_mut();
        (unsafe { aft_library_new(&mut lib) } == 0).then(|| Rc::new(Library(lib)))
    };
}

/// This thread's FreeType, for what is not a face's
fn library() -> Option<*mut c_void> {
    LIBRARY.with(|l| l.as_ref().map(|l| l.0))
}

/// A face of a font file, at its coordinates and a size
pub struct Face {
    raw: *mut c_void,
    // Dropped after the face, which reads them until it is done
    _lib: Rc<Library>,
    _data: Rc<[u8]>,
}

impl Drop for Face {
    fn drop(&mut self) {
        unsafe { aft_face_done(self.raw) }
    }
}

/// How a glyph is loaded: Qt's hinting for the size, synthetic bold, and
/// whether to render it, `subx` 64ths of a pixel right of the pen
#[derive(Clone, Copy, Debug, Default)]
pub struct Load {
    pub hint: bool,
    pub embolden: bool,
    pub render: bool,
    pub subx: i64,
}

/// What a load leaves in FreeType's glyph slot, in 26.6, and the coverage
/// when it was rendered: `w` by `h` bytes, `left` right of the pen and `top`
/// above the baseline
#[derive(Clone, Debug, Default)]
pub struct Glyph {
    /// `linearHoriAdvance >> 10`: the unhinted advance, what Qt lays out with
    pub linear: i64,
    /// `advance.x`: the hinted one, with whatever emboldening added
    pub advance: i64,
    pub bearing_x: i64,
    pub width: i64,
    pub left: i32,
    pub top: i32,
    pub w: usize,
    pub h: usize,
    pub coverage: Vec<u8>,
}

/// A point of a glyph's outline, 26.6 pixels, y up, and what it is
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: i64,
    pub y: i64,
    /// On the curve; else a control point - a conic's when `cubic` is not
    pub on: bool,
    pub cubic: bool,
}

impl Face {
    /// Face `index` of the font in `data`. None if FreeType cannot read it
    pub fn new(data: Rc<[u8]>, index: u32) -> Option<Face> {
        let lib = LIBRARY.with(|l| l.clone())?;
        let mut raw = std::ptr::null_mut();
        let e = unsafe { aft_face_new(lib.0, data.as_ptr(), data.len() as c_long, index as c_long, &mut raw) };
        (e == 0).then_some(Face { raw, _lib: lib, _data: data })
    }

    /// Where its axes stand, in the face's own order (`coords` is empty for a
    /// face with none), and its size in 64ths of a pixel
    pub fn setup(&mut self, coords: &[f32], size: i64) -> bool {
        // Qt's FT_Fixed(value * 65536), which truncates
        let fixed: Vec<c_long> = coords.iter().map(|&v| (v as f64 * 65536.0) as c_long).collect();
        unsafe { aft_face_setup(self.raw, fixed.as_ptr(), fixed.len() as c_uint, size as c_long) == 0 }
    }

    /// Another size, 64ths of a pixel, for the same face and coordinates
    pub fn set_size(&mut self, size: i64) -> bool {
        unsafe { aft_face_size(self.raw, size as c_long) == 0 }
    }

    /// Glyph `gid`'s outline at the face's size: its contours, each its
    /// points in order. `hinted` is FreeType's default hinting, which Qt
    /// loads a path's outline with, at the face's units per em
    /// (`QFontEngineFT::getUnscaledGlyph`). None if it would not load or is
    /// not an outline (a bitmap glyph)
    pub fn outline(&mut self, gid: u16, hinted: bool) -> Option<Vec<Vec<Point>>> {
        let mut buf: Vec<c_long> = vec![0; 512];
        let h = hinted as c_int;
        let n = unsafe { aft_glyph_outline(self.raw, gid as c_uint, h, buf.as_mut_ptr(), buf.len() as c_long) };
        if n < 0 {
            return None;
        }
        if n as usize > buf.len() {
            buf = vec![0; n as usize];
            if unsafe { aft_glyph_outline(self.raw, gid as c_uint, h, buf.as_mut_ptr(), buf.len() as c_long) } != n {
                return None;
            }
        }
        let contours = buf[0].max(0) as usize;
        let ends: Vec<usize> = buf[1..1 + contours].iter().map(|&e| e as usize).collect();
        let pts: Vec<Point> = buf[1 + contours..n as usize].chunks(3).map(|c| Point {
            x: c[0] as i64,
            y: c[1] as i64,
            // FT_CURVE_TAG_ON 1, _CONIC 0, _CUBIC 2
            on: c[2] == 1,
            cubic: c[2] == 2,
        }).collect();
        let mut out = Vec::with_capacity(contours);
        let mut start = 0;
        for &end in &ends {
            if end < start || end >= pts.len() {
                return None;
            }
            out.push(pts[start..=end].to_vec());
            start = end + 1;
        }
        Some(out)
    }

    /// Glyph `gid`, loaded as `l` says. None if FreeType would not load it
    pub fn glyph(&mut self, gid: u16, l: Load) -> Option<Glyph> {
        let flags = if l.hint { HINT } else { 0 } | if l.embolden { EMBOLDEN } else { 0 } | if l.render { RENDER } else { 0 };
        let mut g = RawGlyph { linear: 0, advance: 0, bearing_x: 0, width: 0, left: 0, top: 0, w: 0, h: 0, pitch: 0, buffer: std::ptr::null() };
        if unsafe { aft_glyph_load(self.raw, gid as c_uint, flags, l.subx as c_long, &mut g) } != 0 {
            return None;
        }
        let (w, h) = (g.w as usize, g.h as usize);
        let mut coverage = Vec::with_capacity(w * h);
        if !g.buffer.is_null() && w > 0 {
            for y in 0..h {
                // A gray bitmap flows down; its rows are `pitch` apart
                let row = unsafe { std::slice::from_raw_parts(g.buffer.offset(y as isize * g.pitch as isize), w) };
                coverage.extend_from_slice(row);
            }
        }
        Some(Glyph {
            linear: g.linear as i64,
            advance: g.advance as i64,
            bearing_x: g.bearing_x as i64,
            width: g.width as i64,
            left: g.left,
            top: g.top,
            w,
            h: if coverage.is_empty() { 0 } else { h },
            coverage,
        })
    }
}

#[cfg(test)]
mod tests;
