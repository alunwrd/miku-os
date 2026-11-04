// Window contents from items
//
// Two passes. Layout writes geometry back to item properties, as QtQuick does,
// and repeats until a round moves nothing, since writes run bindings. The
// display list holds visible items in paint order with a fingerprint of what
// each draws; comparing it with the last list gives the rectangles to repaint.

pub mod canvas;
pub mod control;
mod effect;
pub mod icons;
pub mod gpu;
pub mod paint;
pub mod picture;
mod rectangle;
mod shapes;
pub mod svg;
pub mod text;
pub mod xf;

use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use rquickjs::Ctx;

use crate::object::value::Oid;
use crate::object::Core;
use crate::types;
use paint::{Image, Painter, Rect};
use xf::Xf;
use picture::{Picture, Pictures, Want};
use text::layout::{lay, Opts};
use text::{Font, Fonts, Request};

/// Layout rounds before giving up
const ROUNDS: usize = 8;

/// What an item paints, kept between frames
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Draw {
    Rect,
    Text,
    /// The window's own background
    Fill,
    /// Paints nothing but takes the pointer
    Area,
    /// ListView: paints nothing, scrolled by the wheel
    View,
    Image,
    /// TextInput: text or placeholder, and the caret
    Input,
    /// TextEdit or TextArea: lines as a Text, placeholder, and the caret
    Edit,
    /// A Canvas's picture (`canvas/`)
    Canvas,
    /// A Shape's picture (`shapes.rs`)
    Shape,
    /// Item drawn through its layer: into a texture, then directly or through
    /// `layer.effect` (`effect/`)
    Layer,
    /// A mask, a blur, a shadow, a ShaderEffectSource (`effect/`)
    Effect,
    /// Control (`control.rs`): its background and content paint; can be
    /// hovered, and a button pressed
    Control,
}

#[derive(Clone)]
struct Shown {
    oid: Oid,
    draw: Draw,
    /// The pixels it may touch
    rect: Rect,
    /// From the item's own coordinates to the window's (`xf.rs`)
    xf: Xf,
    /// Its width and height, in its own coordinates
    size: (f64, f64),
    clip: Rect,
    opacity: f32,
    print: u64,
    /// False when its layer draws it; the pointer can still find it
    paint: bool,
    /// For a solid, square, axis-aligned Rectangle: a fingerprint excluding
    /// geometry, and its edge width in pixels. A change in size or position
    /// only damages what moved and its edges (`changed`)
    flat: Option<(u64, i32)>,
}

impl Shown {
    /// Whether a window point is inside its box (edges included, as
    /// QQuickItem::contains) and its clip
    fn under(&self, x: f64, y: f64) -> bool {
        let c = &self.clip;
        if x < c.x as f64 || y < c.y as f64 || x >= c.right() as f64 || y >= c.bottom() as f64 {
            return false;
        }
        let Some((lx, ly)) = self.xf.inverse().map(|i| i.map(x, y)) else { return false };
        lx >= 0.0 && ly >= 0.0 && lx <= self.size.0 && ly <= self.size.1
    }
}

/// One window's picture of itself
pub struct WinScene {
    pub image: Image,
    shown: Vec<Shown>,
    /// The next paint covers everything: new memory or a size change
    fresh: bool,
    /// Each item's entry run in `shown` by object slot, letting the next list
    /// copy unchanged runs (`Scene::collect`)
    nodes: Vec<Option<Node>>,
    /// List number of `shown`, from 1, and the store count when it was made
    list: u32,
    drawn: u64,
}

impl WinScene {
    pub fn new(w: i32, h: i32) -> WinScene {
        WinScene { image: Image::new(w, h), shown: Vec::new(), fresh: true, nodes: Vec::new(), list: 0, drawn: 0 }
    }
}

/// One item's run in a display list, with the inputs that produced it
#[derive(Clone, Copy)]
struct Node {
    oid: Oid,
    /// Which list `start..end` is in
    list: u32,
    start: u32,
    end: u32,
    /// Parent matrix, clip, inherited opacity, and whether it paints in place
    given: ([u64; 6], Rect, u32, bool),
    /// The run has a layer or effect, which reads other items, so it is rebuilt
    /// every time
    reads_others: bool,
}

/// Previous list of the window being listed, for `collect` to copy from
struct Keep {
    last: Vec<Shown>,
    nodes: Vec<Option<Node>>,
    /// The number of the list being made
    list: u32,
    /// Rebuilds what changed after this store count. Zero copies nothing
    since: u64,
}

pub struct Scene {
    pub fonts: Fonts,
    pictures: Pictures,
    canvases: canvas::Canvases,
    effects: effect::Effects,
    shapes: shapes::Shapes,
    /// Each ScreencopyView's last capture and capture count (part of its print)
    captures: HashMap<Oid, (Rc<Picture>, u64)>,
    /// Each window's store count after its last layout, and its font epoch
    laid: HashMap<Oid, (u64, u64)>,
    /// Number of font additions; changes metrics for every Text
    font_epoch: u64,
    /// The current layout skips what has not changed since this count. Zero
    /// skips nothing
    since: u64,
    /// The window being listed's last list, while it is (`paint`)
    keep: Option<Keep>,
    /// Items anchored to neither their parent nor a sibling, with their anchor
    /// targets (`mark_far`)
    far: HashMap<Oid, Vec<Oid>>,
}

impl Scene {
    pub fn new(fonts: Fonts) -> Scene {
        Scene {
            fonts, pictures: Pictures::default(), canvases: canvas::Canvases::default(), effects: effect::Effects::default(),
            shapes: shapes::Shapes::default(), captures: HashMap::new(), laid: HashMap::new(), font_epoch: 0,
            since: 0, far: HashMap::new(), keep: None,
        }
    }

    /// A ScreencopyView's pixels, or none any more (`mayland/capture.rs`)
    pub fn set_capture(&mut self, core: &Core, view: Oid, pic: Option<Rc<Picture>>) {
        // Pixels are not a property, so the view is marked directly
        if core.0.borrow().objs.is_live(view) {
            core.touch(view);
        }
        match pic {
            Some(p) => {
                let n = self.captures.get(&view).map_or(0, |c| c.1 + 1);
                self.captures.insert(view, (p, n));
            }
            None => {
                self.captures.remove(&view);
            }
        }
    }

    /// An Image's picture at its `sourceSize`, decoded on first request
    /// (`picture.rs`). Only a file-given size counts; the scene's write-back is
    /// the picture's own size
    fn picture(&mut self, core: &Core, oid: Oid) -> Option<Rc<Picture>> {
        if Self::kind_name(core, oid) == "ScreencopyView" {
            return self.captures.get(&oid).map(|c| c.0.clone());
        }
        let mut src = core.text(oid, "source");
        if src.is_empty() {
            return None;
        }
        let def = core.0.borrow().objs[oid].def;
        let side = |n: &str| if core.is_explicit(oid, n) { core.num(oid, n).max(0.0) as u32 } else { 0 };
        // A Kirigami Icon's source may be a name from the icon theme
        if types::is_a(def, "Icon") && !src.contains(':') && !src.starts_with('/') {
            src = format!("image://icon/{src}");
        }
        // Icon size: an IconImage's smaller side, a Kirigami Icon's size, an
        // Image's `sourceSize`, else 100x100 (quickshell's provider default)
        if let Some(id) = src.strip_prefix("image://icon/") {
            let (w, h) = if types::is_a(def, "IconImage") {
                let s = core.num(oid, "width").min(core.num(oid, "height")).max(0.0) as u32;
                (s, s)
            } else if types::is_a(def, "Icon") {
                (core.num(oid, "width").max(0.0).round() as u32, core.num(oid, "height").max(0.0).round() as u32)
            } else if core.is_explicit(oid, "sourceSize.width") || core.is_explicit(oid, "sourceSize.height") {
                (side("sourceSize.width"), side("sourceSize.height"))
            } else {
                (100, 100)
            };
            return self.pictures.icon(icons::Icons::global(), id, w, h);
        }
        let path = core.path(oid, &src);
        let want = Want {
            w: side("sourceSize.width"),
            h: side("sourceSize.height"),
            keep: matches!(core.num(oid, "fillMode") as i64, 1 | 2),
        };
        let (pic, err) = self.pictures.get(&path, want);
        if let Some(e) = err {
            core.log(format!("Image: {e}"));
        }
        pic
    }

    /// Lays out `win` at `w` by `h`, covering what changed since its last
    /// layout and what that moves
    pub fn layout(&mut self, ctx: &Ctx<'_>, core: &Core, win: Oid, w: f64, h: f64) {
        if !core.0.borrow().objs.is_live(win) {
            return;
        }
        core.store_layout(ctx, win, "width", w);
        core.store_layout(ctx, win, "height", h);
        // Files FontLoaders read since the last layout
        let added = std::mem::take(&mut core.0.borrow_mut().font_files);
        if !added.is_empty() {
            self.font_epoch += 1;
        }
        for f in added {
            self.fonts.add_file(&f, true);
        }
        let since = match self.laid.get(&win) {
            Some(&(clean, fonts)) if fonts == self.font_epoch => clean,
            _ => 0,
        };
        self.mark_far(core, since);
        let canvas_asked = !core.0.borrow().canvas_due.is_empty();
        if since > 0 && below(core, win) <= since && !canvas_asked {
            return;
        }
        self.since = since;
        self.rounds(ctx, core, win);
        // With sizes known, sizes each layer's effect and paints due Canvases
        self.layer_effects(ctx, core, win);
        self.canvases.paint_due(ctx, core, win, since);
        self.since = 0;
        #[cfg(test)]
        if since > 0 {
            self.check_layout(ctx, core, win);
        }
        self.laid.insert(win, (core.0.borrow().changes, self.font_epoch));
    }

    /// Repeats both passes until a round changes nothing
    fn rounds(&mut self, ctx: &Ctx<'_>, core: &Core, win: Oid) {
        for round in 0..ROUNDS {
            let before = core.0.borrow().changes;
            self.implicit(ctx, core, win, None);
            self.place(ctx, core, win);
            if core.0.borrow().changes == before {
                break;
            }
            if round == ROUNDS - 1 {
                core.log(format!("the layout of {} did not settle in {ROUNDS} rounds - a binding \
                                  on a size the layout sets?", core.0.borrow().objs[win].ty));
            }
        }
    }

    /// Whether the current layout enters `oid`: it or its subtree changed since
    /// the last layout
    fn due(&self, core: &Core, oid: Oid) -> bool {
        self.since == 0 || below(core, oid) > self.since
    }

    /// Marks far-anchored items whose target changed since `since`, up their
    /// own tree
    fn mark_far(&mut self, core: &Core, since: u64) {
        self.far.retain(|&c, _| core.0.borrow().objs.is_live(c));
        let moved: Vec<Oid> = self.far.iter()
            .filter(|(_, to)| to.iter().any(|&t| changed_at(core, t) > since))
            .map(|(&c, _)| c)
            .collect();
        for c in moved {
            if changed_at(core, c) <= since {
                core.touch(c);
            }
        }
    }

    /// Test check: after a partial layout, a whole layout must change nothing
    /// (`AOSH_SCENE_CHECK=0` disables it)
    #[cfg(test)]
    fn check_layout(&mut self, ctx: &Ctx<'_>, core: &Core, win: Oid) {
        if !scene_check() {
            return;
        }
        let before = core.0.borrow().changes;
        self.rounds(ctx, core, win);
        self.layer_effects(ctx, core, win);
        let s = core.0.borrow();
        if s.changes != before {
            let moved: Vec<String> = s.objs.ids().into_iter()
                .filter_map(|o| s.objs.get(o).filter(|x| x.changed > before).map(|x| format!("{} ({})", x.ty, s.place(x.file, x.loc))))
                .take(12)
                .collect();
            panic!("a whole layout after one that passed over what had not changed changed {} thing(s): {}",
                   s.changes - before, moved.join(", "));
        }
    }

    fn children(core: &Core, oid: Oid) -> Vec<Oid> {
        core.0.borrow().objs[oid].children.clone()
    }

    fn is_item(core: &Core, oid: Oid) -> bool {
        types::is_a(core.0.borrow().objs[oid].def, "Item")
    }

