// Hyprland-shaped config: what each line means to the window manager. The
// grammar is in `libmay::conf`; `libmay::manifest` decides the file list and
// order. Two passes: variables from all files first, then lines.
//
//     $mod = SUPER
//     general {
//         gaps_in = 12
//         layout = dwindle
//     }
//     input {
//         follow_mouse = 1
//         repeat_rate = 40
//     }
//     source = /etc/mayland/extra.conf
//     exec-once = /bin/aosh
//     bind  = $mod, Q, killactive
//     binde = $mod, L, resizeactive, 10 0
//     submap = resize
//     bind  = , escape, submap, reset
//     submap = reset
//     windowrule = float, class:calculator
//
// A key in a block maps to a `param` number sent to the compositor.
// Unrecognized lines are reported with their line number; a section this
// desktop lacks (`misc`, `cursor`, `debug`) is reported once.

use crate::conf::{self, Note, Sink, Where};
pub use crate::conf::Text;
use crate::manifest::{self, Manifest};
use crate::miku;
use crate::protocol::wm::{param, state};

pub const MAX_BINDS: usize = 64;
pub const MAX_RULES: usize = 32;
/// Layer rules (`shell::anim::MAX_LAYER_RULES`)
pub const MAX_LAYER_RULES: usize = 32;
pub const MAX_PARAMS: usize = 32;
/// Named curves and animation classes, matching the compositor's limits
pub const MAX_CURVES: usize = 16;
pub const MAX_ANIMS: usize = 16;
/// `exec-once` lines
pub const MAX_EXEC: usize = 8;
/// Named key maps, excluding the default
pub const MAX_SUBMAPS: usize = 8;
/// `monitor =` lines
pub const MAX_MONITORS: usize = 8;

/// Settings that are fields rather than params: the layout name and border
/// values
const N_CLAIMS: usize = 6;
const CLAIM_LAYOUT: usize = 0;
const CLAIM_BORDER_SIZE: usize = 1;
const CLAIM_BORDER_ACTIVE: usize = 2;
const CLAIM_BORDER_INACTIVE: usize = 3;
const CLAIM_WALLPAPER: usize = 4;
const CLAIM_MISC_VRR: usize = 5;

/// Compositor chord limit. Also checked at parse time, to report the total at
/// once
pub use crate::protocol::wm::MAX_CHORDS;

/// Complaint print limit
const MAX_NOTES: u32 = 16;

/// A key binding's action: dispatcher and one argument. `text` holds a name or
/// command line
#[derive(Clone, Copy)]
pub struct CurveDef {
    pub name: Text,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

impl CurveDef {
    pub const EMPTY: CurveDef =
        CurveDef { name: Text::EMPTY, x1: 0.0, y1: 0.0, x2: 1.0, y2: 1.0 };
}

/// `animation = windows, 1, 4, easeOut, popin 80%`
#[derive(Clone, Copy)]
pub struct AnimDef {
    pub class: u32,
    pub enabled: bool,
    /// In hundreds of milliseconds, as the file writes it
    pub speed: f32,
    pub curve: u32,
    pub style: u32,
    pub arg: f32,
}

impl AnimDef {
    pub const EMPTY: AnimDef = AnimDef {
        class: 0, enabled: false, speed: 0.0, curve: 0, style: 0, arg: 1.0,
    };
}

#[derive(Clone, Copy)]
pub struct Action {
    pub dispatch: u8,
    pub arg: i32,
    pub text: Text,
}

#[derive(Clone, Copy)]
pub struct Bind {
    pub mods: u32,
    pub key: u32,
    /// Letters after `bind` (`protocol::wm::chord`), acted on by the compositor
    pub flags: u32,
    /// Key map of this binding. 0 is the default, live in every submap
    pub submap: u8,
    pub action: Action,
}

impl Bind {
    pub const EMPTY: Bind = Bind {
        mods: 0, key: 0, flags: 0, submap: 0,
        action: Action { dispatch: 0, arg: 0, text: Text::EMPTY },
    };
}

/// What a window rule does to the window it caught
#[derive(Clone, Copy)]
pub enum Effect {
    /// Booleans: floating, fullscreen, pinned, nofocus
    State { mask: u32, flags: u32 },
    /// One of the numbers, for that window alone
    Param { param: u32, value: f32 },
    /// Open it somewhere else
    Workspace(u32),
    /// `size 800 600`, for floating windows only
    Size(i32, i32),
    /// `move 100 40`
    Move(i32, i32),
    /// `center`
    Center,
    /// `opacity a [i]`: both opacities. One number is both, and each multiplies
    /// the desktop's unless followed by `override`
    Opacity { active: f32, inactive: f32, own_active: bool, own_inactive: bool },
}

#[derive(Clone, Copy)]
pub enum Field {
    AppId,
    Title,
    /// `floating:1` or `floating:0`. Rules on this field rerun on change
    /// (`rules::refloat`)
    Floating(bool),
}

#[derive(Clone, Copy)]
pub struct Rule {
    pub field: Field,
    pub pat: Text,
    pub effect: Effect,
}

pub struct Config {
    pub binds: [Bind; MAX_BINDS],
    pub n_binds: usize,
    pub rules: [Rule; MAX_RULES],
    pub n_rules: usize,
    /// `layerrule = noanim, <namespace>` (`protocol::wm::SET_LAYER_RULE`)
    pub layer_rules: [(u32, Text); MAX_LAYER_RULES],
    pub n_layer_rules: usize,
    /// Params in config order with their source lines, one entry per param. The
    /// `Where` lets a later setter of equal authority be reported
    pub params: [(u32, f32, Where); MAX_PARAMS],
    pub n_params: usize,
    pub curves: [CurveDef; MAX_CURVES],
    pub n_curves: usize,
    pub anims: [AnimDef; MAX_ANIMS],
    pub n_anims: usize,
    /// `exec-once` lines, run after the connection is up
    pub execs: [Text; MAX_EXEC],
    pub n_execs: usize,
    /// Named key maps in first-mention order. Index zero, the default, is not
    /// here
    pub submaps: [Text; MAX_SUBMAPS],
    pub n_submaps: usize,
    /// `general { layout = ... }`
    pub layout: u32,
    /// `general { border_size }` and two `col.` colors. Separate from params,
    /// whose 24.8 fixed point cannot carry a color
    pub border_size: i32,
    pub border_active: u32,
    pub border_inactive: u32,
    /// `decoration { dim_inactive, dim_strength }`, sent as one number
    /// (`param::DIM`). Defaults: off and 0.5
    pub dim_on: bool,
    pub dim_strength: f32,
    /// `theme { wallpaper = <path> }`; the compositor decodes it
    pub wallpaper: Text,
    /// `monitor =` lines (`libmay::monitor`). A later line for the same output
    /// replaces the earlier
    pub monitors: [crate::monitor::Rule; MAX_MONITORS],
    pub n_monitors: usize,
    /// `misc { vrr }`: variable refresh for outputs whose line does not set it.
    /// `None` leaves outputs as they are
    pub misc_vrr: Option<u8>,
    /// Whether a file was read
    pub from_file: bool,
    /// Files of this config, for naming in complaints
    pub manifest: Manifest,

