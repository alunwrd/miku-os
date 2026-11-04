// Behaviors and animations (`anim/`), stepped once a frame by the engine's
// clock with Qt's easing curves

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

const ANIMATION: &[(&str, T, D)] = &[
    ("running", T::Bool, D::Bool(false)),
    ("paused", T::Bool, D::Bool(false)),
    ("loops", T::Int, D::Num(1.0)),
    ("alwaysRunToEnd", T::Bool, D::Bool(false)),
];

const PROPERTY_ANIMATION: &[(&str, T, D)] = &[
    ("duration", T::Int, D::Num(250.0)),
    ("from", T::Var, D::None),
    ("to", T::Var, D::None),
    ("target", T::Obj, D::None),
    ("targets", T::Var, D::None),
    ("property", T::Str, D::None),
    ("properties", T::Str, D::None),
    ("easing.type", T::Int, D::None),
    ("easing.amplitude", T::Real, D::Num(1.0)),
    ("easing.overshoot", T::Real, D::Num(1.70158)),
    ("easing.period", T::Real, D::Num(0.3)),
    // Easing.BezierSpline's points (`anim/easing.rs`)
    ("easing.bezierCurve", T::Var, D::None),
];

pub const TYPES: &[TypeDef] = &[
    TypeDef {
        name: "Behavior",
        base: Some("QtObject"),
        kind: Kind::Behavior,
        // `animation` is the default property
        props: &[("enabled", T::Bool, D::Bool(true)), ("animation", T::Obj, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "Animation",
        base: Some("QtObject"),
        kind: Kind::Animation,
        props: ANIMATION,
        signals: &[("started", &[]), ("stopped", &[]), ("finished", &[])],
        methods: &["start", "stop", "restart", "pause", "resume", "complete"],
    },
    TypeDef { name: "PropertyAnimation", base: Some("Animation"), kind: Kind::Animation, props: PROPERTY_ANIMATION, signals: &[], methods: &[] },
    TypeDef { name: "NumberAnimation", base: Some("PropertyAnimation"), kind: Kind::Animation, props: &[], signals: &[], methods: &[] },
    TypeDef { name: "ColorAnimation", base: Some("PropertyAnimation"), kind: Kind::Animation, props: &[], signals: &[], methods: &[] },
    TypeDef { name: "SequentialAnimation", base: Some("Animation"), kind: Kind::Animation, props: &[], signals: &[], methods: &[] },
    TypeDef { name: "ParallelAnimation", base: Some("Animation"), kind: Kind::Animation, props: &[], signals: &[], methods: &[] },
    // RotationAnimation `direction`: Numerical 0, Clockwise 1, Counterclockwise
    // 2, Shortest 3
    TypeDef {
        name: "RotationAnimation",
        base: Some("PropertyAnimation"),
        kind: Kind::Animation,
        props: &[("direction", T::Int, D::None)],
        signals: &[],
        methods: &[],
    },
    // A property set, at once, when a run reaches it
    TypeDef {
        name: "PropertyAction",
        base: Some("Animation"),
        kind: Kind::Animation,
        props: &[
            ("target", T::Obj, D::None),
            ("targets", T::Var, D::None),
            ("property", T::Str, D::None),
            ("properties", T::Str, D::None),
            ("value", T::Var, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // `script` is a handler of a private signal (`anim::SCRIPT`,
    // `object/build.rs`)
    TypeDef {
        name: "ScriptAction",
        base: Some("Animation"),
        kind: Kind::Animation,
        props: &[("scriptName", T::Str, D::None)],
        signals: &[("__script", &[])],
        methods: &[],
    },
    TypeDef {
        name: "PauseAnimation",
        base: Some("Animation"),
        kind: Kind::Animation,
        props: &[("duration", T::Int, D::Num(250.0))],
        signals: &[],
        methods: &[],
    },
];
