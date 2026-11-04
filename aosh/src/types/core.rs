// QtQml base types: QtObject and Connections

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    TypeDef {
        name: "QtObject",
        base: None,
        kind: Kind::Plain,
        props: &[("objectName", T::Str, D::None)],
        signals: &[],
        // `item.destroy()` (`object/life.rs`)
        methods: &["destroy"],
    },
    // `Connections { target: x; function onClicked() {} }`: handlers for
    // another object's signals, following `target`
    TypeDef {
        name: "Connections",
        base: Some("QtObject"),
        kind: Kind::Connections,
        props: &[
            ("target", T::Obj, D::None),
            ("enabled", T::Bool, D::Bool(true)),
            ("ignoreUnknownSignals", T::Bool, D::Bool(false)),
        ],
        signals: &[],
        methods: &[],
    },
    // `Binding { target; property; value; when }` (`object/hold.rs`),
    // `restoreMode` default RestoreBindingOrValue (3).
    // Qt.labs.synchronizer: keeps its `on` property or `targetObject`'s
    // `targetProperty`, `sourceObject`'s `sourceProperty`, and declared aliases
    // equal (`object/sync.rs`)
    TypeDef {
        name: "Synchronizer",
        base: Some("QtObject"),
        kind: Kind::Synchronizer,
        props: &[
            ("sourceObject", T::Obj, D::None),
            ("sourceProperty", T::Str, D::None),
            ("targetObject", T::Obj, D::None),
            ("targetProperty", T::Str, D::None),
        ],
        signals: &[("valueBounced", &[])],
        methods: &[],
    },
    TypeDef {
        name: "Binding",
        base: Some("QtObject"),
        kind: Kind::Binding,
        props: &[
            ("target", T::Obj, D::None),
            ("property", T::Str, D::None),
            ("value", T::Var, D::None),
            ("when", T::Bool, D::Bool(true)),
            ("delayed", T::Bool, D::Bool(false)),
            ("restoreMode", T::Int, D::Num(3.0)),
        ],
        signals: &[],
        methods: &[],
    },
    // Stand-in for a missing type (`object/build.rs`): an Item accepting any
    // property, with no signals
    TypeDef {
        name: super::INERT,
        base: Some("Item"),
        kind: Kind::Inert,
        props: &[],
        signals: &[],
        methods: &[],
    },
];