    // Parsing state
    /// Submap of the lines being read
    submap_now: u8,
    /// Complaint count, printed or not
    notes: u32,
    /// Which of `ABSENT` has already been spoken about
    said: u32,
    /// Where the non-param settings were last set
    claims: [Where; N_CLAIMS],
}

impl Config {
    pub const fn new() -> Config {
        Config {
            binds: [Bind::EMPTY; MAX_BINDS],
            n_binds: 0,
            rules: [Rule {
                field: Field::AppId, pat: Text::EMPTY,
                effect: Effect::Workspace(0),
            }; MAX_RULES],
            n_rules: 0,
            layer_rules: [(0, Text::EMPTY); MAX_LAYER_RULES],
            n_layer_rules: 0,
            params: [(0u32, 0.0f32, Where::NOWHERE); MAX_PARAMS],
            n_params: 0,
            curves: [CurveDef::EMPTY; MAX_CURVES],
            n_curves: 0,
            anims: [AnimDef::EMPTY; MAX_ANIMS],
            n_anims: 0,
            execs: [Text::EMPTY; MAX_EXEC],
            n_execs: 0,
            submaps: [Text::EMPTY; MAX_SUBMAPS],
            n_submaps: 0,
            layout: crate::protocol::wm::LAYOUT_MASTER,
            // Off unless set
            border_size: 0,
            border_active: 0xCFBDFE,
            border_inactive: 0x4C3E75,
            dim_on: false,
            dim_strength: 0.5,
            wallpaper: Text::EMPTY,
            monitors: [crate::monitor::Rule::EMPTY; MAX_MONITORS],
            n_monitors: 0,
            misc_vrr: None,
            from_file: false,
            manifest: Manifest::new(),
            submap_now: 0,
            notes: 0,
            said: 0,
            claims: [Where::NOWHERE; N_CLAIMS],
        }
    }

    /// Rule for output `name` with description `description`: its own line, a
    /// `desc:` line, then the empty-named one. The last of each kind wins
    pub fn monitor_rule(&self, name: &str, description: &str) -> Option<&crate::monitor::Rule> {
        use crate::monitor::By;
        let rules = &self.monitors[..self.n_monitors];
        [By::Name, By::Description, By::Any].iter().find_map(|by| {
            rules.iter().rev().find(|r| r.by == *by && r.matches(name, description))
        })
    }

    /// The name of a key map, for saying it out loud
    pub fn submap_name(&self, idx: u8) -> &str {
        match idx {
            0 => "default",
            i if (i as usize) <= self.n_submaps => self.submaps[i as usize - 1].as_str(),
            _ => "?",
        }
    }

    /// Complaint count
    pub fn n_notes(&self) -> u32 { self.notes }

    /// Records a param, replacing any earlier value. Double setting is handled
    /// by `clash`
    fn push_param(&mut self, at: Where, p: u32, v: f32) {
        if let Some(i) = (0..self.n_params).find(|&i| self.params[i].0 == p) {
            let before = self.params[i].2;
            self.params[i] = (p, v, at);
            self.clash(at, before, "this setting");
            return;
        }
        if self.n_params < MAX_PARAMS {
            self.params[self.n_params] = (p, v, at);
            self.n_params += 1;
        } else {
            self.complain(at, "no room for another setting", "");
        }
    }

