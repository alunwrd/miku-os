// Animation curves and per-object animation settings.
//
//Curves are sampled once when changed and stored in a lookup table. 
// Runtime evaluation uses the cached values instead of recalculating the curve.
//
// The animation table stores settings for each object type: duration, 
// curve and style.
//
// This code only provides animation parameters and curve values.
// Updating animations and applying them to surfaces is handled elsewhere.

extern crate alloc;

use spin::Mutex;

/// The curve itself lives in `libmay`, because a client eases too
///
/// What stays here is policy: which class of movement gets which curve and for
/// how long
pub use libmay::ease::Curve;

/// Maximum number of named curves. Each curve stores 255 cached samples.
pub const MAX_CURVES: usize = 16;

// Animation classes

/// Animation class IDs matching Hyprland config names: the protocol's, where
/// the window manager reads them too
pub use crate::mayland::protocol::wm::anim_class as class;

/// Animation style IDs.
pub mod style {
    pub const NONE: u32 = 0;
    pub const SLIDE: u32 = 1;
    /// Requires shader pass.
    pub const FADE: u32 = 2;
    /// Scale from `arg` percent, requires shader pass.
    pub const POPIN: u32 = 3;
}

#[derive(Clone, Copy)]
pub struct Spec {
    pub enabled: bool,
    /// Duration in milliseconds. Hyprland `speed` uses 100ms units.
    pub ms: u32,
    pub curve: u32,
    pub style: u32,
    /// Style argument, e.g. `popin 80%` is stored as `0.8`.
    pub arg: f32,
}

impl Spec {
    pub const OFF: Spec = Spec {
        enabled: false, ms: 0, curve: 0, style: style::NONE, arg: 1.0,
    };
}

struct Table {
    curves: [Curve; MAX_CURVES],
    specs: [Spec; class::COUNT as usize],
    /// Enables animation processing when configured.
    on: bool,
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    curves: [Curve::LINEAR; MAX_CURVES],
    specs: [Spec::OFF; class::COUNT as usize],
    on: false,
});

/// Set a curve by its config-defined id.
pub fn set_curve(id: u32, x1: f32, y1: f32, x2: f32, y2: f32) {
    if id as usize >= MAX_CURVES {
        return;
    }
    TABLE.lock().curves[id as usize] = Curve::new((x1, y1), (x2, y2));
}

/// Set animation settings for a class.
pub fn set_spec(class: u32, spec: Spec) {
    if class >= class::COUNT {
        return;
    }
    let mut t = TABLE.lock();
    t.specs[class as usize] = spec;
    // Enable animations when at least one animation class is configured.
    if spec.enabled {
        t.on = true;
    }
}

pub fn enabled() -> bool { TABLE.lock().on }

/// A class's own settings, as set, with no parent's in their place: what a
/// selftest that sets one puts back
pub fn own_spec(class: u32) -> Spec {
    TABLE.lock().specs[(class % class::COUNT) as usize]
}

pub fn set_enabled(on: bool) { TABLE.lock().on = on; }

/// Get animation settings for a class, falling back to its parent class.
pub fn spec(class: u32) -> Spec {
    let t = TABLE.lock();
    if !t.on {
        return Spec::OFF;
    }
    let direct = t.specs[(class % class::COUNT) as usize];
    if direct.enabled {
        return direct;
    }
    // Up the tree until a class that was set: `fadeLayersIn` is two steps
    // below `fade`, the rest one
    let mut at = class;
    loop {
        let up = match at {
            class::WINDOWS_IN | class::WINDOWS_OUT => class::WINDOWS,
            class::LAYERS_IN | class::LAYERS_OUT => class::LAYERS,
            class::WORKSPACES_IN | class::WORKSPACES_OUT => class::WORKSPACES,
            class::FADE_IN | class::FADE_OUT | class::FADE_LAYERS => class::FADE,
            class::FADE_LAYERS_IN | class::FADE_LAYERS_OUT => class::FADE_LAYERS,
            _ => return Spec::OFF,
        };
        let s = t.specs[up as usize];
        if s.enabled {
            return s;
        }
        at = up;
    }
}

