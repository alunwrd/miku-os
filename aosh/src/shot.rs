// `aosh shot`: renders a shell to a PNG with no display server
//
//     aosh shot <shell.qml> [--home D] [--map FROM=TO]... [--size 1920x1080]
//                           [--time 2026-10-07T09:05:00] [--settle 1500] [--out shot.png]
//                           [--geometry where.json] [--profile]
//                           [--against ref.png --crops DIR [--ignore-top 40]]
//                           [--call "target function args"]... [--parts Type,Type...]
//                           [--window "ws,x,y,w,h,app_id,title"]... [--workspace N]
//                           [--type TEXT]
//
// Windows are placed as by a layer-shell compositor: anchors, margins,
// exclusive zones, then band order (Background, Bottom, Top, Overlay) and
// creation order. Blur and window frames are not drawn.
//
// --time: value of `new Date()`. The engine clock steps one frame at a time for
// `--settle` ms
// --map: reads an image directory from the tree
// (`--map /etc/aosh/negi-personal=personal/assets`)
// --home: HOME is a scratch copy of D, removed afterwards
// --against, --crops: compares items with a `configEntryName` (box + 12 px) to
// a reference. DIR gets NAME.png and `scores.txt` (mean channel difference,
// share of pixels off by more than 24)
// --ignore-top: excludes the top N rows from scoring
// --parts: adds a region (box + 4 px) for each shown item of the given types
// --call: IPC call after load, before the clock runs
// (`--call "search workspacesToggle"`)
// --window: a desktop window as `may_desktop` reports it. The first on
// `--workspace` (default 1) has the keyboard
// --type: text typed after the calls
//
// The environment is reduced to PATH, HOME, USER, LOGNAME, SHELL and
// `XDG_CURRENT_DESKTOP=Mayland`

use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::object::engine::Engine;
use crate::object::value::Oid;
use crate::scene::paint::Image;
use crate::scene::text::Fonts;
use crate::scene::{Scene, WinScene};
use crate::types::Kind;

/// A frame's length on the shot's clock
const STEP_MS: u64 = 16;

struct Args {
    file: PathBuf,
    home: Option<PathBuf>,
    map: Vec<(PathBuf, PathBuf)>,
    size: (u32, u32),
    time: Option<String>,
    settle: u64,
    out: PathBuf,
    geometry: Option<PathBuf>,
    profile: bool,
    against: Option<PathBuf>,
    crops: Option<PathBuf>,
    ignore_top: u32,
    calls: Vec<String>,
    parts: Vec<String>,
    windows: Vec<String>,
    workspace: u32,
    typed: String,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut a = Args {
        file: PathBuf::new(), home: None, map: Vec::new(), size: (1920, 1080), time: None, settle: 1500,
        out: PathBuf::from("shot.png"), geometry: None, profile: false, against: None, crops: None, ignore_top: 0,
        calls: Vec::new(), parts: Vec::new(), windows: Vec::new(), workspace: 1,
        typed: String::new(),
    };
    let mut it = args.iter();
    while let Some(x) = it.next() {
        let mut value = || it.next().cloned().ok_or_else(|| format!("{x} wants a value"));
        match x.as_str() {
            "--home" => a.home = Some(PathBuf::from(value()?)),
            "--map" => {
                let v = value()?;
                let (from, to) = v.split_once('=').ok_or("--map is FROM=TO")?;
                let to = PathBuf::from(to).canonicalize().map_err(|e| format!("--map {to}: {e}"))?;
                a.map.push((PathBuf::from(from), to));
            }
            "--size" => {
                let v = value()?;
                let (w, h) = v.split_once('x').ok_or("--size is WIDTHxHEIGHT")?;
                a.size = (w.parse().map_err(|_| "--size is WIDTHxHEIGHT")?, h.parse().map_err(|_| "--size is WIDTHxHEIGHT")?);
            }
            "--time" => a.time = Some(value()?),
            "--settle" => a.settle = value()?.parse().map_err(|_| "--settle is milliseconds")?,
            "--out" => a.out = PathBuf::from(value()?),
            "--geometry" => a.geometry = Some(PathBuf::from(value()?)),
            "--profile" => a.profile = true,
            "--against" => a.against = Some(PathBuf::from(value()?)),
            "--crops" => a.crops = Some(PathBuf::from(value()?)),
            "--ignore-top" => a.ignore_top = value()?.parse().map_err(|_| "--ignore-top is pixels")?,
            "--call" => a.calls.push(value()?),
            "--parts" => a.parts.extend(value()?.split(',').map(str::trim).filter(|t| !t.is_empty()).map(String::from)),
            "--window" => a.windows.push(value()?),
            "--workspace" => a.workspace = value()?.parse().map_err(|_| "--workspace is a number")?,
            "--type" => a.typed = value()?,
            f if !f.starts_with("--") && a.file.as_os_str().is_empty() => a.file = PathBuf::from(f),
            other => return Err(format!("what is {other}?")),
        }
    }
    if a.file.as_os_str().is_empty() {
        return Err("usage: aosh shot <shell.qml> [--home D] [--map FROM=TO] [--size WxH] [--time ISO] [--settle ms] \
                    [--out x.png] [--geometry x.json] [--profile] [--against ref.png --crops DIR [--ignore-top N]] \
                    [--call \"target function args\"]... [--parts Type,Type...]".into());
    }
    if a.against.is_some() != a.crops.is_some() {
        return Err("--against and --crops go together".into());
    }
    Ok(a)
}

