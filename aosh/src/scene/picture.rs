// Decodes Image files at the requested size
//
// Format comes from the first bytes: QOI through libmay's decoder; PNG, JPEG,
// and WebP through the `image` crate.
//
// `sourceSize` for non-SVG pictures keeps the aspect ratio, as in QQuickImage
// (`pictures.qml`):
// - with PreserveAspectFit or Crop, the larger of the sizes each side requests,
//   up or down;
// - otherwise only down: a side counts if smaller than the file, and the
//   smaller of two wins;
// - an unset side follows the other; with neither, the file's size.
//
// Pictures stay premultiplied (`qPremultiply` rounding), so sampling does not
// bleed a transparent edge. The scaled picture (`shrink`) is what is kept.
//
// Cached by file and size. A failed load is cached as nothing

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Decoded picture: premultiplied `0xAARRGGBB` rows
pub struct Picture {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u32>,
    /// What its copy in video memory is kept under (`gpu::serial`)
    pub serial: u64,
}

impl Picture {
    pub fn new(w: usize, h: usize, px: Vec<u32>) -> Picture {
        Picture { w, h, px, serial: super::gpu::serial() }
    }

    /// It, as a texture
    pub fn texture(&self) -> super::gpu::Texture<'_> {
        super::gpu::Texture::new(&self.px, self.w, self.h, self.serial)
    }
}

/// Requested size: `sourceSize` (0 if unset) and whether the Image keeps its
/// shape (PreserveAspectFit or Crop)
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Want {
    pub w: u32,
    pub h: u32,
    pub keep: bool,
}

#[derive(Default)]
pub struct Pictures {
    kept: HashMap<(PathBuf, Want), Option<Rc<Picture>>>,
}

impl Pictures {
    /// Picture in `path` at `want`, decoded on first request. Returns the error
    /// once, for the log
    pub fn get(&mut self, path: &Path, want: Want) -> (Option<Rc<Picture>>, Option<String>) {
        let key = (path.to_path_buf(), want);
        if let Some(p) = self.kept.get(&key) {
            return (p.clone(), None);
        }
        let (pic, err) = match std::fs::read(path) {
            Err(e) => (None, Some(format!("{}: {e}", path.display()))),
            Ok(bytes) => match decode(&bytes) {
                Some(p) => (Some(Rc::new(resize(p, want))), None),
                None => (None, Some(format!("{} is not a picture aosh can read (QOI, PNG, JPEG, WebP)", path.display()))),
            },
        };
        // Cache limit
        if self.kept.len() > 256 {
            self.kept.clear();
        }
        self.kept.insert(key, pic.clone());
        (pic, err)
    }
}

impl Pictures {
    /// quickshell's `image://icon/ID` at `w` by `h`: `NAME`, else the first
    /// `?fallback=` icon from the theme (`icons.rs`), drawn at the smaller
    /// side; otherwise the missing-icon picture. A 0 side requests 2x2, as in
    /// quickshell's provider
    pub fn icon(&mut self, icons: &super::icons::Icons, id: &str, w: u32, h: u32) -> Option<Rc<Picture>> {
        let key = (PathBuf::from(format!("image://icon/{id}")), Want { w, h, keep: false });
        if let Some(p) = self.kept.get(&key) {
            return p.clone();
        }
        let (name, fallback) = match id.split_once("?fallback=") {
            Some((n, f)) => (n, Some(f)),
            None => (id, None),
        };
        let (tw, th) = if w == 0 || h == 0 { (2, 2) } else { (w, h) };
        let s = tw.min(th);
        let pic = icons.picture(name, s)
            .or_else(|| fallback.and_then(|f| icons.picture(f, s)))
            .unwrap_or_else(|| missing(tw as usize, th as usize));
        if self.kept.len() > 256 {
            self.kept.clear();
        }
        let pic = Some(Rc::new(pic));
        self.kept.insert(key, pic.clone());
        pic
    }
}

