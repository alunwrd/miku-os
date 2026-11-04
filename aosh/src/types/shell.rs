// quickshell containers: the shell root and Scope

use super::{Kind, TypeDef};

use super::Default as D;
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    TypeDef { name: "ShellRoot", base: Some("QtObject"), kind: Kind::Plain, props: &[], signals: &[], methods: &[] },
    TypeDef { name: "Scope", base: Some("QtObject"), kind: Kind::Plain, props: &[], signals: &[], methods: &[] },
    // Root of a `pragma Singleton` file (`qml/import.rs`)
    TypeDef { name: "Singleton", base: Some("Scope"), kind: Kind::Plain, props: &[], signals: &[], methods: &[] },
    // Layer surface. An Item, so children lay out in it; `width` and `height`
    // are the configured size
    TypeDef {
        name: "PanelWindow",
        base: Some("Item"),
        kind: Kind::Window,
        props: &[
            ("anchors.top", T::Bool, D::Bool(false)),
            ("anchors.bottom", T::Bool, D::Bool(false)),
            ("anchors.left", T::Bool, D::Bool(false)),
            ("anchors.right", T::Bool, D::Bool(false)),
            ("margins.top", T::Int, D::None),
            ("margins.bottom", T::Int, D::None),
            ("margins.left", T::Int, D::None),
            ("margins.right", T::Int, D::None),
            // -1 is quickshell's "as big as the window along its edge"
            ("exclusiveZone", T::Int, D::Num(-1.0)),
            ("color", T::Color, D::Str("transparent")),
            ("aboveWindows", T::Bool, D::Bool(true)),
            ("focusable", T::Bool, D::Bool(false)),
            // Attached `WlrLayershell` (`mayland/`): band WlrLayer.Background
            // 0, Bottom 1, Top 2, Overlay 3 (default from `aboveWindows`),
            // keyboard WlrKeyboardFocus.None 0, Exclusive 1, OnDemand 2
            ("WlrLayershell.layer", T::Int, D::Num(2.0)),
            ("WlrLayershell.keyboardFocus", T::Int, D::None),
            ("WlrLayershell.namespace", T::Str, D::None),
            // ExclusionMode.Normal 0, Ignore 1, Auto 2
            ("exclusionMode", T::Int, D::Num(2.0)),
            // Screen, one of `Quickshell.screens`. Only one output is used
            ("screen", T::Var, D::None),
            // Input region: a Region, or the whole window with none
            ("mask", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `Region`: a rectangle or item's box, with regions added or
    // subtracted
    TypeDef {
        name: "Region",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("item", T::Obj, D::None),
            ("x", T::Int, D::None),
            ("y", T::Int, D::None),
            ("width", T::Int, D::None),
            ("height", T::Int, D::None),
            // Intersection.Combine 0, Subtract 1, Intersect 2, Xor 3;
            // RegionShape.Rect 0, Ellipse 1
            ("intersection", T::Int, D::None),
            ("shape", T::Int, D::None),
            ("radius", T::Int, D::None),
            ("regions", T::List, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `ShellScreen`, sized by `may_desktop` (`services/`), or
    // `--size` in `aosh shot`
    TypeDef {
        name: "ShellScreen",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("name", T::Str, D::None),
            ("model", T::Str, D::None),
            ("serialNumber", T::Str, D::None),
            ("x", T::Int, D::None),
            ("y", T::Int, D::None),
            ("width", T::Int, D::None),
            ("height", T::Int, D::None),
            ("devicePixelRatio", T::Real, D::Num(1.0)),
            ("physicalPixelDensity", T::Real, D::None),
            ("logicalPixelDensity", T::Real, D::None),
            ("orientation", T::Int, D::None),
            ("primaryOrientation", T::Int, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // Read by `Quickshell.screens`, so bindings track it
    TypeDef { name: "AoshScreens", base: Some("QtObject"), kind: Kind::Plain, props: &[("screens", T::List, D::None)],
              signals: &[], methods: &[] },
    // `Quickshell.Widgets.IconImage`: an Image for an icon with `implicitSize`
    // both ways, requested at the smaller side and fitted (`scene/icons.rs`)
    TypeDef {
        name: "IconImage",
        base: Some("Image"),
        kind: Kind::Plain,
        props: &[("implicitSize", T::Real, D::None), ("actualSize", T::Real, D::None), ("backer", T::Obj, D::None)],
        signals: &[],
        methods: &[],
    },
    // Kirigami `Icon`: theme name or URL, at its own size, fitted. Mask
    // coloring is not implemented
    TypeDef {
        name: "Icon",
        base: Some("Image"),
        kind: Kind::Plain,
        props: &[
            ("fallback", T::Str, D::Str("unknown")),
            ("placeholder", T::Str, D::Str("image-x-icon")),
            ("color", T::Color, D::Str("transparent")),
            ("isMask", T::Bool, D::Bool(false)),
            ("roundToIconSize", T::Bool, D::Bool(true)),
            ("animated", T::Bool, D::Bool(false)),
            ("active", T::Bool, D::Bool(false)),
            ("selected", T::Bool, D::Bool(false)),
            ("valid", T::Bool, D::Bool(false)),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `Variants`: a `delegate` instance per `model` element, with
    // `modelData`. Not an item
    TypeDef {
        name: "Variants",
        base: Some("QtObject"),
        kind: Kind::Instantiator,
        props: &[
            ("model", T::Var, D::None),
            ("delegate", T::Obj, D::None),
            ("instances", T::List, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `Quickshell.Io.IpcHandler` (`ipc/`)
    TypeDef {
        name: "IpcHandler",
        base: Some("QtObject"),
        kind: Kind::Ipc,
        props: &[("target", T::Str, D::None), ("enabled", T::Bool, D::Bool(true))],
        signals: &[],
        methods: &[],
    },
];
