// A window's outline on the processor: `libmay::shape`'s corner and border,
// read from tables (`dev/mayland-window-shape.md`)
//
// The shape is the function the compute pass runs per pixel
// (`shaders/composite.cu`). The processor reads a table: the kernel is
// soft-float, and three `pow`s per pixel would cost the cursor patch
// milliseconds on a corner.
//
// One table per shape and size, holding a corner square's worth, the same at
// all four corners as the shape folds every corner onto one; the size is part
// of the key, as Hyprland's fold nudges by `1 / size`. Outside the square a
// window's pixel is whole and the border is solid within its width of the edge,
// which `surface` and `border` answer without a table.

extern crate alloc;

use alloc::vec::Vec;
use spin::Mutex;

use libmay::shape::{self, Shape};

pub struct Outline {
    key: (u32, u32, u32, i32, i32),
    /// Side of the corner square, in the window's box and in the border's
    reach: i32,
    border: i32,
    surface: Vec<u8>,
    ring: Vec<u8>,
}

impl Outline {
    fn build(s: Shape, w: i32, h: i32) -> Outline {
        let reach = s.corner_reach();
        let border = s.border as i32;
        let (bw, bh) = ((w + 2 * border) as f32, (h + 2 * border) as f32);
        let n = (reach * reach) as usize;
        let mut surface = Vec::with_capacity(n);
        let mut ring = Vec::with_capacity(n);
        for j in 0..reach {
            for i in 0..reach {
                let (px, py) = (i as f32 + 0.5, j as f32 + 0.5);
                surface.push(unit(shape::surface_coverage(px, py, w as f32, h as f32, &s)));
                ring.push(if border > 0 { unit(shape::border_alpha(px, py, bw, bh, &s)) } else { 0 });
            }
        }
        Outline { key: key(s, w, h), reach, border, surface, ring }
    }

    /// How much of the window's own pixel at `(x, y)` in its `w` by `h` box
    /// is left, 0 to 255
    #[inline]
    pub fn surface(&self, x: i32, y: i32, w: i32, h: i32) -> u32 {
        let (i, j) = (x.min(w - 1 - x), y.min(h - 1 - y));
        if i >= self.reach || j >= self.reach || i < 0 || j < 0 {
            return 255;
        }
        self.surface[(j * self.reach + i) as usize] as u32
    }

    /// How much of the border's colour the pixel at `(x, y)` of the border's
    /// `bw` by `bh` box takes, 0 to 255
    #[inline]
    pub fn border(&self, x: i32, y: i32, bw: i32, bh: i32) -> u32 {
        let (i, j) = (x.min(bw - 1 - x), y.min(bh - 1 - y));
        if i < 0 || j < 0 {
            return 0;
        }
        if i < self.reach && j < self.reach {
            return self.ring[(j * self.reach + i) as usize] as u32;
        }
        if i.min(j) < self.border { 255 } else { 0 }
    }

    /// The corner square's side, for a caller cutting a box into the parts
    /// that blend and the parts that copy
    pub fn reach(&self) -> i32 {
        self.reach
    }
}

#[inline]
fn unit(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

fn key(s: Shape, w: i32, h: i32) -> (u32, u32, u32, i32, i32) {
    (s.rounding.to_bits(), s.power.to_bits(), s.border.to_bits(), w, h)
}

/// The shapes seen lately, most recent last. Eight: a handful of windows, and
/// the popin of one of them, each frame a new size
static TABLES: Mutex<Vec<Outline>> = Mutex::new(Vec::new());
const KEPT: usize = 8;

/// `f` with the outline of a `w` by `h` window of shape `s`, built on first
/// sight
pub fn with<R>(s: Shape, w: i32, h: i32, f: impl FnOnce(&Outline) -> R) -> R {
    let k = key(s, w, h);
    let mut t = TABLES.lock();
    let i = match t.iter().position(|o| o.key == k) {
        Some(i) => i,
        None => {
            if t.len() >= KEPT {
                t.remove(0);
            }
            t.push(Outline::build(s, w, h));
            t.len() - 1
        }
    };
    // Most recent last, so the one evicted is the one least recently used
    let o = t.remove(i);
    t.push(o);
    f(t.last().unwrap())
}