    /// `push_param` for a number two keys share
    fn put_param(&mut self, at: Where, p: u32, v: f32) {
        match (0..self.n_params).find(|&i| self.params[i].0 == p) {
            Some(i) => self.params[i] = (p, v, at),
            None => self.push_param(at, p, v),
        }
    }

    /// The same for a field setting
    fn claim(&mut self, at: Where, slot: usize) {
        let before = self.claims[slot];
        self.claims[slot] = at;
        self.clash(at, before, "this setting");
    }

    /// Reports a setting set twice at equal authority, since only filename
    /// order decides between them. A higher layer overriding a lower one is
    /// silent
    fn clash(&mut self, now: Where, before: Where, what: &str) {
        if before == Where::NOWHERE
            || self.manifest.tier(before.file) != self.manifest.tier(now.file)
        {
            return;
        }
        // `already set in <path>:<line>`. The manifest borrow ends with the
        // copy
        let mut buf = [0u8; 128];
        let mut k = 0;
        let mut put = |bytes: &[u8], k: &mut usize| {
            let take = bytes.len().min(buf.len() - *k);
            buf[*k..*k + take].copy_from_slice(&bytes[..take]);
            *k += take;
        };
        put(b"already set in ", &mut k);
        put(self.manifest.path(before.file).as_bytes(), &mut k);
        put(b":", &mut k);
        let mut num = [0u8; 20];
        let n = miku::fmt_dec(&mut num, before.line as u64);
        put(&num[..n], &mut k);
        drop(put);
        let msg = core::str::from_utf8(&buf[..k]).unwrap_or("already set elsewhere");
        self.complain(now, what, msg);
    }

    fn push_bind(&mut self, at: Where, b: Bind) {
        if self.n_binds == MAX_BINDS {
            self.complain(at, "no room for another binding", "");
            return;
        }
        self.binds[self.n_binds] = b;
        self.n_binds += 1;
    }

    fn push_rule(&mut self, at: Where, r: Rule) {
        if self.n_rules == MAX_RULES {
            self.complain(at, "no room for another window rule", "");
            return;
        }
        self.rules[self.n_rules] = r;
        self.n_rules += 1;
    }

    fn push_exec(&mut self, at: Where, cmd: &str) {
        if !Text::fits(cmd) {
            // Refuses an over-long command
            self.complain(at, "exec-once: that command is too long to carry", cmd);
            return;
        }
        if self.n_execs == MAX_EXEC {
            self.complain(at, "no room for another exec-once", cmd);
            return;
        }
        self.execs[self.n_execs] = Text::from(cmd);
        self.n_execs += 1;
    }

    /// One `monitor =` line, replacing an earlier one about the same output
    fn push_monitor(&mut self, at: Where, value: &str) {
        let rule = match crate::monitor::parse(value) {
            Ok(r) => r,
            Err((why, what)) => {
                self.complain(at, why, what);
                return;
            }
        };
        let same = (0..self.n_monitors).find(|&i| {
            self.monitors[i].by == rule.by && self.monitors[i].name().eq_ignore_ascii_case(rule.name())
        });
        match same {
            Some(i) => self.monitors[i] = rule,
            None if self.n_monitors == MAX_MONITORS => {
                self.complain(at, "no room for another monitor line", rule.name());
            }
            None => {
                self.monitors[self.n_monitors] = rule;
                self.n_monitors += 1;
            }
        }
    }

    /// The index of a named key map, making one if this is its first mention
    fn submap_index(&mut self, at: Where, name: &str) -> u8 {
        if let Some(i) = (0..self.n_submaps)
            .find(|&i| self.submaps[i].as_str() == name)
        {
            return i as u8 + 1;
        }
        if self.n_submaps == MAX_SUBMAPS {
            self.complain(at, "no room for another submap", name);
            return 0;
        }
        self.submaps[self.n_submaps] = Text::from(name);
        self.n_submaps += 1;
        self.n_submaps as u8
    }

    /// Say something about one line, at most `MAX_NOTES` times in all
    fn complain(&mut self, at: Where, what: &str, detail: &str) {
        self.notes += 1;
        if self.notes > MAX_NOTES {
            if self.notes == MAX_NOTES + 1 {
                miku::println("  maywm: ...and more; the rest of the file is read in silence");
            }
            return;
        }
        miku::print("  maywm: ");
        // File-wide complaints have no line number
        if at.line > 0 {
            let mut buf = [0u8; 20];
            let n = miku::fmt_dec(&mut buf, at.line as u64);
            // The file by name
            miku::print(self.manifest.path(at.file));
            miku::print(":");
            miku::print(core::str::from_utf8(&buf[..n]).unwrap_or("?"));
            miku::print(": ");
        }
        miku::print(what);
        if !detail.is_empty() {
            miku::print(" - ");
            miku::print(detail);
        }
        miku::println("");
    }

    /// Say one of the standing facts once, however many lines ask for it
    fn once(&mut self, at: Where, i: usize) {
        let bit = 1u32 << i;
        if self.said & bit != 0 {
            // Counted regardless of the print limit
            self.notes += 1;
            return;
        }
        self.said |= bit;
        let (_, why) = ABSENT[i];
        self.complain(at, why, "");
    }

