// QtQuick.Shapes: a Shape and its ShapePaths (`scene/shapes/`). Defaults follow
// Qt: white fill and one-pixel stroke, odd-even, bevel joins, square caps

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    TypeDef {
        name: "Shape",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            // Shape.UnknownRenderer 0 ... CurveRenderer 4: requested and active
            ("preferredRendererType", T::Int, D::Num(0.0)),
            ("rendererType", T::Int, D::Num(1.0)),
            ("asynchronous", T::Bool, D::None),
            ("vendorExtensionsEnabled", T::Bool, D::Bool(true)),
            ("containsMode", T::Int, D::None),
            ("fillMode", T::Int, D::None),
            ("status", T::Int, D::Num(1.0)),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "ShapePath",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("startX", T::Real, D::None),
            ("startY", T::Real, D::None),
            ("strokeColor", T::Color, D::Str("white")),
            ("strokeWidth", T::Real, D::Num(1.0)),
            ("fillColor", T::Color, D::Str("white")),
            // ShapePath.OddEvenFill 0, WindingFill 1
            ("fillRule", T::Int, D::Num(0.0)),
            // ShapePath.MiterJoin 0, BevelJoin 64, RoundJoin 128
            ("joinStyle", T::Int, D::Num(64.0)),
            ("miterLimit", T::Real, D::Num(2.0)),
            // ShapePath.FlatCap 0, SquareCap 16, RoundCap 32
            ("capStyle", T::Int, D::Num(16.0)),
            // ShapePath.SolidLine 1, DashLine 2
            ("strokeStyle", T::Int, D::Num(1.0)),
            ("dashOffset", T::Real, D::None),
            ("dashPattern", T::Var, D::None),
            ("pathHints", T::Int, D::None),
            ("fillGradient", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "PathLine",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("x", T::Real, D::None), ("y", T::Real, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "PathMove",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("x", T::Real, D::None), ("y", T::Real, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "PathQuad",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("x", T::Real, D::None), ("y", T::Real, D::None),
            ("controlX", T::Real, D::None), ("controlY", T::Real, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "PathCubic",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("x", T::Real, D::None), ("y", T::Real, D::None),
            ("control1X", T::Real, D::None), ("control1Y", T::Real, D::None),
            ("control2X", T::Real, D::None), ("control2Y", T::Real, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // Ellipse arc by center, from `startAngle` through `sweepAngle`, degrees
    // clockwise on screen. Joined to what came before by a line unless it
    // starts a subpath
    TypeDef {
        name: "PathAngleArc",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("centerX", T::Real, D::None), ("centerY", T::Real, D::None),
            ("radiusX", T::Real, D::None), ("radiusY", T::Real, D::None),
            ("startAngle", T::Real, D::None), ("sweepAngle", T::Real, D::None),
            ("moveToStart", T::Bool, D::Bool(true)),
        ],
        signals: &[],
        methods: &[],
    },
    // Points: move to the first, line to the rest. `__points` is scene text,
    // `x,y x,y` (`Core::hook`)
    TypeDef {
        name: "PathPolyline",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("path", T::Var, D::None), ("__points", T::Str, D::None)],
        signals: &[],
        methods: &[],
    },
];