    fn kind_name(core: &Core, oid: Oid) -> &'static str {
        let def = core.0.borrow().objs[oid].def;
        Self::kind_of(def)
    }

    /// Nearest kind the layout and painter distinguish, cached per type
    /// (`types::chain`)
    fn kind_of(def: &'static types::TypeDef) -> &'static str {
        thread_local! {
            static KINDS: std::cell::RefCell<HashMap<usize, &'static str>> = std::cell::RefCell::new(HashMap::new());
        }
        let key = def as *const types::TypeDef as usize;
        if let Some(k) = KINDS.with(|k| k.borrow().get(&key).copied()) {
            return k;
        }
        let k = Self::kind_uncached(def);
        KINDS.with(|m| m.borrow_mut().insert(key, k));
        k
    }

    fn kind_uncached(def: &'static types::TypeDef) -> &'static str {
        for t in types::chain(def) {
            if matches!(t.name, "Text" | "Row" | "Column" | "Grid" | "Flow" | "RowLayout" | "ColumnLayout" | "GridLayout" | "Rectangle" | "MouseArea"
                | "PanelWindow" | "Repeater" | "ListView" | "Loader" | "Image" | "TextEdit" | "TextInput" | "Canvas"
                | "OpacityMask" | "FastBlur" | "GaussianBlur" | "DropShadow" | "RectangularShadow" | "ShaderEffectSource"
                | "RadialGradient" | "MultiEffect" | "ScreencopyView" | "Control" | "Shape")
            {
                return t.name;
            }
        }
        "Item"
    }

    /// Items a positioner or layout places: visible and not a Repeater. `Row`,
    /// `Column`, and `Flow` also skip sizeless items, as in Qt
    fn laid_out(core: &Core, kids: &[Oid], positioner: bool) -> Vec<Oid> {
        kids.iter().copied()
            .filter(|&c| core.flag(c, "visible") && Self::kind_name(core, c) != "Repeater")
            .filter(|&c| !positioner || (core.num(c, "width") > 0.0 && core.num(c, "height") > 0.0))
            .collect()
    }

    /// Bottom-up implicit sizes. A child is revisited if it, its subtree, or
    /// this item changed, since its size reads the parent's layout
    /// (`sized_by_others`)
    fn implicit(&mut self, ctx: &Ctx<'_>, core: &Core, oid: Oid, parent: Option<Oid>) {
        if Self::kind_name(core, oid) == "Control" {
            control::adopt(ctx, core, oid);
        }
        let kids: Vec<Oid> = Self::children(core, oid).into_iter().filter(|&c| Self::is_item(core, c)).collect();
        let itself = self.since == 0 || changed_at(core, oid) > self.since;
        for &c in &kids {
            if itself || self.due(core, c) {
                self.implicit(ctx, core, c, Some(oid));
            }
        }
        // A window's size comes from the compositor
        if Self::kind_name(core, oid) == "PanelWindow" {
            return;
        }
        let kind = Self::kind_name(core, oid);
        let shown = Self::laid_out(core, &kids, matches!(kind, "Row" | "Column" | "Grid" | "Flow"));
        let size = match kind {
            "Text" => self.text_size(ctx, core, oid, parent),
            "TextInput" => self.input_size(ctx, core, oid),
            "TextEdit" => self.edit_size(ctx, core, oid, parent),
            "Image" => {
                let pic = self.picture(core, oid);
                let status = if pic.is_some() { 1.0 } else if core.text(oid, "source").is_empty() { 0.0 } else { 3.0 };
                core.store_layout(ctx, oid, "status", status);
                // An icon item uses its given size, not its picture's
                let def = core.0.borrow().objs[oid].def;
                if types::is_a(def, "IconImage") {
                    let s = core.num(oid, "implicitSize");
                    core.store_layout(ctx, oid, "actualSize", core.num(oid, "width").min(core.num(oid, "height")));
                    (s, s)
                } else if types::is_a(def, "Icon") {
                    (32.0, 32.0)
                } else {
                    // The picture is already at `sourceSize`; an unset
                    // `sourceSize` reads the picture's size
                    match pic {
                        Some(p) => {
                            for (n, v) in [("sourceSize.width", p.w), ("sourceSize.height", p.h)] {
                                if !core.is_explicit(oid, n) {
                                    core.store_layout(ctx, oid, n, v as f64);
                                }
                            }
                            (p.w as f64, p.h as f64)
                        }
                        None => (0.0, 0.0),
                    }
                }
            }
            // Window size limited to `constraintSize` as in quickshell,
            // preserving aspect ratio
            "ScreencopyView" => {
                let (sw, sh) = (core.num(oid, "sourceSize.width"), core.num(oid, "sourceSize.height"));
                let (cw, ch) = (core.num(oid, "constraintSize.width"), core.num(oid, "constraintSize.height"));
                if cw != 0.0 && ch != 0.0 && sw > 0.0 && sh > 0.0 {
                    let k = (cw / sw).min(ch / sh);
                    (sw * k, sh * k)
                } else if cw != 0.0 {
                    (cw, sh / cw)
                } else if ch != 0.0 {
                    (sw / ch, ch)
                } else {
                    (sw, sh)
                }
            }
            // Loader size is its item's size, or the item's implicit size when
            // the Loader's own size sets the item's (Qt's rule)
            "Loader" => match core.peek_named(oid, "item").and_then(|v| v.as_obj()).filter(|&i| core.0.borrow().objs.is_live(i)) {
                Some(i) => {
                    let (gw, gh) = size_given(core, oid);
                    (core.num(i, if gw { "implicitWidth" } else { "width" }), core.num(i, if gh { "implicitHeight" } else { "height" }))
                }
                None => (0.0, 0.0),
            },
            "Grid" => {
                let g = grid(core, oid, &shown);
                (g.width(), g.height())
            }
            "Flow" => {
                let places = flow(core, oid, &shown);
                shown.iter().zip(&places).fold((0.0, 0.0), |(w, h), (&c, &(x, y))| {
                    (f64::max(w, x + core.num(c, "width")), f64::max(h, y + core.num(c, "height")))
                })
            }
            "Row" => {
                let sp = core.num(oid, "spacing");
                let w: f64 = shown.iter().map(|&c| core.num(c, "width")).sum::<f64>() + gaps(sp, shown.len());
                let h = shown.iter().map(|&c| core.num(c, "height")).fold(0.0, f64::max);
                (w, h)
            }
            "Column" => {
                let sp = core.num(oid, "spacing");
                let w = shown.iter().map(|&c| core.num(c, "width")).fold(0.0, f64::max);
                let h: f64 = shown.iter().map(|&c| core.num(c, "height")).sum::<f64>() + gaps(sp, shown.len());
                (w, h)
            }
            "RowLayout" | "ColumnLayout" | "GridLayout" => {
                let (horizontal, sp) = linear(core, oid, shown.len());
                let along: f64 = shown.iter().map(|&c| cell(core, c, horizontal)).sum::<f64>() + gaps(sp, shown.len());
                let across = shown.iter().map(|&c| cell(core, c, !horizontal)).fold(0.0, f64::max);
                if horizontal { (along, across) } else { (across, along) }
            }
            "Control" => control::implicit(ctx, core, oid),
            "Shape" => shapes::implicit(core, oid),
            _ => (f64::NAN, f64::NAN),
        };
        let (iw, ih) = size;
        if !iw.is_nan() && !core.is_explicit(oid, "implicitWidth") {
            core.store_layout(ctx, oid, "implicitWidth", iw);
        }
        if !ih.is_nan() && !core.is_explicit(oid, "implicitHeight") {
            core.store_layout(ctx, oid, "implicitHeight", ih);
        }
        // QtQuick's rule: width defaults to implicit width, unless the parent's
        // layout or anchors size the item in the next pass
        let (by_w, by_h) = sized_by_others(core, oid, parent);
        if !by_w && !core.is_explicit(oid, "width") {
            let unrounded = (kind == "TextInput" && !core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "TextField")))
                .then(|| {
                    let (l, r, _, _) = paddings(core, oid);
                    core.num(oid, "contentWidth") + l + r
                });
            core.store_layout(ctx, oid, "width", unrounded.unwrap_or_else(|| core.num(oid, "implicitWidth")));
        }
        if !by_h && !core.is_explicit(oid, "height") {
            core.store_layout(ctx, oid, "height", core.num(oid, "implicitHeight"));
        }
    }

    /// Text implicit size, as QQuickText: widest unbroken line wide, lines at
    /// the current width high, rounded up (`text/layout.rs`). Content size,
    /// line count, and truncation are written back
    fn text_size(&mut self, ctx: &Ctx<'_>, core: &Core, oid: Oid, parent: Option<Oid>) -> (f64, f64) {
        let Some(font) = font_of(&mut self.fonts, core, oid) else { return (0.0, 0.0) };
        let text = core.text(oid, "text");
        // Breaks and elides at a width given by the file, anchors, or layout,
        // not its own text width
        let given = core.is_explicit(oid, "width") || sized_by_others(core, oid, parent).0;
        let o = text_opts(core, oid, Some(core.num(oid, "width") as f32).filter(|_| given));
        let l = lay(&mut self.fonts, &font, &text, &o);
        core.store_layout(ctx, oid, "contentWidth", l.content as f64);
        core.store_layout(ctx, oid, "contentHeight", l.height.ceil() as f64);
        core.store_layout(ctx, oid, "lineCount", l.lines.len() as f64);
        // The first baseline, in the height the Text has or will have
        let h = if core.is_explicit(oid, "height") || sized_by_others(core, oid, parent).1 {
            core.num(oid, "height") as f32
        } else {
            l.height.ceil()
        };
        core.store_layout(ctx, oid, "baselineOffset", (text_top(core, oid, &l, h) + l.metrics.ascent) as f64);
        if let Some(i) = core.prop_idx(oid, "truncated") {
            core.store(ctx, oid, i, crate::object::value::Value::Bool(l.truncated));
        }
        (l.natural as f64, l.height.ceil() as f64)
    }

    /// TextInput: text plus padding wide. TextField (Basic style): as wide as
    /// its background or its text or placeholder plus padding, as high as the
    /// highest
    fn input_size(&mut self, ctx: &Ctx<'_>, core: &Core, oid: Oid) -> (f64, f64) {
        let Some(font) = font_of(&mut self.fonts, core, oid) else { return (0.0, 0.0) };
        let shown = crate::input::text::shown(core, oid);
        let cw = self.fonts.width(&font, &shown) as f64;
        let ch = self.fonts.metrics(&font).height().ceil() as f64;
        core.store_layout(ctx, oid, "contentWidth", cw);
        core.store_layout(ctx, oid, "contentHeight", ch);
        let p = control::store_pads(ctx, core, oid);
        let (l, r, t, b) = (p.left, p.right, p.top, p.bottom);
        let field = core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "TextField"));
        if field {
            let pw = self.fonts.width(&font, &core.text(oid, "placeholderText")) as f64;
            let (bw, bh) = control::field_background(core, oid);
            (if bw > 0.0 { bw } else { cw.max(pw) + l + r }, bh.max(ch + t + b))
        } else {
            // A TextInput's implicit width is rounded up; its own width is not
            // (QQuickTextInput `getImplicitWidth`, `updateLayout`)
            (cw.ceil() + l + r, ch + t + b)
        }
    }

    /// TextEdit implicit size: lines as a Text, broken at the given width when
    /// wrapping, plus padding (TextArea), or a larger TextArea background.
    /// Content size and line count are written back
    fn edit_size(&mut self, ctx: &Ctx<'_>, core: &Core, oid: Oid, parent: Option<Oid>) -> (f64, f64) {
        let Some(font) = font_of(&mut self.fonts, core, oid) else { return (0.0, 0.0) };
        let p = control::store_pads(ctx, core, oid);
        let (l, r, t, b) = (p.left, p.right, p.top, p.bottom);
        let given = core.is_explicit(oid, "width") || sized_by_others(core, oid, parent).0;
        let width = given.then(|| (core.num(oid, "width") - l - r).max(0.0) as f32);
        let step = self.fonts.metrics(&font).height().ceil();
        let laid = lay(&mut self.fonts, &font, &core.text(oid, "text"), &edit_opts(core, oid, width, step));
        core.store_layout(ctx, oid, "contentWidth", laid.content as f64);
        core.store_layout(ctx, oid, "contentHeight", laid.height.ceil() as f64);
        core.store_layout(ctx, oid, "lineCount", laid.lines.len() as f64);
        let placeholder = core.text(oid, "placeholderText");
        let pw = if placeholder.is_empty() { 0.0 } else { self.fonts.width(&font, &placeholder) as f64 };
        let (bw, bh) = control::field_background(core, oid);
        ((laid.natural.max(pw as f32) as f64 + l + r).max(bw), (laid.height.ceil() as f64 + t + b).max(bh))
    }

    /// Which character a press `x` pixels into a TextInput lands before
    pub fn cursor_at(&mut self, core: &Core, oid: Oid, x: f64) -> usize {
        let Some(font) = font_of(&mut self.fonts, core, oid) else { return 0 };
        let g = input_geometry(&mut self.fonts, core, oid, &font, core.num(oid, "width"));
        let shown = crate::input::text::shown(core, oid);
        let n = shown.chars().count();
        let at = x - g.left;
        let mut prev = 0.0;
        for i in 1..=n {
            let w = self.fonts.caret(&font, &shown, i) as f64;
            if at < (prev + w) / 2.0 {
                return i - 1;
            }
            prev = w;
        }
        n
    }

    /// Top-down placement and layout sizing
    fn place(&mut self, ctx: &Ctx<'_>, core: &Core, oid: Oid) {
        let kids: Vec<Oid> = Self::children(core, oid).into_iter().filter(|&c| Self::is_item(core, c)).collect();
        let kind = Self::kind_name(core, oid);
        let shown = Self::laid_out(core, &kids, matches!(kind, "Row" | "Column" | "Flow"));
        let (pw, ph) = (core.num(oid, "width"), core.num(oid, "height"));
        match kind {
            // Sequential with scroll offset; the total sets the scroll limit
            "ListView" => {
                let horizontal = core.num(oid, "orientation") as i64 == 1;
                let sp = core.num(oid, "spacing");
                let (cx, cy) = (core.num(oid, "contentX"), core.num(oid, "contentY"));
                // The rows start past the margins (`model::margins`)
                let (mx, my) = (core.num(oid, "leftMargin"), core.num(oid, "topMargin"));
                let (before, after) = crate::model::margins(core, oid, !horizontal);
                let mut at = 0.0;
                let mut across: f64 = 0.0;
                let items: Vec<Oid> = crate::model::items(core, oid).into_iter()
                    .filter(|&c| core.flag(c, "visible")).collect();
                for (k, &c) in items.iter().enumerate() {
                    if k > 0 {
                        at += sp;
                    }
                    if horizontal {
                        core.store_layout(ctx, c, "x", mx + at - cx);
                        core.store_layout(ctx, c, "y", my - cy);
                        at += core.num(c, "width");
                        across = across.max(core.num(c, "height"));
                    } else {
                        core.store_layout(ctx, c, "x", mx - cx);
                        core.store_layout(ctx, c, "y", my + at - cy);
                        at += core.num(c, "height");
                        across = across.max(core.num(c, "width"));
                    }
                }
                let (cw, ch) = if horizontal { (at, across) } else { (across, at) };
                core.store_layout(ctx, oid, "contentWidth", cw);
                core.store_layout(ctx, oid, "contentHeight", ch);
                let length = at + before + after;
                let end = if horizontal { (length - pw).max(0.0) } else { (length - ph).max(0.0) };
                let pos = if horizontal { cx } else { cy };
                core.set_layout_value(ctx, oid, "atYBeginning", crate::object::value::Value::Bool(pos <= 0.0));
                core.set_layout_value(ctx, oid, "atYEnd", crate::object::value::Value::Bool(pos >= end));
            }
            // A Loader with a size of its own gives it to its item
            "Loader" => {
                if let Some(i) = core.peek_named(oid, "item").and_then(|v| v.as_obj()).filter(|&i| core.0.borrow().objs.is_live(i)) {
                    let (given_w, given_h) = size_given(core, oid);
                    if given_w {
                        core.store_layout(ctx, i, "width", pw);
                    }
                    if given_h {
                        core.store_layout(ctx, i, "height", ph);
                    }
                }
            }
            "Row" | "Column" => {
                let horizontal = Self::kind_name(core, oid) == "Row";
                let sp = core.num(oid, "spacing");
                let mut at = 0.0;
                for &c in &shown {
                    if horizontal {
                        core.store_layout(ctx, c, "x", at);
                        at += core.num(c, "width") + sp;
                    } else {
                        core.store_layout(ctx, c, "y", at);
                        at += core.num(c, "height") + sp;
                    }
                }
            }
            "Flow" => {
                for (&c, (x, y)) in shown.iter().zip(flow(core, oid, &shown)) {
                    core.store_layout(ctx, c, "x", x);
                    core.store_layout(ctx, c, "y", y);
                }
            }
            "Grid" => {
                let g = grid(core, oid, &shown);
                for (k, &c) in shown.iter().enumerate() {
                    let (x, y) = g.at(k);
                    core.store_layout(ctx, c, "x", x);
                    core.store_layout(ctx, c, "y", y);
                }
            }
            "RowLayout" | "ColumnLayout" | "GridLayout" => {
                let (horizontal, sp) = linear(core, oid, shown.len());
                let (main, cross) = if horizontal { (pw, ph) } else { (ph, pw) };
                let fill_key = if horizontal { "Layout.fillWidth" } else { "Layout.fillHeight" };
                let cross_fill = if horizontal { "Layout.fillHeight" } else { "Layout.fillWidth" };
                // An item's cell is the item and its `Layout` margins
                let along = |c: Oid| {
                    let (l, r, t, b) = layout_margins(core, c);
                    if horizontal { (l, r, t, b) } else { (t, b, l, r) }
                };
                let fixed: f64 = shown.iter().filter(|&&c| !core.flag(c, fill_key))
                    .map(|&c| {
                        let (lo, hi) = limits(core, c, horizontal);
                        let (m0, m1, _, _) = along(c);
                        preferred(core, c, horizontal).clamp(lo, hi.max(lo)) + m0 + m1
                    })
                    .sum::<f64>() + gaps(sp, shown.len());
                let fillers = shown.iter().filter(|&&c| core.flag(c, fill_key)).count();
                let share = if fillers > 0 { ((main - fixed) / fillers as f64).max(0.0) } else { 0.0 };
                // Cell size along the layout: fillers share what is left; with
                // none, spare space is split among the largest cells
                // (`layouts_are_qts`)
                let cells: Vec<f64> = shown.iter().map(|&c| {
                    let (lo, hi) = limits(core, c, horizontal);
                    let (m0, m1, _, _) = along(c);
                    if core.flag(c, fill_key) {
                        (share - m0 - m1).clamp(lo, hi.max(lo)) + m0 + m1
                    } else {
                        preferred(core, c, horizontal).clamp(lo, hi.max(lo)) + m0 + m1
                    }
                }).collect();
                let spare = main - fixed;
                let natural: f64 = cells.iter().sum();
                let cells: Vec<f64> = if fillers == 0 && spare > 0.0 && natural > 0.0 {
                    cells.iter().map(|c| c + spare * c / natural).collect()
                } else {
                    cells
                };
                let mut at = 0.0;
                for (k, &c) in shown.iter().enumerate() {
                    let (lo, hi) = limits(core, c, horizontal);
                    let (m0, m1, c0, c1) = along(c);
                    // The cell's ends on whole pixels
                    let (start, end) = ((at + 0.5f64).floor(), (at + cells[k] + 0.5f64).floor());
                    let fills = core.flag(c, fill_key);
                    let size = if fills { end - start - m0 - m1 } else { preferred(core, c, horizontal) };
                    let size = size.clamp(lo, hi.max(lo));
                    let room = (cross - c0 - c1).max(0.0);
                    let csize = if core.flag(c, cross_fill) { room } else { preferred(core, c, !horizontal) };
                    // Across: centered unless `Layout.alignment` sets an edge.
                    // Along: likewise within the cell
                    let align = core.num(c, "Layout.alignment") as i64;
                    // A centre on a whole pixel either way (`layouts_are_qts`,
                    // g15)
                    let centre = ((room - csize) / 2.0 + 0.5).floor();
                    let cpos = c0 + if horizontal {
                        if align & 0x20 != 0 { 0.0 } else if align & 0x40 != 0 { room - csize } else { centre }
                    } else if align & 0x2 != 0 { room - csize } else if align & 0x4 != 0 { centre } else { 0.0 };
                    let inner = end - start - m0 - m1;
                    let pos = start + m0 + if fills {
                        0.0
                    } else if horizontal {
                        if align & 0x2 != 0 { inner - size } else if align & 0x4 != 0 { ((inner - size) / 2.0 + 0.5).floor() } else { 0.0 }
                    } else if align & 0x20 != 0 {
                        0.0
                    } else if align & 0x40 != 0 {
                        inner - size
                    } else {
                        ((inner - size) / 2.0 + 0.5).floor()
                    };
                    let (x, y, w, h) = if horizontal { (pos, cpos, size, csize) } else { (cpos, pos, csize, size) };
                    core.store_layout(ctx, c, "x", x);
                    core.store_layout(ctx, c, "y", y);
                    core.store_layout(ctx, c, "width", w);
                    core.store_layout(ctx, c, "height", h);
                    at += cells[k] + sp;
                }
            }
            "Control" => control::place(ctx, core, oid),
            "RadialGradient" => effect::radii(ctx, core, oid),
            _ => {}
        }
        for &c in &kids {
            self.anchor(ctx, core, c, oid, pw, ph);
            if self.due(core, c) {
                self.place(ctx, core, c);
            }
        }
    }

    /// Anchors: `fill`, `centerIn`, and the six lines, relative to the parent
    /// or a sibling.
    ///
    /// `fill` overrides everything; `centerIn` keeps the item's size and
    /// ignores lines (as in qml6).
    ///
    /// Centers snap to whole pixels (`alignWhenCentered`): an item's center
    /// line is half its size rounded, and a centered item moves by half its own
    /// size rounded, unless `anchors.alignWhenCentered: false`
    fn anchor(&mut self, ctx: &Ctx<'_>, core: &Core, c: Oid, parent: Oid, pw: f64, ph: f64) {
        let fill = core.peek_named(c, "anchors.fill").and_then(|v| v.as_obj());
        let center = core.peek_named(c, "anchors.centerIn").and_then(|v| v.as_obj());
        let line = |name: &str| core.peek_named(c, name).and_then(|v| match v {
            crate::object::value::Value::Str(s) => parse_line(&s),
            _ => None,
        });
        let (l, r, t, b) = (line("anchors.left"), line("anchors.right"), line("anchors.top"), line("anchors.bottom"));
        let (hc, vc) = (line("anchors.horizontalCenter"), line("anchors.verticalCenter"));
        if fill.is_none() && center.is_none() && l.is_none() && r.is_none() && t.is_none() && b.is_none() && hc.is_none() && vc.is_none() {
            self.far.remove(&c);
            return;
        }
        let m = core.num(c, "anchors.margins");
        let margin = |name: &str| if core.is_explicit(c, name) { core.num(c, name) } else { m };
        let (lm, rm, tm, bm) = (margin("anchors.leftMargin"), margin("anchors.rightMargin"),
                                margin("anchors.topMargin"), margin("anchors.bottomMargin"));
        // Half an item's size, as a centre is taken
        let half = |item: Oid, size: f64| {
            if core.flag(item, "anchors.alignWhenCentered") { (size / 2.0 + 0.5).floor() } else { size / 2.0 }
        };
        // Anchored to neither parent nor sibling: tracked in `far` for
        // `mark_far`
        let far: Vec<Oid> = [fill, center].into_iter().flatten()
            .chain([&l, &r, &t, &b, &hc, &vc].into_iter().flatten().map(|(o, _)| *o))
            .filter(|&o| o != parent && core.0.borrow().objs.get(o).is_some_and(|x| x.parent != Some(parent)))
            .collect();
        if far.is_empty() {
            self.far.remove(&c);
        } else {
            self.far.insert(c, far);
        }
        // A line's place in the parent's coordinates
        let at = |target: Oid, line: &str| -> f64 {
            let (x, y, w, h) = if target == parent {
                (0.0, 0.0, pw, ph)
            } else {
                (core.num(target, "x"), core.num(target, "y"), core.num(target, "width"), core.num(target, "height"))
            };
            match line {
                "left" => x,
                "right" => x + w,
                "horizontalCenter" => x + half(target, w),
                "top" => y,
                "bottom" => y + h,
                "verticalCenter" => y + half(target, h),
                _ => 0.0,
            }
        };
        let (mut x, mut y, mut w, mut h) = (core.num(c, "x"), core.num(c, "y"), core.num(c, "width"), core.num(c, "height"));
        let (mut set_w, mut set_h) = (false, false);
        // Writes only anchored axes; an item anchored vertically keeps its own
        // `x`
        let Held { across, down, .. } = held(core, c);
        if let Some(f) = fill {
            x = at(f, "left") + lm;
            y = at(f, "top") + tm;
            w = at(f, "right") - rm - x;
            h = at(f, "bottom") - bm - y;
            set_w = true;
            set_h = true;
        } else if let Some(cn) = center {
            x = at(cn, "horizontalCenter") - half(c, w) + core.num(c, "anchors.horizontalCenterOffset");
            y = at(cn, "verticalCenter") - half(c, h) + core.num(c, "anchors.verticalCenterOffset");
        } else {
            match (l, r) {
                (Some((lo, ll)), Some((ro, rl))) => {
                    x = at(lo, &ll) + lm;
                    w = at(ro, &rl) - rm - x;
                    set_w = true;
                }
                (Some((lo, ll)), None) => x = at(lo, &ll) + lm,
                (None, Some((ro, rl))) => x = at(ro, &rl) - rm - w,
                _ => {
                    if let Some((o, ln)) = hc {
                        x = at(o, &ln) - half(c, w) + core.num(c, "anchors.horizontalCenterOffset");
                    }
                }
            }
            match (t, b) {
                (Some((to, tl)), Some((bo, bl))) => {
                    y = at(to, &tl) + tm;
                    h = at(bo, &bl) - bm - y;
                    set_h = true;
                }
                (Some((to, tl)), None) => y = at(to, &tl) + tm,
                (None, Some((bo, bl))) => y = at(bo, &bl) - bm - h,
                _ => {
                    if let Some((o, ln)) = vc {
                        y = at(o, &ln) - half(c, h) + core.num(c, "anchors.verticalCenterOffset");
                    }
                }
            }
        }
        if across {
            core.store_layout(ctx, c, "x", x);
        }
        if down {
            core.store_layout(ctx, c, "y", y);
        }
        if set_w {
            core.store_layout(ctx, c, "width", w.max(0.0));
        }
        if set_h {
            core.store_layout(ctx, c, "height", h.max(0.0));
        }
    }

    /// Everything visible in `win` that paints or takes the pointer, in draw
    /// order
    fn display(&mut self, core: &Core, win: Oid) -> Vec<Shown> {
        let mut out = Vec::new();
        let (w, h) = (core.num(win, "width"), core.num(win, "height"));
        let bounds = Rect::new(0, 0, w as i32, h as i32);
        let fill = core.text(win, "color");
        out.push(Shown { flat: None,
            oid: win,
            draw: Draw::Fill,
            rect: bounds,
            xf: Xf::IDENTITY,
            size: (w, h),
            clip: bounds,
            opacity: 1.0,
            print: hash(&("fill", &fill, bounds.w, bounds.h)),
            paint: true,
        });
        // A window's items are its content item's
        for c in Self::by_z(core, win) {
            self.collect(core, c, &Xf::IDENTITY, bounds, 1.0, true, &mut out);
        }
        out
    }

    /// Children in draw order: by `z`, stable among equals
    fn by_z(core: &Core, oid: Oid) -> Vec<Oid> {
        let mut kids = Self::children(core, oid);
        kids.sort_by(|a, b| core.num(*a, "z").partial_cmp(&core.num(*b, "z")).unwrap_or(std::cmp::Ordering::Equal));
        kids
    }

    /// Runs `f` for a separate list (a texture or its print), outside the
    /// window's run tracking
    fn apart<R>(&mut self, f: impl FnOnce(&mut Self) -> R) -> R {
        let keep = self.keep.take();
        let r = f(self);
        self.keep = keep;
        r
    }

    /// Entries of `oid` and its subtree, copied from the last list when
    /// unchanged with the same inputs
    #[allow(clippy::too_many_arguments)]
    fn collect(&mut self, core: &Core, oid: Oid, parent: &Xf, clip: Rect, opacity: f32, paint: bool, out: &mut Vec<Shown>) {
        let given = ([parent.a, parent.b, parent.c, parent.d, parent.tx, parent.ty].map(f64::to_bits), clip, opacity.to_bits(), paint);
        let slot = crate::object::table::slot(oid);
        if let Some(k) = self.keep.as_mut() {
            let kept = k.nodes.get(slot).copied().flatten().filter(|n| {
                k.since > 0 && n.oid == oid && n.list + 1 == k.list && n.given == given && !n.reads_others
            });
            if let Some(n) = kept.filter(|_| below(core, oid) <= k.since) {
                let start = out.len() as u32;
                out.extend_from_slice(&k.last[n.start as usize..n.end as usize]);
                k.nodes[slot] = Some(Node { list: k.list, start, end: out.len() as u32, ..n });
                return;
            }
        }
        let start = out.len();
        self.collect_new(core, oid, parent, clip, opacity, paint, out);
        if let Some(k) = self.keep.as_mut() {
            if k.nodes.len() <= slot {
                k.nodes.resize(slot + 1, None);
            }
            let reads_others = out[start..].iter().any(|e| matches!(e.draw, Draw::Layer | Draw::Effect));
            k.nodes[slot] = Some(Node { oid, list: k.list, start: start as u32, end: out.len() as u32, given, reads_others });
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_new(&mut self, core: &Core, oid: Oid, parent: &Xf, clip: Rect, opacity: f32, paint: bool, out: &mut Vec<Shown>) {
        if !Self::is_item(core, oid) || !core.flag(oid, "visible") {
            return;
        }
        let opacity = opacity * core.num(oid, "opacity") as f32;
        if opacity <= 0.0 {
            return;
        }
        let xf = parent.times(&xf::to_parent(core, oid));
        self.collect_at(core, oid, xf, clip, opacity, paint, false, out);
    }

    /// `oid` at `xf` and its subtree. `own` is for a texture of it (`effect/`),
    /// skipping its own layer. In the scene a layered item is one layer entry,
    /// followed by its contents, unpainted, for the pointer
    #[allow(clippy::too_many_arguments)]
    fn collect_at(&mut self, core: &Core, oid: Oid, xf: Xf, clip: Rect, opacity: f32, paint: bool, own: bool,
                  out: &mut Vec<Shown>) {
        let (w, h) = (core.num(oid, "width"), core.num(oid, "height"));
        let size = (w, h);
        // Rotated and antialiased items draw up to half a pixel past their box
        let box_ = xf.bounds(0.0, 0.0, w, h);
        let rect = if xf.is_translation() { box_ } else { box_.grown(1) };
        let kind = Self::kind_name(core, oid);
        let at = [xf.a, xf.b, xf.c, xf.d, xf.tx, xf.ty].map(f64::to_bits);
        let print_base = (kind, at, w.to_bits(), h.to_bits(), (opacity * 255.0) as u32, clip.x, clip.y, clip.w, clip.h);
        if !own && core.flag(oid, "layer.enabled") {
            let reach = self.effects.layered.get(&oid).map(|&e| effect::reach(core, e)).unwrap_or(0.0);
            let rect = xf.bounds(-reach, -reach, w + 2.0 * reach, h + 2.0 * reach).grown(1);
            let start = out.len();
            out.push(Shown { flat: None, oid, draw: Draw::Layer, rect, xf, size, clip, opacity, print: 0, paint });
            self.collect_at(core, oid, xf, clip, opacity, false, true, out);
            let inside: Vec<u64> = out[start + 1..].iter().map(|s| s.print).collect();
            let fx = self.effects.layered.get(&oid).copied().map(|e| self.effect_print(core, e, Some(oid)));
            out[start].print = hash(&(print_base, inside, fx, core.flag(oid, "layer.smooth")));
            return;
        }
        match kind {
            "Rectangle" => {
                let spec = rect_spec(core, oid, size);
                let rect = if spec.antialiasing { box_.grown(1) } else { rect };
                let square = spec.radius <= 0.0 && spec.corners.iter().all(|c| c.is_none_or(|r| r <= 0.0));
                let flat = (xf.is_translation() && square && spec.stops.is_empty()).then(|| {
                    let style = hash(&(kind, (opacity * 255.0) as u32, clip.x, clip.y, clip.w, clip.h, spec.color,
                                       spec.border_color, spec.pen_width.to_bits(), spec.antialiasing));
                    (style, spec.pen_width.ceil() as i32)
                });
                out.push(Shown { flat, oid, draw: Draw::Rect, rect, xf, size, clip, opacity, paint, print: hash(&(print_base, spec.print())) })
            }
            "Text" => out.push(Shown { flat: None,
                oid, draw: Draw::Text, rect, xf, size, clip, opacity, paint,
                // All inputs to a Text's pixels: font, layout, glyph rendering
                print: hash(&(print_base, core.text(oid, "text"), core.text(oid, "color"),
                              (core.num(oid, "font.pixelSize").to_bits(), core.num(oid, "font.pointSize").to_bits(),
                               core.flag(oid, "font.bold"), core.num(oid, "font.weight") as i64, core.flag(oid, "font.italic"),
                               core.text(oid, "font.family"), core.text(oid, "font.styleName"), core.text(oid, "font.__axes"),
                               core.num(oid, "font.hintingPreference") as i64),
                              (core.num(oid, "horizontalAlignment") as i64, core.num(oid, "verticalAlignment") as i64,
                               core.num(oid, "elide") as i64, core.num(oid, "wrapMode") as i64,
                               core.num(oid, "maximumLineCount") as i64, core.num(oid, "lineHeight").to_bits()),
                              (core.num(oid, "style") as i64, core.text(oid, "styleColor")))),
            }),
            "MouseArea" => out.push(Shown { flat: None, oid, draw: Draw::Area, rect, xf, size, clip, opacity, paint, print: hash(&print_base) }),
            "Control" => out.push(Shown { flat: None, oid, draw: Draw::Control, rect, xf, size, clip, opacity, paint, print: hash(&print_base) }),
            "ListView" => out.push(Shown { flat: None, oid, draw: Draw::View, rect, xf, size, clip, opacity, paint, print: hash(&print_base) }),
            "TextInput" => {
                let active = core.flag(oid, "activeFocus");
                let caret = active && core.0.borrow().keys.caret_on;
                out.push(Shown { flat: None,
                    oid, draw: Draw::Input, rect, xf, size, clip, opacity, paint,
                    print: hash(&(print_base, crate::input::text::shown(core, oid), core.text(oid, "placeholderText"),
                                  (core.text(oid, "color"), core.text(oid, "placeholderTextColor")),
                                  (core.num(oid, "font.pixelSize").to_bits(), core.flag(oid, "font.bold"),
                                   core.text(oid, "font.family")),
                                  (core.num(oid, "horizontalAlignment") as i64, core.num(oid, "verticalAlignment") as i64),
                                  (core.num(oid, "cursorPosition") as i64, caret, paddings(core, oid).0.to_bits()))),
                })
            }
            "TextEdit" => {
                let caret = core.flag(oid, "activeFocus") && core.0.borrow().keys.caret_on;
                out.push(Shown { flat: None,
                    oid, draw: Draw::Edit, rect, xf, size, clip, opacity, paint,
                    print: hash(&(print_base, core.text(oid, "text"), core.text(oid, "placeholderText"),
                                  (core.text(oid, "color"), core.text(oid, "placeholderTextColor")),
                                  (core.num(oid, "font.pixelSize").to_bits(), core.num(oid, "font.weight") as i64,
                                   core.text(oid, "font.family"), core.flag(oid, "font.bold")),
                                  (core.num(oid, "horizontalAlignment") as i64, core.num(oid, "verticalAlignment") as i64,
                                   core.num(oid, "wrapMode") as i64),
                                  (core.num(oid, "cursorPosition") as i64, caret, paddings(core, oid).0.to_bits()))),
                })
            }
            "Canvas" => {
                let generation = self.canvases.picture(oid).map(|p| p.generation).unwrap_or(0);
                out.push(Shown { flat: None, oid, draw: Draw::Canvas, rect, xf, size, clip, opacity, paint, print: hash(&(print_base, generation)) })
            }
            "Shape" => {
                let made = self.shapes.print(core, oid);
                let rect = shapes::reach(core, oid).map_or(rect, |(x, y, w, h)| xf.bounds(x, y, w, h).grown(1));
                out.push(Shown { flat: None, oid, draw: Draw::Shape, rect, xf, size, clip, opacity, paint, print: hash(&(print_base, made)) })
            }
            "OpacityMask" | "FastBlur" | "GaussianBlur" | "DropShadow" | "RectangularShadow" | "ShaderEffectSource"
            | "RadialGradient" | "MultiEffect" => {
                let reach = effect::reach(core, oid);
                let rect = xf.bounds(-reach, -reach, w + 2.0 * reach, h + 2.0 * reach).grown(1);
                let fx = self.effect_print(core, oid, None);
                out.push(Shown { flat: None, oid, draw: Draw::Effect, rect, xf, size, clip, opacity, paint, print: hash(&(print_base, fx)) })
            }
            // Window pixels stretched over the item with nearest sampling;
            // quickshell 7511545 sets no filter and ignores `smooth`
            "ScreencopyView" => {
                if let Some(&(_, n)) = self.captures.get(&oid) {
                    out.push(Shown { flat: None, oid, draw: Draw::Image, rect, xf, size, clip, opacity, paint, print: hash(&(print_base, n)) });
                }
            }
            "Image" => out.push(Shown { flat: None,
                oid, draw: Draw::Image, rect, xf, size, clip, opacity, paint,
                print: hash(&(print_base, core.text(oid, "source"), core.num(oid, "fillMode") as i64,
                              core.num(oid, "horizontalAlignment") as i64, core.num(oid, "verticalAlignment") as i64,
                              core.flag(oid, "smooth"))),
            }),
            _ => {}
        }
        // Clip to the box's pixels; a rotated item clips to its bounding box
        let inner = if core.flag(oid, "clip") { clip.intersect(&box_).unwrap_or_default() } else { clip };
        for c in Self::by_z(core, oid) {
            self.collect(core, c, &xf, inner, opacity, paint, out);
        }
    }

    /// Paints what changed since the last frame. Returns the covered rectangles
    pub fn paint(&mut self, core: &Core, win: Oid, ws: &mut WinScene) -> Vec<Rect> {
        let damage = self.list(core, win, ws);
        for &d in &damage {
            self.paint_rect(core, ws, d);
        }
        #[cfg(test)]
        if gpu::check::wanted() {
            let list = std::mem::take(&mut ws.shown);
            self.check_gpu(core, &list, ws, &damage);
            ws.shown = list;
        }
        damage
    }

    /// Rebuilds the window's list without painting. The new list is what the
    /// pointer uses. Returns rectangles that differ
    pub fn list(&mut self, core: &Core, win: Oid, ws: &mut WinScene) -> Vec<Rect> {
        if !core.0.borrow().objs.is_live(win) {
            return Vec::new();
        }
        // Nothing in the window changed since its last list
        if !ws.fresh && ws.list > 0 && below(core, win) <= ws.drawn {
            return Vec::new();
        }
        let drawn = core.0.borrow().changes;
        let since = if ws.fresh { 0 } else { ws.drawn };
        self.keep = Some(Keep { last: std::mem::take(&mut ws.shown), nodes: std::mem::take(&mut ws.nodes), list: ws.list + 1, since });
        let now = self.display(core, win);
        if let Some(k) = self.keep.take() {
            ws.shown = k.last;
            ws.nodes = k.nodes;
            ws.list = k.list;
        }
        ws.drawn = drawn;
        #[cfg(test)]
        if since > 0 {
            self.check_list(core, win, &now);
        }
        let mut damage = Damage::default();
        if ws.fresh {
            damage.add(ws.image.bounds());
        } else {
            changed(&now, &ws.shown, &mut |r| damage.add(r));
        }
        ws.fresh = false;
        let bounds = ws.image.bounds();
        ws.shown = now;
        self.effects.prune(core);
        self.shapes.prune(core);
        // Drops captures of destroyed views
        self.captures.retain(|&v, _| core.0.borrow().objs.is_live(v));
        damage.0.into_iter().filter_map(|d| d.intersect(&bounds)).collect()
    }

    /// Repaints one rectangle on the processor into the window's image
    pub fn paint_rect(&mut self, core: &Core, ws: &mut WinScene, d: Rect) {
        ws.image.clear(d);
        let mut p = Painter::new(&mut ws.image, d);
        self.draw_list(core, &ws.shown, &mut p);
    }

    /// Records rectangles for the GPU into `target`. Unrecordable ones are
    /// painted on the processor into the image and returned, for the caller to
    /// copy
    pub fn record(&mut self, core: &Core, ws: &mut WinScene, rects: &[Rect], rec: &mut gpu::record::Record, target: u32) -> Vec<Rect> {
        let mut cpu = Vec::new();
        for &d in rects {
            rec.begin(target, d);
            {
                let mut p = Painter::record(rec, d);
                self.draw_list(core, &ws.shown, &mut p);
            }
            if !rec.end() {
                self.paint_rect(core, ws, d);
                cpu.push(d);
            }
        }
        cpu
    }

    /// Test check: records and runs the processor's painted damage on the
    /// host's card through CUDA, panicking on a difference (`gpu/check.rs`)
    #[cfg(test)]
    fn check_gpu(&mut self, core: &Core, list: &[Shown], ws: &WinScene, damage: &[Rect]) {
        use gpu::check::{Cuda, HostMemory};
        use gpu::record::Record;
        type Gpu = Result<(Cuda, Record, HostMemory), String>;
        thread_local! {
            static GPU: std::cell::RefCell<Option<Gpu>> = const { std::cell::RefCell::new(None) };
            static SAID: std::cell::RefCell<Vec<&'static str>> = const { std::cell::RefCell::new(Vec::new()) };
        }
        let mut held = GPU.with(|g| g.borrow_mut().take()).unwrap_or_else(|| {
            Cuda::open().map(|c| {
                let mem = HostMemory::new();
                let shared = HostMemory(mem.0.clone());
                (c, Record::new(Box::new(mem)), shared)
            })
        });
        if let Ok((cuda, rec, mem)) = held.as_mut() {
            let (w, h) = (ws.image.w, ws.image.h);
            for &d in damage {
                rec.frame();
                let t = rec.target(0, 0, w as u32, w as u32, h as u32);
                rec.begin(t, d);
                {
                    let mut p = Painter::record(rec, d);
                    self.draw_list(core, list, &mut p);
                }
                if !rec.end() {
                    let why = rec.refused().unwrap_or("?");
                    if SAID.with(|s| !s.borrow().contains(&why)) {
                        SAID.with(|s| s.borrow_mut().push(why));
                        // Records entries one at a time until one is refused
                        let mut whose = String::new();
                        for e in list.iter().filter(|e| e.paint && e.rect.intersect(&d).is_some()) {
                            rec.frame();
                            let t = rec.target(0, 0, w as u32, w as u32, h as u32);
                            rec.begin(t, d);
                            {
                                let mut p = Painter::record(rec, d);
                                self.draw_list(core, std::slice::from_ref(e), &mut p);
                            }
                            if !rec.end() {
                                let s = core.0.borrow();
                                whose = s.objs.get(e.oid).map(|o| format!("{} {}", o.ty, s.place(o.file, o.loc))).unwrap_or_default();
                                break;
                            }
                        }
                        eprintln!("aosh gpu check: a rectangle the processor paints - {why}: {whose}");
                    }
                    continue;
                }
                let out = cuda.textures(mem).and_then(|_| cuda.run(&rec.words(), rec.tiles(), (w * h * 4) as usize, 0xA5));
                let out = match out {
                    Ok(o) => o,
                    Err(e) => panic!("aosh gpu check: {e}"),
                };
                let mut wrong = 0usize;
                let mut first = None;
                for y in d.y..d.bottom() {
                    for x in d.x..d.right() {
                        let i = (y * w + x) as usize;
                        let got = u32::from_le_bytes([out[i * 4], out[i * 4 + 1], out[i * 4 + 2], out[i * 4 + 3]]);
                        let want = paint::to_straight(ws.image.px[i]);
                        if got != want {
                            wrong += 1;
                            first.get_or_insert((x, y, want, got));
                        }
                    }
                }
                if let Some((x, y, want, got)) = first {
                    let s = core.0.borrow();
                    let at: Vec<String> = list.iter().filter(|e| e.paint && e.rect.intersect(&Rect::new(x, y, 1, 1)).is_some())
                        .map(|e| format!("{:?} {}", e.draw, s.objs.get(e.oid).map(|o| s.place(o.file, o.loc)).unwrap_or_default()))
                        .collect();
                    panic!("aosh gpu check: {wrong} of {} pixels of {d:?} differ; the first at ({x}, {y}): \
                            processor {want:08x}, GPU {got:08x}; drawn there: {}", d.w * d.h, at.join(", "));
                }
            }
        } else if let Err(e) = &held {
            panic!("aosh gpu check: {e}");
        }
        GPU.with(|g| *g.borrow_mut() = Some(held));
    }

    /// A list that copied runs of the last one must be the list made whole
    #[cfg(test)]
    fn check_list(&mut self, core: &Core, win: Oid, list: &[Shown]) {
        if !scene_check() {
            return;
        }
        let whole = self.display(core, win);
        let key = |e: &Shown| (e.oid, e.draw, e.rect, [e.xf.a, e.xf.b, e.xf.c, e.xf.d, e.xf.tx, e.xf.ty].map(f64::to_bits),
                               e.size.0.to_bits(), e.size.1.to_bits(), e.clip, e.opacity.to_bits(), e.print, e.paint, e.flat);
        let first = list.iter().zip(&whole).position(|(a, b)| key(a) != key(b));
        if list.len() != whole.len() || first.is_some() {
            let at = first.unwrap_or(list.len().min(whole.len()));
            let name = |o: Oid| core.0.borrow().objs.get(o).map(|x| x.ty.clone()).unwrap_or_default();
            panic!("a list that copied runs of the last one is not the list made whole: {} entries against {}, \
                    first differing at {at} ({} against {})", list.len(), whole.len(),
                   list.get(at).map(|e| name(e.oid)).unwrap_or_default(), whole.get(at).map(|e| name(e.oid)).unwrap_or_default());
        }
    }

    /// What `list` paints, in order, inside the painter's clip
    fn draw_list(&mut self, core: &Core, list: &[Shown], p: &mut Painter<'_>) {
        let area = p.clip();
        for s in list {
            if !s.paint || s.rect.intersect(&area).is_none() {
                continue;
            }
            if matches!(s.draw, Draw::Layer | Draw::Effect) {
                p.with_clip(s.clip, |p| self.draw_effect(core, s, p));
                continue;
            }
            let pic = if s.draw == Draw::Image { self.picture(core, s.oid) } else { None };
            let canvas = if s.draw == Draw::Canvas { self.canvases.picture(s.oid).map(|c| &c.image) } else { None };
            if s.draw == Draw::Shape {
                // One picture pixel per item pixel where its paths are
                if let Some(c) = self.shapes.picture(core, s.oid) {
                    let tex = c.image.texture();
                    let at = (c.at.0 as f64, c.at.1 as f64, c.image.w as f64, c.image.h as f64);
                    p.with_clip(s.clip, |p| p.quad(&s.xf, at, &tex, (0.0, 0.0, 1.0, 1.0), s.opacity, gpu::Sampling::Linear));
                }
                continue;
            }
            p.with_clip(s.clip, |p| draw(&mut self.fonts, core, s, p, pic.as_deref(), canvas));
        }
    }

    /// Topmost enabled `MouseArea` or button at `(x, y)`, from the last paint
    pub fn hit(&self, core: &Core, ws: &WinScene, x: f64, y: f64) -> Option<Oid> {
        ws.shown.iter().rev()
            .filter(|s| (s.draw == Draw::Area || (s.draw == Draw::Control && is_button(core, s.oid)))
                && core.0.borrow().objs.is_live(s.oid))
            .find(|s| s.under(x, y) && core.flag(s.oid, "enabled"))
            .map(|s| s.oid)
    }

    /// Press target at `(x, y)`: topmost enabled `MouseArea` or TextInput, and
    /// whether it is the TextInput
    pub fn press_target(&self, core: &Core, ws: &WinScene, x: f64, y: f64) -> Option<(Oid, bool)> {
        ws.shown.iter().rev()
            .filter(|s| matches!(s.draw, Draw::Area | Draw::Input | Draw::Edit) && core.0.borrow().objs.is_live(s.oid))
            .find(|s| s.under(x, y) && core.flag(s.oid, "enabled"))
            .map(|s| (s.oid, matches!(s.draw, Draw::Input | Draw::Edit)))
    }

    /// Hover-tracking `MouseArea`s and Controls at `(x, y)`, topmost first
    pub fn areas_at(&self, core: &Core, ws: &WinScene, x: f64, y: f64) -> Vec<Oid> {
        ws.shown.iter().rev()
            .filter(|s| matches!(s.draw, Draw::Area | Draw::Control) && core.0.borrow().objs.is_live(s.oid))
            .filter(|s| s.under(x, y) && core.flag(s.oid, "enabled") && core.flag(s.oid, "hoverEnabled"))
            .map(|s| s.oid)
            .collect()
    }

    /// Wheel target: topmost enabled `MouseArea` with `onWheel`, else the
    /// innermost ListView
    pub fn wheel_target(&self, core: &Core, ws: &WinScene, x: f64, y: f64) -> Option<(Oid, bool)> {
        let under = |s: &&Shown| s.under(x, y) && core.0.borrow().objs.is_live(s.oid);
        let area = ws.shown.iter().rev().filter(under)
            .find(|s| s.draw == Draw::Area && core.flag(s.oid, "enabled") && core.has_handler(s.oid, "wheel"));
        if let Some(a) = area {
            return Some((a.oid, true));
        }
        ws.shown.iter().rev().filter(under).find(|s| s.draw == Draw::View).map(|s| (s.oid, false))
    }

    /// Window point in an item's coordinates, for `mouse.x`
    pub fn local(&self, ws: &WinScene, oid: Oid, x: f64, y: f64) -> Option<(f64, f64)> {
        let s = ws.shown.iter().find(|s| s.oid == oid)?;
        Some(s.xf.inverse()?.map(x, y))
    }
}

fn draw(fonts: &mut Fonts, core: &Core, s: &Shown, p: &mut Painter<'_>, pic: Option<&Picture>, canvas: Option<&Image>) {
    let (w, h) = s.size;
    match s.draw {
        // The picture on the item's box, as Qt draws a Canvas's texture
        Draw::Canvas => {
            let Some(c) = canvas else { return };
            let tex = c.texture();
            let smooth = core.flag(s.oid, "smooth");
            let sampling = if smooth { gpu::Sampling::Linear } else { gpu::Sampling::Nearest };
            p.quad(&s.xf, (0.0, 0.0, w, h), &tex, (0.0, 0.0, 1.0, 1.0), s.opacity, sampling);
        }
        Draw::Image => {
            let Some(pic) = pic else { return };
            let (target, source) = image_rects(core, s.oid, w, h, pic);
            let (iw, ih) = (pic.w as f64, pic.h as f64);
            let uv = ((source.0 / iw) as f32, (source.1 / ih) as f32, ((source.0 + source.2) / iw) as f32, ((source.1 + source.3) / ih) as f32);
            let tex = pic.texture();
            let smooth = core.flag(s.oid, "smooth") && Scene::kind_name(core, s.oid) != "ScreencopyView";
            let sampling = if smooth { gpu::Sampling::Linear } else { gpu::Sampling::Nearest };
            p.quad(&s.xf, target, &tex, uv, s.opacity, sampling);
        }
        Draw::Fill => {
            let c = colour(&core.text(s.oid, "color"));
            if c >> 24 != 0 {
                p.fill(c);
            }
        }
        Draw::Rect => {
            if let Some(g) = rectangle::geometry(&rect_spec(core, s.oid, s.size)) {
                p.geometry(&g, &s.xf, s.opacity);
            }
        }
        Draw::Text => {
            let Some(font) = font_of(fonts, core, s.oid) else { return };
            let colour = colour(&core.text(s.oid, "color"));
            let style = paint::TextStyle::from_qml(core.num(s.oid, "style") as i64);
            let style_colour = self::colour(&core.text(s.oid, "styleColor"));
            let text = core.text(s.oid, "text");
            let l = lay(fonts, &font, &text, &text_opts(core, s.oid, Some(w as f32)));
            for (line, left, baseline) in placed_lines(core, s.oid, &l, (0.0, 0.0, w as f32, h as f32), |l| l.width) {
                fonts.draw_styled(p, &font, &s.xf, left, baseline, &line.text, colour, style, style_colour, s.opacity);
            }
        }
        // As a Text inside its padding, or its placeholder when empty, with the
        // caret at the cursor
        Draw::Edit => {
            let Some(font) = font_of(fonts, core, s.oid) else { return };
            let (pl, pr, pt, pb) = paddings(core, s.oid);
            let area = (pl as f32, pt as f32, (w - pl - pr).max(0.0) as f32, (h - pt - pb).max(0.0) as f32);
            let text = core.text(s.oid, "text");
            let placeholder = core.text(s.oid, "placeholderText");
            let (shown, ink) = if text.is_empty() && !placeholder.is_empty() {
                (placeholder, colour(&core.text(s.oid, "placeholderTextColor")))
            } else {
                (text.clone(), colour(&core.text(s.oid, "color")))
            };
            let step = fonts.metrics(&font).height().ceil();
            let l = lay(fonts, &font, &shown, &edit_opts(core, s.oid, Some(area.2), step));
            // A TextEdit line aligns by advances, ignoring glyph overhang
            let advances: Vec<f32> = l.lines.iter().map(|line| fonts.caret(&font, &line.text, line.text.chars().count())).collect();
            let lines = placed_lines(core, s.oid, &l, area, |line| {
                l.lines.iter().position(|x| std::ptr::eq(x, line)).map(|i| advances[i]).unwrap_or(line.width)
            });
            for (line, left, baseline) in &lines {
                fonts.draw(p, &font, &s.xf, *left, *baseline, &line.text, ink, s.opacity);
            }
            if core.flag(s.oid, "activeFocus") && core.0.borrow().keys.caret_on {
                let at = core.num(s.oid, "cursorPosition").max(0.0) as usize;
                let (row, col) = cursor_line(&text, &l, at);
                let (x, top) = match lines.get(row) {
                    Some((line, left, baseline)) => {
                        let before: String = line.text.chars().take(col).collect();
                        (*left + fonts.width(&font, &before), *baseline - l.metrics.ascent)
                    }
                    None => (area.0, area.1),
                };
                let at = s.xf.times(&Xf::translate((x as f64).round(), top as f64));
                let spec = rectangle::Spec { w: CARET_W as f32, h: l.metrics.height(), color: colour(&core.text(s.oid, "color")),
                                             ..rectangle::Spec::default() };
                if let Some(g) = rectangle::geometry(&spec) {
                    p.geometry(&g, &at, s.opacity);
                }
            }
        }
        Draw::Input => {
            let Some(font) = font_of(fonts, core, s.oid) else { return };
            let g = input_geometry(fonts, core, s.oid, &font, w);
            let shown = crate::input::text::shown(core, s.oid);
            let placeholder = core.text(s.oid, "placeholderText");
            let ink = colour(&core.text(s.oid, "color"));
            let (txt, c) = if shown.is_empty() && !placeholder.is_empty() {
                (placeholder, colour(&core.text(s.oid, "placeholderTextColor")))
            } else {
                (shown, ink)
            };
            let (l, r, t, b) = paddings(core, s.oid);
            let m = fonts.metrics(&font);
            let line = m.height();
            let ah = (h - t - b).max(0.0) as f32;
            let valign = core.num(s.oid, "verticalAlignment") as i64;
            // Vertical position from content height, on a whole pixel
            let top = if valign & 128 != 0 {
                t as f32 + (ah - line.ceil()) / 2.0
            } else if valign & 64 != 0 {
                t as f32 + ah - line.ceil()
            } else {
                t as f32
            };
            // Damage box is a pixel wider than the text on each side, for the
            // caret
            let area = s.xf.bounds(l - CARET_W, t, (w - l - r).max(0.0) + 2.0 * CARET_W, (h - t - b).max(0.0));
            let caret = core.flag(s.oid, "activeFocus") && core.0.borrow().keys.caret_on;
            p.with_clip(area, |p| {
                fonts.draw(p, &font, &s.xf, g.left as f32, top + m.ascent, &txt, c, s.opacity);
                if caret {
                    let at = s.xf.times(&Xf::translate((g.left + g.caret).round(), top as f64));
                    let spec = rectangle::Spec { w: CARET_W as f32, h: line, color: ink, ..rectangle::Spec::default() };
                    if let Some(g) = rectangle::geometry(&spec) {
                        p.geometry(&g, &at, s.opacity);
                    }
                }
            });
        }
        // Drawn by the list itself (`draw_list`, `draw_effect`), or nothing
        Draw::Area | Draw::View | Draw::Layer | Draw::Effect | Draw::Control | Draw::Shape => {}
    }
}

/// Text lines with x start and baseline in the box `(x, y, w, h)`, per
/// `horizontalAlignment` and `verticalAlignment`
fn placed_lines<'a>(core: &Core, oid: Oid, l: &'a text::layout::Laid, (x, y, w, h): (f32, f32, f32, f32),
                    width: impl Fn(&text::layout::Line) -> f32) -> Vec<(&'a text::layout::Line, f32, f32)> {
    let top = y + text_top(core, oid, l, h);
    let halign = core.num(oid, "horizontalAlignment") as i64;
    l.lines.iter().enumerate().map(|(i, line)| {
        let lw = width(line);
        let left = x + if halign & 4 != 0 {
            (w - lw) / 2.0
        } else if halign & 2 != 0 {
            w - lw
        } else {
            0.0
        };
        (line, left, top + l.step * i as f32 + l.metrics.ascent)
    }).collect()
}

