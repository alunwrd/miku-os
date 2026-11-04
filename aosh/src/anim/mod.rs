// Animation Engine Architecture
//
// Animations run as `Run` instances, tracking start times and leaf states
// (NumberAnimation, ColorAnimation). Targets and from/to ranges resolve
// when each leaf starts, matching Qt behavior.
//
// Every frame, `tick` samples active runs:
// 1) Groups sample children sequentially or in parallel
// 2) Leaves write eased values directly via `Core::store_raw` (bypassing
//    Behaviors)
// 3) Completed runs apply target values and terminate
//
// Behaviors trigger on property writes (`behave`), animating from the current
// value to the target and redirecting mid-flight if updated.
// Value sources (`NumberAnimation on x`) start automatically once the tree
// builds.
//
// Instant actions (zero duration):
// 1) PropertyAction: immediately sets `value`
// 2) ScriptAction: runs `script` once per pass or loop iteration

pub mod easing;

use std::collections::{HashMap, HashSet};

use rquickjs::Ctx;

use crate::object::value::{normalise_colour, Oid, Value};
use crate::object::{Core, State};
use crate::types::{self, Kind};

#[derive(Default)]
pub struct Animator {
    runs: Vec<Run>,
    /// Target object and property binding (e.g., `Behavior on x`,
    /// `NumberAnimation on x`)
    on: HashMap<Oid, (Oid, String)>,
}

struct Run {
    /// Stepped execution target: top-level animation vs. Behavior animation
    anim: Oid,
    start: u64,
    paused: Option<u64>,
    /// Active Behavior run: references the Behavior instance and target
    /// property animated towards `to`
    behavior: Option<(Oid, Oid, u32, Value)>,
    leaves: HashMap<Oid, Leaf>,
    /// Tracks the loop iteration at the last tick. When a step crosses a loop
    /// boundary,
    /// it commits the current loop's end values before starting the next
    /// iteration (matching Qt)
    loops_seen: HashMap<Oid, u64>,
    /// Tracks ScriptActions already executed during the current loop iteration
    fired: HashSet<Oid>,
    /// Started outside the tick loop without being stepped yet.
    /// `start` defaults to the last tick's timestamp, which may be stale
    /// (`begin_fresh`)
    fresh: bool,
}

struct Leaf {
    targets: Vec<(Oid, u32)>,
    from: Vec<Value>,
    to: Value,
    /// `easing.bezierCurve`, for Easing.BezierSpline
    curve: Vec<f64>,
    /// A RotationAnimation's `direction`: which way round an angle goes
    turn: i64,
}

#[derive(PartialEq)]
enum Family {
    Leaf,
    Pause,
    Sequential,
    Parallel,
    Script,
}

fn family(core: &Core, a: Oid) -> Option<Family> {
    let def = core.0.borrow().objs.get(a)?.def;
    if def.kind != Kind::Animation {
        return None;
    }
    Some(if types::is_a(def, "SequentialAnimation") {
        Family::Sequential
    } else if types::is_a(def, "ParallelAnimation") {
        Family::Parallel
    } else if types::is_a(def, "PauseAnimation") {
        Family::Pause
    } else if types::is_a(def, "ScriptAction") {
        Family::Script
    } else {
        Family::Leaf
    })
}

/// `Behavior on prop`, `X on prop`: remembered until the tree is complete
pub fn on(core: &Core, anim: Oid, target: Oid, prop: &str) {
    core.0
        .borrow_mut()
        .anims
        .on
        .insert(anim, (target, prop.to_string()));
}

impl Animator {
    /// Identifies `X on prop`: target object and its property.
    /// A binding can also be attached to a property via `on` (see
    /// `object/hold.rs`)
    pub fn on_target(&self, oid: Oid) -> Option<(Oid, String)> {
        self.on.get(&oid).cloned()
    }
}