pub fn run(args: &[String]) -> i32 {
    let a = match parse(args) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("aosh shot: {e}");
            return 2;
        }
    };
    let scratch = std::env::temp_dir().join(format!("aosh-shot-{}", std::process::id()));
    let r = shoot(&a, &scratch);
    let _ = std::fs::remove_dir_all(&scratch);
    match r {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("aosh shot: {e}");
            1
        }
    }
}

fn shoot(a: &Args, scratch: &Path) -> Result<(), String> {
    let t0 = Instant::now();
    // Single-threaded here, before anything reads the environment
    for (k, _) in std::env::vars_os() {
        if !matches!(k.to_str(), Some("PATH" | "HOME" | "USER" | "LOGNAME" | "SHELL")) {
            std::env::remove_var(&k);
        }
    }
    // And what maysession adds for the programs of a graphical session
    std::env::set_var("XDG_CURRENT_DESKTOP", "Mayland");
    if let Some(home) = &a.home {
        let copy = scratch.join("home");
        copy_tree(home, &copy).map_err(|e| format!("copying {}: {e}", home.display()))?;
        std::env::set_var("HOME", &copy);
    }
    let mut engine = Engine::new()?;
    engine.core.0.borrow_mut().path_map = a.map.clone();
    engine.set_screen(a.size.0, a.size.1);
    let mut services = crate::services::Services::new(&mut engine)?;
    desktop(&mut services, &engine, &a)?;
    if let Some(t) = &a.time {
        set_clock(&engine, t)?;
    }
    let file = a.file.canonicalize().map_err(|e| format!("{}: {e}", a.file.display()))?;
    engine.load_file(&file)?;
    for word in &a.calls {
        if !engine.with_ctx(|ctx, core| crate::ipc::word(ctx, core, word)) {
            return Err(format!("--call {word:?}: no IpcHandler has that target"));
        }
    }
    let loaded = t0.elapsed();
    let mut scene = Scene::new(Fonts::system());

    // Steps the clock one frame at a time, laying out every window each frame.
    // Layout writes properties, which may start animations
    let mut now = 0;
    let mut wins: Vec<(Oid, Placed, WinScene)> = Vec::new();
    let mut told = None;
    loop {
        engine.advance(now);
        let kept = place_windows(&engine, a.size, &mut wins);
        if told != Some(kept) {
            work_area(&mut services, &engine, a.size, kept);
            told = Some(kept);
        }
        for (oid, p, _) in &wins {
            let (oid, w, h) = (*oid, p.w as f64, p.h as f64);
            engine.with_ctx(|ctx, core| scene.layout(ctx, core, oid, w, h));
        }
        // Types at 1 s, once the window that opened has the keyboard
        if !a.typed.is_empty() && now >= 1000 && now < 1000 + STEP_MS {
            // The keyboard goes to the last window that requested it
            let asking = wins.iter().map(|(o, _, _)| *o).filter(|&o| {
                engine.core.is_explicit(o, "WlrLayershell.keyboardFocus")
                    && matches!(engine.core.num(o, "WlrLayershell.keyboardFocus") as i64, 1 | 2)
            }).last();
            engine.with_ctx(|ctx, core| crate::input::keys::keyboard(ctx, core, asking));
            for c in a.typed.chars() {
                engine.key(0, Some(c), 0);
            }
        }
        if now >= a.settle {
            break;
        }
        now += STEP_MS;
    }
    let settled = t0.elapsed();
    let mut screen = Image::new(a.size.0 as i32, a.size.1 as i32);
    wins.sort_by_key(|(_, p, _)| p.band);
    for (oid, p, ws) in &mut wins {
        scene.paint(&engine.core, *oid, ws);
        over(&mut screen, &ws.image, p.x, p.y);
    }
    let painted = t0.elapsed();
    write_png(&screen, &a.out)?;
    if let Some(g) = &a.geometry {
        std::fs::write(g, geometry(&engine)).map_err(|e| format!("{}: {e}", g.display()))?;
    }
    if let (Some(r), Some(dir)) = (&a.against, &a.crops) {
        compare(&engine, &screen, r, dir, a.ignore_top, &a.parts)?;
    }
    for line in engine.take_log() {
        eprintln!("aosh: {line}");
    }
    if a.profile {
        eprintln!("aosh shot: loaded in {} ms, settled {} ms of clock in {} ms, painted {} windows in {} ms",
            loaded.as_millis(), a.settle, (settled - loaded).as_millis(), wins.len(), (painted - settled).as_millis());
    }
    eprintln!("aosh shot: {} windows, {}x{}, {}", wins.len(), a.size.0, a.size.1, a.out.display());
    Ok(())
}