/// Top of the first line in a Text `h` high, aligned by content height (lines
/// rounded up)
fn text_top(core: &Core, oid: Oid, l: &text::layout::Laid, h: f32) -> f32 {
    let valign = core.num(oid, "verticalAlignment") as i64;
    let content = l.height.ceil();
    if valign & 128 != 0 {
        (h - content) / 2.0
    } else if valign & 64 != 0 {
        h - content
    } else {
        0.0
    }
}

/// Laid-out line and offset of character `at`. Lines cover the text in order,
/// split at a newline or space
fn cursor_line(text: &str, l: &text::layout::Laid, at: usize) -> (usize, usize) {
    let chars: Vec<char> = text.chars().collect();
    let mut pos = 0;
    for (i, line) in l.lines.iter().enumerate() {
        let n = line.text.chars().count();
        if at <= pos + n || i + 1 == l.lines.len() {
            return (i, at.saturating_sub(pos).min(n));
        }
        pos += n;
        // Skips the newline or wrapping space at a break
        if chars.get(pos).is_some_and(|c| *c == '\n' || c.is_whitespace()) {
            pos += 1;
        }
    }
    (0, 0)
}

/// TextEdit layout options: no eliding or line limit; line `step` is font
/// height rounded up
fn edit_opts(core: &Core, oid: Oid, width: Option<f32>, step: f32) -> Opts {
    Opts {
        width,
        wrap: core.num(oid, "wrapMode") as i64,
        elide: 0,
        max_lines: usize::MAX,
        line_height: step,
        line_height_fixed: true,
    }
}

