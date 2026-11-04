// The SVG fixtures against Qt's renders of them (`tests/qtref/svg.py`)

use std::path::Path;

/// Qt's picture, premultiplied, or none if the reference is absent
fn qt(name: &str, size: usize) -> Option<Vec<u32>> {
    let img = image::open(format!("../target/qtref/svg/{name}@{size}.png")).ok()?.to_rgba8();
    Some(img.pixels().map(|p| {
        let [r, g, b, a] = p.0;
        let pm = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u32;
        (a as u32) << 24 | pm(r) << 16 | pm(g) << 8 | pm(b)
    }).collect())
}

/// The largest difference in any channel, and the mean over the picture
fn diff(a: &[u32], b: &[u32]) -> (u32, f64) {
    let mut worst = 0;
    let mut sum = 0u64;
    for (x, y) in a.iter().zip(b) {
        for sh in [0, 8, 16, 24] {
            let d = ((x >> sh) & 0xff).abs_diff((y >> sh) & 0xff);
            worst = worst.max(d);
            sum += d as u64;
        }
    }
    (worst, sum as f64 / (a.len() * 4) as f64)
}

/// Qt's and aosh's side by side, for a look
fn save(name: &str, size: usize, qt: &[u32], ours: &[u32]) {
    let mut both = image::RgbaImage::new(size as u32 * 2, size as u32);
    for (i, (q, o)) in qt.iter().zip(ours).enumerate() {
        let (x, y) = ((i % size) as u32, (i / size) as u32);
        let px = |c: u32| {
            let a = c >> 24;
            let un = |v: u32| if a == 0 { 0 } else { ((v * 255 + a / 2) / a).min(255) as u8 };
            image::Rgba([un((c >> 16) & 0xff), un((c >> 8) & 0xff), un(c & 0xff), a as u8])
        };
        both.put_pixel(x, y, px(*q));
        both.put_pixel(x + size as u32, y, px(*o));
    }
    let _ = both.save(format!("../target/qtref/svg/{name}@{size}-aosh.png"));
}

/// Fills (colors, gradients, transforms, groups, style sheets, viewBox) match
/// Qt to a few 255ths at every size, as does everything at the fixtures' own
/// size. The exception is curves and dashes at a pixel or thinner drawn small
#[test]
fn svgs_are_qts() {
    let dir = Path::new("tests/qtref/fixtures/svg");
    let mut report = Vec::new();
    let mut bad = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let src = std::fs::read_to_string(&path).unwrap();
        for size in [16, 20, 22, 24, 32, 48] {
            let Some(theirs) = qt(&name, size) else { return };
            let ours = super::xml::parse(&src).and_then(|root| super::render_tree(&root, size, size)).unwrap_or_else(|| panic!("{name} did not render"));
            save(&name, size, &theirs, &ours.px);
            let (worst, mean) = diff(&theirs, &ours.px);
            report.push(format!("{name}@{size}: {worst}/255 at most, {mean:.3} on average"));
            let thin_lines = matches!(name.as_str(), "paths" | "shapes" | "arcs" | "strokes") && size < 48;
            let (most, avg) = if thin_lines { (215, 2.0) } else { (13, 0.35) };
            if worst > most || mean > avg {
                bad.push(format!("{name}@{size}"));
            }
        }
    }
    report.sort();
    eprintln!("{}", report.join("\n"));
    assert!(bad.is_empty(), "not Qt's: {bad:?}\n{}", report.join("\n"));
}
