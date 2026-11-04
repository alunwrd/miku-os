// Built-in QML types
//
// Each is a table: base, properties with types and defaults, signals with
// parameter names, methods. Objects are built by walking bases. Behavior (a
// Timer firing, a Connections following its target) is selected by `Kind`;
// everything else is data.

mod anim;
mod controls;
mod core;
mod effects;
mod hyprland;
mod io;
mod item;
mod model;
mod services;
mod shapes;
mod shell;
mod wayland;

use crate::object::value::{PropType, Value};

/// What a type does beyond holding properties
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Plain,
    Timer,
    Connections,
    /// Its own surface: `PanelWindow`
    Window,
    /// `SystemClock`, updated by the shell
    Clock,
    /// `Desktop` service: methods are compositor requests
    Desktop,
    /// A definition to instantiate
    Component,
    /// Makes a delegate per model row, beside itself in its parent
    Repeater,
    /// Makes a delegate per model row, inside itself, one after another
    ListView,
    /// One delegate per model row, not shown: `Variants`, `Instantiator`
    Instantiator,
    /// One component while `active`: `Loader`, and `LazyLoader` for windows
    Loader,
    /// Rows that are objects (`model/`)
    ListModel,
    /// quickshell's JavaScript list model, keeping rows across changes
    /// (`model/script.rs`)
    ScriptModel,
    /// One row: a `ListElement` or `append` result; takes any property
    Row,
    /// Every animation, the groups and `PauseAnimation` included (`anim/`)
    Animation,
    Behavior,
    Process,
    /// `StdioCollector`: whole process output
    Collector,
    /// `SplitParser`: process output by marker
    Splitter,
    FileView,
    /// `JsonAdapter` and nested `JsonObject`s: a FileView's file as properties
    /// (`io/json.rs`)
    Json,
    /// `TextInput`, `TextField`: edits text on key input
    Input,
    /// `IpcHandler`: functions called by `aosh ipc` and signal words
    Ipc,
    /// `DesktopEntries`: programs from apps.d
    Apps,
    /// Stand-in for a missing type; takes any property
    Inert,
    /// QtQml `Binding`: holds another object's property (`object/hold.rs`)
    Binding,
    /// Qt.labs.synchronizer `Synchronizer`: keeps properties equal both ways
    /// (`object/sync.rs`)
    Synchronizer,
    /// `FontLoader`: a font file with its family as `name` (`io/`)
    FontLoader,
    /// `Canvas`: a picture painted by `onPaint` through a "2d" context
    /// (`scene/canvas/`)
    Canvas,
    /// `ScreencopyView`: window pixels from the compositor
    /// (`services/capture.rs`)
    Screencopy,
}

/// Stand-in type name; not writable in a file
pub const INERT: &str = "__Inert";

pub struct TypeDef {
    pub name: &'static str,
    pub base: Option<&'static str>,
    pub kind: Kind,
    pub props: &'static [(&'static str, PropType, Default)],
    pub signals: &'static [(&'static str, &'static [&'static str])],
    pub methods: &'static [&'static str],
}

/// A property's starting value, as a constant a table can hold
#[derive(Clone, Copy)]
pub enum Default {
    None,
    Bool(bool),
    Num(f64),
    Str(&'static str),
}

impl Default {
    pub fn value(self, ty: PropType) -> Value {
        match self {
            Default::None => ty.default_value(),
            Default::Bool(b) => Value::Bool(b),
            Default::Num(n) => Value::Num(n),
            Default::Str(s) => ty.coerce(Value::str(s)),
        }
    }
}

const ALL: &[&[TypeDef]] = &[core::TYPES, item::TYPES, item::INPUT_TYPES, controls::TYPES, effects::TYPES, model::TYPES,
    anim::TYPES, io::TYPES, shell::TYPES, services::TYPES, hyprland::TYPES, wayland::TYPES, shapes::TYPES];

/// Whether `t` is, or derives from, the type named `base`
pub fn is_a(t: &'static TypeDef, base: &str) -> bool {
    chain(t).iter().any(|x| x.name == base)
}

pub fn find(name: &str) -> Option<&'static TypeDef> {
    // `QtQuick.Item` names the same type as `Item`
    let short = name.rsplit('.').next().unwrap_or(name);
    ALL.iter().flat_map(|g| g.iter()).find(|t| t.name == short)
}

/// The type and its bases, most derived first. Computed once per type, since
/// `is_a` runs several times per item per frame
pub fn chain(t: &'static TypeDef) -> &'static [&'static TypeDef] {
    thread_local! {
        static CHAINS: std::cell::RefCell<std::collections::HashMap<usize, &'static [&'static TypeDef]>> =
            std::cell::RefCell::new(std::collections::HashMap::new());
    }
    let key = t as *const TypeDef as usize;
    if let Some(c) = CHAINS.with(|c| c.borrow().get(&key).copied()) {
        return c;
    }
    let mut out = vec![t];
    let mut cur = t;
    while let Some(b) = cur.base.and_then(find) {
        out.push(b);
        cur = b;
    }
    let c: &'static [&'static TypeDef] = Box::leak(out.into_boxed_slice());
    CHAINS.with(|m| m.borrow_mut().insert(key, c));
    c
}