/// A region of the screen to score, by name
struct Region {
    name: String,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

/// Regions for items with `configEntryName` and items of the `parts` types.
/// Repeated names are numbered
fn regions(engine: &Engine, parts: &[String]) -> Vec<Region> {
    let core = &engine.core;
    let ids: Vec<Oid> = core.0.borrow().objs.ids();
    let mut out: Vec<Region> = Vec::new();
    for o in ids {
        let entry = core.peek_named(o, "configEntryName").map(|v| v.to_text()).filter(|n| !n.is_empty());
        let part = part_of(core, o).filter(|t| parts.contains(t));
        let (name, pad) = match (entry, part) {
            (Some(n), _) => (n, 12.0),
            (None, Some(t)) => (t, 4.0),
            (None, None) => continue,
        };
        if !shown(core, o) {
            continue;
        }
        let (x, y) = window_xy(engine, o);
        let (w, h) = (core.num(o, "width"), core.num(o, "height"));
        if w <= 0.0 || h <= 0.0 {
            continue;
        }
        let n = out.iter().filter(|r| r.name == name || r.name.starts_with(&format!("{name}-"))).count();
        let name = if n == 0 { name } else { format!("{name}-{n}") };
        out.push(Region {
            name,
            x: (x - pad).floor() as i32,
            y: (y - pad).floor() as i32,
            w: (w + 2.0 * pad).ceil() as i32,
            h: (h + 2.0 * pad).ceil() as i32,
        });
    }
    out
}

/// Whether an item and all its ancestors are visible and not fully transparent
fn shown(core: &crate::object::Core, o: Oid) -> bool {
    let mut p = Some(o);
    while let Some(q) = p {
        let (item, parent) = {
            let s = core.0.borrow();
            let Some(qo) = s.objs.get(q) else { return true };
            (qo.def.kind != Kind::Window && crate::types::is_a(qo.def, "Item"), qo.parent)
        };
        if !item {
            return true;
        }
        if !core.flag(q, "visible") || core.num(q, "opacity") <= 0.0 {
            return false;
        }
        p = parent;
    }
    true
}

/// An item's type name, or the file name for a Loader with a `source`
/// (`.../ClockWidget.qml` is `ClockWidget`)
fn part_of(core: &crate::object::Core, o: Oid) -> Option<String> {
    let ty = core.0.borrow().objs.get(o)?.ty.clone();
    if ty == "Loader" {
        let source = core.peek_named(o, "source").map(|v| v.to_text()).unwrap_or_default();
        if let Some(stem) = Path::new(&source).file_stem().and_then(|s| s.to_str()) {
            return Some(stem.to_string());
        }
    }
    Some(ty)
}

/// Where an item's corner is in its window, summed up its parents
fn window_xy(engine: &Engine, o: Oid) -> (f64, f64) {
    let s = engine.core.0.borrow();
    let (mut x, mut y, mut p) = (0.0, 0.0, Some(o));
    while let Some(q) = p {
        let Some(qo) = s.objs.get(q) else { break };
        if qo.def.kind == Kind::Window || !crate::types::is_a(qo.def, "Item") {
            break;
        }
        let get = |n: &str| qo.props.iter().find(|pp| &*pp.name == n).map(|pp| pp.value.as_num()).unwrap_or(0.0);
        x += get("x");
        y += get("y");
        p = qo.parent;
    }
    (x, y)
}

/// The shot against the reference, region by region (`--against`)
fn compare(engine: &Engine, shot: &Image, reference: &Path, dir: &Path, ignore_top: u32, parts: &[String]) -> Result<(), String> {
    let r = image::open(reference).map_err(|e| format!("{}: {e}", reference.display()))?.to_rgba8();
    if (r.width() as i32, r.height() as i32) != (shot.w, shot.h) {
        return Err(format!("{} is {}x{}, the shot {}x{}", reference.display(), r.width(), r.height(), shot.w, shot.h));
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let ours = |x: i32, y: i32| -> [u8; 3] {
        let c = crate::scene::paint::to_straight(shot.px[(y * shot.w + x) as usize]);
        [(c >> 16) as u8, (c >> 8) as u8, c as u8]
    };
    let theirs = |x: i32, y: i32| -> [u8; 3] {
        let p = r.get_pixel(x as u32, y as u32).0;
        [p[0], p[1], p[2]]
    };
    let top = ignore_top as i32;
    let score = |g: &Region| -> (f64, f64) {
        let (x0, y0) = (g.x.max(0), g.y.max(top));
        let (x1, y1) = ((g.x + g.w).min(shot.w), (g.y + g.h).min(shot.h));
        let (mut sum, mut seen, mut n) = (0u64, 0u64, 0u64);
        for y in y0..y1 {
            for x in x0..x1 {
                let (a, b) = (ours(x, y), theirs(x, y));
                let d: Vec<u32> = (0..3).map(|k| a[k].abs_diff(b[k]) as u32).collect();
                sum += d.iter().sum::<u32>() as u64;
                seen += (d.iter().any(|&v| v > 24)) as u64;
                n += 1;
            }
        }
        if n == 0 { (0.0, 0.0) } else { (sum as f64 / (3 * n) as f64, seen as f64 * 100.0 / n as f64) }
    };
    let mut lines = Vec::new();
    let mut all = regions(engine, parts);
    all.sort_by(|a, b| a.name.cmp(&b.name));
    for g in &all {
        let (mean, share) = score(g);
        lines.push(format!("{:<14} {:>6.1} mean  {:>5.1}% differ  at {},{} {}x{}", g.name, mean, share, g.x, g.y, g.w, g.h));
        // The reference above, the shot below, a red line between
        let (x0, y0) = (g.x.max(0), g.y.max(0));
        let (w, h) = (((g.x + g.w).min(shot.w) - x0).max(1), ((g.y + g.h).min(shot.h) - y0).max(1));
        let mut pair = image::RgbImage::from_pixel(w as u32, (2 * h + 4) as u32, image::Rgb([255, 0, 0]));
        for y in 0..h {
            for x in 0..w {
                pair.put_pixel(x as u32, y as u32, image::Rgb(theirs(x0 + x, y0 + y)));
                pair.put_pixel(x as u32, (h + 4 + y) as u32, image::Rgb(ours(x0 + x, y0 + y)));
            }
        }
        let p = dir.join(format!("{}.png", g.name));
        pair.save(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let whole = Region { name: "screen".into(), x: 0, y: 0, w: shot.w, h: shot.h };
    let (mean, share) = score(&whole);
    lines.push(format!("{:<14} {:>6.1} mean  {:>5.1}% differ", "screen", mean, share));
    let text = lines.join("\n") + "\n";
    std::fs::write(dir.join("scores.txt"), &text).map_err(|e| format!("{}: {e}", dir.display()))?;
    eprint!("{text}");
    Ok(())
}

/// Sets `new Date()` to `iso`, then advances with real time
fn set_clock(engine: &Engine, iso: &str) -> Result<(), String> {
    let src = format!(r#"(function () {{
        const Real = Date;
        const off = new Real({iso:?}).getTime() - Real.now();
        if (isNaN(off)) throw new Error("--time is not a date: " + {iso:?});
        class ShotDate extends Real {{
            constructor(...a) {{ if (a.length === 0) super(Real.now() + off); else super(...a); }}
            static now() {{ return Real.now() + off; }}
        }}
        globalThis.Date = ShotDate;
    }})()"#);
    engine.with_ctx(|ctx, _| ctx.eval::<(), _>(src).map_err(|e| crate::js::describe(ctx, e)))
}

/// Where a window is on the screen, as a layer-shell compositor puts it
struct Placed {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    band: u32,
}

/// Reports the `--window` and `--workspace` desktop to the shell, as
/// `may_desktop` does
fn desktop(services: &mut crate::services::Services, engine: &Engine, a: &Args) -> Result<(), String> {
    use libmay::conn::Event;
    let text = |s: &str| {
        let mut b = [0u8; 32];
        let n = s.len().min(32);
        b[..n].copy_from_slice(&s.as_bytes()[..n]);
        (b, n)
    };
    services.desk(engine, &Event::DeskWorkspace { index: a.workspace });
    let mut focused = None;
    for (i, spec) in a.windows.iter().enumerate() {
        let f: Vec<&str> = spec.splitn(7, ',').collect();
        let num = |k: usize| f.get(k).and_then(|v| v.trim().parse::<i32>().ok());
        let (Some(ws), Some(x), Some(y), Some(width), Some(height), Some(app), Some(title)) =
            (num(0), num(1), num(2), num(3), num(4), f.get(5), f.get(6))
        else {
            return Err(format!("--window {spec:?} is ws,x,y,w,h,app_id,title"));
        };
        let ((title, title_len), (app_id, app_id_len)) = (text(title), text(app.trim()));
        let surface = i as u32 + 1;
        services.desk(engine, &Event::DeskWindow {
            client: 1, surface, title, title_len, app_id, app_id_len, x, y, width, height,
            flags: 0, workspace: ws as u32,
        });
        if focused.is_none() && ws as u32 == a.workspace {
            focused = Some(surface);
        }
    }
    if let Some(surface) = focused {
        services.desk(engine, &Event::DeskFocused { client: 1, surface });
    }
    Ok(())
}

/// What the layers keep of each edge, told the shell as the work area
fn work_area(services: &mut crate::services::Services, engine: &Engine, screen: (u32, u32), kept: [i32; 4]) {
    let (sw, sh) = (screen.0 as i32, screen.1 as i32);
    services.desk(engine, &libmay::conn::Event::DeskWorkArea {
        x: kept[0], y: kept[1], width: sw - kept[0] - kept[2], height: sh - kept[1] - kept[3],
        screen_w: sw, screen_h: sh,
    });
}

/// Places every visible window and drops the scene of a closed one. Returns the
/// reserved edges: left, top, right, bottom
fn place_windows(engine: &Engine, screen: (u32, u32), wins: &mut Vec<(Oid, Placed, WinScene)>) -> [i32; 4] {
    let core = &engine.core;
    let live: Vec<Oid> = {
        let s = core.0.borrow();
        s.objs.ids().into_iter().filter(|&o| s.objs[o].def.kind == Kind::Window).collect()
    };
    wins.retain(|(o, _, _)| live.contains(o) && core.flag(*o, "visible"));
    // Reserved edges, as in `server::reserve_edges`: left, top, right, bottom
    let mut kept = [0i32; 4];
    for &oid in live.iter().filter(|&&o| core.flag(o, "visible")) {
        let n = crate::mayland::exclusive_of(core, oid);
        let flag = |a: &str| core.flag(oid, a);
        let (l, r, t, b) = (flag("anchors.left"), flag("anchors.right"), flag("anchors.top"), flag("anchors.bottom"));
        if n <= 0 {
            continue;
        }
        if t && !b { kept[1] += n } else if b && !t { kept[3] += n } else if l && !r { kept[0] += n } else if r && !l { kept[2] += n }
    }
    for oid in live.into_iter().filter(|&o| core.flag(o, "visible")) {
        let flag = |n: &str| core.flag(oid, n);
        let m = |n: &str| core.num(oid, n) as i32;
        // A window keeping clear of the others is placed in what they leave
        let (ox, oy, sw, sh) = if crate::mayland::exclusive_of(core, oid) == 0 {
            (kept[0], kept[1], screen.0 as i32 - kept[0] - kept[2], screen.1 as i32 - kept[1] - kept[3])
        } else {
            (0, 0, screen.0 as i32, screen.1 as i32)
        };
        let (l, r, t, b) = (flag("anchors.left"), flag("anchors.right"), flag("anchors.top"), flag("anchors.bottom"));
        let size = |stretched: bool, implicit: &str, explicit: &str, room: i32| -> i32 {
            if stretched {
                room
            } else if core.is_explicit(oid, explicit) {
                core.num(oid, explicit) as i32
            } else {
                core.num(oid, implicit) as i32
            }
        };
        let w = size(l && r, "implicitWidth", "implicitWidth", sw - m("margins.left") - m("margins.right")).max(1);
        let h = size(t && b, "implicitHeight", "implicitHeight", sh - m("margins.top") - m("margins.bottom")).max(1);
        let x = ox + if l { m("margins.left") } else if r { sw - w - m("margins.right") } else { (sw - w) / 2 };
        let y = oy + if t { m("margins.top") } else if b { sh - h - m("margins.bottom") } else { (sh - h) / 2 };
        let band = if core.is_explicit(oid, "WlrLayershell.layer") {
            core.num(oid, "WlrLayershell.layer") as u32
        } else if flag("aboveWindows") {
            2
        } else {
            1
        };
        let placed = Placed { x, y, w, h, band };
        match wins.iter_mut().find(|(o, _, _)| *o == oid) {
            Some((_, p, ws)) => {
                if (p.w, p.h) != (w, h) {
                    *ws = WinScene::new(w, h);
                }
                *p = placed;
            }
            None => wins.push((oid, placed, WinScene::new(w, h))),
        }
    }
    kept
}

/// `src`, premultiplied, over `dst` at `(x, y)`
fn over(dst: &mut Image, src: &Image, x: i32, y: i32) {
    for sy in 0..src.h {
        let dy = y + sy;
        if dy < 0 || dy >= dst.h {
            continue;
        }
        for sx in 0..src.w {
            let dx = x + sx;
            if dx < 0 || dx >= dst.w {
                continue;
            }
            let s = src.px[(sy * src.w + sx) as usize];
            let sa = s >> 24;
            if sa == 0 {
                continue;
            }
            let d = &mut dst.px[(dy * dst.w + dx) as usize];
            if sa == 255 {
                *d = s;
                continue;
            }
            let k = 255 - sa;
            let ch = |sh: u32| {
                let v = ((s >> sh) & 0xff) + ((((*d >> sh) & 0xff) * k + 127) / 255);
                v.min(255) << sh
            };
            *d = ch(24) | ch(16) | ch(8) | ch(0);
        }
    }
}

fn write_png(img: &Image, path: &Path) -> Result<(), String> {
    let mut rgba = image::RgbaImage::new(img.w as u32, img.h as u32);
    for (i, p) in rgba.pixels_mut().enumerate() {
        let c = crate::scene::paint::to_straight(img.px[i]);
        *p = image::Rgba([(c >> 16) as u8, (c >> 8) as u8, c as u8, (c >> 24) as u8]);
    }
    rgba.save_with_format(path, image::ImageFormat::Png).map_err(|e| format!("{}: {e}", path.display()))
}

/// One line per item with its window position:
/// `{"type", "id", "at", "x", "y", "w", "h", "visible"}`
fn geometry(engine: &Engine) -> String {
    let core = &engine.core;
    let s = core.0.borrow();
    // Object id from its creation scope
    let id_of = |o: Oid| -> Option<String> {
        let cx = s.objs.get(o)?.cx;
        s.cxs.get(cx as usize)?.ids.iter().find(|(_, &v)| v == o).map(|(k, _)| k.clone())
    };
    let mut out = String::from("[\n");
    let mut first = true;
    for o in s.objs.ids() {
        if !crate::types::is_a(s.objs[o].def, "Item") {
            continue;
        }
        let at = s.place(s.objs[o].file, s.objs[o].loc);
        let ty = s.objs[o].ty.clone();
        let id = id_of(o);
        let num = |n: &str| s.objs[o].props.iter().find(|p| &*p.name == n).map(|p| p.value.as_num()).unwrap_or(0.0);
        let vis = s.objs[o].props.iter().find(|p| &*p.name == "visible").is_some_and(|p| p.value.truthy());
        // Top left corner in window coordinates, with rotation and scale
        let (x, y) = crate::scene::xf::to_window(core, o).map(0.0, 0.0);
        if !first {
            out.push_str(",\n");
        }
        first = false;
        out.push_str(&format!(
            "  {{\"type\": {}, \"id\": {}, \"at\": {}, \"x\": {}, \"y\": {}, \"w\": {}, \"h\": {}, \"visible\": {vis}, \"opacity\": {}, \"rotation\": {}, \"scale\": {}}}",
            json_str(&ty), id.map(|i| json_str(&i)).unwrap_or_else(|| "null".into()), json_str(&at),
            num_json(x), num_json(y), num_json(num("width")), num_json(num("height")), num_json(num("opacity")),
            num_json(num("rotation")), num_json(num("scale")),
        ));
    }
    out.push_str("\n]\n");
    out
}

fn num_json(v: f64) -> String {
    if v.is_finite() { format!("{v}") } else { "null".into() }
}

fn json_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Copies `from` to `to` recursively, following links
fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        let dst = to.join(e.file_name());
        if p.is_dir() {
            copy_tree(&p, &dst)?;
        } else {
            std::fs::copy(&p, &dst)?;
        }
    }
    Ok(())
}