/// The layer rules the window manager sent: what each does, and the
/// namespace it is for (`protocol::wm::SET_LAYER_RULE`)
static LAYER_RULES: Mutex<alloc::vec::Vec<(u32, libmay::conf::Text)>> = Mutex::new(alloc::vec::Vec::new());

/// More rules than a config writes for every layer a shell has
const MAX_LAYER_RULES: usize = 32;

/// One more layer rule. Past `MAX_LAYER_RULES` the rest are dropped: they
/// are a config's, and a config that long is a mistake to survive
pub fn add_layer_rule(rule: u32, pattern: &str) {
    let mut r = LAYER_RULES.lock();
    if r.len() < MAX_LAYER_RULES {
        r.push((rule, libmay::conf::Text::from(pattern)));
    }
}

/// Whether a layer of this namespace has `rule`
pub fn layer_has(namespace: &libmay::conf::Text, rule: u32) -> bool {
    LAYER_RULES.lock().iter().any(|(r, p)| *r == rule && p.matches(namespace.as_str()))
}

/// Get curve progress at normalized time `x`.
pub fn curve_y(id: u32, x: f32) -> f32 {
    let t = TABLE.lock();
    t.curves[(id as usize).min(MAX_CURVES - 1)].y_at(x)
}

/// Forgets every curve and every class, back to nothing animating
///
/// For the selftest, so a case leaves no animation behind, and for
/// `may_wm.reset`, because a deleted `animation = layers, ...` line has no
/// other way to stop being true.
///
/// Running animations are not touched: one half way across the screen has its
/// curve and duration, and stopping it would leave the window wherever the last
/// frame put it.
///
/// The layer rules go with them: they come from the same file
pub fn reset() {
    let mut t = TABLE.lock();
    t.curves = [Curve::LINEAR; MAX_CURVES];
    t.specs = [Spec::OFF; class::COUNT as usize];
    t.on = false;
    drop(t);
    LAYER_RULES.lock().clear();
}

// One running animation

/// State of one running surface animation.
///
/// Starting values are required for correct interpolation.
#[derive(Clone, Copy)]
pub struct Anim {
    pub running: bool,
    /// Start time in microseconds, from `layout::tick`.
    pub begun_us: u64,
    pub ms: u32,
    pub curve: u32,
    pub style: u32,
    /// Where the position started
    pub from: (f32, f32),
    /// Initial and target alpha/scale values.
    pub from_alpha: f32,
    pub to_alpha: f32,
    pub from_scale: f32,
    pub to_scale: f32,
    /// Current interpolated values.
    pub alpha: f32,
    pub scale: f32,
    /// The alpha's own clock: Hyprland moves a surface's alpha on the `fade`
    /// classes and its size on the others, with their own durations and curves.
    /// 0 is the alpha going with `ms` and `curve`
    pub alpha_ms: u32,
    pub alpha_curve: u32,
}

impl Anim {
    pub const NONE: Anim = Anim {
        running: false,
        begun_us: 0,
        ms: 0,
        curve: 0,
        style: style::NONE,
        from: (0.0, 0.0),
        from_alpha: 1.0,
        to_alpha: 1.0,
        from_scale: 1.0,
        to_scale: 1.0,
        alpha: 1.0,
        scale: 1.0,
        alpha_ms: 0,
        alpha_curve: 0,
    };

    /// Returns animation progress in the range 0.0..1.0.
    pub fn progress(&self, now_us: u64) -> f32 {
        if self.ms == 0 {
            return 1.0;
        }
        let spent = now_us.saturating_sub(self.begun_us) as f32;
        (spent / (self.ms as f32 * 1000.0)).clamp(0.0, 1.0)
    }

    /// The alpha's progress and the curve it is read through: its own clock
    /// when it has one, else the movement's
    pub fn alpha_progress(&self, now_us: u64) -> (f32, u32) {
        if self.alpha_ms == 0 {
            return (self.progress(now_us), self.curve);
        }
        let spent = now_us.saturating_sub(self.begun_us) as f32;
        ((spent / (self.alpha_ms as f32 * 1000.0)).clamp(0.0, 1.0), self.alpha_curve)
    }
}