/// Anchor result: position and size on each axis
#[derive(Clone, Copy, Default)]
pub struct Held {
    pub across: bool,
    pub down: bool,
    pub width: bool,
    pub height: bool,
}

pub fn held(core: &Core, c: Oid) -> Held {
    let set = |name: &str| core.peek_named(c, name).is_some_and(|v| match v {
        crate::object::value::Value::Str(s) => parse_line(&s).is_some(),
        v => v.as_obj().is_some(),
    });
    let (fill, center) = (set("anchors.fill"), set("anchors.centerIn"));
    let (l, r, t, b) = (set("anchors.left"), set("anchors.right"), set("anchors.top"), set("anchors.bottom"));
    let (hc, vc) = (set("anchors.horizontalCenter"), set("anchors.verticalCenter"));
    Held {
        across: fill || center || l || r || hc,
        down: fill || center || t || b || vc,
        width: fill || (!center && l && r),
        height: fill || (!center && t && b),
    }
}

/// Whether an item is a pressable button (`control.rs`)
pub fn is_button(core: &Core, oid: Oid) -> bool {
    core.0.borrow().objs.get(oid).is_some_and(|o| types::is_a(o.def, "AbstractButton"))
}

/// Image target and source rects, as in QQuickImage::updatePaintNode: Stretch
/// (0) fills the box, PreserveAspectFit (1) uses painted size at the aligned
/// offset, PreserveAspectCrop (2) fills the box showing the aligned part.
/// `(x, y, w, h)` in the item, then in picture pixels
fn image_rects(core: &Core, oid: Oid, w: f64, h: f64, pic: &Picture) -> ((f64, f64, f64, f64), (f64, f64, f64, f64)) {
    let (iw, ih) = (pic.w as f64, pic.h as f64);
    // An icon is fitted keeping its shape, whatever the Image under it says
    let def = core.0.borrow().objs[oid].def;
    let fill = if types::is_a(def, "IconImage") || types::is_a(def, "Icon") { 1 } else { core.num(oid, "fillMode") as i64 };
    let (ha, va) = (core.num(oid, "horizontalAlignment") as i64, core.num(oid, "verticalAlignment") as i64);
    let (pw, ph) = if fill == 1 {
        let (kw, kh) = (w / iw, h / ih);
        if kw <= kh { (w, kw * ih) } else { (kh * iw, h) }
    } else {
        (iw, ih)
    };
    // Image.AlignHCenter (4) and AlignVCenter (128) are the defaults
    let offset = |room: f64, centre: bool, far: bool| -> f64 {
        if centre { (room / 2.0).trunc() } else if far { room.ceil() } else { 0.0 }
    };
    let xo = offset(w - pw, ha & 4 != 0, ha & 2 != 0);
    let yo = offset(h - ph, va & 128 != 0, va & 64 != 0);
    match fill {
        1 => ((xo, yo, pw, ph), (0.0, 0.0, iw, ih)),
        2 => {
            let (ws, hs) = (w / iw, h / ih);
            let source = if ws > hs {
                let src = ((hs / ws) * ih).trunc();
                let y = if va & 128 != 0 { ((ih - src) / 2.0).ceil() } else if va & 64 != 0 { (ih - src).ceil() } else { 0.0 };
                (0.0, y, iw, src)
            } else {
                let src = ((ws / hs) * iw).trunc();
                let x = if ha & 4 != 0 { ((iw - src) / 2.0).ceil() } else if ha & 2 != 0 { (iw - src).ceil() } else { 0.0 };
                (x, 0.0, src, ih)
            };
            ((0.0, 0.0, w, h), source)
        }
        _ => ((0.0, 0.0, w, h), (0.0, 0.0, iw, ih)),
    }
}