/// negi frame cost on the host, painted like the shell's loop at 240 Hz:
///
///     CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test-host --target-dir ../target/aosh-bench \
///         bench:: -- --ignored --nocapture
///
/// Requires `personal/`. The background is hidden for 3 s
/// (`startupLockPending`), so the clock starts past that
#[cfg(test)]
mod bench {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use crate::scene::paint::Rect;

    struct Negi {
        a: Args,
        engine: Engine,
        services: crate::services::Services,
        scene: Scene,
        wins: Vec<(Oid, Placed, WinScene)>,
        told: Option<[i32; 4]>,
        now: u64,
        scratch: PathBuf,
        /// `AOSH_BENCH_GPU=1`: records frames and runs them on the host's card
        gpu: Option<Gpu>,
    }

    struct Gpu {
        cuda: crate::scene::gpu::check::Cuda,
        rec: crate::scene::gpu::record::Record,
        mem: crate::scene::gpu::check::HostMemory,
        /// Cost of the last frame: listing, recording, card
        took: (Duration, Duration, Duration),
    }

    impl Negi {
        fn up() -> Negi {
            Negi::up_with(&[])
        }

        /// With `--window`s, as `may_desktop` tells the shell of them
        fn up_with(windows: &[&str]) -> Negi {
            let mut args: Vec<String> = ["../etc/aosh/negi/shell.qml", "--home", "../personal/home",
                "--map", "/etc/aosh/negi-personal=../personal/assets", "--time", "2026-10-20T20:57:00"]
                .iter().map(|s| s.to_string()).collect();
            for w in windows {
                args.push("--window".into());
                args.push(w.to_string());
            }
            let a = parse(&args).unwrap();
            let scratch = std::env::temp_dir().join(format!("aosh-bench-{}", std::process::id()));
            copy_tree(a.home.as_ref().unwrap(), &scratch.join("home")).unwrap();
            std::env::set_var("HOME", scratch.join("home"));
            std::env::set_var("XDG_CURRENT_DESKTOP", "Mayland");
            let mut engine = Engine::new().unwrap();
            engine.core.0.borrow_mut().path_map = a.map.clone();
            engine.set_screen(a.size.0, a.size.1);
            let mut services = crate::services::Services::new(&mut engine).unwrap();
            desktop(&mut services, &engine, &a).unwrap();
            set_clock(&engine, a.time.as_ref().unwrap()).unwrap();
            engine.load_file(&a.file.canonicalize().unwrap()).unwrap();
            let gpu = std::env::var("AOSH_BENCH_GPU").is_ok_and(|v| v == "1").then(|| {
                let cuda = crate::scene::gpu::check::Cuda::open().expect("CUDA");
                let mem = crate::scene::gpu::check::HostMemory::new();
                let shared = crate::scene::gpu::check::HostMemory(mem.0.clone());
                Gpu { cuda, rec: crate::scene::gpu::record::Record::new(Box::new(mem)), mem: shared, took: Default::default() }
            });
            let mut n = Negi { a, engine, services, scene: Scene::new(Fonts::system()), wins: Vec::new(), told: None,
                               now: 0, scratch, gpu };
            while n.now < 4000 {
                n.frame(STEP_MS);
            }
            n
        }