    /// Whole-file checks: chords fit in the compositor, and every submap can be
    /// left
    fn finish(&mut self) {
        // Chord budget: the default map plus the largest submap, since a submap
        // binds instead of the others
        let count = |cfg: &Config, m: u8| {
            (0..cfg.n_binds).filter(|&i| cfg.binds[i].submap == m).count()
        };
        let global = count(self, 0);
        let mut worst = 0;
        for m in 1..=self.n_submaps as u8 {
            worst = worst.max(count(self, m));
        }
        if global + worst > MAX_CHORDS {
            self.complain(Where::NOWHERE,
                "more chords than the compositor can hold at once - some will be refused", "");
        }
        // Warns about a submap with no way out
        for m in 1..=self.n_submaps as u8 {
            let has_exit = (0..self.n_binds).any(|i| {
                self.binds[i].submap == m
                    && self.binds[i].action.dispatch == crate::binds::D_SUBMAP
            });
            if !has_exit {
                let name = Text::from(self.submap_name(m));
                self.complain(Where::NOWHERE,
                    "this submap has no binding that leaves it", name.as_str());
            }
        }
    }
}

/// Sections and top-level lines this desktop lacks, reported once each. Capped
/// at 32 by the bitmask
static ABSENT: &[(&str, &str)] = &[
    ("misc",      "misc {} - of Hyprland's 39 settings there, only vrr exists here"),
    ("debug",     "debug {} - this compositor has no debug knobs"),
    ("cursor",    "cursor {} - the cursor is drawn by the compositor and has no settings"),
    ("render",    "render {} - nothing there is settable yet"),
    ("group",     "group {} - there are no window groups or tabs"),
    ("gestures",  "gestures {} - there is no touchpad"),
    ("binds",     "binds {} - none of its behaviour switches exist"),
    ("scrolling", "scrolling {} - there is no scrolling layout, only master, dwindle and float"),
    ("xwayland",  "xwayland {} - there is no X11 here and there will not be"),
    ("ecosystem", "ecosystem {} - nothing to opt in or out of"),
    ("opengl",    "opengl {} - the compositor does not use OpenGL"),
    ("experimental", "experimental {} - nothing experimental to switch on"),
    ("env",       "env = - this system has no environment variables to set"),
    ("workspace", "workspace = - workspaces take no rules yet"),
    ("permission","permission = - there is no permission system"),
    ("plugin",    "plugin = - plugins would mean loading code into the compositor"),
    ("exec-shutdown", "exec-shutdown = - nothing runs at shutdown"),
];

fn absent_index(name: &str) -> Option<usize> {
    ABSENT.iter().position(|(n, _)| *n == name)
}

/// Sections read by other programs, ignored silently: the panel's `theme`, the
/// login screen's `greeter`
static SOMEBODY_ELSE_S: &[&str] = &["theme", "panel", "greeter"];

/// Section and key to param. Sections join with a dot (`decoration.blur` /
/// `size`). Names follow Hyprland's
static KNOBS: &[(&str, &str, u32)] = &[
    ("general",           "gaps_in",         param::GAPS_IN),
    ("general",           "gaps_out",        param::GAPS_OUT),
    ("master",            "mfact",           param::MASTER_FACTOR),
    ("dwindle",           "split_ratio",     param::SPLIT_RATIO),
    ("dwindle",           "default_split_ratio", param::SPLIT_RATIO),
    ("decoration",        "rounding",        param::ROUNDING),
    ("decoration",        "rounding_power",  param::ROUNDING_POWER),
    ("decoration",        "active_opacity",  param::ALPHA),
    ("decoration",        "inactive_opacity", param::INACTIVE_ALPHA),
    ("decoration",        "shadow_strength", param::SHADOW),
    ("decoration",        "shadow_range",    param::SHADOW_SIZE),
    ("decoration.blur",   "enabled",         param::EFFECTS),
    ("decoration.blur",   "size",            param::BLUR_SIZE),
    ("decoration.shadow", "range",           param::SHADOW_SIZE),
    ("animations",        "enabled",         param::ANIM_ENABLED),
    ("animations",        "speed",           param::ANIM_SPEED),
    ("animations",        "curve",           param::ANIM_CURVE),
    // Input numbers that reach real settings: typematic, pointer scale, wheel
    // direction, keyboard follows cursor. `kb_layout` and `kb_options` are
    // handled above the lookup
    ("input",             "repeat_rate",     param::REPEAT_RATE),
    ("input",             "repeat_delay",    param::REPEAT_DELAY),
    ("input",             "follow_mouse",    param::FOLLOW_MOUSE),
    ("input",             "sensitivity",     param::SENSITIVITY),
    ("input",             "natural_scroll",  param::NATURAL_SCROLL),
];

/// `kb_layout = us,ru` as `param::KB_LAYOUTS`: each name's `keymap::LAYOUTS`
/// index plus one, four bits each, first lowest. Returns the unknown name on
/// failure
fn layouts_param(value: &str) -> Result<u32, &str> {
    use crate::keymap::{self, MAX_GROUPS};
    let mut packed = 0u32;
    let mut n = 0;
    for name in value.split(',').map(str::trim) {
        if n == MAX_GROUPS {
            return Err(name);
        }
        let Some(i) = keymap::layout(name) else { return Err(name) };
        packed |= (i as u32 + 1) << (4 * n);
        n += 1;
    }
    Ok(packed)
}

/// `kb_options` as `param::KB_SWITCH`: 1 for `grp:alt_shift_toggle`, 0 for
/// none. Others are reported
fn switch_param(value: &str) -> Result<u32, &str> {
    let mut v = 0;
    for opt in value.split(',').map(str::trim).filter(|o| !o.is_empty()) {
        if opt != "grp:alt_shift_toggle" {
            return Err(opt);
        }
        v = 1;
    }
    Ok(v)
}

pub fn load() -> Config {
    let mut files = manifest::DiskFiles;
    from_files(&mut files)
}

/// Reads a config through `Files`, so the kernel's selftest can use a table of
/// strings
pub fn from_files<F: manifest::Files>(files: &mut F) -> Config {
    let mut cfg = Config::new();
    cfg.manifest = Manifest::build(files, &manifest::LAYERS);
    if cfg.manifest.is_empty() {
        // Nothing to read; uses the compiled-in desktop
        return cfg;
    }
    cfg.from_file = true;
    if let Some((note, text)) = cfg.manifest.complaint() {
        cfg.complain(Where::NOWHERE, note.as_str(), text.as_str());
    }

    // First pass: `$name = value` from every file, so a later file can redefine
    // a variable
    let mut vars = conf::Vars::new();
    for i in 0..cfg.manifest.len() {
        let text = cfg.manifest.text(i as u8);
        conf::collect_vars(text, &mut vars, i as u8, &mut cfg);
    }

    // Second pass: layer files in order. `source =` files are walked by the
    // grammar at their line, so the later line still wins
    let mut roots = [0u8; manifest::MAX_FILES];
    let n_roots = cfg.manifest.roots().len();
    roots[..n_roots].copy_from_slice(cfg.manifest.roots());
    for k in 0..n_roots {
        let file = roots[k];
        let text = cfg.manifest.text(file);
        conf::parse_into(text, &mut cfg, &vars, file);
    }

    cfg.finish();
    cfg
}

/// Parser over a single string, for tests and the built-in default
pub fn parse(text: &str, cfg: &mut Config) {
    conf::parse(text, cfg);
    cfg.finish();
}

/// The window manager's dictionary: interprets each line and reports unknown
/// ones
impl Sink for Config {
    fn is_list(&self, key: &str) -> bool {
        // Every spelling of `bind`
        conf::bind_key(key).is_some()
            || matches!(key,
                "unbind"
                | "windowrule" | "windowrulev2" | "bezier" | "animation"
                | "exec" | "exec-once" | "exec-shutdown"
                | "env" | "monitor" | "layerrule" | "workspace"
                | "permission" | "plugin")
    }