/// Rectangle node request, as in QQuickRectangle::updatePaintNode: antialiased
/// if set or when a radius is over 0; a border only if width or color was set,
/// visible, and at least a pixel when rounded (QQuickPen's `m_valid`), rounded
/// to a whole pixel when pixel aligned; gradient stops from `gradient_of`
fn rect_spec(core: &Core, oid: Oid, (w, h): (f64, f64)) -> rectangle::Spec {
    let set = |n: &str| core.is_explicit(oid, n);
    let radius = core.num(oid, "radius");
    let corner = |n: &str| Some(core.num(oid, n)).filter(|r| set(n) && *r >= 0.0).map(|r| r as f32);
    let corners = [corner("topLeftRadius"), corner("topRightRadius"), corner("bottomLeftRadius"), corner("bottomRightRadius")];
    let antialiasing = if set("antialiasing") {
        core.flag(oid, "antialiasing")
    } else {
        radius > 0.0 || corners.iter().flatten().any(|r| *r > 0.0)
    };
    let width = if set("border.width") { core.num(oid, "border.width") } else { 1.0 };
    let aligned = core.flag(oid, "border.pixelAligned");
    let border_color = colour(&core.text(oid, "border.color"));
    let pen = (set("border.width") || set("border.color"))
        && border_color >> 24 != 0
        && (width.round() >= 1.0 || (!aligned && width > 0.0));
    let pen_width = if !pen { 0.0 } else if aligned { width.round() } else { width };
    let (stops, vertical) = gradient_of(core, oid).unwrap_or((Vec::new(), true));
    rectangle::Spec {
        w: w as f32,
        h: h as f32,
        color: colour(&core.text(oid, "color")),
        border_color,
        pen_width: pen_width as f32,
        radius: radius as f32,
        corners,
        antialiasing,
        stops,
        vertical,
    }
}