        /// Lays out and paints every window `step` ms after the last frame;
        /// returns the cost
        fn frame(&mut self, step: u64) -> (Duration, Duration, Vec<Rect>) {
            self.engine.advance(self.now);
            self.now += step;
            let kept = place_windows(&self.engine, self.a.size, &mut self.wins);
            if self.told != Some(kept) {
                work_area(&mut self.services, &self.engine, self.a.size, kept);
                self.told = Some(kept);
            }
            let t = Instant::now();
            for (oid, p, _) in self.wins.iter() {
                let (oid, w, h, scene) = (*oid, p.w as f64, p.h as f64, &mut self.scene);
                self.engine.with_ctx(|ctx, core| scene.layout(ctx, core, oid, w, h));
            }
            let laid = t.elapsed();
            let mut painted = Vec::new();
            if let Some(g) = self.gpu.as_mut() {
                // One run per frame, as on MikuOS
                let (mut listing, mut recording) = (Duration::ZERO, Duration::ZERO);
                g.rec.frame();
                let mut at = 0u32;
                for (oid, _, ws) in self.wins.iter_mut() {
                    let l = Instant::now();
                    let damage = self.scene.list(&self.engine.core, *oid, ws);
                    listing += l.elapsed();
                    let r = Instant::now();
                    let target = g.rec.target(0, at, ws.image.w as u32, ws.image.w as u32, ws.image.h as u32);
                    let cpu = self.scene.record(&self.engine.core, ws, &damage, &mut g.rec, target);
                    recording += r.elapsed();
                    assert!(cpu.is_empty(), "rectangles the processor painted: {cpu:?}");
                    at += (ws.image.w * ws.image.h * 4) as u32;
                    painted.extend(damage);
                }
                let r = Instant::now();
                let words = g.rec.words();
                recording += r.elapsed();
                let c = Instant::now();
                let tex_bytes: usize = g.mem.0.borrow().dirty.iter().map(|d| d.1).sum();
                g.cuda.textures(&g.mem).unwrap();
                let tex_time = c.elapsed();
                g.cuda.go(&words, g.rec.tiles(), at as usize, None).unwrap();
                g.took = (listing, recording, c.elapsed());
                if std::env::var("AOSH_BENCH_GPU_SPLIT").is_ok() {
                    eprintln!("      textures {} KB in {:.2} ms, commands {} KB in {:.2} ms, {} tiles run in {:.2} ms",
                              tex_bytes / 1024, tex_time.as_secs_f64() * 1e3, words.len() * 4 / 1024,
                              g.cuda.last_upload.as_secs_f64() * 1e3, g.rec.tiles(), g.cuda.last_run.as_secs_f64() * 1e3);
                }
                return (laid, listing + recording, painted);
            }
            for (oid, _, ws) in self.wins.iter_mut() {
                painted.extend(self.scene.paint(&self.engine.core, *oid, ws));
            }
            (laid, t.elapsed() - laid, painted)
        }