pub fn completed(ctx: &Ctx<'_>, core: &Core, oid: Oid) {
    let kind = core.0.borrow().objs[oid].def.kind;
    let on = core.0.borrow().anims.on.get(&oid).cloned();
    match kind {
        // Behavior starts observing from here to ignore initial values;
        // prevents UI elements from sliding in from zero on open
        Kind::Behavior => {
            let Some((target, prop)) = on else { return };
            match core.prop_idx(target, &prop) {
                Some(i) => {
                    let mut s = core.0.borrow_mut();
                    s.objs[target].props[i as usize].behavior = Some(oid);
                }
                None => {
                    let ty = core.0.borrow().objs[target].ty.clone();
                    core.log(format!("Behavior on {prop}: {ty} has no property '{prop}'"));
                }
            }
        }
        Kind::Animation if !in_group(core, oid) => {
            // Value sources run by default; other animations are opt-in
            let go = match on {
                Some(_) => !(core.is_explicit(oid, "running") && !core.flag(oid, "running")),
                None => core.flag(oid, "running"),
            };
            if go {
                start(ctx, core, oid);
            }
        }
        _ => {}
    }
}

/// Driven by the enclosing group or Behavior rather than running independently
fn in_group(core: &Core, a: Oid) -> bool {
    let s = core.0.borrow();
    let Some(p) = s.objs.get(a).and_then(|o| o.parent) else {
        return false;
    };
    s.objs
        .get(p)
        .is_some_and(|o| matches!(o.def.kind, Kind::Animation | Kind::Behavior))
}