/// An item's `gradient`: stops sorted by position, a later stop at the same
/// position first (`gradientStops`), and whether it is vertical (not
/// Gradient.Horizontal)
pub(super) fn gradient_of(core: &Core, oid: Oid) -> Option<(Vec<(f32, u32)>, bool)> {
    let g = core.peek_named(oid, "gradient").and_then(|v| v.as_obj())?;
    let mut stops: Vec<(f32, u32)> = Vec::new();
    let list = core.peek_named(g, "stops").and_then(|v| v.as_list()).unwrap_or_default();
    for st in list.iter().filter_map(|v| v.as_obj()) {
        let pos = core.num(st, "position") as f32;
        let j = stops.iter().position(|(p, _)| *p >= pos).unwrap_or(stops.len());
        stops.insert(j, (pos, colour(&core.text(st, "color"))));
    }
    Some((stops, core.num(g, "orientation") as i64 != 1))
}

/// How wide a TextInput's caret is
const CARET_W: f64 = 2.0;

/// TextInput text origin, scrolled to keep the caret visible, and caret offset
struct InputGeometry {
    left: f64,
    caret: f64,
}

fn input_geometry(fonts: &mut Fonts, core: &Core, oid: Oid, font: &Font, w: f64) -> InputGeometry {
    let (l, r, _, _) = paddings(core, oid);
    let aw = (w - l - r).max(0.0);
    let shown = crate::input::text::shown(core, oid);
    let at = core.num(oid, "cursorPosition").max(0.0) as usize;
    let caret = fonts.caret(font, &shown, at) as f64;
    let tw = if shown.is_empty() {
        fonts.width(font, &core.text(oid, "placeholderText")) as f64
    } else {
        fonts.width(font, &shown) as f64
    };
    let halign = core.num(oid, "horizontalAlignment") as i64;
    let base = if tw <= aw {
        if halign & 2 != 0 { l + aw - tw } else if halign & 4 != 0 { l + (aw - tw) / 2.0 } else { l }
    } else {
        l
    };
    // Scrolls only when the text is wider than the box; otherwise alignment
    // applies, as in Qt
    let scroll = if core.flag(oid, "autoScroll") && tw > aw { (base - l + caret + CARET_W - aw).max(0.0) } else { 0.0 };
    InputGeometry { left: base - scroll, caret }
}

