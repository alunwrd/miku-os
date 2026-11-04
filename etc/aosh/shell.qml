// The desktop's shell: what aosh draws when nothing else is asked of it
//
// `~/.config/aosh/shell.qml` is read instead when there is one, as quickshell
// reads `~/.config/quickshell/shell.qml`. Every other `.qml` file in the same
// directory is a type this one can use by its name: `Bar` is `Bar.qml`.
//
// Its text is drawn natively, as FreeType renders it, and not from the distance
// fields a Text gets by default (`scene/text/field.rs`): small labels are
// crisper so.
//@ pragma NativeTextRendering

import Quickshell

ShellRoot {
    Bar {}
    // Hidden until `signal, launcher toggle` (the $mod+D chord) or
    // `aosh ipc call launcher toggle`
    Launcher {}
}
