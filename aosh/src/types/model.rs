// Component, Repeater, ListView, Loader, LazyLoader, ListModel, ListElement
// (`model/`)

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    // Definition instantiated on demand. `createObject(parent, props)` creates
    // one from a script
    TypeDef {
        name: "Component",
        base: Some("QtObject"),
        kind: Kind::Component,
        props: &[],
        signals: &[],
        methods: &["createObject"],
    },
    TypeDef {
        name: "Repeater",
        base: Some("Item"),
        kind: Kind::Repeater,
        props: &[
            ("model", T::Var, D::None),
            ("delegate", T::Obj, D::None),
            ("count", T::Int, D::None),
        ],
        signals: &[("itemAdded", &["index", "item"]), ("itemRemoved", &["index", "item"])],
        methods: &["itemAt"],
    },
    // QtQml Instantiator: Variants with `active`, and the first object as
    // `object`
    TypeDef {
        name: "Instantiator",
        base: Some("QtObject"),
        kind: Kind::Instantiator,
        props: &[
            ("model", T::Var, D::None),
            ("delegate", T::Obj, D::None),
            ("active", T::Bool, D::Bool(true)),
            ("asynchronous", T::Bool, D::Bool(false)),
            ("count", T::Int, D::None),
            ("object", T::Obj, D::None),
        ],
        signals: &[("objectAdded", &["index", "object"]), ("objectRemoved", &["index", "object"])],
        methods: &["objectAt"],
    },
    TypeDef {
        name: "ListView",
        base: Some("Item"),
        kind: Kind::ListView,
        props: &[
            ("model", T::Var, D::None),
            ("delegate", T::Obj, D::None),
            ("count", T::Int, D::None),
            ("spacing", T::Real, D::None),
            // ListView.Horizontal is 1, Vertical 2
            ("orientation", T::Int, D::Num(2.0)),
            ("currentIndex", T::Int, D::Num(-1.0)),
            ("currentItem", T::Obj, D::None),
            ("contentX", T::Real, D::None),
            ("contentY", T::Real, D::None),
            ("contentWidth", T::Real, D::None),
            ("contentHeight", T::Real, D::None),
            // Flickable margins, not part of `contentHeight`
            // (`crate::model::margins`)
            ("topMargin", T::Real, D::None),
            ("bottomMargin", T::Real, D::None),
            ("leftMargin", T::Real, D::None),
            ("rightMargin", T::Real, D::None),
            ("interactive", T::Bool, D::Bool(true)),
            ("atYBeginning", T::Bool, D::Bool(true)),
            ("atYEnd", T::Bool, D::Bool(true)),
            ("boundsBehavior", T::Int, D::None),
            ("highlightFollowsCurrentItem", T::Bool, D::Bool(true)),
            ("keyNavigationEnabled", T::Bool, D::Bool(true)),
            ("cacheBuffer", T::Int, D::None),
            ("snapMode", T::Int, D::None),
            ("highlightMoveDuration", T::Int, D::Num(-1.0)),
            ("highlightRangeMode", T::Int, D::None),
            ("highlight", T::Obj, D::None),
            ("header", T::Obj, D::None),
            ("footer", T::Obj, D::None),
            // Row Transitions: accepted, not implemented
            ("add", T::Obj, D::None),
            ("remove", T::Obj, D::None),
            ("move", T::Obj, D::None),
            ("populate", T::Obj, D::None),
            ("displaced", T::Obj, D::None),
            ("addDisplaced", T::Obj, D::None),
            ("removeDisplaced", T::Obj, D::None),
            ("moveDisplaced", T::Obj, D::None),
            ("maximumFlickVelocity", T::Real, D::Num(2500.0)),
            ("flickDeceleration", T::Real, D::Num(1500.0)),
            ("reuseItems", T::Bool, D::Bool(false)),
            // Controls' attached ScrollBar
            ("ScrollBar.vertical", T::Obj, D::None),
            ("ScrollBar.horizontal", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[
            "positionViewAtIndex", "positionViewAtBeginning", "positionViewAtEnd",
            "incrementCurrentIndex", "decrementCurrentIndex", "itemAtIndex", "indexAt", "forceLayout",
        ],
    },
    TypeDef {
        name: "Loader",
        base: Some("Item"),
        kind: Kind::Loader,
        props: &[
            ("active", T::Bool, D::Bool(true)),
            ("source", T::Str, D::None),
            ("sourceComponent", T::Obj, D::None),
            ("item", T::Obj, D::None),
            // Loader.Null 0, Ready 1, Loading 2, Error 3
            ("status", T::Int, D::None),
            ("progress", T::Real, D::None),
            ("asynchronous", T::Bool, D::Bool(false)),
        ],
        signals: &[("loaded", &[])],
        methods: &["setSource"],
    },
    // quickshell's Loader for non-items such as windows
    TypeDef {
        name: "LazyLoader",
        base: Some("QtObject"),
        kind: Kind::Loader,
        props: &[
            ("active", T::Bool, D::Bool(false)),
            ("loading", T::Bool, D::Bool(false)),
            ("activeAsync", T::Bool, D::Bool(false)),
            ("component", T::Obj, D::None),
            ("source", T::Str, D::None),
            ("item", T::Obj, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "ListModel",
        base: Some("QtObject"),
        kind: Kind::ListModel,
        props: &[("count", T::Int, D::None), ("dynamicRoles", T::Bool, D::Bool(false))],
        signals: &[],
        methods: &["append", "insert", "remove", "move", "clear", "get", "set", "setProperty", "sync"],
    },
    // quickshell's `ScriptModel`: a row per element of `values`, keeping
    // delegates of remaining values (`model/script.rs`)
    TypeDef {
        name: "ScriptModel",
        base: Some("QtObject"),
        kind: Kind::ScriptModel,
        props: &[("values", T::Var, D::None), ("objectProp", T::Str, D::None)],
        signals: &[],
        methods: &[],
    },
    // A row. Its roles are whatever properties it is given
    TypeDef {
        name: "ListElement",
        base: Some("QtObject"),
        kind: Kind::Row,
        props: &[],
        signals: &[],
        methods: &[],
    },
];