        fn find(&self, prop: &str, value: &str) -> Oid {
            self.engine.core.0.borrow().objs.ids().into_iter()
                .find(|&o| self.engine.core.peek_named(o, prop).map(|v| v.to_text()).as_deref() == Some(value))
                .unwrap_or_else(|| panic!("nothing has {prop} {value}"))
        }

        /// `frames` 4 ms frames, each after `each`, said and summed up
        fn run(&mut self, what: &str, frames: usize, mut each: impl FnMut(&Negi, usize)) {
            let ms = |d: Duration| d.as_secs_f64() * 1e3;
            let (mut worst, mut sum) = (0.0f64, 0.0);
            for i in 0..frames {
                each(self, i);
                let at = self.now;
                let (laid, paint, painted) = self.frame(4);
                let gpu = self.gpu.as_ref().map(|g| format!(" (list {:.2}, record {:.2}, card {:.2})", ms(g.took.0), ms(g.took.1), ms(g.took.2)))
                    .unwrap_or_default();
                eprintln!("{:>5} ms: layout {:>6.2} ms, paint {:>6.2} ms{gpu} over {:?}", at, ms(laid), ms(paint),
                    painted.iter().map(|d| (d.w, d.h)).collect::<Vec<_>>());
                worst = worst.max(ms(laid + paint));
                sum += ms(laid + paint);
            }
            eprintln!("{what}: {frames} frames, mean {:.1} ms, worst {:.1} ms", sum / frames as f64, worst);
        }
    }