/// Resolved `font`: set values over the application font (`QFont::resolve`,
/// `text/platform.rs`). `font.bold` is weight 700 or 400, point sizes are 96/72
/// px. An empty family is the application's
fn font_of(fonts: &mut Fonts, core: &Core, oid: Oid) -> Option<Font> {
    let app = fonts.app().clone();
    let set = |name: &str| core.is_explicit(oid, name);
    let (px, pt) = (core.num(oid, "font.pixelSize"), core.num(oid, "font.pointSize"));
    let px = if set("font.pixelSize") && px > 0.0 {
        px as f32
    } else if set("font.pointSize") && pt > 0.0 {
        text::platform::points_to_px(pt as f32)
    } else {
        app.px()
    };
    let weight = if set("font.bold") {
        if core.flag(oid, "font.bold") { 700.0 } else { 400.0 }
    } else if set("font.weight") {
        core.num(oid, "font.weight") as f32
    } else {
        app.weight
    };
    let family = Some(core.text(oid, "font.family")).filter(|f| set("font.family") && !f.is_empty()).unwrap_or(app.family.clone());
    let style = if set("font.styleName") { core.text(oid, "font.styleName") } else { app.style.clone() };
    let italic = if set("font.italic") { core.flag(oid, "font.italic") } else { app.italic };
    let tags = |name: &str| -> Vec<([u8; 4], f64)> {
        core.text(oid, name).split(',').filter_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            let tag: [u8; 4] = k.as_bytes().try_into().ok()?;
            Some((tag, v.parse::<f64>().ok()?))
        }).collect()
    };
    let axes = tags("font.__axes").into_iter().map(|(t, v)| (t, v as f32)).collect();
    let features = tags("font.__features").into_iter().map(|(t, v)| (t, v as u32)).collect();
    fonts.resolve(&Request {
        family,
        weight: if weight > 0.0 { weight } else { 400.0 },
        italic,
        px: px.max(1.0),
        style,
        axes,
        features,
        hint: text::Hint::from_preference(core.num(oid, "font.hintingPreference") as i64),
        render: render_of(core, oid),
    })
}

/// Text rendering: `renderType` Text.QtRendering (0) uses distance fields,
/// NativeRendering (1) FreeType, CurveRendering (2) native here; otherwise the
/// shell's setting
fn render_of(core: &Core, oid: Oid) -> text::Render {
    let native = if core.is_explicit(oid, "renderType") {
        core.num(oid, "renderType") as i64 != 0
    } else {
        core.0.borrow().native_text
    };
    if native { text::Render::Native } else { text::Render::Field }
}

/// A Text's wrapping, cutting and spacing, at `width` when it has one
fn text_opts(core: &Core, oid: Oid, width: Option<f32>) -> Opts {
    let max = core.num(oid, "maximumLineCount");
    Opts {
        width,
        wrap: core.num(oid, "wrapMode") as i64,
        elide: core.num(oid, "elide") as i64,
        max_lines: if max >= 1.0 && max < 2147483647.0 { max as usize } else { usize::MAX },
        line_height: core.num(oid, "lineHeight") as f32,
        line_height_fixed: core.num(oid, "lineHeightMode") as i64 == 1,
    }
}

/// TextInput padding: left, right, top, bottom (`control::pads`)
fn paddings(core: &Core, oid: Oid) -> (f64, f64, f64, f64) {
    let p = control::pads(core, oid);
    (p.left, p.right, p.top, p.bottom)
}

/// Whether `place` sets `oid`'s size: a `RowLayout` or `ColumnLayout` item,
/// `anchors.fill`, or opposite anchors on one axis.
/// Latest mark of `oid` and its subtree (`Core::touch`). A destroyed object
/// counts as newest
fn below(core: &Core, oid: Oid) -> u64 {
    core.0.borrow().objs.get(oid).map_or(u64::MAX, |o| o.below)
}

/// When `oid` itself last changed
fn changed_at(core: &Core, oid: Oid) -> u64 {
    core.0.borrow().objs.get(oid).map_or(u64::MAX, |o| o.changed)
}

/// Whether partial layouts and lists are checked against whole ones: in tests,
/// unless `AOSH_SCENE_CHECK=0`
#[cfg(test)]
fn scene_check() -> bool {
    std::env::var("AOSH_SCENE_CHECK").map_or(true, |v| v != "0")
}

fn sized_by_others(core: &Core, oid: Oid, parent: Option<Oid>) -> (bool, bool) {
    if let Some(p) = parent {
        match Scene::kind_name(core, p) {
            "RowLayout" | "ColumnLayout" | "GridLayout" => return (true, true),
            // A Loader's item, when the Loader has a size of its own
            "Loader" => {
                let (w, h) = size_given(core, p);
                if w || h {
                    return (w, h);
                }
            }
            // Control's content, and its background unless it has a size
            // (`control.rs`)
            "Control" => {
                if let Some(sized) = control::sizes(core, p, oid) {
                    return sized;
                }
            }
            _ => {}
        }
    }
    if core.peek_named(oid, "anchors.fill").and_then(|v| v.as_obj()).is_some() {
        return (true, true);
    }
    // Centred, the lines hold nothing (`Scene::anchor`)
    if core.peek_named(oid, "anchors.centerIn").and_then(|v| v.as_obj()).is_some() {
        return (false, false);
    }
    let line = |name: &str| core.peek_named(oid, name).is_some_and(|v| match v {
        crate::object::value::Value::Str(s) => parse_line(&s).is_some(),
        _ => false,
    });
    (line("anchors.left") && line("anchors.right"), line("anchors.top") && line("anchors.bottom"))
}

