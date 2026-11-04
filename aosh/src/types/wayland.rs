// quickshell's `Quickshell.Wayland` windows: `ToplevelManager` and `Toplevel`s,
// from `may_desktop` (`services/hyprland.rs`). A Toplevel per window, paired
// with a `HyprlandToplevel` whose `wayland` points back

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    // `ToplevelManager` singleton. `toplevels` is an object model
    TypeDef {
        name: "AoshToplevelManager",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("toplevels", T::Var, D::None), ("activeToplevel", T::Obj, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "Toplevel",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("appId", T::Str, D::None),
            ("title", T::Str, D::None),
            ("activated", T::Bool, D::Bool(false)),
            ("maximized", T::Bool, D::Bool(false)),
            ("minimized", T::Bool, D::Bool(false)),
            ("fullscreen", T::Bool, D::Bool(false)),
            ("parent", T::Obj, D::None),
            ("screens", T::Var, D::None),
            // The attached `HyprlandToplevel`
            ("HyprlandToplevel", T::Obj, D::None),
        ],
        signals: &[("closed", &[])],
        methods: &["activate", "close", "setMaximized", "setMinimized", "setFullscreen", "fullscreenOn", "setRectangle", "unsetRectangle"],
    },
    // quickshell's `ScreencopyView`: a window's pixels when `captureSource` is
    // a Toplevel (`services/capture.rs`, `mayland/capture.rs`), drawn
    // stretched. Repeats per frame while `live`. Screen sources are not
    // implemented
    TypeDef {
        name: "ScreencopyView",
        base: Some("Item"),
        kind: Kind::Screencopy,
        props: &[
            ("captureSource", T::Obj, D::None),
            ("live", T::Bool, D::Bool(false)),
            ("paintCursor", T::Bool, D::Bool(false)),
            ("hasContent", T::Bool, D::Bool(false)),
            ("sourceSize.width", T::Real, D::None),
            ("sourceSize.height", T::Real, D::None),
            ("constraintSize.width", T::Real, D::None),
            ("constraintSize.height", T::Real, D::None),
        ],
        signals: &[("stopped", &[])],
        methods: &["captureFrame"],
    },
];