    fn list(&mut self, at: Where, key: &str, value: &str) {
        // An empty value clears the list, so a drop-in can start over
        if value.is_empty() {
            let cleared = if conf::bind_key(key).is_some() {
                self.n_binds = 0;
                self.n_submaps = 0;
                self.submap_now = 0;
                true
            } else {
                match key {
                    "windowrule" | "windowrulev2" => { self.n_rules = 0; true }
                    "layerrule" => { self.n_layer_rules = 0; true }
                    "animation" => { self.n_anims = 0; true }
                    "bezier" => { self.n_curves = 0; true }
                    "exec" | "exec-once" => { self.n_execs = 0; true }
                    "monitor" => { self.n_monitors = 0; true }
                    _ => false,
                }
            };
            if cleared {
                return;
            }
        }
        if let Some(bk) = conf::bind_key(key) {
            for &c in bk.unhandled() {
                let letter = [c];
                self.complain(at, "this letter after `bind` does nothing",
                              core::str::from_utf8(&letter).unwrap_or("?"));
            }
            parse_bind(at, bk.flags, value, self);
            return;
        }
        match key {
            "bezier" => parse_bezier(value, self),
            "animation" => parse_animation(value, self),
            "windowrule" | "windowrulev2" => parse_rule(at, value, self),
            "layerrule" => parse_layer_rule(at, value, self),
            "unbind" => parse_unbind(at, value, self),
            // `exec` runs once, with no reload
            "exec" | "exec-once" => self.push_exec(at, value),
            "monitor" => self.push_monitor(at, value),
            _ => {
                if let Some(i) = absent_index(key) {
                    self.once(at, i);
                }
            }
        }
    }