/// Whether an item's size is explicit (file, anchors, or layout) rather than
/// implicit: QQuickItem's `widthValid()`
fn size_given(core: &Core, oid: Oid) -> (bool, bool) {
    let parent = core.0.borrow().objs.get(oid).and_then(|o| o.parent);
    let (w, h) = sized_by_others(core, oid, parent);
    (w || core.is_explicit(oid, "width"), h || core.is_explicit(oid, "height"))
}

/// Parses `#aarrggbb`; anything else is transparent
pub fn colour(s: &str) -> u32 {
    s.strip_prefix('#').filter(|h| h.len() == 8).and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or(0)
}

/// `anchor:<object>:<line>`
fn parse_line(s: &str) -> Option<(Oid, String)> {
    let rest = s.strip_prefix("anchor:")?;
    let (o, l) = rest.split_once(':')?;
    Some((o.parse().ok()?, l.to_string()))
}

/// Grid cells, as in QQuickGrid: `columns` across or `rows` down (default 4
/// across). Columns fit their widest item, rows their tallest. `flow` is
/// LeftToRight (0) or TopToBottom (1). `rowSpacing` and `columnSpacing`
/// override `spacing`
struct GridCells {
    cols: usize,
    rows: usize,
    by_rows: bool,
    col_w: Vec<f64>,
    row_h: Vec<f64>,
    col_gap: f64,
    row_gap: f64,
}

impl GridCells {
    fn cell(&self, k: usize) -> (usize, usize) {
        if self.by_rows { (k / self.cols, k % self.cols) } else { (k % self.rows, k / self.rows) }
    }

    fn at(&self, k: usize) -> (f64, f64) {
        let (r, c) = self.cell(k);
        let x: f64 = self.col_w[..c].iter().map(|w| w + self.col_gap).sum();
        let y: f64 = self.row_h[..r].iter().map(|h| h + self.row_gap).sum();
        (x, y)
    }

    fn width(&self) -> f64 {
        self.col_w.iter().sum::<f64>() + gaps(self.col_gap, self.col_w.len())
    }

    fn height(&self) -> f64 {
        self.row_h.iter().sum::<f64>() + gaps(self.row_gap, self.row_h.len())
    }
}

fn grid(core: &Core, oid: Oid, shown: &[Oid]) -> GridCells {
    let n = shown.len().max(1);
    let (c, r) = (core.num(oid, "columns") as i64, core.num(oid, "rows") as i64);
    let (cols, rows) = match (c > 0, r > 0) {
        (true, true) => (c as usize, r as usize),
        (true, false) => (c as usize, n.div_ceil(c as usize)),
        (false, true) => (n.div_ceil(r as usize), r as usize),
        (false, false) => (4, n.div_ceil(4)),
    };
    // Columns or rows past the last item take no space
    let by_rows = core.num(oid, "flow") as i64 == 0;
    let (cols, rows) = if by_rows { (cols.min(n), rows) } else { (cols, rows.min(n)) };
    let sp = core.num(oid, "spacing");
    let gap = |name: &str| if core.is_explicit(oid, name) && core.num(oid, name) >= 0.0 { core.num(oid, name) } else { sp };
    let mut g = GridCells {
        cols, rows, by_rows, col_w: vec![0.0; cols], row_h: vec![0.0; rows],
        col_gap: gap("columnSpacing"), row_gap: gap("rowSpacing"),
    };
    for (k, &item) in shown.iter().enumerate() {
        let (r, c) = g.cell(k);
        if r >= rows || c >= cols {
            continue;
        }
        g.col_w[c] = g.col_w[c].max(core.num(item, "width"));
        g.row_h[r] = g.row_h[r].max(core.num(item, "height"));
    }
    g
}

/// Axis and spacing of a layout's line: RowLayout across, ColumnLayout down,
/// GridLayout down with one column or top-to-bottom flow with no row limit,
/// otherwise across. Multi-line grids are not implemented
fn linear(core: &Core, oid: Oid, n: usize) -> (bool, f64) {
    match Scene::kind_name(core, oid) {
        "ColumnLayout" => (false, core.num(oid, "spacing")),
        "GridLayout" => {
            let (cols, rows) = (core.num(oid, "columns") as i64, core.num(oid, "rows") as i64);
            let down = cols == 1 || (core.num(oid, "flow") as i64 == 1 && rows < 0);
            let lines = if down { if rows > 0 { (n as i64 + rows - 1) / rows } else { 1 } }
                        else if cols > 0 { (n as i64 + cols - 1) / cols } else { 1 };
            if lines > 1 {
                core.log_once(oid, "a GridLayout of more than one row and column is laid out as one line for now");
            }
            if down { (false, core.num(oid, "rowSpacing")) } else { (true, core.num(oid, "columnSpacing")) }
        }
        _ => (true, core.num(oid, "spacing")),
    }
}

/// Flow item positions: along a line `spacing` apart, wrapping when the next
/// item would pass the Flow's width (height when flowing down). With no size
/// there is one line
fn flow(core: &Core, oid: Oid, shown: &[Oid]) -> Vec<(f64, f64)> {
    let sp = core.num(oid, "spacing");
    let across = core.num(oid, "flow") as i64 == 0;
    let (gw, gh) = size_given(core, oid);
    let edge = match (across, gw, gh) {
        (true, true, _) => core.num(oid, "width"),
        (false, _, true) => core.num(oid, "height"),
        _ => f64::INFINITY,
    };
    let (mut along, mut line, mut thick) = (0.0, 0.0, 0.0f64);
    shown.iter().map(|&c| {
        let (len, th) = if across { (core.num(c, "width"), core.num(c, "height")) } else { (core.num(c, "height"), core.num(c, "width")) };
        if along > 0.0 && along + len > edge {
            line += thick + sp;
            along = 0.0;
            thick = 0.0;
        }
        let at = if across { (along, line) } else { (line, along) };
        along += len + sp;
        thick = thick.max(th);
        at
    }).collect()
}

fn gaps(spacing: f64, n: usize) -> f64 {
    if n > 1 { spacing * (n - 1) as f64 } else { 0.0 }
}

/// Preferred size of a layout item along one axis: `Layout.preferred` or its
/// own, rounded up as in Qt (`layouts_are_qts`). A value just over an integer
/// counts as that integer, since float advance sums land there
fn preferred(core: &Core, c: Oid, horizontal: bool) -> f64 {
    let (pref, explicit, implicit) = if horizontal {
        ("Layout.preferredWidth", "width", "implicitWidth")
    } else {
        ("Layout.preferredHeight", "height", "implicitHeight")
    };
    let p = core.num(c, pref);
    let v = if p >= 0.0 && core.is_explicit(c, pref) {
        p
    } else if core.is_explicit(c, explicit) {
        core.num(c, explicit)
    } else {
        core.num(c, implicit)
    };
    (v - 1e-4).ceil().max(0.0)
}

/// Space an item takes in a layout along one axis: preferred size plus margins
fn cell(core: &Core, c: Oid, horizontal: bool) -> f64 {
    let (l, r, t, b) = layout_margins(core, c);
    preferred(core, c, horizontal) + if horizontal { l + r } else { t + b }
}

/// `Layout.margins` with per-side overrides: left, right, top, bottom
fn layout_margins(core: &Core, c: Oid) -> (f64, f64, f64, f64) {
    let m = core.num(c, "Layout.margins");
    let side = |n: &str| if core.is_explicit(c, n) { core.num(c, n) } else { m };
    (side("Layout.leftMargin"), side("Layout.rightMargin"), side("Layout.topMargin"), side("Layout.bottomMargin"))
}

fn limits(core: &Core, c: Oid, horizontal: bool) -> (f64, f64) {
    let (lo, hi) = if horizontal {
        ("Layout.minimumWidth", "Layout.maximumWidth")
    } else {
        ("Layout.minimumHeight", "Layout.maximumHeight")
    };
    let hi = core.peek_named(c, hi).map(|v| v.as_num()).unwrap_or(f64::INFINITY);
    (core.num(c, lo), if hi.is_nan() { f64::INFINITY } else { hi })
}

fn hash<T: Hash>(t: &T) -> u64 {
    let mut h = DefaultHasher::new();
    t.hash(&mut h);
    h.finish()
}

/// Damage for one frame as a few rectangles. Overlapping or touching rectangles
/// join when their box costs no more; past `DAMAGE_RECTS`, the pair whose box
/// wastes least is joined
#[derive(Default)]
struct Damage(Vec<Rect>);

/// How many rectangles one frame keeps apart
const DAMAGE_RECTS: usize = 8;

impl Damage {
    fn add(&mut self, r: Rect) {
        if r.is_empty() {
            return;
        }
        let mut r = r;
        while let Some(i) = self.0.iter().position(|d| d.union(&r).area() <= d.area() + r.area()) {
            r = r.union(&self.0.swap_remove(i));
        }
        self.0.push(r);
        while self.0.len() > DAMAGE_RECTS {
            let mut best = (0, 1, i64::MAX);
            for i in 0..self.0.len() {
                for j in i + 1..self.0.len() {
                    let (a, b) = (self.0[i], self.0[j]);
                    let waste = a.union(&b).area() - a.area() - b.area();
                    if waste < best.2 {
                        best = (i, j, waste);
                    }
                }
            }
            let b = self.0.swap_remove(best.1);
            let a = self.0.swap_remove(best.0);
            self.add(a.union(&b));
        }
    }
}

/// Rectangles to repaint between two display lists.
///
/// Entries match by identity (item, what it draws, and its index among that
/// item's entries of that kind), not list position, so inserting one does not
/// dirty the rest. An added or removed entry costs its rectangle; a changed one
/// costs both, except a flat rectangle that only moved or resized
/// (`Shown::flat`), which costs the difference and its edges.
///
/// Order matters where items overlap: an entry now after one it used to precede
/// costs both rectangles
fn changed(now: &[Shown], was: &[Shown], add: &mut impl FnMut(Rect)) {
    let seen = |s: &Shown| s.rect.intersect(&s.clip).unwrap_or_default();
    let keys = |list: &[Shown]| -> Vec<(Oid, Draw, u32)> {
        let mut count: HashMap<(Oid, Draw), u32> = HashMap::new();
        list.iter().map(|s| {
            let n = count.entry((s.oid, s.draw)).or_insert(0);
            *n += 1;
            (s.oid, s.draw, *n)
        }).collect()
    };
    let old: HashMap<(Oid, Draw, u32), usize> = keys(was).into_iter().enumerate().map(|(i, k)| (k, i)).collect();
    let mut kept = vec![false; was.len()];
    let mut furthest: Option<usize> = None;
    for (s, k) in now.iter().zip(keys(now)) {
        let Some(&j) = old.get(&k) else {
            add(seen(s));
            continue;
        };
        kept[j] = true;
        let reordered = furthest.is_some_and(|f| j < f);
        furthest = Some(furthest.map_or(j, |f| f.max(j)));
        if reordered || s.print == was[j].print {
            if reordered {
                add(seen(s));
                add(seen(&was[j]));
            }
            continue;
        }
        match (s.flat, was[j].flat) {
            // Only geometry changed: the area gained or lost, and old and new
            // edges with their antialiasing pixel
            (Some((a, edge)), Some((b, _))) if a == b && s.paint && was[j].paint => {
                let (new, old) = (seen(s), seen(&was[j]));
                for r in new.minus(&old).into_iter().chain(old.minus(&new))
                    .chain(new.ring(edge + 2)).chain(old.ring(edge + 2))
                {
                    add(r);
                }
            }
            _ => {
                add(seen(s));
                add(seen(&was[j]));
            }
        }
    }
    for (b, kept) in was.iter().zip(kept) {
        if !kept {
            add(seen(b));
        }
    }
}

#[cfg(test)]
mod tests;
