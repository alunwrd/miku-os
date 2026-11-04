// Qt5Compat.GraphicalEffects masks, blurs, shadows, and radial gradient;
// QtQuick.Effects RectangularShadow and MultiEffect; ShaderEffectSource. Drawn
// by the scene (`scene/effect/`). `cached` is accepted and ignored, since
// passes are always cached

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

/// Common Qt5Compat effect properties: the source, and whether blur reaches
/// past the box
const SOURCE: [(&str, T, D); 3] = [
    ("source", T::Var, D::None),
    ("cached", T::Bool, D::Bool(false)),
    ("transparentBorder", T::Bool, D::Bool(false)),
];

pub static TYPES: &[TypeDef] = &[
    TypeDef {
        name: "ShaderEffectSource",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            ("sourceItem", T::Obj, D::None),
            ("sourceRect.x", T::Real, D::None),
            ("sourceRect.y", T::Real, D::None),
            ("sourceRect.width", T::Real, D::None),
            ("sourceRect.height", T::Real, D::None),
            ("textureSize.width", T::Real, D::None),
            ("textureSize.height", T::Real, D::None),
            ("hideSource", T::Bool, D::Bool(false)),
            ("live", T::Bool, D::Bool(true)),
            ("recursive", T::Bool, D::Bool(false)),
            ("mipmap", T::Bool, D::Bool(false)),
            ("format", T::Int, D::None),
            ("wrapMode", T::Int, D::None),
            ("textureMirroring", T::Int, D::None),
            ("samples", T::Int, D::None),
        ],
        signals: &[("scheduledUpdateCompleted", &[])],
        methods: &["scheduleUpdate"],
    },
    TypeDef {
        name: "OpacityMask",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[SOURCE[0], SOURCE[1], ("maskSource", T::Var, D::None), ("invert", T::Bool, D::Bool(false))],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "FastBlur",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[SOURCE[0], SOURCE[1], SOURCE[2], ("radius", T::Real, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "GaussianBlur",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            SOURCE[0], SOURCE[1], SOURCE[2],
            ("radius", T::Real, D::Num(4.0)),
            ("samples", T::Int, D::Num(9.0)),
            ("deviation", T::Real, D::Num(5.0 / 3.3333)),
        ],
        signals: &[],
        methods: &[],
    },
    // Its own `transparentBorder` is on unless it is turned off, as Qt's is
    TypeDef {
        name: "DropShadow",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            SOURCE[0], SOURCE[1],
            ("transparentBorder", T::Bool, D::Bool(true)),
            ("radius", T::Real, D::Num(4.0)),
            ("samples", T::Int, D::Num(9.0)),
            ("color", T::Color, D::Str("black")),
            ("horizontalOffset", T::Real, D::None),
            ("verticalOffset", T::Real, D::None),
            ("spread", T::Real, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "RectangularShadow",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            ("blur", T::Real, D::Num(10.0)),
            ("spread", T::Real, D::None),
            ("offset.x", T::Real, D::None),
            ("offset.y", T::Real, D::None),
            ("radius", T::Real, D::None),
            ("topLeftRadius", T::Real, D::Num(-1.0)),
            ("topRightRadius", T::Real, D::Num(-1.0)),
            ("bottomLeftRadius", T::Real, D::Num(-1.0)),
            ("bottomRightRadius", T::Real, D::Num(-1.0)),
            ("color", T::Color, D::Str("black")),
            ("cached", T::Bool, D::Bool(false)),
            ("material", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // MultiEffect: color and mask are drawn (`scene/effect/multi.rs`); blur and
    // shadow are not implemented and logged
    TypeDef {
        name: "MultiEffect",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            ("source", T::Var, D::None),
            ("brightness", T::Real, D::None),
            ("contrast", T::Real, D::None),
            ("saturation", T::Real, D::None),
            ("colorization", T::Real, D::None),
            ("colorizationColor", T::Color, D::Str("#ff0000")),
            ("maskEnabled", T::Bool, D::Bool(false)),
            ("maskSource", T::Var, D::None),
            ("maskInverted", T::Bool, D::Bool(false)),
            ("maskThresholdMin", T::Real, D::None),
            ("maskSpreadAtMin", T::Real, D::None),
            ("maskThresholdMax", T::Real, D::Num(1.0)),
            ("maskSpreadAtMax", T::Real, D::None),
            ("blurEnabled", T::Bool, D::Bool(false)),
            ("blur", T::Real, D::None),
            ("blurMax", T::Int, D::Num(32.0)),
            ("blurMultiplier", T::Real, D::None),
            ("shadowEnabled", T::Bool, D::Bool(false)),
            ("shadowOpacity", T::Real, D::Num(1.0)),
            ("shadowBlur", T::Real, D::Num(1.0)),
            ("shadowColor", T::Color, D::Str("black")),
            ("shadowHorizontalOffset", T::Real, D::None),
            ("shadowVerticalOffset", T::Real, D::None),
            ("shadowScale", T::Real, D::Num(1.0)),
            ("autoPaddingEnabled", T::Bool, D::Bool(true)),
            ("paddingRect", T::Var, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // Radii default to width and height (`scene/effect/gradient.rs`); with no
    // `gradient`, white to black
    TypeDef {
        name: "RadialGradient",
        base: Some("Item"),
        kind: Kind::Plain,
        props: &[
            SOURCE[0], SOURCE[1],
            ("horizontalOffset", T::Real, D::None),
            ("verticalOffset", T::Real, D::None),
            ("horizontalRadius", T::Real, D::None),
            ("verticalRadius", T::Real, D::None),
            ("angle", T::Real, D::None),
            ("gradient", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
];