/// quickshell's missing-icon picture: black with purple top right and bottom
/// left quarters, sides made even
fn missing(w: usize, h: usize) -> Picture {
    let (w, h) = ((w + w % 2).max(2), (h + h % 2).max(2));
    let px = (0..w * h).map(|i| {
        let (x, y) = (i % w, i / w);
        if (x >= w / 2) != (y >= h / 2) { 0xFFD9_00D8 } else { 0xFF00_0000 }
    }).collect();
    Picture::new(w, h, px)
}

/// A file's picture at the size the file has it
pub fn decode(bytes: &[u8]) -> Option<Picture> {
    if bytes.starts_with(b"qoif") {
        let (w, h) = libmay::qoi::size(bytes)?;
        let (w, h) = (w as usize, h as usize);
        let mut out = vec![0u8; w * h * 4];
        if libmay::qoi::decode(bytes, &mut out) == 0 {
            return None;
        }
        let px = out.chunks_exact(4).map(|c| premultiply(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))).collect();
        return Some(Picture::new(w, h, px));
    }
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    let px = img.as_raw().chunks_exact(4)
        .map(|c| premultiply(u32::from_be_bytes([c[3], c[0], c[1], c[2]])))
        .collect();
    Some(Picture::new(w, h, px))
}

/// Straight `0xAARRGGBB` premultiplied the way Qt's `qPremultiply` does it
fn premultiply(x: u32) -> u32 {
    let a = x >> 24;
    let mut rb = (x & 0x00ff_00ff) * a;
    rb = ((rb + ((rb >> 8) & 0x00ff_00ff) + 0x0080_0080) >> 8) & 0x00ff_00ff;
    let mut g = ((x >> 8) & 0xff) * a;
    g = (g + ((g >> 8) & 0xff) + 0x80) & 0xff00;
    (a << 24) | rb | g
}

/// The size `want` makes a `w` by `h` picture, by Qt's rule above
pub fn target(w: usize, h: usize, want: Want) -> (usize, usize) {
    if w == 0 || h == 0 {
        return (w, h);
    }
    let asked = |side: u32, has: usize| (side > 0 && (want.keep || (side as usize) < has)).then(|| side as f64 / has as f64);
    let k = match (asked(want.w, w), asked(want.h, h)) {
        (Some(a), Some(b)) if want.keep => a.max(b),
        (Some(a), Some(b)) => a.min(b),
        (Some(k), None) | (None, Some(k)) => k,
        (None, None) => return (w, h),
    };
    (((w as f64 * k).round() as usize).max(1), ((h as f64 * k).round() as usize).max(1))
}

/// Resizes `p` to `want` like QImage::scaled's smooth transformation, per axis
/// in premultiplied color (`pictures_are_qts`):
/// - enlarging or shrinking by at most 2 samples between the two neighboring
///   pixels at each new pixel's center;
/// - shrinking by more averages what each new pixel covers, with partial edge
///   pixels.
///
/// Qt's fixed-point rounding differs by up to 2/255. libjpeg-scaled JPEGs
/// differ by about 0.1/255 on average at half size
fn resize(p: Picture, want: Want) -> Picture {
    let (tw, th) = target(p.w, p.h, want);
    scaled(p, tw, th)
}

/// Resizes `p` to exactly `tw` by `th` (`scene/icons.rs`)
pub fn scaled(p: Picture, tw: usize, th: usize) -> Picture {
    if (tw, th) == (p.w, p.h) || p.w == 0 || p.h == 0 || tw == 0 || th == 0 {
        return p;
    }
    let (across, down) = (taps(p.w, tw), taps(p.h, th));
    let channels = |c: u32| [(c >> 24) as f32, ((c >> 16) & 0xff) as f32, ((c >> 8) & 0xff) as f32, (c & 0xff) as f32];
    // Across each row, then down each column
    let mut rows = vec![[0f32; 4]; tw * p.h];
    for y in 0..p.h {
        let line = &p.px[y * p.w..(y + 1) * p.w];
        for (x, (start, w)) in across.iter().enumerate() {
            let mut sum = [0f32; 4];
            for (k, &wk) in w.iter().enumerate() {
                let c = channels(line[start + k]);
                for i in 0..4 {
                    sum[i] += c[i] * wk;
                }
            }
            rows[y * tw + x] = sum;
        }
    }
    let mut out = vec![0u32; tw * th];
    for (y, (start, w)) in down.iter().enumerate() {
        for x in 0..tw {
            let mut sum = [0f32; 4];
            for (k, &wk) in w.iter().enumerate() {
                let c = rows[(start + k) * tw + x];
                for i in 0..4 {
                    sum[i] += c[i] * wk;
                }
            }
            let q = |v: f32| (v + 0.5).clamp(0.0, 255.0) as u32;
            let a = q(sum[0]);
            // Premultiplied: no channel over alpha
            out[y * tw + x] = a << 24 | q(sum[1]).min(a) << 16 | q(sum[2]).min(a) << 8 | q(sum[3]).min(a);
        }
    }
    Picture::new(tw, th, out)
}

