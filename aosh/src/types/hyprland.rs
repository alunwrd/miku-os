// quickshell's `Quickshell.Hyprland`, backed by `may_desktop`
// (`services/hyprland.rs`), shaped as in `src/wayland/hyprland/ipc`: one
// monitor, nine workspaces, and a toplevel per window. Values Mayland does not
// report take Hyprland's default

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    // `Hyprland` singleton. `monitors`, `workspaces`, and `toplevels` are
    // object models
    TypeDef {
        name: "AoshHyprland",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("usingLua", T::Bool, D::Bool(false)),
            ("requestSocketPath", T::Str, D::None),
            ("eventSocketPath", T::Str, D::None),
            ("focusedMonitor", T::Obj, D::None),
            ("focusedWorkspace", T::Obj, D::None),
            ("activeToplevel", T::Obj, D::None),
            ("monitors", T::Var, D::None),
            ("workspaces", T::Var, D::None),
            ("toplevels", T::Var, D::None),
        ],
        signals: &[("rawEvent", &["event"])],
        methods: &["dispatch", "monitorFor", "refreshMonitors", "refreshWorkspaces", "refreshToplevels"],
    },
    TypeDef {
        name: "HyprlandMonitor",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("id", T::Int, D::None),
            ("name", T::Str, D::None),
            ("description", T::Str, D::None),
            ("x", T::Int, D::None),
            ("y", T::Int, D::None),
            ("width", T::Int, D::None),
            ("height", T::Int, D::None),
            ("scale", T::Real, D::Num(1.0)),
            ("lastIpcObject", T::Var, D::None),
            ("activeWorkspace", T::Obj, D::None),
            ("focused", T::Bool, D::Bool(true)),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "HyprlandWorkspace",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("id", T::Int, D::None),
            ("name", T::Str, D::None),
            ("active", T::Bool, D::Bool(false)),
            ("focused", T::Bool, D::Bool(false)),
            ("urgent", T::Bool, D::Bool(false)),
            ("hasFullscreen", T::Bool, D::Bool(false)),
            ("lastIpcObject", T::Var, D::None),
            ("monitor", T::Obj, D::None),
            ("toplevels", T::Var, D::None),
        ],
        signals: &[],
        methods: &["activate"],
    },
    TypeDef {
        name: "HyprlandToplevel",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("address", T::Str, D::None),
            ("handle", T::Obj, D::None),
            // quickshell's Wayland toplevel (`appId`, `title`, `activated`,
            // `fullscreen`, `maximized`)
            ("wayland", T::Var, D::None),
            ("title", T::Str, D::None),
            ("activated", T::Bool, D::Bool(false)),
            ("urgent", T::Bool, D::Bool(false)),
            ("lastIpcObject", T::Var, D::None),
            ("workspace", T::Obj, D::None),
            ("monitor", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `HyprlandFocusGrab`, over `may_focus_grab`
    // (`mayland/mod.rs`, `sync_grabs`): while `active`, the pointer reaches
    // only `windows`, and a press outside emits `cleared` and clears `active`
    TypeDef {
        name: "HyprlandFocusGrab",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("active", T::Bool, D::Bool(false)), ("windows", T::List, D::None)],
        signals: &[("cleared", &[])],
        methods: &[],
    },
];
