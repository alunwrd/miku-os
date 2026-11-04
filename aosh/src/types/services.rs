// Global service objects: desktop state, machine state, colors, and the clock

use super::{Default as D, Kind, TypeDef};
use crate::object::value::PropType as T;

pub const TYPES: &[TypeDef] = &[
    // `Desktop`: workspaces and windows from `may_desktop`
    TypeDef {
        name: "AoshDesktop",
        base: Some("QtObject"),
        kind: Kind::Desktop,
        props: &[
            ("workspace", T::Int, D::Num(1.0)),
            // [{ n, active, busy }], one per workspace
            ("workspaces", T::Var, D::None),
            // [{ title, appId, workspace, focused }]
            ("windows", T::Var, D::None),
            ("focusedTitle", T::Str, D::None),
            ("screenWidth", T::Int, D::None),
            ("screenHeight", T::Int, D::None),
            ("workTop", T::Int, D::None),
            // Active keyboard layout: "us", "English (US)", and index of count
            // (`keyboard_layout`)
            ("keyboardLayout", T::Str, D::None),
            ("keyboardLayoutName", T::Str, D::None),
            ("keyboardLayoutIndex", T::Int, D::None),
            ("keyboardLayouts", T::Int, D::None),
        ],
        signals: &[("signalled", &["word"])],
        methods: &["switchTo", "signal"],
    },
    // `Sys`: /proc values, updated each second
    TypeDef {
        name: "AoshSys",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("memUsed", T::Real, D::None),
            ("memTotal", T::Real, D::None),
            ("memPercent", T::Int, D::None),
            ("netIp", T::Str, D::Str("offline")),
            ("uptime", T::Int, D::None),
            ("uptimeText", T::Str, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // `Theme`: the desktop palette, from the `theme { }` block
    TypeDef {
        name: "AoshTheme",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("page", T::Color, D::Str("#ff0f0d13")),
            ("card", T::Color, D::Str("#bf211f24")),
            ("accent", T::Color, D::Str("#eecfbdfe")),
            ("onAccent", T::Color, D::Str("#ff141218")),
            ("text", T::Color, D::Str("#ffe6e1e9")),
            ("textMuted", T::Color, D::Str("#ff948f99")),
            ("border", T::Color, D::Str("#ff2b292f")),
            ("wallpaper", T::Str, D::None),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's `DesktopEntries`, over `/etc/mayland/apps.d`
    // (`services/apps.rs`)
    TypeDef {
        name: "AoshDesktopEntries",
        base: Some("QtObject"),
        kind: Kind::Apps,
        props: &[("applications", T::Var, D::None)],
        signals: &[],
        methods: &["byId", "heuristicLookup"],
    },
    // quickshell's `Mpris` and `UPower` with no backend, since MikuOS has no
    // D-Bus. `players` and `devices` are empty models; the display device is
    // not `ready`
    TypeDef {
        name: "AoshMpris",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("players", T::Var, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "AoshUPower",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("displayDevice", T::Obj, D::None), ("devices", T::Var, D::None), ("onBattery", T::Bool, D::Bool(false))],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "UPowerDevice",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("type", T::Int, D::None),
            ("powerSupply", T::Bool, D::Bool(false)),
            ("energy", T::Real, D::None),
            ("energyCapacity", T::Real, D::None),
            ("changeRate", T::Real, D::None),
            ("timeToEmpty", T::Real, D::None),
            ("timeToFull", T::Real, D::None),
            ("percentage", T::Real, D::None),
            ("isPresent", T::Bool, D::Bool(false)),
            ("state", T::Int, D::None),
            ("healthPercentage", T::Real, D::None),
            ("healthSupported", T::Bool, D::Bool(false)),
            ("iconName", T::Str, D::None),
            ("isLaptopBattery", T::Bool, D::Bool(false)),
            ("nativePath", T::Str, D::None),
            ("model", T::Str, D::None),
            ("ready", T::Bool, D::Bool(false)),
        ],
        signals: &[],
        methods: &[],
    },
    // quickshell's system tray, PipeWire, and Bluetooth with no backend: empty
    // models and null defaults (`nothing_behind`)
    TypeDef {
        name: "AoshSystemTray",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("items", T::Var, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "AoshPipewire",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("nodes", T::Var, D::None),
            ("links", T::Var, D::None),
            ("linkGroups", T::Var, D::None),
            ("defaultAudioSink", T::Obj, D::None),
            ("defaultAudioSource", T::Obj, D::None),
            ("preferredDefaultAudioSink", T::Obj, D::None),
            ("preferredDefaultAudioSource", T::Obj, D::None),
            ("ready", T::Bool, D::Bool(false)),
        ],
        signals: &[],
        methods: &[],
    },
    // Keeps PipeWire objects bound; a no-op
    TypeDef {
        name: "PwObjectTracker",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("objects", T::Var, D::None)],
        signals: &[],
        methods: &[],
    },
    TypeDef {
        name: "AoshBluetooth",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[("adapters", T::Var, D::None), ("devices", T::Var, D::None), ("defaultAdapter", T::Obj, D::None)],
        signals: &[],
        methods: &[],
    },
    // quickshell's notification server with no senders; `notification` never
    // fires
    TypeDef {
        name: "NotificationServer",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("keepOnReload", T::Bool, D::Bool(true)),
            ("persistenceSupported", T::Bool, D::Bool(false)),
            ("bodySupported", T::Bool, D::Bool(true)),
            ("bodyMarkupSupported", T::Bool, D::Bool(false)),
            ("bodyHyperlinksSupported", T::Bool, D::Bool(false)),
            ("bodyImagesSupported", T::Bool, D::Bool(false)),
            ("actionsSupported", T::Bool, D::Bool(false)),
            ("actionIconsSupported", T::Bool, D::Bool(false)),
            ("imageSupported", T::Bool, D::Bool(false)),
            ("inlineReplySupported", T::Bool, D::Bool(false)),
            ("extraHints", T::Var, D::None),
            ("trackedNotifications", T::Var, D::None),
        ],
        signals: &[("notification", &["notification"])],
        methods: &[],
    },
    // QtPositioning `PositionSource` with no plugin: not `valid`, and `start()`
    // finds nothing
    TypeDef {
        name: "PositionSource",
        base: Some("QtObject"),
        kind: Kind::Plain,
        props: &[
            ("active", T::Bool, D::Bool(false)),
            ("valid", T::Bool, D::Bool(false)),
            ("name", T::Str, D::None),
            ("updateInterval", T::Int, D::None),
            ("preferredPositioningMethods", T::Int, D::None),
            ("supportedPositioningMethods", T::Int, D::None),
            ("position", T::Var, D::None),
            ("sourceError", T::Int, D::None),
        ],
        signals: &[("validityChanged", &[]), ("updateTimeout", &[]), ("errorOccurred", &["sourceError"])],
        methods: &["start", "stop", "update"],
    },
    // quickshell's `SystemClock`, ticking at `precision` (`SystemClock.Hours`,
    // `.Minutes`, `.Seconds`)
    TypeDef {
        name: "SystemClock",
        base: Some("QtObject"),
        kind: Kind::Clock,
        props: &[
            ("enabled", T::Bool, D::Bool(true)),
            ("precision", T::Int, D::Num(2.0)),
            ("date", T::Var, D::None),
            ("hours", T::Int, D::None),
            ("minutes", T::Int, D::None),
            ("seconds", T::Int, D::None),
        ],
        signals: &[],
        methods: &[],
    },
];