/// For `n` new pixels over `of` old ones: each one's first old pixel and
/// weights
fn taps(of: usize, n: usize) -> Vec<(usize, Vec<f32>)> {
    let scale = of as f64 / n as f64;
    (0..n).map(|i| {
        if scale <= 2.0 {
            let c = ((i as f64 + 0.5) * scale - 0.5).max(0.0);
            let j = (c.floor() as usize).min(of - 1);
            let t = (c - j as f64) as f32;
            if j + 1 < of && t > 0.0 { (j, vec![1.0 - t, t]) } else { (j, vec![1.0]) }
        } else {
            let (a, b) = (i as f64 * scale, ((i + 1) as f64 * scale).min(of as f64));
            let j0 = a.floor() as usize;
            let j1 = (b.ceil() as usize).min(of);
            let w = (j0..j1).map(|j| ((b.min(j as f64 + 1.0) - a.max(j as f64)) / scale) as f32).collect();
            (j0, w)
        }
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A picture's PNG, as the `image` crate writes one
    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let mut img = image::RgbaImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgba(f(x, y));
        }
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_png_is_its_pixels() {
        let bytes = png(3, 2, |x, y| [x as u8 * 100, y as u8 * 200, 7, if x == 2 { 0 } else { 255 }]);
        let p = decode(&bytes).unwrap();
        assert_eq!((p.w, p.h), (3, 2));
        assert_eq!(p.px[1], 0xff64_0007, "x 1: red 100, green 0, blue 7, opaque");
        assert_eq!(p.px[3] >> 8 & 0xff, 200, "row 1 is green");
        assert_eq!(p.px[2], 0, "transparent is nothing, premultiplied");
        assert!(decode(b"not a picture").is_none());
    }

    #[test]
    fn source_size_is_qts() {
        let want = |w, h, keep| Want { w, h, keep };
        // Wallpaper at screen size, exact either way
        assert_eq!(target(3840, 2160, want(1920, 1080, true)), (1920, 1080));
        // Only one side requested
        assert_eq!(target(512, 512, want(200, 0, false)), (200, 200));
        // Inside both when stretched; the larger when keeping the shape
        assert_eq!(target(400, 200, want(100, 100, false)), (100, 50));
        assert_eq!(target(400, 200, want(100, 100, true)), (200, 100));
        assert_eq!(target(300, 300, want(200, 150, true)), (200, 200));
        // Upscaling only when keeping the shape; a larger side otherwise does
        // not count
        assert_eq!(target(300, 300, want(360, 360, true)), (360, 360));
        assert_eq!(target(64, 64, want(256, 256, false)), (64, 64));
        assert_eq!(target(300, 300, want(0, 330, false)), (300, 300));
    }

    /// Averaged in premultiplied color: white beside transparent black is
    /// half-transparent white
    #[test]
    fn shrinking_averages_without_dark_edges() {
        let bytes = png(4, 2, |x, _| if x % 2 == 0 { [255, 255, 255, 255] } else { [0, 0, 0, 0] });
        let p = resize(decode(&bytes).unwrap(), Want { w: 2, h: 0, keep: false });
        assert_eq!((p.w, p.h), (2, 1));
        assert_eq!(crate::scene::paint::to_straight(p.px[0]), 0x80ff_ffff, "{:08x}", p.px[0]);
    }
}