pub fn hook(ctx: &Ctx<'_>, core: &Core, oid: Oid, kind: Kind, name: &str) {
    if kind != Kind::Animation || !core.0.borrow().objs[oid].complete || in_group(core, oid) {
        return;
    }
    match name {
        "running" => {
            if core.flag(oid, "running") {
                start(ctx, core, oid);
            } else {
                stop(ctx, core, oid);
            }
        }
        "paused" => {
            let now = core.0.borrow().now_ms;
            let paused = core.flag(oid, "paused");
            let mut s = core.0.borrow_mut();
            if let Some(r) = s.anims.runs.iter_mut().find(|r| r.anim == oid) {
                match (paused, r.paused) {
                    (true, None) => r.paused = Some(now),
                    (false, Some(at)) => {
                        r.start += now - at;
                        r.paused = None;
                    }
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

pub fn call<'js>(
    ctx: &Ctx<'js>,
    core: &Core,
    oid: Oid,
    kind: Kind,
    name: &str,
    _args: &[rquickjs::Value<'js>],
) -> Option<Value> {
    if kind != Kind::Animation {
        return None;
    }
    match name {
        "start" => core.set_named(ctx, oid, "running", Value::Bool(true)),
        "stop" => core.set_named(ctx, oid, "running", Value::Bool(false)),
        "restart" => {
            stop(ctx, core, oid);
            core.set_named(ctx, oid, "running", Value::Bool(false));
            core.set_named(ctx, oid, "running", Value::Bool(true));
        }
        "pause" => core.set_named(ctx, oid, "paused", Value::Bool(true)),
        "resume" => core.set_named(ctx, oid, "paused", Value::Bool(false)),
        "complete" => {
            let run = take_run(core, |r| r.anim == oid);
            if let Some(mut r) = run {
                step(ctx, core, &mut r, oid, f64::MAX);
                finished(ctx, core, r);
            }
        }
        _ => return None,
    }
    Some(Value::Undefined)
}

pub fn forget(core: &Core, oid: Oid) {
    let mut s = core.0.borrow_mut();
    s.anims.on.remove(&oid);
    s.anims.runs.retain(|r| {
        r.anim != oid
            && !r
                .behavior
                .as_ref()
                .is_some_and(|(b, t, _, _)| *b == oid || *t == oid)
    });
}

/// Determines if property assignments should target Behavior `b`
pub fn behavior_on(s: &State, b: Oid) -> bool {
    let Some(o) = s.objs.get(b) else { return false };
    o.complete
        && o.props
            .iter()
            .find(|p| &*p.name == "enabled")
            .is_some_and(|p| p.value.truthy())
}

/// Stops a disabled Behavior's running animation so it cannot overwrite a new
/// value
pub fn behavior_off(core: &Core, b: Oid) {
    let _ = take_run(core, |r| r.behavior.as_ref().is_some_and(|x| x.0 == b));
}

/// Animates the property toward the newly assigned value via its Behavior
pub fn behave(ctx: &Ctx<'_>, core: &Core, b: Oid, o: Oid, i: u32, v: Value) {
    let v = core.prop_type(o, i).coerce(v);
    // Resolves `animation`: explicit inline definition, default property,
    // or passed from outside via an alias
    let anim = core
        .peek_named(b, "animation")
        .and_then(|v| v.as_obj())
        .filter(|&a| {
            core.0
                .borrow()
                .objs
                .get(a)
                .is_some_and(|x| x.def.kind == Kind::Animation)
        })
        .or_else(|| {
            let s = core.0.borrow();
            s.objs[b]
                .children
                .iter()
                .copied()
                .find(|&c| s.objs.get(c).is_some_and(|x| x.def.kind == Kind::Animation))
        });
    let Some(anim) = anim else {
        core.store_raw(ctx, o, i, v);
        return;
    };
    {
        let s = core.0.borrow();
        let going = s
            .anims
            .runs
            .iter()
            .find(|r| r.behavior.as_ref().is_some_and(|x| x.0 == b));
        // Transition already in progress
        if going.is_some_and(|r| r.behavior.as_ref().is_some_and(|x| same(&x.3, &v))) {
            return;
        }
    }
    let _ = take_run(core, |r| r.behavior.as_ref().is_some_and(|x| x.0 == b));
    if same(&core.peek(o, i), &v) {
        return;
    }
    let now = core.0.borrow().now_ms;
    let mut s = core.0.borrow_mut();
    s.anims.runs.push(Run {
        anim,
        start: now,
        paused: None,
        behavior: Some((b, o, i, v)),
        leaves: HashMap::new(),
        loops_seen: HashMap::new(),
        fired: HashSet::new(),
        fresh: true,
    });
    s.dirty = true;
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => x == y,
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Obj(x), Value::Obj(y)) => x == y,
        (Value::Undefined, Value::Undefined) | (Value::Null, Value::Null) => true,
        _ => false,
    }
}

fn start(ctx: &Ctx<'_>, core: &Core, a: Oid) {
    let now = core.0.borrow().now_ms;
    let new = {
        let mut s = core.0.borrow_mut();
        if s.anims
            .runs
            .iter()
            .any(|r| r.anim == a && r.behavior.is_none())
        {
            false
        } else {
            s.anims.runs.push(Run {
                anim: a,
                start: now,
                paused: None,
                behavior: None,
                leaves: HashMap::new(),
                loops_seen: HashMap::new(),
                fired: HashSet::new(),
                fresh: true,
            });
            s.dirty = true;
            true
        }
    };
    if !core.flag(a, "running") {
        if let Some(i) = core.prop_idx(a, "running") {
            core.store_raw(ctx, a, i, Value::Bool(true));
        }
    }
    if new {
        core.emit(ctx, a, "started", Vec::new());
    }
}

fn stop(ctx: &Ctx<'_>, core: &Core, a: Oid) {
    if take_run(core, |r| r.anim == a && r.behavior.is_none()).is_some() {
        core.emit(ctx, a, "stopped", Vec::new());
    }
}

fn take_run(core: &Core, f: impl Fn(&Run) -> bool) -> Option<Run> {
    let mut s = core.0.borrow_mut();
    let i = s.anims.runs.iter().position(f)?;
    Some(s.anims.runs.remove(i))
}

/// Returns whether any animations are active; the shell requests frames while
/// true
pub fn animating(core: &Core) -> bool {
    core.0
        .borrow()
        .anims
        .runs
        .iter()
        .any(|r| r.paused.is_none())
}

/// Binds newly triggered animations (input, replies) to `now` instead of the
/// previous tick.
///
/// When idle, the loop may sleep up to a second; using the last tick's time
/// would cause
/// wake-up animations to skip straight to near-completion on their first frame
///
/// Called before `advance`
pub fn begin_fresh(core: &Core, now: u64) {
    for r in core.0.borrow_mut().anims.runs.iter_mut() {
        if r.fresh && r.paused.is_none() {
            r.start = now;
            r.fresh = false;
        }
    }
}

/// Executes on every tick, synchronized with the engine clock
pub fn tick(ctx: &Ctx<'_>, core: &Core) {
    let now = core.0.borrow().now_ms;
    let runs = std::mem::take(&mut core.0.borrow_mut().anims.runs);
    let mut keep = Vec::new();
    let mut done = Vec::new();
    for mut r in runs {
        if r.paused.is_some() {
            keep.push(r);
            continue;
        }
        r.fresh = false;
        let t = now.saturating_sub(r.start) as f64;
        let a = r.anim;
        if !core.0.borrow().objs.is_live(a) {
            continue;
        }
        if step(ctx, core, &mut r, a, t) {
            done.push(r);
        } else {
            keep.push(r);
        }
    }
    // Any tasks queued during execution (e.g., handlers, Behaviors)
    // are appended to the list and processed in subsequent iterations
    {
        let mut s = core.0.borrow_mut();
        let mut added = std::mem::take(&mut s.anims.runs);
        // Initialized with the current tick timestamp
        for r in added.iter_mut() {
            r.fresh = false;
        }
        keep.extend(added);
        s.anims.runs = keep;
    }
    for r in done {
        finished(ctx, core, r);
    }
}

// Completed run
fn finished(ctx: &Ctx<'_>, core: &Core, r: Run) {
    if r.behavior.is_some() {
        return;
    }
    let a = r.anim;
    if !core.0.borrow().objs.is_live(a) {
        return;
    }
    if let Some(i) = core.prop_idx(a, "running") {
        core.store_raw(ctx, a, i, Value::Bool(false));
    }
    core.emit(ctx, a, "stopped", Vec::new());
    core.emit(ctx, a, "finished", Vec::new());
}

/// Loop duration in ms, or infinite if it contains an endless loop
fn one_loop(core: &Core, a: Oid) -> f64 {
    match family(core, a) {
        Some(Family::Leaf) | Some(Family::Pause) => core.num(a, "duration").max(0.0),
        Some(Family::Sequential) => parts(core, a).iter().map(|&c| total(core, c)).sum(),
        Some(Family::Parallel) => parts(core, a)
            .iter()
            .map(|&c| total(core, c))
            .fold(0.0, f64::max),
        Some(Family::Script) | None => 0.0,
    }
}

fn total(core: &Core, a: Oid) -> f64 {
    let loops = core.num(a, "loops") as i64;
    let d = one_loop(core, a);
    if loops < 0 {
        f64::INFINITY
    } else {
        d * loops.max(1) as f64
    }
}

fn parts(core: &Core, a: Oid) -> Vec<Oid> {
    let s = core.0.borrow();
    s.objs[a]
        .children
        .iter()
        .copied()
        .filter(|&c| s.objs.get(c).is_some_and(|o| o.def.kind == Kind::Animation))
        .collect()
}

/// State `a` at elapsed time `t` (ms). True if the run has ended
fn step(ctx: &Ctx<'_>, core: &Core, r: &mut Run, a: Oid, t: f64) -> bool {
    let d = one_loop(core, a);
    let all = total(core, a);
    let over = t >= all;
    // Current loop position. When finished, holds at the end of the
    // final loop so all parts retain their final values
    let lt = if over {
        d
    } else if d > 0.0 && d.is_finite() {
        t % d
    } else {
        t
    };
    if d > 0.0 && d.is_finite() && !over {
        let lap = (t / d) as u64;
        let seen = r.loops_seen.insert(a, lap).unwrap_or(0);
        if lap > seen {
            sample(ctx, core, r, a, d);
            // A new loop: its ScriptActions run again
            for sc in scripts_in(core, a) {
                r.fired.remove(&sc);
            }
        }
    }
    sample(ctx, core, r, a, lt);
    over
}

/// Single iteration of `a` at offset `lt`
fn sample(ctx: &Ctx<'_>, core: &Core, r: &mut Run, a: Oid, lt: f64) {
    let d = one_loop(core, a);
    match family(core, a) {
        Some(Family::Leaf) => {
            let p = if d > 0.0 { (lt / d).min(1.0) } else { 1.0 };
            apply(ctx, core, r, a, p);
        }
        Some(Family::Sequential) => {
            let mut acc = 0.0;
            for c in parts(core, a) {
                let ct = total(core, c);
                if lt < acc + ct {
                    step(ctx, core, r, c, lt - acc);
                    break;
                }
                // Clamped to the end
                step(ctx, core, r, c, ct);
                acc += ct;
            }
        }
        Some(Family::Parallel) => {
            for c in parts(core, a) {
                step(ctx, core, r, c, lt);
            }
        }
        Some(Family::Script) => {
            if r.fired.insert(a) {
                core.emit(ctx, a, SCRIPT, Vec::new());
            }
        }
        Some(Family::Pause) | None => {}
    }
}

/// The signal handled by this `ScriptAction`'s `script` (`object/build.rs`)
pub const SCRIPT: &str = "__script";

/// All `ScriptAction`s recursively collected from `a`
fn scripts_in(core: &Core, a: Oid) -> Vec<Oid> {
    let mut out = Vec::new();
    for c in parts(core, a) {
        if family(core, c) == Some(Family::Script) {
            out.push(c);
        } else {
            out.extend(scripts_in(core, c));
        }
    }
    out
}

/// The leaf state at progress `p`
fn apply(ctx: &Ctx<'_>, core: &Core, r: &mut Run, leaf: Oid, p: f64) {
    if !r.leaves.contains_key(&leaf) {
        let l = resolve(ctx, core, r, leaf);
        r.leaves.insert(leaf, l);
    }
    let params = easing::Params {
        amplitude: core.num(leaf, "easing.amplitude"),
        period: core.num(leaf, "easing.period"),
        overshoot: core.num(leaf, "easing.overshoot"),
    };
    let Some(l) = r.leaves.get(&leaf) else { return };
    let kind = core.num(leaf, "easing.type") as i64;
    let e = if kind == easing::BEZIER_SPLINE {
        easing::bezier(&l.curve, p)
    } else {
        easing::ease(kind, p, &params)
    };
    let writes: Vec<(Oid, u32, Value)> = l
        .targets
        .iter()
        .zip(l.from.iter())
        .map(|(&(o, i), from)| (o, i, lerp(from, &turned(from, &l.to, l.turn), e)))
        .collect();
    for (o, i, v) in writes {
        if core.0.borrow().objs.is_live(o) {
            core.store_raw(ctx, o, i, v);
        }
    }
}

/// Target property, range, and start time for a leaf animation
fn resolve(ctx: &Ctx<'_>, core: &Core, r: &Run, leaf: Oid) -> Leaf {
    let mut targets: Vec<(Oid, u32)> = Vec::new();
    if let Some((_, o, i, _)) = &r.behavior {
        // A Behavior's animation always targets the Behavior's property;
        // any target set on the animation itself is ignored
        targets.push((*o, *i));
    } else {
        let mut objs: Vec<Oid> = Vec::new();
        if let Some(t) = core.peek_named(leaf, "target").and_then(|v| v.as_obj()) {
            objs.push(t);
        }
        let mut names: Vec<String> = core
            .text(leaf, "properties")
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        let prop = core.text(leaf, "property");
        if !prop.is_empty() {
            names.push(prop);
        }
        // Top-level value source: e.g. `NumberAnimation on x`, or an animation
        // group acting as one
        let source = core.0.borrow().anims.on.get(&r.anim).cloned();
        if let Some((t, p)) = source {
            if objs.is_empty() {
                objs.push(t);
            }
            if names.is_empty() {
                names.push(p);
            }
        }
        for o in objs
            .into_iter()
            .filter(|&o| core.0.borrow().objs.is_live(o))
        {
            for n in &names {
                if let Some(i) = core.prop_idx(o, n) {
                    targets.push((o, i));
                }
            }
        }
    }
    let explicit_from = core.is_explicit(leaf, "from");
    let from: Vec<Value> = targets
        .iter()
        .map(|&(o, i)| {
            if explicit_from {
                core.text_or_value(leaf, "from")
            } else {
                core.peek(o, i)
            }
        })
        .collect();
    let action = core
        .0
        .borrow()
        .objs
        .get(leaf)
        .is_some_and(|o| types::is_a(o.def, "PropertyAction"));
    let to = match &r.behavior {
        Some((_, _, _, v)) => v.clone(),
        None if action => core.text_or_value(leaf, "value"),
        None if core.is_explicit(leaf, "to") => core.text_or_value(leaf, "to"),
        None => targets
            .first()
            .map(|&(o, i)| core.peek(o, i))
            .unwrap_or(Value::Undefined),
    };
    let curve = core
        .peek_named(leaf, "easing.bezierCurve")
        .map(|v| numbers(ctx, &v))
        .unwrap_or_default();
    let turn = if core
        .0
        .borrow()
        .objs
        .get(leaf)
        .is_some_and(|o| types::is_a(o.def, "RotationAnimation"))
    {
        core.num(leaf, "direction") as i64
    } else {
        0
    };
    Leaf {
        targets,
        from,
        to,
        curve,
        turn,
    }
}

/// Numeric list property: either `property list<real>` or an array
fn numbers(ctx: &Ctx<'_>, v: &Value) -> Vec<f64> {
    match v {
        Value::List(items) => items.iter().map(|i| i.as_num()).collect(),
        Value::Js(p) => {
            let Ok(v) = p.as_ref().clone().restore(ctx) else {
                return Vec::new();
            };
            match v.as_array() {
                Some(a) => a
                    .iter::<rquickjs::Value>()
                    .flatten()
                    .filter_map(|x| x.as_number())
                    .collect(),
                None => Vec::new(),
            }
        }
        _ => Vec::new(),
    }
}

/// Computes the effective end angle for a `RotationAnimation` based on
/// direction:
/// `Clockwise` only adds, `Counterclockwise` only subtracts, and `Shortest`
/// spans at most half a turn
fn turned(from: &Value, to: &Value, turn: i64) -> Value {
    let (Value::Num(a), Value::Num(b)) = (from, to) else {
        return to.clone();
    };
    let mut d = b - a;
    match turn {
        1 => {
            while d < 0.0 {
                d += 360.0
            }
        }
        2 => {
            while d > 0.0 {
                d -= 360.0
            }
        }
        3 => {
            while d < -180.0 {
                d += 360.0
            }
            while d > 180.0 {
                d -= 360.0
            }
        }
        _ => return to.clone(),
    }
    Value::Num(a + d)
}

/// Interpolates from `a` to `b` at progress `e`: numbers via lerp, colors per
/// channel (with alpha), and all other types swap at completion
fn lerp(a: &Value, b: &Value, e: f64) -> Value {
    match (a, b) {
        (Value::Num(x), Value::Num(y)) => Value::Num(x + (y - x) * e),
        (Value::Str(x), Value::Str(y)) => match (rgba(x), rgba(y)) {
            (Some(p), Some(q)) => {
                let ch = |k: usize| -> u32 {
                    let v = p[k] + (q[k] - p[k]) * e;
                    v.round().clamp(0.0, 255.0) as u32
                };
                Value::str(&format!(
                    "#{:02x}{:02x}{:02x}{:02x}",
                    ch(0),
                    ch(1),
                    ch(2),
                    ch(3)
                ))
            }
            _ => {
                if e >= 1.0 {
                    b.clone()
                } else {
                    a.clone()
                }
            }
        },
        _ => {
            if e >= 1.0 {
                b.clone()
            } else {
                a.clone()
            }
        }
    }
}

/// Color hex (`#aarrggbb`) or name as RGBA components
fn rgba(s: &str) -> Option<[f64; 4]> {
    let n = normalise_colour(s);
    let h = n.strip_prefix('#').filter(|h| h.len() == 8)?;
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([
        (v >> 24) as f64,
        ((v >> 16) & 0xff) as f64,
        ((v >> 8) & 0xff) as f64,
        (v & 0xff) as f64,
    ])
}

impl Core {
    /// Raw property value for untyped `from` / `to` (`var`)
    fn text_or_value(&self, oid: Oid, name: &str) -> Value {
        self.peek_named(oid, name).unwrap_or(Value::Undefined)
    }
}

#[cfg(test)]
mod tests;
