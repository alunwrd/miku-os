// Timer, Process and its parsers, and FileView (`io/`)

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    // QtQml Timer, on the engine's clock (`Engine::advance`)
    TypeDef {
        name: "Timer",
        base: Some("QtObject"),
        kind: Kind::Timer,
        props: &[
            ("interval", T::Int, D::Num(1000.0)),
            ("running", T::Bool, D::Bool(false)),
            ("repeat", T::Bool, D::Bool(false)),
            ("triggeredOnStart", T::Bool, D::Bool(false)),
        ],
        signals: &[("triggered", &[])],
        methods: &["start", "stop", "restart"],
    },
    TypeDef {
        name: "Process",
        base: Some("QtObject"),
        kind: Kind::Process,
        props: &[
            ("running", T::Bool, D::Bool(false)),
            ("command", T::Var, D::None),
            ("workingDirectory", T::Str, D::None),
            ("environment", T::Var, D::None),
            ("clearEnvironment", T::Bool, D::Bool(false)),
            ("stdout", T::Obj, D::None),
            ("stderr", T::Obj, D::None),
            ("stdinEnabled", T::Bool, D::Bool(false)),
            ("processId", T::Var, D::None),
        ],
        signals: &[("started", &[]), ("exited", &["exitCode", "exitStatus"])],
        methods: &["exec", "signal", "write", "startDetached"],
    },
    TypeDef {
        name: "StdioCollector",
        base: Some("QtObject"),
        kind: Kind::Collector,
        props: &[
            ("text", T::Str, D::None),
            ("data", T::Str, D::None),
            ("waitForEnd", T::Bool, D::Bool(true)),
        ],
        signals: &[("streamFinished", &[])],
        methods: &[],
    },
    TypeDef {
        name: "SplitParser",
        base: Some("QtObject"),
        kind: Kind::Splitter,
        props: &[("splitMarker", T::Str, D::Str("\n"))],
        signals: &[("read", &["data"])],
        methods: &[],
    },
    TypeDef {
        name: "FileView",
        base: Some("QtObject"),
        kind: Kind::FileView,
        props: &[
            ("path", T::Str, D::None),
            ("watchChanges", T::Bool, D::Bool(false)),
            ("preload", T::Bool, D::Bool(true)),
            ("blockLoading", T::Bool, D::Bool(false)),
            ("blockAllReads", T::Bool, D::Bool(false)),
            ("blockWrites", T::Bool, D::Bool(false)),
            ("atomicWrites", T::Bool, D::Bool(true)),
            ("printErrors", T::Bool, D::Bool(true)),
            ("loaded", T::Bool, D::Bool(false)),
            // Default property: the inner JsonAdapter
            ("adapter", T::Obj, D::None),
            // Read by `text()`, so a binding calling it tracks it
            ("__text", T::Str, D::None),
        ],
        signals: &[
            ("loaded", &[]), ("loadFailed", &["error"]), ("saved", &[]), ("saveFailed", &["error"]),
            ("fileChanged", &[]), ("adapterUpdated", &[]),
        ],
        methods: &["text", "data", "reload", "setText", "setData", "waitForJob", "writeAdapter"],
    },
    // QtQuick FontLoader: loads a font file and sets `name`. Null 0, Ready 1,
    // Loading 2, Error 3
    TypeDef {
        name: "FontLoader",
        base: Some("QtObject"),
        kind: Kind::FontLoader,
        props: &[("source", T::Str, D::None), ("name", T::Str, D::None), ("status", T::Int, D::None)],
        signals: &[],
        methods: &[],
    },
    // quickshell's JSON adapter (`io/json.rs`): declared properties are file
    // keys, and a JsonObject's are a nested object's
    TypeDef {
        name: "JsonAdapter",
        base: Some("QtObject"),
        kind: Kind::Json,
        props: &[],
        signals: &[("adapterUpdated", &[])],
        methods: &[],
    },
    TypeDef { name: "JsonObject", base: Some("QtObject"), kind: Kind::Json, props: &[], signals: &[], methods: &[] },
];
