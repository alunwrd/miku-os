// `DesktopEntries.applications`: quickshell's object model of entries, whose
// `values` is them and whose `execute()` starts one (`services/apps.rs`)
function __aosh_entries(list) {
    const values = list.map(e => Object.assign(e, {
        execute() { Quickshell.execDetached(this.command); },
    }));
    return { values };
}

// quickshell's `Quickshell` global (`core/qmlglobal.hpp`): the shell's own
// directory, the environment, and starting a program without a `Process`
var Quickshell = {
    // `null` when it is not set, as quickshell's `QuickshellGlobal::env`
    env(name) { const v = __aosh_env(String(name)); return v === undefined ? null : v; },
    execDetached(cmd) {
        const argv = Array.isArray(cmd) ? cmd.map(String) : ["/bin/msh", "-c", String(cmd)];
        __aosh_exec(argv);
    },
    // The directory `shell.qml` is in, without a slash at the end
    get shellDir() { return __aosh_shell_dir(); },
    get shellRoot() { return __aosh_shell_dir(); },
    get configDir() { return __aosh_shell_dir(); },
    shellPath(p) { return __aosh_shell_dir() + "/" + p; },
    configPath(p) { return __aosh_shell_dir() + "/" + p; },
    // An icon from the theme, as quickshell's image provider names one;
    // with `check`, nothing when the theme has no such icon (`scene/icons.rs`)
    iconPath(icon, fallback) {
        if (fallback === true) return __aosh_icon_exists(String(icon)) ? "image://icon/" + icon : "";
        let u = "image://icon/" + icon;
        if (typeof fallback === "string" && fallback !== "") u += "?fallback=" + fallback;
        return u;
    },
    hasThemeIcon(icon) { return __aosh_icon_exists(String(icon)); },
    // Not connected to the compositor's clipboard: what is put here stays here
    clipboardText: "",
    // ShellScreen objects, whose size follows the output (`object/engine.rs`)
    get screens() { return __aosh_screens.screens; },
    reload(hard) { console.warn("Quickshell.reload(): aosh reloads when a file is saved"); },
    inhibitReloadPopup() {},
};

// QtCore's StandardPaths (`io/xdg.rs`): where a user's files are, as
// `file://` URLs - Qt's QUrls, which a string property holds as these
var StandardPaths = {
    DesktopLocation: 0, DocumentsLocation: 1, FontsLocation: 2, ApplicationsLocation: 3,
    MusicLocation: 4, MoviesLocation: 5, PicturesLocation: 6, TempLocation: 7, HomeLocation: 8,
    AppLocalDataLocation: 9, CacheLocation: 10, GenericDataLocation: 11, RuntimeLocation: 12,
    ConfigLocation: 13, DownloadLocation: 14, GenericCacheLocation: 15, GenericConfigLocation: 16,
    AppDataLocation: 17, AppConfigLocation: 18, PublicShareLocation: 19, TemplatesLocation: 20,
    StateLocation: 21, GenericStateLocation: 22,
    standardLocations(kind) { return __aosh_locations(Number(kind)); },
    writableLocation(kind) { const l = __aosh_locations(Number(kind)); return l.length ? l[0] : ""; },
};

// Quickshell.Io's FileViewError, what `loadFailed` and `saveFailed` say
var FileViewError = {
    Success: 0, Unknown: 1, FileNotFound: 2, PermissionDenied: 3, NotAFile: 4,
    toString(e) {
        return ["Success", "An unknown error has occurred", "The specified file does not exist",
                "Permission denied", "The specified path was not a file"][e] ?? "Invalid error";
    },
};

