// Converts pictures to QOI at build time. Included with `#[path]`. The inverse
// of `libmay::qoi`; the machine only decodes.
//
// Unlike `imagegen.rs`, the result is a file, since a config chooses it.
// Pictures are scaled down to a sane size and never up.

use std::path::Path;

/// Longest side of a shipped wallpaper: enough to cover 1920x1080
pub const WALLPAPER: u32 = 1920;

/// Longest side of a shipped icon
pub const ICON: u32 = 64;

/// Converts every picture in `dir` plus `extra` into `out`, at most `max_side`
/// on the longest edge. `extra` is the mascot, always converted. `what` is the
/// word printed
pub fn generate(dir: &Path, extra: &[&Path], out: &Path, max_side: u32, what: &str) {
    std::fs::create_dir_all(out).ok();
    let mut sources: Vec<std::path::PathBuf> =
        extra.iter().map(|p| p.to_path_buf()).collect();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            // Skips READMEs
            if p.is_file() && p.extension().is_some_and(|x| x != "md") {
                sources.push(p);
            }
        }
    }
    for src in &sources {
        if !src.exists() {
            continue;
        }
        let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or(what);
        let dst = out.join(format!("{stem}.qoi"));
        match convert(src, &dst, max_side) {
            Some(n) => println!("cargo:warning={what} '{stem}': {} KB", n / 1024),
            // A warning, not a failure
            None => println!("cargo:warning={} is not a picture this build can read",
                             src.display()),
        }
        println!("cargo:rerun-if-changed={}", src.display());
    }
    println!("cargo:rerun-if-changed={}", dir.display());
}

/// Converts one picture to a round QOI of `size` by `size`: the greeter's
/// portrait. A file, so a config can name it. Masked with
/// `imagegen::circle_cov`, and drawn with `Canvas::blit_over`
pub fn generate_round(src: &Path, out: &Path, name: &str, size: u32) {
    std::fs::create_dir_all(out).ok();
    let dst = out.join(format!("{name}.qoi"));
    match convert_round(src, &dst, size) {
        Some(n) => println!("cargo:warning=art '{name}': {}x{}, {} KB", size, size, n / 1024),
        None => println!("cargo:warning={} is not a picture this build can read",
                         src.display()),
    }
    println!("cargo:rerun-if-changed={}", src.display());
}

fn convert_round(src: &Path, dst: &Path, size: u32) -> Option<usize> {
    let img = image::ImageReader::open(src).ok()?
        .with_guessed_format().ok()?
        .decode().ok()?;
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    // Center crop to a square
    let side = w.min(h);
    let img = img.crop_imm((w - side) / 2, (h - side) / 2, side, side)
        .resize_exact(size, size, image::imageops::FilterType::Lanczos3);

    let mut rgba = img.to_rgba8();
    let r = size as f32 / 2.0;
    for y in 0..size {
        for x in 0..size {
            let p = rgba.get_pixel_mut(x, y);
            // Straight alpha, for `libmay::qoi` and `Canvas::blit_over`
            p.0[3] = (p.0[3] as f32 * super::imagegen::circle_cov(x, y, r)) as u8;
        }
    }
    let bytes = encode(rgba.as_raw(), size, size);
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    std::fs::write(dst, &bytes).ok()?;
    Some(bytes.len())
}

/// Read any picture, write a QOI. Returns what it wrote, in bytes
fn convert(src: &Path, dst: &Path, max_side: u32) -> Option<usize> {
    // Sniffs the format, since the mascot named .png is a JPEG
    let img = image::ImageReader::open(src)
        .ok()?
        .with_guessed_format()
        .ok()?
        .decode()
        .ok()?;

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let long = w.max(h);
    let img = if long > max_side {
        let scale = max_side as f32 / long as f32;
        img.resize(
            ((w as f32 * scale) as u32).max(1),
            ((h as f32 * scale) as u32).max(1),
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        img
    };

    let rgba = img.to_rgba8();
    let bytes = encode(rgba.as_raw(), rgba.width(), rgba.height());
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).ok()?;
    }
    std::fs::write(dst, &bytes).ok()?;
    Some(bytes.len())
}

/// RGBA in, QOI file out: run, index, small delta, luma, or literal
fn encode(rgba: &[u8], w: u32, h: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(14 + (w * h) as usize + 8);
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.push(4); // channels
    out.push(0); // sRGB with linear alpha

    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0, 0, 255];
    let mut run: u8 = 0;
    let n = (w * h) as usize;

    for i in 0..n {
        let px = [rgba[i * 4], rgba[i * 4 + 1], rgba[i * 4 + 2], rgba[i * 4 + 3]];

        if px == prev {
            run += 1;
            // 62 is the longest run; 63 and 64 collide with the two byte-wide
            // tags
            if run == 62 || i + 1 == n {
                out.push(0xC0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xC0 | (run - 1));
            run = 0;
        }

        let slot = hash(px);
        if index[slot] == px {
            out.push(slot as u8);
            prev = px;
            continue;
        }
        index[slot] = px;

        if px[3] == prev[3] {
            let dr = px[0].wrapping_sub(prev[0]) as i8;
            let dg = px[1].wrapping_sub(prev[1]) as i8;
            let db = px[2].wrapping_sub(prev[2]) as i8;
            let (dr_g, db_g) = (dr.wrapping_sub(dg), db.wrapping_sub(dg));

            if (-2..=1).contains(&dr) && (-2..=1).contains(&dg) && (-2..=1).contains(&db) {
                out.push(0x40
                    | (((dr + 2) as u8) << 4)
                    | (((dg + 2) as u8) << 2)
                    | ((db + 2) as u8));
            } else if (-32..=31).contains(&dg)
                && (-8..=7).contains(&dr_g)
                && (-8..=7).contains(&db_g)
            {
                out.push(0x80 | ((dg + 32) as u8));
                out.push((((dr_g + 8) as u8) << 4) | ((db_g + 8) as u8));
            } else {
                out.push(0xFE);
                out.extend_from_slice(&px[..3]);
            }
        } else {
            out.push(0xFF);
            out.extend_from_slice(&px);
        }
        prev = px;
    }

    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out
}

fn hash(px: [u8; 4]) -> usize {
    (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7 + px[3] as usize * 11) % 64
}