    fn key(&mut self, at: Where, section: &str, key: &str, value: &str) {
        // Key map of following lines: `submap = resize` opens, `submap = reset`
        // closes
        if section.is_empty() && key == "submap" {
            self.submap_now = match value.trim() {
                "reset" | "" => 0,
                name => self.submap_index(at, name),
            };
            return;
        }
        // The one `theme` key read here, before the block is skipped
        if section == "theme" && key == "wallpaper" {
            let path = value.trim();
            if !Text::fits(path) {
                // Refuses an over-long path
                self.complain(at, "that wallpaper path is too long to carry", path);
                return;
            }
            self.claim(at, CLAIM_WALLPAPER);
            self.wallpaper = Text::from(path);
            return;
        }
        // A name, not a number
        if section == "general" && key == "layout" {
            self.claim(at, CLAIM_LAYOUT);
            self.layout = match value {
                "dwindle" => crate::protocol::wm::LAYOUT_DWINDLE,
                "float" | "floating" => crate::protocol::wm::LAYOUT_FLOAT,
                _ => crate::protocol::wm::LAYOUT_MASTER,
            };
            return;
        }
        // Border values, whose colors cannot use the param channel
        if section == "general" {
            match key {
                "border_size" => {
                    if let Some(v) = conf::number(value) {
                        self.claim(at, CLAIM_BORDER_SIZE);
                        self.border_size = v as i32;
                    }
                    return;
                }
                "col.active_border" => {
                    if let Some(c) = conf::colour(value) {
                        self.claim(at, CLAIM_BORDER_ACTIVE);
                        self.border_active = c;
                    }
                    return;
                }
                "col.inactive_border" => {
                    if let Some(c) = conf::colour(value) {
                        self.claim(at, CLAIM_BORDER_INACTIVE);
                        self.border_inactive = c;
                    }
                    return;
                }
                _ => {}
            }
        }
        // `misc { vrr }`: 2 and 3 are Hyprland's fullscreen-only modes,
        // unimplemented
        if section == "misc" && key == "vrr" {
            match conf::number(value) {
                Some(v) if v == 0.0 || v == 1.0 => {
                    self.claim(at, CLAIM_MISC_VRR);
                    self.misc_vrr = Some(v as u8);
                }
                Some(v) if v == 2.0 || v == 3.0 => self.complain(at,
                    "vrr 2 and 3 are fullscreen-only, which means nothing here yet - use 0 or 1", value),
                _ => self.complain(at, "vrr takes 0 or 1", value),
            }
            return;
        }
        // Keyboard layouts: names here, numbers to the compositor
        if section == "input" {
            match key {
                // Empty is the default
                "kb_layout" if value.trim().is_empty() => return,
                "kb_layout" => {
                    match layouts_param(value) {
                        Ok(p) => self.push_param(at, param::KB_LAYOUTS, p as f32),
                        Err(bad) => self.complain(at,
                            "no such keyboard layout here - there are us and ru, up to four of them", bad),
                    }
                    return;
                }
                "kb_options" => {
                    match switch_param(value) {
                        Ok(v) => self.push_param(at, param::KB_SWITCH, v as f32),
                        Err(bad) => self.complain(at,
                            "the only keyboard option here is grp:alt_shift_toggle", bad),
                    }
                    return;
                }
                // Usually empty; empty values are ignored silently
                "kb_variant" | "kb_model" | "kb_rules" | "kb_file" => {
                    if !value.trim().is_empty() {
                        self.complain(at,
                            "there is no xkb here, so a layout has no variants, models or rules", key);
                    }
                    return;
                }
                _ => {}
            }
        }
        // Two keys, one number: the strength while dimming, zero otherwise, in
        // either order
        if section == "decoration" && (key == "dim_inactive" || key == "dim_strength") {
            match conf::number(value) {
                Some(v) => {
                    if key == "dim_inactive" {
                        self.dim_on = v != 0.0;
                    } else {
                        self.dim_strength = v;
                    }
                    let d = if self.dim_on { self.dim_strength } else { 0.0 };
                    self.put_param(at, param::DIM, d);
                }
                None => self.complain(at, "not a number", value),
            }
            return;
        }
        if let Some(&(_, _, p)) = KNOBS.iter()
            .find(|(sec, k, _)| *sec == section && *k == key)
        {
            match conf::number(value) {
                Some(v) => self.push_param(at, p, v),
                None => self.complain(at, "not a number", value),
            }
            return;
        }
        // No match: a missing section reports once, otherwise one line per
        // setting
        let top = match conf::split_once(section, '.') {
            Some((first, _)) => first,
            None => section,
        };
        if SOMEBODY_ELSE_S.contains(&top) {
            return;
        }
        if let Some(i) = absent_index(top) {
            self.once(at, i);
            return;
        }
        self.complain(at, "this desktop has no such setting", key);
    }

    fn source(&mut self, _at: Where, path: &str) -> Option<(&'static str, u8)> {
        // `Manifest` already read every `source =`; a miss means it could not
        // be read
        self.manifest.find(path)
    }