    impl Drop for Negi {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.scratch);
        }
    }

    /// The Notes card flipping to a new note and back
    #[test]
    #[ignore]
    fn notes_flip_cost() {
        let mut n = Negi::up();
        let notes = n.find("configEntryName", "notes");
        n.engine.eval(notes, "openNewNote()").unwrap();
        n.run("flip", 90, |_, _| {});
    }

    /// Frame cost and object count before and after dragging a widget with the
    /// grid on
    #[test]
    #[ignore]
    fn desktop_drag_cost() {
        let mut n = Negi::up();
        let canvas = n.find("isWidgetCanvas", "true");
        let objects = |n: &Negi| n.engine.core.0.borrow().objs.ids().len();
        eprintln!("windows: {:?}", n.wins.iter().map(|(o, p, _)| (n.engine.core.text(*o, "WlrLayershell.namespace"), p.w, p.h)).collect::<Vec<_>>());
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let idle = |n: &mut Negi| (0..20).map(|_| ms(n.frame(4).0)).sum::<f64>() / 20.0;
        eprintln!("before: {} objects, layout {:.2} ms", objects(&n), idle(&mut n));
        for round in 0..3 {
            n.engine.eval(canvas, "setDragging(true)").unwrap();
            n.run("drag", 10, |_, _| {});
            eprintln!("  dragging: {} objects", objects(&n));
            n.engine.eval(canvas, "(setDragging(false), flashLines([960], [540]))").unwrap();
            for _ in 0..700 {
                n.frame(4);
            }
            eprintln!("after drag {}: {} objects, layout {:.2} ms", round + 1, objects(&n), idle(&mut n));
        }
    }

    /// One minute with two windows open, drawing only on change, with periodic
    /// sampling, flips and selections: layout cost per window over time
    #[test]
    #[ignore]
    fn desktop_session_cost() {
        let mut n = Negi::up_with(&["1,4,44,954,1032,mayterm,console", "1,962,44,954,1032,mayterm,shell"]);
        let notes = n.find("configEntryName", "notes");
        let canvas = n.find("isWidgetCanvas", "true");
        let ms = |d: Duration| d.as_secs_f64() * 1e3;
        let mut worst: HashMap<String, (f64, f64, u32)> = HashMap::new();
        let mut next_sample = n.now;
        let end = n.now + 60_000;
        while n.now < end {
            let t = n.now;
            if t >= next_sample {
                n.services.sample(&n.engine);
                next_sample = t + 1000;
            }
            if t % 10_000 < 4 {
                n.engine.eval(notes, "openNewNote()").unwrap();
            }
            if t % 15_000 < 4 {
                n.engine.eval(canvas, "(selecting = true, selectionStartPoint = Qt.point(60, 80))").unwrap();
            }
            if t % 15_000 >= 4 && t % 15_000 < 400 {
                let k = (t % 15_000) as f64;
                n.engine.eval(canvas, &format!("(selectionRect = Qt.rect(60, 80, {k}, {}), selectWithinRect(selectionRect))", k * 0.6)).unwrap();
            }
            if t % 15_000 >= 400 && t % 15_000 < 404 {
                n.engine.eval(canvas, "selecting = false").unwrap();
            }
            n.engine.advance(t);
            n.now += 4;
            if !(n.engine.core.0.borrow().dirty || n.engine.animating()) {
                continue;
            }
            n.engine.core.0.borrow_mut().dirty = false;
            let kept = place_windows(&n.engine, n.a.size, &mut n.wins);
            if n.told != Some(kept) {
                work_area(&mut n.services, &n.engine, n.a.size, kept);
                n.told = Some(kept);
            }
            for (oid, p, ws) in n.wins.iter_mut() {
                let (oid, w, h, scene) = (*oid, p.w as f64, p.h as f64, &mut n.scene);
                let l = Instant::now();
                n.engine.with_ctx(|ctx, core| scene.layout(ctx, core, oid, w, h));
                let laid = ms(l.elapsed());
                scene.paint(&n.engine.core, oid, ws);
                let name = n.engine.core.text(oid, "WlrLayershell.namespace");
                let e = worst.entry(name).or_insert((0.0, 0.0, 0));
                e.0 = e.0.max(laid);
                e.1 += laid;
                e.2 += 1;
            }
            if t % 5000 < 4 || (t + 4) / 5000 != t / 5000 {
                let objects = n.engine.core.0.borrow().objs.ids().len();
                let mut names: Vec<_> = worst.drain().collect();
                names.sort_by(|a, b| a.0.cmp(&b.0));
                eprintln!("{:>6} ms, {objects} objects: {}", t, names.iter()
                    .map(|(k, (w, sum, c))| format!("{k} {c} layouts, mean {:.1} worst {:.1}", sum / *c as f64, w))
                    .collect::<Vec<_>>().join("; "));
            }
        }
    }

    /// Overview opening (`search toggle`) with two windows, then closing
    #[test]
    #[ignore]
    fn overview_open_cost() {
        let mut n = Negi::up_with(&["1,4,44,954,1032,mayterm,console", "1,962,44,954,1032,mayterm,shell"]);
        for round in 0..2 {
            // Toggle cost before the first frame: bindings and the Loader
            // building the grid
            let t = std::time::Instant::now();
            assert!(n.engine.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "search toggle")));
            eprintln!("open {}: the toggle took {:.1} ms", round + 1, t.elapsed().as_secs_f64() * 1e3);
            n.run(&format!("open {}", round + 1), 120, |_, _| {});
            assert!(n.engine.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "search toggle")));
            n.run(&format!("close {}", round + 1), 120, |_, _| {});
        }
    }

    /// Ctrl+2 and back: workspace change and the bar's highlight
    #[test]
    #[ignore]
    fn workspace_switch_cost() {
        let mut n = Negi::up_with(&["1,4,44,1912,1152,mayterm,console", "2,4,44,1912,1152,mayterm,shell"]);
        for (round, to) in [2u32, 1, 2].into_iter().enumerate() {
            let t = std::time::Instant::now();
            n.services.desk(&n.engine, &libmay::conn::Event::DeskWorkspace { index: to });
            eprintln!("switch {}: the event took {:.1} ms", round + 1, t.elapsed().as_secs_f64() * 1e3);
            n.run(&format!("switch {} to {to}", round + 1), 90, |_, _| {});
        }
    }

    /// Selection rectangle dragged across the desktop, one pointer motion per
    /// frame
    #[test]
    #[ignore]
    fn desktop_select_cost() {
        let mut n = Negi::up();
        let canvas = n.find("isWidgetCanvas", "true");
        n.engine.eval(canvas, "(selecting = true, selectionStartPoint = Qt.point(60, 80), clearSelection())").unwrap();
        n.run("select", 60, |n, i| {
            let (w, h) = (20.0 * (i + 1) as f64, 12.0 * (i + 1) as f64);
            n.engine.eval(canvas, &format!("(selectionRect = Qt.rect(60, 80, {w}, {h}), selectWithinRect(selectionRect))")).unwrap();
        });
    }
}