    fn note(&mut self, at: Where, note: Note, text: &str) {
        self.complain(at, note.as_str(), text);
    }
}

/// `easeOut, 0.22, 1, 0.36, 1`: two control points; the others are (0,0) and
/// (1,1)
fn parse_bezier(value: &str, cfg: &mut Config) {
    if cfg.n_curves >= MAX_CURVES {
        return;
    }
    let mut parts = value.split(',');
    let Some(name) = parts.next() else { return };
    let mut n = [0.0f32; 4];
    for i in 0..4 {
        let Some(p) = parts.next() else { return };
        let Some(v) = conf::number(p.trim()) else { return };
        n[i] = v;
    }
    cfg.curves[cfg.n_curves] = CurveDef {
        name: Text::from(name.trim()), x1: n[0], y1: n[1], x2: n[2], y2: n[3],
    };
    cfg.n_curves += 1;
}

/// `windows, 1, 4, easeOut, popin 80%`: class, enabled, speed, curve, optional
/// style with number
fn parse_animation(value: &str, cfg: &mut Config) {
    use crate::protocol::wm::{anim_class as ac, anim_style as st};
    if cfg.n_anims >= MAX_ANIMS {
        return;
    }
    let mut parts = value.split(',');
    let Some(class_name) = parts.next() else { return };
    let class = match class_name.trim() {
        "windows" => ac::WINDOWS,
        "windowsIn" => ac::WINDOWS_IN,
        "windowsOut" => ac::WINDOWS_OUT,
        "layers" => ac::LAYERS,
        "layersIn" => ac::LAYERS_IN,
        "layersOut" => ac::LAYERS_OUT,
        "fade" => ac::FADE,
        "fadeIn" => ac::FADE_IN,
        "fadeOut" => ac::FADE_OUT,
        "fadeLayers" => ac::FADE_LAYERS,
        "fadeLayersIn" => ac::FADE_LAYERS_IN,
        "fadeLayersOut" => ac::FADE_LAYERS_OUT,
        "workspaces" => ac::WORKSPACES,
        "workspacesIn" => ac::WORKSPACES_IN,
        "workspacesOut" => ac::WORKSPACES_OUT,
        // Unknown classes are ignored
        _ => return,
    };
    let enabled = parts.next().and_then(|v| conf::number(v.trim())).unwrap_or(0.0) != 0.0;
    let speed = parts.next().and_then(|v| conf::number(v.trim())).unwrap_or(0.0);
    let curve_name = parts.next().unwrap_or("").trim();
    let curve = cfg.curves[..cfg.n_curves].iter()
        .position(|c| c.name.as_str() == curve_name)
        .unwrap_or(0) as u32;

    // `popin 80%`: the number belongs to the style
    let tail = parts.next().unwrap_or("").trim();
    let (word, rest) = match tail.find(' ') {
        Some(i) => (&tail[..i], tail[i + 1..].trim()),
        None => (tail, ""),
    };
    let pct = rest.trim_end_matches('%');
    let arg = conf::number(pct).map(|v| v / 100.0).unwrap_or(1.0);
    let style = match word {
        "popin" => st::POPIN,
        "fade" => st::FADE,
        s if s.starts_with("slide") => st::SLIDE,
        // No style: a fade
        "" => st::FADE,
        _ => st::NONE,
    };

    cfg.anims[cfg.n_anims] = AnimDef { class, enabled, speed, curve, style, arg };
    cfg.n_anims += 1;
}

/// `SUPER SHIFT, Q, dispatcher, argument`. The argument is everything after the
/// third comma
fn parse_bind(at: Where, flags: u32, value: &str, cfg: &mut Config) {
    let mut parts = value.splitn(4, ',');
    let (Some(mods), Some(key), Some(name)) = (parts.next(), parts.next(), parts.next())
        else {
            cfg.complain(at, "a binding is modifiers, key, dispatcher", value);
            return;
        };
    let arg = parts.next().unwrap_or("").trim();

    let Some(key) = crate::binds::keycode(key.trim()) else {
        cfg.complain(at, "no key of that name", key.trim());
        return;
    };
    let Some(dispatch) = crate::binds::dispatcher(name.trim()) else {
        cfg.complain(at, "no dispatcher of that name", name.trim());
        return;
    };
    // `movewindow` and `resizewindow` differ by key type, so the choice is made
    // here
    let dispatch = crate::binds::for_key(dispatch, key);
    let submap = cfg.submap_now;
    cfg.push_bind(at, Bind {
        mods: crate::binds::modmask(mods),
        key,
        flags,
        submap,
        action: Action {
            dispatch,
            arg: crate::binds::argument(dispatch, arg),
            text: Text::from(arg),
        },
    });
}

/// `unbind = SUPER, D`: removes earlier bindings of that chord in the current
/// map, since a key goes to the first binding naming it
fn parse_unbind(at: Where, value: &str, cfg: &mut Config) {
    let Some((mods, key)) = conf::split_once(value, ',') else {
        cfg.complain(at, "an unbinding is modifiers and a key", value);
        return;
    };
    let Some(key) = crate::binds::keycode(key.trim()) else {
        cfg.complain(at, "no key of that name", key.trim());
        return;
    };
    let mods = crate::binds::modmask(mods);
    let submap = cfg.submap_now;
    let mut kept = 0;
    for i in 0..cfg.n_binds {
        let b = cfg.binds[i];
        if b.mods == mods && b.key == key && b.submap == submap {
            continue;
        }
        cfg.binds[kept] = b;
        kept += 1;
    }
    cfg.n_binds = kept;
}

/// `opacity 0.8`, `opacity 0.82 0.75`, `opacity 1.0 override 0.9` (Hyprland's
/// `parseOpacityRule`)
fn opacity(arg: &str) -> Option<Effect> {
    let mut vals = [1.0f32; 2];
    let mut own = [false; 2];
    let mut n = 0;
    for w in arg.split_whitespace() {
        if w == "override" {
            if n == 0 {
                return None;
            }
            own[n - 1] = true;
            continue;
        }
        if n == 2 {
            return None;
        }
        vals[n] = conf::number(w)?;
        n += 1;
    }
    match n {
        0 => None,
        1 => Some(Effect::Opacity { active: vals[0], inactive: vals[0], own_active: own[0], own_inactive: own[0] }),
        _ => Some(Effect::Opacity { active: vals[0], inactive: vals[1], own_active: own[0], own_inactive: own[1] }),
    }
}

/// `float, class:calculator` and `opacity 0.8, title:log`.
/// `layerrule = noanim, quickshell:overview`: `.*` is every layer, otherwise a
/// substring
fn parse_layer_rule(at: Where, value: &str, cfg: &mut Config) {
    let Some((rule, ns)) = value.split_once(',') else {
        cfg.complain(at, "a layer rule is a rule and a namespace", value);
        return;
    };
    let (rule, ns) = (rule.trim(), ns.trim());
    let rule = match rule {
        "noanim" | "no_anim" => crate::protocol::wm::layer_rule::NOANIM,
        _ => {
            cfg.complain(at, "the one layer rule there is is noanim", rule);
            return;
        }
    };
    // v2 spelling names the field
    let ns = ns.strip_prefix("namespace:").unwrap_or(ns).trim();
    if ns.is_empty() || !Text::fits(ns) {
        cfg.complain(at, "a layer rule needs a namespace of at most 64 bytes", ns);
        return;
    }
    if cfg.n_layer_rules == MAX_LAYER_RULES {
        cfg.complain(at, "too many layer rules; this one is dropped", ns);
        return;
    }
    cfg.layer_rules[cfg.n_layer_rules] = (rule, Text::from(ns));
    cfg.n_layer_rules += 1;
}

fn parse_rule(at: Where, value: &str, cfg: &mut Config) {
    let Some((what, on)) = conf::split_once(value, ',') else {
        cfg.complain(at, "a window rule is an effect and something to match on", value);
        return;
    };
    let (what, on) = (what.trim(), on.trim());

    let (field, pat) = if let Some(p) = on.strip_prefix("class:") {
        (Field::AppId, p.trim())
    } else if let Some(p) = on.strip_prefix("title:") {
        (Field::Title, p.trim())
    } else if let Some(p) = on.strip_prefix("floating:") {
        match p.trim() {
            "1" | "true" => (Field::Floating(true), ""),
            "0" | "false" => (Field::Floating(false), ""),
            other => {
                cfg.complain(at, "floating: takes 0 or 1", other);
                return;
            }
        }
    } else {
        // Bare, as in v1: the class
        (Field::AppId, on)
    };

    // Effect: a boolean or a name with numbers
    let (name, arg) = match conf::split_once(what, ' ') {
        Some((a, b)) => (a.trim(), b.trim()),
        None => (what, ""),
    };
    // Two numbers, for the effects that are about a rectangle
    let two = || -> Option<(i32, i32)> {
        let mut it = arg.split_whitespace();
        let a = conf::number(it.next()?)? as i32;
        let b = conf::number(it.next()?)? as i32;
        Some((a, b))
    };

    let effect = match name {
        "float" | "floating" =>
            Effect::State { mask: state::FLOATING, flags: state::FLOATING },
        "tile" | "tiled" =>
            Effect::State { mask: state::FLOATING, flags: 0 },
        "fullscreen" =>
            Effect::State { mask: state::FULLSCREEN, flags: state::FULLSCREEN },
        "pin" | "pinned" =>
            Effect::State { mask: state::PINNED, flags: state::PINNED },
        // Layer surface focus mechanism
        "nofocus" | "noinitialfocus" =>
            Effect::State { mask: state::NO_FOCUS, flags: state::NO_FOCUS },
        "opacity" => match opacity(arg) {
            Some(e) => e,
            None => {
                cfg.complain(at, "opacity takes one or two numbers, each maybe followed by override", arg);
                return;
            }
        },
        "rounding" => Effect::Param { param: param::ROUNDING, value: conf::number(arg).unwrap_or(0.0) },
        "roundingpower" | "rounding_power" =>
            Effect::Param { param: param::ROUNDING_POWER, value: conf::number(arg).unwrap_or(2.0) },
        // `no_blur` stops only the blur, keeping corner, border, and opacity
        "noblur" | "no_blur" => Effect::Param { param: param::BLUR_SIZE, value: 0.0 },
        "blur" => Effect::Param { param: param::EFFECTS, value: 1.0 },
        "noshadow" | "no_shadow" => Effect::Param { param: param::SHADOW, value: 0.0 },
        "nodim" | "no_dim" => Effect::Param { param: param::DIM, value: 0.0 },
        "workspace" => Effect::Workspace(conf::number(arg).unwrap_or(0.0) as u32),
        // `minsize` and `maxsize` are constraints the compositor cannot hold,
        // so they are reported
        "size" => match two() {
            Some((w, h)) => Effect::Size(w, h),
            None => {
                cfg.complain(at, "size takes a width and a height", arg);
                return;
            }
        },
        "move" => match two() {
            Some((x, y)) => Effect::Move(x, y),
            None => {
                cfg.complain(at, "move takes an x and a y", arg);
                return;
            }
        },
        "center" | "centre" => Effect::Center,
        _ => {
            cfg.complain(at, "no window rule of that name", name);
            return;
        }
    };
    cfg.push_rule(at, Rule { field, pat: Text::from(pat), effect });
}
