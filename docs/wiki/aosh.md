# aosh

aosh is the MikuOS desktop shell: one program that reads a directory of QML files and draws the bar, the launcher, the overview and any other panel from them, as Quickshell does on Hyprland. The declarative half of QML is parsed by aosh itself; every expression, binding and function body is real JavaScript run by QuickJS. Text, rectangles, canvases and effects are drawn the way Qt Quick draws them, measured against Qt's own renders, and since 2026-10-21 a window can be drawn on the NVIDIA card by a kernel GPU program with the processor as the fallback. aosh is a Rust `std` program in `aosh/` and a client of [Mayland](mayland.md).

## Contents

- [What aosh is](#what-aosh-is)
- [Running it](#running-it)
- [Where the config lives, and reloading](#where-the-config-lives-and-reloading)
- [A minimal config](#a-minimal-config)
- [Architecture](#architecture)
- [The QML language as aosh reads it](#the-qml-language-as-aosh-reads-it)
- [QML type reference](#qml-type-reference)
- [Global objects and the JavaScript environment](#global-objects-and-the-javascript-environment)
- [Layout, the display list and damage](#layout-the-display-list-and-damage)
- [Text](#text)
- [The painter and effects](#the-painter-and-effects)
- [Drawing on the card](#drawing-on-the-card)
- [IPC: IpcHandler and `aosh ipc`](#ipc-ipchandler-and-aosh-ipc)
- [The launcher, the keyboard and the overview](#the-launcher-the-keyboard-and-the-overview)
- [Running aosh on the Linux host](#running-aosh-on-the-linux-host)
- [The negi desktop](#the-negi-desktop)
- [Tests](#tests)
- [Performance](#performance)
- [Debugging](#debugging)
- [Known gaps compared with Quickshell](#known-gaps-compared-with-quickshell)
- [Code map](#code-map)

## What aosh is

| Fact | Value |
|---|---|
| Role | The desktop shell (bar, launcher, panels), started by `maysession` from `/etc/mayland/session.d/10-desktop.conf` |
| Language | A subset of QML for the tree; JavaScript (QuickJS, through the `rquickjs` 0.14 crate) for every expression |
| Model | Quickshell's: `ShellRoot`, `PanelWindow`, `Variants`, `Process`, `IpcHandler`, `qs.*` module imports |
| Display | Layer surfaces, popups and toplevels over `libmay` |
| Build | A `std` program, built by the builder's `std` step (by hand: `cd builder && cargo run --release -- std aosh`); QuickJS's C is compiled by `lib/c/miku-cc` |
| Size | About 34,000 lines of Rust in `aosh/src` |

**Relation to Quickshell and Qt.** aosh reproduces Qt Quick's observable behaviour: layout rules, text metrics to a 64th of a pixel, how a rectangle's edge or a Canvas path is rasterised, what each effect's shader passes compute. The rule (decided by the user on 2026-10-08) is that the code is aosh's own, written from what Qt is seen to do and measured against `qml6` renders; Qt's LGPL source is read to learn constants and rules but never transliterated. FreeType (FTL licence) is used directly: aosh vendors FreeType 2.14.3, the version on the developer's Arch machine, in `lib/freetype`. The tree is Apache-2.0. The negi desktop config in `etc/aosh/negi/` is a fork of a GPL-3 Quickshell config and keeps its licence inside that directory; aosh interprets those files and does not link them.

aosh replaced `mayshell` and `libmay`'s `doc`/`lay`/`view` toolkit on 2026-10-06 (stage A4 of [../../dev/aosh-architecture.md](../../dev/aosh-architecture.md)).

## Running it

| Command | Effect |
|---|---|
| `aosh` | The shell: the config named in `~/.config/aosh/default` or `/etc/aosh/default`, else `~/.config/aosh/shell.qml`, else `/etc/aosh/shell.qml` |
| `aosh file.qml` | That file as the shell |
| `aosh -c NAME` | A named config: `~/.config/aosh/NAME/shell.qml`, else `/etc/aosh/NAME/shell.qml` (Quickshell's `qs -c NAME`). Runs beside another shell with its own IPC socket |
| `aosh --eval '<js>'` | Evaluate JavaScript and print the result |
| `aosh check <file or dir>...` | Parse every QML file and compile every piece of JavaScript in it, running nothing; reports where a file stops parsing or a binding fails to compile |
| `aosh run [file.qml]` | Load a file with no window; run timers, animations and processes on the real clock until nothing is left |
| `aosh eval <file.qml> '<expr>'` | Load a file and evaluate an expression in the scope of its root object |
| `aosh ipc [-c NAME] call <target> <function> [args...]` | Call a running shell's `IpcHandler` |
| `aosh ipc [-c NAME] show` | List a running shell's IPC targets and functions |
| `aosh shot <file.qml> [options]` | Render a shell to a PNG with no display server (host development; see below) |

The mayland command `mayland shell` also starts `/bin/aosh` with the caller's environment.

## Where the config lives, and reloading

- `/etc/aosh/` is the default config shipped in the image: `shell.qml`, `Bar.qml`, `Launcher.qml`, `Pill.qml`, `Chip.qml`, `hello.qml`, and `negi/`. On a live boot `/etc` is the read-only boot image.
- `~/.config/aosh/shell.qml` takes the place of `/etc/aosh/shell.qml` when it exists, the same precedence Quickshell gives `~/.config/quickshell`. The shell is chosen again at every reload.
- `~/.config/aosh/default` (or `/etc/aosh/default`) may hold one word naming a config, which a bare `aosh` then runs; a package that ships a whole desktop uses it.
- Every other `.qml` file in the shell's directory is a type by its file name: `Bar.qml` is `Bar`.

**Editing.** Copy the default and edit the copy: `cp -r /etc/aosh ~/.config/aosh`. aosh follows `~/.config/aosh` even before it exists (it says `... is followed: a shell.qml there takes over` at start) and picks up a `shell.qml` that appears there.

**Reload.** aosh watches its config directories with inotify. When a file is saved, the tree is rebuilt after a short delay; aosh logs `<path> reloaded`. A reload that fails to load leaves what was on the screen up and logs `the reload did not load, so what was up stays up: <error>`. `Quickshell.reload()` only says that aosh reloads when a file is saved.

**Fonts** are read from `/etc/aosh/fonts` (shipped: `ui-regular.ttf`, `ui-bold.ttf`, `JetBrainsMono-Regular.ttf`) and from the user's `$XDG_DATA_HOME/fonts`. With none, aosh logs `no fonts in /etc/aosh/fonts - text will not be drawn`. The application font, icon theme and icon colours come from `kdeglobals`, read as KConfig reads it: the user's `$XDG_CONFIG_HOME` or `~/.config` first, then `$XDG_CONFIG_DIRS` (`/etc/xdg`).

## A minimal config

`~/.config/aosh/shell.qml`:

```qml
import Quickshell
import QtQuick

ShellRoot {
    PanelWindow {
        anchors.top: true
        anchors.left: true
        anchors.right: true
        implicitHeight: 32
        exclusiveZone: 32
        color: "transparent"

        Rectangle {
            anchors.fill: parent
            anchors.margins: 4
            radius: 12
            color: Theme.card

            Text {
                anchors.centerIn: parent
                text: Desktop.focusedTitle + "  ·  " + clock.hours + ":" +
                      String(clock.minutes).padStart(2, "0")
                color: Theme.text
            }
        }
    }

    SystemClock { id: clock; precision: SystemClock.Minutes }

    IpcHandler {
        target: "hello"
        function say(word: string): string { console.log(word); return "ok" }
    }
}
```

`aosh ipc call hello say hi` then prints `hi` to the log and answers `ok`. The shipped `/etc/aosh/shell.qml` is a `ShellRoot` with `Bar {}` and `Launcher {}`.

## Architecture

```
aosh/src/
  main.rs       command line
  shell.rs      the shell: config choice, the event loop, reload
  qml/          lexer, parser -> AST, imports and modules
  js/           the QuickJS bridge, the prelude (Qt, Quickshell, console, enums), JS libraries
  object/       object model: type registry use, properties, bindings, signals, life and death
  types/        the built-in types, one file per group
  model/        ListModel, ScriptModel, Repeater, ListView, Loader, Instantiator
  anim/         the animator, Behaviors, Qt's easing curves
  io/           Process, FileView, JsonAdapter, inotify, kdeglobals, XDG paths
  input/        pointer, keyboard focus and Keys, text editing
  scene/        layout, display list, damage, painter, text, effects, Canvas, Shapes, SVG, icons, GPU
  mayland/      windows over libmay: layer surfaces, frames, buffers, capture, the slow-frame log
  services/     Desktop, Sys, Theme, DesktopEntries, Hyprland, empty stand-ins
  ipc/          IpcHandler calls, the local socket, compositor signal words
  check.rs      aosh check
  shot.rs       aosh shot, and the benches
```

**The loop** (`shell.rs`). One thread waits in `poll` on the display socket, every `Process`'s pipes, the inotify descriptor and the IPC socket, for as long as nothing is due (the next `Timer`, the next one-second service sample, the next frame while something animates). Then, in turn: the windows' events, the pointer's and the desktop's; pipes and the watch; the clock (timers, animations, exited processes); `/proc`; requests a script made of the compositor; a reload if a file was saved; and a frame for every window that has something new to show. After the wait the engine's clock is moved to the wake time (`engine.woke`), and animations started by an event are moved to "now" just before they are advanced (`engine.start_new`), as Qt's animation timer does. Before 2026-10-27 an animation started by a key could begin up to a second in the past and be over by its first frame.

**The object model** (`object/`). Every object has a type (walked through its bases: `Rectangle` has everything `Item` and `QtObject` have), property slots, signals and methods. Object ids are a slot plus a generation (`object/table.rs`), so an id held after its object is destroyed names nothing. `object/life.rs` makes definitions at run time and destroys subtrees. A type's starting properties and signals are cached as a template, and each object has a name index, so making an object costs what it costs once.

**Bindings.** A binding is a JavaScript function. While it runs, every property it reads through an accessor is recorded; that set is its dependencies, recomputed on every run, because a binding with a conditional reads different things on different runs. When a property changes, every binding that read it is re-evaluated and every item whose look depends on it is marked dirty. A binding that writes what it reads is stopped and named with its file and line. Assignments from script break a binding as in QML; `Qt.binding(() => ...)` makes a new one. An instance's assignment replaces the binding its file's root gave the same property. Properties written by anchors and layouts never trigger `Behavior`s, as in Qt, where C++ setters bypass the QML interceptor.

**QuickJS integration** (`js/`). Objects are exposed to JavaScript as objects with accessors. Bindings, handlers and functions are compiled once per source location and cached. Errors QuickJS reports are put back at the place in the QML file the code came from. The prelude (`js/prelude/*.js`) defines `Qt`, `Quickshell`, `console`, `Locale`, `StandardPaths`, `FileViewError` and the enum objects.

## The QML language as aosh reads it

| Feature | Support |
|---|---|
| Object tree, `id`, `name: value`, grouped (`border.width`) and grouped blocks (`border { width: 2 }`), attached properties (`Layout.fillWidth`, `Keys.onPressed`, `WlrLayershell.layer`, `ScrollBar.vertical`) | yes |
| `property <type> name[: value]`, `default`, `readonly`, `required` | yes |
| `property alias name: target.path`, `default property alias` | yes |
| `signal name(params)`, `on<Signal>` handlers, `on<Property>Changed` | yes |
| `function name(a: type, b): ret { ... }` (types kept for IPC conversion) | yes |
| `Behavior on prop { ... }` and other `X on prop` value sources | yes |
| `component Name: Type { ... }` inline components | yes |
| `enum Name { A, B = 3 }` | yes, numbered as QML numbers them |
| `pragma Singleton`, `pragma ComponentBehavior: Bound` | read; singletons are made the first time they are named |
| `//@ pragma NativeTextRendering` | yes: Text draws natively instead of from distance fields |
| Imports: `import QtQuick`, `import Quickshell.Io` and other module names | every file has all of aosh's types; the module name is not checked |
| `import qs.modules.common`, `import qs.x as Alias` | the shell's directory is module `qs`; each subdirectory is a module; `Name.qml` is a type |
| `import "dir"`, `import "file.js" as Name`, `.import "x.js" as X`, `.pragma library` | yes; each JS file is evaluated once and its top-level declarations shared |
| Values without semicolons | a value runs to the end of its line unless a bracket is open, the line ends on an operator, or the next line starts with one that can only continue it |

**One deviation:** a singleton is global, nameable from any file whether or not it imports the module. Qt confines it to importers. This only accepts files Qt would refuse.

**Unknown types.** A type aosh does not have is replaced by an inert item that accepts any property and has no signals, so the rest of the file loads. aosh logs, once per type and file, `<file>: there is no type 'X' yet - an inert item stands in for it`.

## QML type reference

Each type has its base's properties as well (`QtObject` → `Item` → ...). "Accepted" means the property or type is read and stored so that configs load, without the behaviour Qt gives it. Unless noted, properties listed are implemented.

### QtQml and core

| Type | Base | Properties | Signals | Methods | Notes |
|---|---|---|---|---|---|
| `QtObject` | — | `objectName` | | `destroy()` | |
| `Connections` | QtObject | `target`, `enabled`, `ignoreUnknownSignals` | | | Handlers for another object's signals; follows `target` |
| `Binding` | QtObject | `target`, `property`, `value`, `when`, `delayed`, `restoreMode` | | | Holds another object's property while `when` (`object/hold.rs`) |
| `Synchronizer` | QtObject | `sourceObject`, `sourceProperty`, `targetObject`, `targetProperty` | `valueBounced()` | | Qt.labs.synchronizer: properties kept equal both ways |
| `Component` | QtObject | | | `createObject()` | |
| `Timer` | QtObject | `interval`, `running`, `repeat`, `triggeredOnStart` | `triggered()` | `start`, `stop`, `restart` | |

### QtQuick items and layouts

| Type | Base | Properties | Signals | Methods | Notes |
|---|---|---|---|---|---|
| `Item` | QtObject | `parent`, `x`, `y`, `z`, `width`, `height`, `implicitWidth`, `implicitHeight`, `visible`, `enabled`, `opacity`, `scale`, `rotation`, `clip`, `focus`, `activeFocus`, `activeFocusOnTab`, `children`, `resources`, `data`, `left`/`right`/`top`/`bottom`/`horizontalCenter`/`verticalCenter`, `anchors.*` (`fill`, `centerIn`, the six edges, `margins`, the four margins, the two centre offsets, `alignWhenCentered`), `Layout.*` (`fillWidth`, `fillHeight`, preferred/minimum/maximum width and height, `alignment`, margins, `row`, `column`, `rowSpan`, `columnSpan`), `Keys.enabled`, `Keys.priority`, `antialiasing`, `smooth`, `transform`, `transformOrigin`, `layer.*` (`enabled`, `effect`, `smooth`, `mipmap`, `samples`, `live`, `textureSize`, `sourceRect`, `format`, `samplerName`), `containmentMask`, `baselineOffset`; `state`, `states`, `transitions` accepted | `Keys.pressed`, `Keys.released`, and `Keys.<key>Pressed(event)` for return, enter, escape, up, down, left, right, tab, backtab, space, delete, asterisk, digit0..digit9 | `forceActiveFocus`, `nextItemInFocusChain`, `mapToItem`, `mapFromItem`, `mapToGlobal`, `mapFromGlobal` | `State`/`Transition` objects are not types (inert) |
| `Rectangle` | Item | `color`, `radius`, `topLeftRadius`, `topRightRadius`, `bottomLeftRadius`, `bottomRightRadius`, `gradient`, `border.width`, `border.color`, `border.pixelAligned` | | | Drawn as Qt Quick hands rectangles to the GPU |
| `Gradient` | QtObject | `orientation`, `stops` | | | |
| `GradientStop` | QtObject | `position`, `color` | | | |
| `Text` | Item | `text`, `color`, `font.*` (`pixelSize`, `pointSize`, `family`, `bold`, `italic`, `weight`, `variableAxes`, `features`, `styleName`, `hintingPreference`, `letterSpacing`, `wordSpacing`, `capitalization`, `underline`, `strikeout`, `kerning`, `preferShaping`), `horizontalAlignment`, `verticalAlignment`, `elide`, `wrapMode`, `lineHeight`, `lineHeightMode`, `textFormat`, `renderType`, `linkColor`, `style`, `styleColor`, `maximumLineCount`, `fontSizeMode`, `minimumPixelSize`, `truncated`, `lineCount`, `contentWidth`, `contentHeight` | | | Distance-field text by default, native with `renderType: Text.NativeRendering` |
| `Image` | Item | `source`, `fillMode`, `sourceSize.width`, `sourceSize.height`, `paintedWidth`, `paintedHeight`, `status`, `smooth`, `asynchronous`, `cache`, `mipmap`, `mirror`, `horizontalAlignment`, `verticalAlignment`, `retainWhileLoading`, `autoTransform`, `progress` | | | PNG, JPEG, WebP, SVG, QOI; `image://icon/` |
| `MouseArea` | Item | `hoverEnabled`, `containsMouse`, `pressed`, `acceptedButtons`, `mouseX`, `mouseY`, `containsPress`, `pressedButtons`, `preventStealing`, `propagateComposedEvents`, `scrollGestureEnabled`, `pressAndHoldInterval`, `drag.*` (`target`, `axis`, `active`, min/max X/Y, `filterChildren`, `smoothed`, `threshold`); `cursorShape` accepted | `clicked`, `doubleClicked`, `pressAndHold`, `pressed`, `released`, `positionChanged` (mouse), `canceled`, `entered`, `exited`, `wheel(wheel)` | | aosh does not set the cursor shape |
| `Row`, `Column` | Item | `spacing` | | | |
| `Flow` | Item | `spacing`, `flow` | | | |
| `Grid` | Item | `spacing`, `rowSpacing`, `columnSpacing`, `columns`, `rows`, `flow`, `horizontalItemAlignment`, `verticalItemAlignment` | | | |
| `RowLayout`, `ColumnLayout` | Item | `spacing` | | | Children's `Layout.*` |
| `GridLayout` | Item | `columns`, `rows`, `flow`, `columnSpacing`, `rowSpacing` | | | |
| `Scale` | QtObject | `xScale`, `yScale`, `origin.x`, `origin.y` | | | For `transform` |
| `Rotation` | QtObject | `angle`, `origin.x`, `origin.y`, `axis.x`, `axis.y`, `axis.z` | | | |
| `Translate` | QtObject | `x`, `y` | | | |
| `Canvas` | Item | `available`, `context`, `contextType`, `canvasSize`, `canvasWindow`, `tileSize`, `renderTarget`, `renderStrategy` | `paint(region)`, `painted()`, `imageLoaded()` | `getContext`, `requestPaint`, `markDirty`, `requestAnimationFrame`, `cancelRequestAnimationFrame`, `loadImage`, `unloadImage`, `isImageLoaded`, `isImageLoading`, `isImageError`, `save`, `toDataURL` | Context2D drawn as Qt's raster engine draws (`scene/canvas/`) |

### Text input and controls

| Type | Base | Properties | Signals | Methods | Notes |
|---|---|---|---|---|---|
| `TextInput` | Item | `text`, `displayText`, `length`, `color`, `font.*`, `renderType`, alignments, `cursorPosition`, `cursorVisible`, `readOnly`, `maximumLength`, `echoMode`, `passwordCharacter`, `activeFocusOnPress`, `acceptableInput`, `autoScroll`, `contentWidth`, `contentHeight`, padding (all sides); selection-related (`selectByMouse`, `persistentSelection`, `selectedText`, `selectionStart`, `selectionEnd`, `selectionColor`, `selectedTextColor`), `inputMethodHints`, `inputMask`, `validator`, `cursorDelegate` accepted | `accepted()`, `editingFinished()`, `textEdited()` | `clear`, `insert`, `remove`, `positionAt`, `ensureVisible`, `getText`, `copy`, `cut`, `paste`, `undo`, `redo`; `select`, `selectAll`, `selectWord`, `deselect` select nothing | Positions count characters; no selection |
| `TextEdit` | TextInput | `wrapMode`, `textFormat`, `lineCount`, `tabStopDistance`, `textMargin`, `baseUrl` | `linkActivated(link)` | | |
| `TextField` | TextInput | `placeholderText`, `placeholderTextColor`, `verticalAlignment`, `background`, `hoverEnabled`, `hovered`, `padding` | | | |
| `TextArea` | TextEdit | `placeholderText`, `placeholderTextColor`, `background`, `hoverEnabled`, `hovered`, `padding` | | | |
| `Control` | Item | paddings and insets, `spacing`, `availableWidth`, `availableHeight`, implicit content and background sizes, `background`, `contentItem`, `hovered`, `hoverEnabled`, `mirrored`, `visualFocus`, `focusPolicy`, `focusReason`, `wheelEnabled`, `palette`, `locale`, `font.*` | | | Controls style: Basic |
| `AbstractButton` | Control | `text`, `down`, `pressed`, `checked`, `checkable`, `autoExclusive`, `autoRepeat`, `display`, `action`, `indicator`, implicit indicator size, `pressX`, `pressY`, `icon.*` (`name`, `source`, `width`, `height`, `color`, `cache`) | `clicked`, `pressed`, `released`, `canceled`, `toggled`, `pressAndHold`, `doubleClicked` | `click`, `toggle`, `animateClick` | |
| `Button` | AbstractButton | `padding`, `spacing`, `flat`, `highlighted` | | | |
| `RoundButton` | Button | `radius` | | | |
| `ToolButton` | Button | | | | |
| `ItemDelegate` | AbstractButton | `padding`, `spacing`, `highlighted` | | | |
| `SwipeDelegate` | ItemDelegate | `spacing`, `swipe.*` (`left`, `right`, `behind`, their items, `position`, `complete`, `enabled`, `transition`) | `swipeCompleted()` | | |
| `ScrollBar` | Control | `padding`, `size`, `position`, `stepSize`, `active`, `pressed`, `orientation`, `snapMode`, `interactive`, `policy`, `horizontal`, `vertical`, `minimumSize`, `visualSize`, `visualPosition` | | `increase`, `decrease` | |

### Models and instantiators

| Type | Base | Properties | Signals | Methods | Notes |
|---|---|---|---|---|---|
| `Repeater` | Item | `model`, `delegate`, `count` | `itemAdded(index, item)`, `itemRemoved(index, item)` | `itemAt` | Delegates beside itself in its parent |
| `ListView` | Item | `model`, `delegate`, `count`, `spacing`, `orientation`, `currentIndex`, `currentItem`, `contentX`, `contentY`, `contentWidth`, `contentHeight`, `topMargin`, `bottomMargin`, `leftMargin`, `rightMargin`, `interactive`, `atYBeginning`, `atYEnd`, `boundsBehavior`, `highlightFollowsCurrentItem`, `keyNavigationEnabled`, `cacheBuffer`, `snapMode`, `highlightMoveDuration`, `highlightRangeMode`, `highlight`, `header`, `footer`, the transition properties (`add`, `remove`, `move`, `populate`, `displaced` and the three `*Displaced`), `maximumFlickVelocity`, `flickDeceleration`, `reuseItems`, `ScrollBar.vertical`, `ScrollBar.horizontal` | | `positionViewAtIndex`, `positionViewAtBeginning`, `positionViewAtEnd`, `incrementCurrentIndex`, `decrementCurrentIndex`, `itemAtIndex`, `indexAt`, `forceLayout` | Walks rows on Up/Down |
| `Instantiator` | QtObject | `model`, `delegate`, `active`, `asynchronous`, `count`, `object` | `objectAdded(index, object)`, `objectRemoved(index, object)` | `objectAt` | Makes objects, shows none |
| `Variants` | QtObject | `model`, `delegate`, `instances` | | | Quickshell's; `model: Quickshell.screens` gives one per screen (one output) |
| `Loader` | Item | `active`, `source`, `sourceComponent`, `item`, `status`, `progress`, `asynchronous` | `loaded()` | `setSource` | |
| `LazyLoader` | QtObject | `active`, `loading`, `activeAsync`, `component`, `source`, `item` | | | For windows, which are not items |
| `ListModel` | QtObject | `count`, `dynamicRoles` | | `append`, `insert`, `remove`, `move`, `clear`, `get`, `set`, `setProperty`, `sync` | |
| `ListElement` | QtObject | any | | | |
| `ScriptModel` | QtObject | `values`, `objectProp` | | | Quickshell's model of a JS list; rows kept across changes |

### Animation

| Type | Base | Properties | Signals | Methods |
|---|---|---|---|---|
| `Behavior` | QtObject | `enabled`, `animation` | | |
| `Animation` | QtObject | `running`, `paused`, `loops`, `alwaysRunToEnd` | `started()`, `stopped()`, `finished()` | `start`, `stop`, `restart`, `pause`, `resume`, `complete` |
| `PropertyAnimation` | Animation | `duration`, `from`, `to`, `target`, `targets`, `property`, `properties`, `easing.type`, `easing.amplitude`, `easing.overshoot`, `easing.period`, `easing.bezierCurve` | | |
| `NumberAnimation`, `ColorAnimation` | PropertyAnimation | | | |
| `RotationAnimation` | PropertyAnimation | `direction` | | |
| `SequentialAnimation`, `ParallelAnimation` | Animation | | | |
| `PauseAnimation` | Animation | `duration` | | |
| `PropertyAction` | Animation | `target`, `targets`, `property`, `properties`, `value` | | |
| `ScriptAction` | Animation | `scriptName`, and its `script` | | |

Easing curves are Qt's (`anim/easing.rs`), including `bezierCurve`.

### Effects

| Type | Base | Properties | Notes |
|---|---|---|---|
| `ShaderEffectSource` | Item | `sourceItem`, `sourceRect.*`, `textureSize.*`, `hideSource`, `live`, `recursive`, `mipmap`, `format`, `wrapMode`, `textureMirroring`, `samples`; signal `scheduledUpdateCompleted()`, method `scheduleUpdate()` | An item rendered into a texture |
| `OpacityMask` | Item | `maskSource`, `invert` | Qt5Compat |
| `FastBlur` | Item | `radius` | Qt5Compat |
| `GaussianBlur` | Item | `radius`, `samples`, `deviation` | Qt5Compat |
| `DropShadow` | Item | `transparentBorder`, `radius`, `samples`, `color`, `horizontalOffset`, `verticalOffset`, `spread` | Qt5Compat |
| `RectangularShadow` | Item | `blur`, `spread`, `offset.x`, `offset.y`, `radius`, four corner radii, `color`, `cached`, `material` | |
| `RadialGradient` | Item | `horizontalOffset`, `verticalOffset`, `horizontalRadius`, `verticalRadius`, `angle`, `gradient` | |
| `MultiEffect` | Item | `source`, `brightness`, `contrast`, `saturation`, `colorization`, `colorizationColor`, mask (`maskEnabled`, `maskSource`, `maskInverted`, thresholds and spreads), `autoPaddingEnabled`, `paddingRect`; blur and shadow properties accepted | Colour and mask are drawn; blur and shadow are not yet (logged once) |

`layer.enabled` and `layer.effect` on any item work as in Qt: the layered item is rendered into a texture and the effect drawn in its place.

### Shapes

| Type | Base | Properties |
|---|---|---|
| `Shape` | Item | `preferredRendererType`, `rendererType`, `asynchronous`, `vendorExtensionsEnabled`, `containsMode`, `fillMode`, `status` |
| `ShapePath` | QtObject | `startX`, `startY`, `strokeColor`, `strokeWidth`, `fillColor`, `fillRule`, `joinStyle`, `miterLimit`, `capStyle`, `strokeStyle`, `dashOffset`, `dashPattern`, `pathHints`, `fillGradient` |
| `PathLine`, `PathMove` | QtObject | `x`, `y` |
| `PathQuad` | QtObject | `x`, `y`, `controlX`, `controlY` |
| `PathCubic` | QtObject | `x`, `y`, `control1X`, `control1Y`, `control2X`, `control2Y` |
| `PathAngleArc` | QtObject | `centerX`, `centerY`, `radiusX`, `radiusY`, `startAngle`, `sweepAngle`, `moveToStart` |
| `PathPolyline` | QtObject | `path` |

Shapes are drawn as Qt's curve renderer draws them (`scene/shapes/`).

### Quickshell: shell, windows and I/O

| Type | Base | Properties | Signals | Methods | Notes |
|---|---|---|---|---|---|
| `ShellRoot`, `Scope` | QtObject | | | | |
| `Singleton` | Scope | | | | Root of a `pragma Singleton` file |
| `PanelWindow` | Item | `anchors.top`/`bottom`/`left`/`right`, `margins.*`, `exclusiveZone`, `color`, `aboveWindows`, `focusable`, `WlrLayershell.layer`, `WlrLayershell.keyboardFocus`, `WlrLayershell.namespace`, `exclusionMode`, `screen`, `mask` | | | A `may_layer` surface; `mask` becomes the input region |
| `Region` | QtObject | `item`, `x`, `y`, `width`, `height`, `intersection`, `shape`, `radius`, `regions` | | | For `PanelWindow.mask` |
| `ShellScreen` | QtObject | `name`, `model`, `serialNumber`, `x`, `y`, `width`, `height`, `devicePixelRatio`, `physicalPixelDensity`, `logicalPixelDensity`, `orientation`, `primaryOrientation` | | | Size follows the output |
| `IconImage` | Image | `implicitSize`, `actualSize`, `backer` | | | Theme icons from `kdeglobals`' icon theme |
| `Icon` | Image | `fallback`, `placeholder`, `color`, `isMask`, `roundToIconSize`, `animated`, `active`, `selected`, `valid` | | | Kirigami's; no mask colouring |
| `Process` | QtObject | `running`, `command`, `workingDirectory`, `environment`, `clearEnvironment`, `stdout`, `stderr`, `stdinEnabled`, `processId` | `started()`, `exited(exitCode, exitStatus)` | `exec`, `signal`, `write`, `startDetached` | Quickshell's semantics: `running` is wish and state |
| `StdioCollector` | QtObject | `text`, `data`, `waitForEnd` | `streamFinished()` | | Output whole |
| `SplitParser` | QtObject | `splitMarker` | `read(data)` | | Output split at a marker |
| `FileView` | QtObject | `path`, `watchChanges`, `preload`, `blockLoading`, `blockAllReads`, `blockWrites`, `atomicWrites`, `printErrors`, `loaded`, `adapter` | `loaded()`, `loadFailed(error)`, `saved()`, `saveFailed(error)`, `fileChanged()`, `adapterUpdated()` | `text`, `data`, `reload`, `setText`, `setData`, `waitForJob`, `writeAdapter` | `watchChanges` over inotify |
| `JsonAdapter`, `JsonObject` | QtObject | declared properties | `adapterUpdated()` (adapter) | | A FileView's JSON as properties |
| `FontLoader` | QtObject | `source`, `name`, `status` | | | |
| `IpcHandler` | QtObject | `target`, `enabled` | | | Its functions are IPC calls |
| `SystemClock` | QtObject | `enabled`, `precision` (`SystemClock.Hours`/`Minutes`/`Seconds`), `date`, `hours`, `minutes`, `seconds` | | | |

`PopupWindow`, `FloatingWindow`, `Flickable`, `GridView`, `State` and `Transition` are not types yet and load as inert items.

### Services (global objects)

These are single objects aosh makes at start and publishes under a global name. The type names are aosh's.

| Global | Type | Properties | Signals | Methods | Backed by |
|---|---|---|---|---|---|
| `Desktop` | `AoshDesktop` | `workspace`, `workspaces` (`[{n, active, busy}]`), `windows` (`[{title, appId, workspace, focused}]`), `focusedTitle`, `screenWidth`, `screenHeight`, `workTop`, `keyboardLayout`, `keyboardLayoutName`, `keyboardLayoutIndex`, `keyboardLayouts` | `signalled(word)` | `switchTo(n)`, `signal(word)` | `may_desktop` |
| `Sys` | `AoshSys` | `memUsed`, `memTotal`, `memPercent`, `netIp` (or `offline`), `uptime`, `uptimeText` | | | `/proc/meminfo`, `/proc/net`, `/proc/uptime`, sampled once a second |
| `Theme` | `AoshTheme` | `page`, `card`, `accent`, `onAccent`, `text`, `textMuted`, `border`, `wallpaper` | | | the `theme { }` block of `/etc/mayland` (see [mayland.md](mayland.md)) |
| `DesktopEntries` | `AoshDesktopEntries` | `applications` (`.values`, each with `execute()`) | | `byId`, `heuristicLookup` | `/etc/mayland/apps.d` |
| `Hyprland` | `AoshHyprland` | `usingLua`, `requestSocketPath`, `eventSocketPath`, `focusedMonitor`, `focusedWorkspace`, `activeToplevel`, `monitors`, `workspaces`, `toplevels` | `rawEvent(event)` | `dispatch(text)`, `monitorFor`, `refreshMonitors`, `refreshWorkspaces`, `refreshToplevels` | `may_desktop`, in Quickshell.Hyprland's shape |
| — | `HyprlandMonitor` | `id`, `name`, `description`, `x`, `y`, `width`, `height`, `scale`, `lastIpcObject`, `activeWorkspace`, `focused` | | | |
| — | `HyprlandWorkspace` | `id`, `name`, `active`, `focused`, `urgent`, `hasFullscreen`, `lastIpcObject`, `monitor`, `toplevels` | | `activate()` | |
| — | `HyprlandToplevel` | `address`, `handle`, `wayland`, `title`, `activated`, `urgent`, `lastIpcObject`, `workspace`, `monitor` | | | |
| — | `HyprlandFocusGrab` | `active`, `windows` | `cleared()` | | `may_focus_grab` |
| `ToplevelManager` | `AoshToplevelManager` | `toplevels`, `activeToplevel` | | | `may_desktop` |
| — | `Toplevel` | `appId`, `title`, `activated`, `maximized`, `minimized`, `fullscreen`, `parent`, `screens`, `HyprlandToplevel` | `closed()` | `activate`, `close`; `setMaximized`, `setMinimized`, `setFullscreen`, `fullscreenOn`, `setRectangle`, `unsetRectangle` accepted | |
| — | `ScreencopyView` (item) | `captureSource`, `live`, `paintCursor`, `hasContent`, `sourceSize.*`, `constraintSize.*` | `stopped()` | `captureFrame()` | `may_desktop.capture`, drawn stretched |
| `Mpris` | `AoshMpris` | `players` (empty) | | | nothing: no D-Bus |
| `UPower` | `AoshUPower` | `displayDevice` (a `UPowerDevice` that is not `ready`), `devices` (empty), `onBattery` | | | nothing |
| `SystemTray` | `AoshSystemTray` | `items` (empty) | | | nothing |
| `Pipewire` | `AoshPipewire` | `nodes`, `links`, `linkGroups` (empty), default sinks and sources (null), `ready` | | | nothing: no audio stack |
| — | `PwObjectTracker` | `objects` | | | |
| `Bluetooth` | `AoshBluetooth` | `adapters`, `devices` (empty), `defaultAdapter` | | | nothing |
| — | `NotificationServer` | the `*Supported` flags, `keepOnReload`, `extraHints`, `trackedNotifications` | `notification` (never fires) | | nothing: no notification protocol |
| — | `PositionSource` | `active`, `valid` (false), `name`, `updateInterval`, positioning methods, `position`, `sourceError` | `validityChanged`, `updateTimeout`, `errorOccurred` | `start`, `stop`, `update` | nothing |

`Hyprland.dispatch` understands what end4-style configs ask, in Hyprland's text form and its Lua form: `workspace N` (and relative forms), `focuswindow`, `closewindow`, `movetoworkspacesilent`, and the Lua `focus`, `window.close` and `window.move`. Anything else is logged (`Hyprland.dispatch: not on MikuOS: ...`) and not done. Each object's `lastIpcObject` is what `hyprctl -j` would say, and changes are emitted as Hyprland's `rawEvent`s (`openwindow`, `closewindow`, `movewindow`, `windowtitle`, `activewindow`, `workspace`, their `v2` forms, and `activelayout`).

Services with nothing behind them report what Quickshell reports where the service is not running, so a config shows no tray, no volume and no battery rather than invented values.

## Global objects and the JavaScript environment

The JavaScript is QuickJS (ES2023-level, as `rquickjs` 0.14 provides it). On top of the language, aosh defines:

| Name | Contents |
|---|---|
| `Qt` | `color`, `rgba`, `rgb`, `hsva`, `hsla`, `colorEqual`, `alpha`, `lighter`, `darker`, `tint`, `point`, `size`, `rect`, `vector2d`/`3d`/`4d`, `callLater`, `formatDateTime`, `formatTime`, `formatDate`, `locale`, `binding`, `resolvedUrl` (per file, against the file's directory), `md5`, `quit`, `openUrlExternally`, and the constants (alignments, orientations, layout directions, date formats, mouse buttons, modifiers, cursor shapes, `transparent`, `Key_*` key numbers) |
| `Quickshell` | `env(name)` (null when unset), `execDetached(cmd)` (an array is argv; a string runs through `/bin/msh -c`), `shellDir`, `shellRoot`, `configDir`, `shellPath(p)`, `configPath(p)`, `iconPath(icon, fallback)`, `hasThemeIcon(icon)`, `screens`, `clipboardText` (a plain string: no system clipboard is wired), `reload()` (logs only), `inhibitReloadPopup()` |
| `console` | `log`, `info`, `debug` (printed), `warn`, `error` (logged as `warn: ...` / `error: ...`) |
| `StandardPaths` | the location constants and `standardLocations(kind)`, `writableLocation(kind)`, as `file://` URLs |
| `FileViewError` | Quickshell.Io's error numbers and `toString` |
| `Locale` | Qt's date and time format letters (`js/prelude/locale.js`) |
| Enum objects | `Text`, `TextInput`, `TextField`, `TextEdit`, `TextArea`, `Image`, `Loader`, `Component`, `ListView`, `Flickable`, `Grid`, `GridLayout`, `Flow`, `Font`, `Easing`, `Animation`, `RotationAnimation`, `Item`, `Drag`, `Binding`, `Gradient`, `Canvas`, `ScrollBar`, `AbstractButton`, `Button`, `FontLoader`, `SystemClock`, `WlrLayer` (`Background` 0, `Bottom` 1, `Top` 2, `Overlay` 3), `WlrKeyboardFocus` (`None` 0, `Exclusive` 1, `OnDemand` 2), `ExclusionMode` (`Normal` 0, `Ignore` 1, `Auto` 2), `MprisPlaybackState` |
| Services | `Desktop`, `Sys`, `Theme`, `DesktopEntries`, `Hyprland`, `ToplevelManager`, `Mpris`, `UPower`, `SystemTray`, `Pipewire`, `Bluetooth` (table above) |

Programs started by `Process` or `execDetached` run without a shell (unless the string form of `execDetached` is used) and without `CAP_MAY_PRIV`, so a program opened from the launcher cannot cover the screen or read the keyboard. A `Process`'s pipes are non-blocking and read only when `poll` says so.

## Layout, the display list and damage

A frame of a window has two passes (`scene/mod.rs`):

1. **Layout** writes geometry back into the items' properties, as Qt Quick does: a `Text` is as wide as its text unless told otherwise, a `Row` places its children, anchors and layouts size what they hold. Writing a property runs whatever binding reads it, so the pass repeats until a round moves nothing (with a cap on rounds, after which it says it would not settle).
2. **The display list** is every visible item that paints, in paint order, with a fingerprint of what it would draw. It is compared with the last frame's list, and only the rectangles of what changed are painted again.

Both passes are incremental since 2026-10-21: every object carries `changed` and `below` marks (`Core::touch`), layout and the list skip clean subtrees, and the list keeps per-item runs (`WinScene::nodes`). Under `cfg(test)` every partial layout and list is checked against a whole one; `AOSH_SCENE_CHECK=0` turns that off for timing.

**Damage.** List entries are matched by identity, not by position, so an item appearing or disappearing does not make every item after it look changed (before 2026-10-20 a press on a widget repainted all 1920x1080 of the background, 109 ms). A window's damage is at most 8 rectangles (`scene::Damage`); a flat `Rectangle` that moved or grew costs only the difference and its edges. A commit's damage is what the frame painted. Window children are sorted by `z`.

## Text

`scene/text/` lays out and draws text as Qt does on the developer's Arch machine, matched to the 64th of a pixel for metrics and pixel for pixel for native glyphs (since 2026-10-08):

- **Face selection** is what Qt's font database would pick: the family's style of the given `styleName`, else the nearest weight. A named instance of a variable face is its own style; `font.variableAxes` is set over it. A weight of 700 or more on a face that is not bold is emboldened synthetically.
- **The application font** comes from `kdeglobals` `[General] font`, as Quickshell under `QT_QPA_PLATFORMTHEME=kde` reads it. A Text's own font properties are applied over it.
- **Shaping** is rustybuzz (`shape.rs`), as Qt's HarfBuzz shapes: default features (ligatures, kerning, contextual forms), direction and script guessed from the text, at the named instance fontconfig would list (Material Symbols ligatures such as `planner_review` become one glyph here).
- **Advances** are FreeType's unhinted ones at the face's axes, in 64ths, with GPOS kerning applied as HarfBuzz rounds it (`kern.rs`). A line is `ascent + descent` from `hhea`, then OS/2.
- **Glyph pixels** come from FreeType 2.14.3 (`lib/freetype`, with a C shim making the calls Qt's FreeType engine makes, and `lib/freetype/src/hb.rs` answering the autohinter's HarfBuzz calls) at light hinting.
- **Distance-field text.** A Text without `renderType: Text.NativeRendering`, in a shell without `//@ pragma NativeTextRendering`, is drawn from distance fields, Qt Quick's default (`field.rs`, since 2026-10-13): the hinted outline at units-per-em scaled to a 54 px base, a field covering the box plus 5 px, and a smoothstep threshold. The shipped bar uses native rendering.
- A character the face lacks is drawn from the first face that has it.

## The painter and effects

`scene/paint.rs`, `scene/rectangle.rs` and `scene/gpu/raster.rs` paint what Qt Quick hands its GPU renderer: rectangles as Qt's geometry (rounded edges as triangle runs), images and glyphs as textured quads with filtering in 256ths, native glyphs placed per item as Qt's text shader places them. The painter blends in floating point like a desktop GPU; on `rects.qml` it is within 1/255 of `qml6` (the remaining 1/255 is the reference renderer's 8-bit blending).

- **Canvas** (`scene/canvas/`) is a Context2D drawn as Qt's raster engine draws it, including its coarse curve flattening and wide-line offsetting; within 2/255 on `canvases.qml`. Fills use FreeType's rasteriser.
- **Effects** (`scene/effect/`) are Qt's shader passes done pass for pass: an item or layer is rendered into a texture, each pass computes what Qt's shader computes (read from Qt's compiled shaders, written afresh), and the last pass is a quad in the scene. Every texture and intermediate pass is cached under a print of what it was made from. Within 2/255 on `effects.qml`.
- **SVG** (`scene/svg/`) is drawn with the Canvas raster engine; **icons** (`scene/icons.rs`) come from the icon theme `kdeglobals` names; **pictures** (`scene/picture.rs`) are decoded with the `image` crate (PNG, JPEG, WebP) and QOI.

Effects of a whole window or layer (blur behind, rounded window corners, shadows) are the compositor's, asked for through maywm rules. Effects inside a surface are aosh's.

## Drawing on the card

Since 2026-10-21 (`dev/aosh-gpu.md`), a window can be drawn by the kernel's built-in `paint` GPU program (`kernel/drivers/gpu/nvidia/shaders/paint.cu`) into a buffer in video memory that the compositor reads in place.

- **The programs are the kernel's**, built beside the compositor's shaders, so aosh runs them without `CAP_GPU`. Every buffer is passed with its size (`PARAM_BUFFER_SIZED`) and every read and write is bounds-checked in the program.
- **One launch per frame** carries the commands, the textures and up to four pools of window buffers. `paint.cu` runs a block per 16x16 tile of damage and a thread per pixel, executing the tile's operations in order (`FILL`, `TRIS`, `QUAD`, `QUAD_TURNED`, `SHADED`, `SHADED_TURNED`, `MASK`, `STYLED`) with the same arithmetic as the processor painter (`--fmad=false`, no fused multiply-add). Shaders with a GPU form: rectangular shadow, opacity mask, distance-field ink, the blur's mix and Gauss, radial gradient, MultiEffect.
- **The window's two buffers** are cut from a `gpu_alloc` pool; each commit carries the launch's fence with `may_surface.set_fence` instead of waiting.
- **The processor still does** the layout and the list, the recording (corner placement, taps), and textures made of other things (a layer's contents, an effect's earlier passes, a Canvas, a Shape, a glyph mask), each uploaded once and cached in VRAM.
- **Fallback.** A rectangle of damage that cannot be fully recorded is painted on the processor and written into the buffer. The whole window is drawn on the processor in QEMU (aosh logs `drawing on the processor: there is no card to draw on`), with a kernel without `paint`, when `gpu_alloc` or a launch is refused, or after `aosh ipc call aosh render cpu`. `aosh ipc call aosh render gpu` switches back, and `aosh ipc call aosh render` says which is asked for.

Built-in programs may not use a stack, local memory or constant bank 2; the build refuses them (`lib/buildgen/shadergen.rs`). See [../Nvidia_gb206.md](../Nvidia_gb206.md) for GPU programs.

## IPC: IpcHandler and `aosh ipc`

An `IpcHandler { target: "name"; function f(a: int, b: string): string {...} }` offers its functions to the outside. Arguments arrive as strings and are converted to the declared parameter types (`int`, `real`, `bool`, `string`, `color`); the return value comes back as text.

Two ways in:

1. **A compositor signal word.** `bind = $mod, D, signal, launcher toggle` in maywm's config, or `mayland signal launcher toggle`, sends the words on `may_desktop`. aosh treats `target function args...` as a call: `launcher toggle` is `launcher.toggle()`.
2. **`aosh ipc call <target> <function> [args...]`** over a local socket at `$XDG_RUNTIME_DIR/aosh/ipc.sock`, or `~/.cache/aosh/ipc.sock` where that is unset (as on MikuOS). A config's shell (`aosh -c NAME`) listens on `NAME.sock` beside it; reach it with `aosh ipc -c NAME ...`. `aosh ipc show` lists targets and functions.

Wire format: the request's fields (`call`, target, function, arguments, or `show`) joined by NUL after a four-byte length; the answer is a status byte (`0` value, `1` error) and text until the server closes. Only the same user's programs are answered: the socket directory is 0700 and the server checks the peer's uid (`SO_PEERCRED`). A client that stalls costs the shell at most 200 ms.

The shell's own target `aosh` has `render [cpu|gpu]`. Errors read like `no target 'x'`, `target 'x' has no function 'y'`, `f takes 1 argument(s), not 2`.

## The launcher, the keyboard and the overview

**The launcher** (`/etc/aosh/Launcher.qml`) is a `PanelWindow` in the `Overlay` layer with `WlrKeyboardFocus.Exclusive`. It lists `DesktopEntries.applications.values`, filters them as you type, walks the list with Up and Down, starts the selected entry on Return (`entry.execute()`) and closes on Escape. Its `IpcHandler` has target `launcher` with `toggle`, `open` and `close`; Super+D in the shipped config sends `signal, launcher toggle`.

**Keyboard focus** (`input/keys.rs`) is Qt Quick's, without `FocusScope`: a window has one focus item, the last item given `focus` (by the file, a binding or `forceActiveFocus()`); it has `activeFocus` while its window has the keyboard, which the compositor gives. A key goes to the focus item, then up its parents to the window, until one accepts it: first its `Keys.on<Key>Pressed` (whose event starts accepted), then `Keys.onPressed` (whose event does not), then what the item does itself (a `TextInput` edits, a `ListView` walks rows). Keycodes from the compositor are mapped to Qt's key numbers. `TextInput` positions count characters, and Cyrillic and other layouts arrive through the seat's keymap ([input-and-terminals.md](input-and-terminals.md)).

**The overview** is part of the negi config, not the default shell: it opens on a tap of Super (the release binding negi's key file ships), on `signal, search toggle` or an `aosh ipc call search toggle`, shows the workspaces with live window thumbnails from `ScreencopyView` (`may_desktop.capture`), and closes on Escape or a click beside it through a `HyprlandFocusGrab`.

## Running aosh on the Linux host

aosh builds and tests on the build host for development.

```
cd aosh
cargo test-host                         # the host test suite (alias in aosh/.cargo/config.toml)
cargo build-host                        # a host binary, target/x86_64-unknown-linux-gnu/debug/aosh
```

The aliases exist because the repository root's cargo config targets the kernel; `test-host` is `test --target x86_64-unknown-linux-gnu -Z build-std=std,panic_unwind,proc_macro,test`. The builder's overlay must have been assembled once (`cd builder && cargo run --release -- overlay`, or any image build), because the `rquickjs-sys` and `libc` patches point into `target/rust-src-miku/`.

**The host safety rule.** A config loaded on the host is somebody's desktop, and its start-up commands would run on the host: on 2026-10-07 the first host load of end4's config ran `rm -rf /tmp/quickshell/...` and `hyprctl hyprsunset identity` against the developer's live session. So on any system but MikuOS, outside aosh's own tests, aosh refuses to start programs (it logs `not started on this host (AOSH_HOST_EXEC=1 would): <argv>`) and refuses to write files outside `$HOME`. An environment variable lifts both refusals; do not set it. Point `HOME` at a scratch copy instead.

**Loading a config on the host:**

```
cp -r personal/home "$SCRATCH/home"
HOME="$SCRATCH/home" aosh/target/x86_64-unknown-linux-gnu/debug/aosh eval etc/aosh/negi/shell.qml 'Config.ready'
```

It prints the expression's value and the load's complaints, and refuses every program the config would start.

**`aosh shot`** renders a shell to a PNG with no display server:

```
aosh shot <shell.qml> [--home D] [--map FROM=TO]... [--size 1920x1080]
          [--time 2026-10-07T09:05:00] [--settle 1500] [--out shot.png]
          [--geometry where.json] [--profile]
          [--against ref.png --crops DIR [--ignore-top 40]]
          [--call "target function args"]... [--parts Type,Type...]
          [--window "ws,x,y,w,h,app_id,title"]... [--workspace N] [--type TEXT]
```

- Windows are placed as a layer-shell compositor would (anchors, margins, exclusive zones, band order).
- `--time` fixes what `new Date()` says; the engine's clock is stepped a frame at a time for `--settle` ms.
- `--home D` is copied to a scratch HOME first and removed afterwards, so what the shell writes never reaches D.
- `--map FROM=TO` reads an image directory from the tree (`--map /etc/aosh/negi-personal=personal/assets`).
- The environment is a MikuOS session's (PATH, HOME, USER, LOGNAME, SHELL, `XDG_CURRENT_DESKTOP=Mayland`) so the host's Hyprland variables do not leak in.
- `--against` with `--crops` writes each widget's pair and their difference; `--geometry` dumps where every item landed.
- `--window`, `--workspace`, `--call` and `--type` simulate windows, a workspace, IPC calls and typing.
- What a compositor adds (blur behind a layer, window frames) is not in a shot.

**qtref: comparing with Qt.** `aosh/tests/qtref/make.sh [name]` has `qml6` render each fixture in `aosh/tests/qtref/fixtures/` (18 files: `advances`, `bigtext`, `canvases`, `centring`, `controls`, `dftext`, `effects`, `gradients`, `labels`, `layouts`, `multieffect`, `pictures`, `rects`, `shapes`, `text`, `textedit`, plus `pattern.png` and `svg/`) as Quickshell renders on the Arch machine: through Qt's RHI on OpenGL under `xvfb-run` (the offscreen platform's software renderer draws large text differently), with `QT_QPA_PLATFORMTHEME=kde`, Controls style Basic and grey distance-field antialiasing. Output goes to `target/qtref/<name>.png`, plus `<name>.json` where a fixture reports numbers. Host tests read them and pass over a fixture whose render is missing. It needs Qt 6 and qt6-5compat on the host.

## The negi desktop

`etc/aosh/negi/` is a fork of end4-pC (upstream revision `009cec4`), the Quickshell config the user runs on Arch: Material colours, a widget canvas on the wallpaper, a bar, sidebars, an overview. It runs as `aosh -c negi`. Edits to upstream files carry `// MikuOS: <why>` and are listed in [../../dev/aosh-negi.md](../../dev/aosh-negi.md). Upstream's GPL-3 `LICENSE` stays in the directory.

- Personal files (stickers, notes, `config.json`, colours, fonts, a password hash) live in a git-ignored `personal/` directory. When it exists, the builder makes a `miku-personal` package (`builder/src/pkg.rs`, `personal_into`) that ships `assets/` to `/etc/aosh/negi-personal/`, `home/` to `/etc/skel/`, `hostname` to `/etc/hostname`, `/etc/aosh/default` containing `negi` (so the one `aosh` the session starts runs negi in place of the shipped bar), and `/etc/mayland/default.d/70-negi.conf` with negi's keys: a tap of Super (`bindrn = SUPER, Super_L, signal, search toggle`, and `Super_R`), Super+D for `search toggle` and Super+Tab for `search workspacesToggle`, unbinding the shipped Super+D and Super+Tab first. A `personal/shadow` line replaces the locked `alunwrd` line in `/etc/shadow`. Without `personal/` the image has the default bar only. See [building.md](building.md). (`dev/aosh-negi.md` describes an earlier arrangement in which negi ran beside the bar from `session.d/20-negi.conf`; the builder no longer does that.)
- `mikudod` copies `/etc/skel` into a home at its first login each boot (the root filesystem is a tmpfs).
- Services MikuOS lacks stay empty; widgets show what upstream shows when a service is empty.

Status: negi ran in QEMU on 2026-10-13 and matched the host shot (mean differences 0.0 to 5.9 per widget, the larger ones live values such as the time). Overview, workspace switching and bar interactions passed QEMU checks on 2026-10-14. The card showed it smooth on 2026-10-21 after the GPU path; the wallpaper flash seen then was fixed the same day and has not been re-run on the card.

## Tests

`cd aosh && cargo test-host` runs 187 `#[test]` functions:

| File | What it covers |
|---|---|
| `scene/tests.rs` (40) | Layout, display list, damage, rectangles against `rects.qml` (`rectangles_are_qts`), canvases and effects against qtref |
| `object/tests.rs` (32) | Properties, bindings, dependency tracking, signals, ids, destruction, aliases, singletons |
| `model/tests.rs` (13) | ListModel, Repeater, ListView, Loader, ScriptModel |
| `io/tests.rs` (9) | Process semantics, FileView, JsonAdapter, the host refusals |
| `anim/tests.rs` (9), `anim/easing.rs` (3) | Animations, Behaviors, Qt's curves |
| `input/tests.rs` (7) | Focus, Keys, TextInput editing |
| `scene/text/tests.rs` (6) and text modules | Metrics and pixels against Qt (`pixels_are_qts`), distance fields, kerning |
| `qml/parser.rs` (6) | Parsing, value termination |
| `ipc/tests.rs` (3) | Calls, argument conversion, the socket |
| `shot.rs` (6, `#[ignore]`) | Benches: `notes_flip_cost`, `desktop_drag_cost`, `desktop_session_cost`, `overview_open_cost`, `workspace_switch_cost`, `desktop_select_cost` (need `personal/`) |
| others | Canvas strokes and shapes, SVG paths, pictures, icons, blur, apps.d reading, Hyprland objects, JS libraries |

Environment switches:

| Variable | Effect |
|---|---|
| `AOSH_GPU_CHECK=1` | Compile `paint.cu` with nvcc and run every paint of every scene test (and the benches with `-- --ignored`) on the host's NVIDIA card through `libcuda`, comparing pixel for pixel with the processor painter |
| `AOSH_BENCH_GPU=1` | Time bench frames the way MikuOS draws them on the card |
| `AOSH_BENCH_GPU_SPLIT=1` | Split out the card's part of that time |
| `AOSH_SCENE_CHECK=0` | Skip the check of incremental layout against a whole layout (for timing) |

Benches: `CARGO_PROFILE_DEV_OPT_LEVEL=3 cargo test-host --target-dir ../target/aosh-bench bench:: -- --ignored --nocapture --test-threads 1`.

`aosh check etc/aosh` parses and compiles a config without running it. See [testing.md](testing.md) for how this fits with the other suites.

## Performance

Measured on the build host (same CPU as the card machine), mean frame cost:

| Case | Processor, before 2026-10-21 | Processor, only what changed | On the card (card's part) |
|---|---|---|---|
| Notes card flipping | 11.8 ms | 5.7 ms | 1.3 ms (0.2) |
| A selection dragged | 9.0 ms | 3.8 ms | 1.6 ms (0.15) |
| The overview opening | 25 ms | 16 ms | 1.3 ms (1.1) |

At 240 Hz the frame budget is 4.17 ms. Opening the overview's object tree (`search toggle`) went from 50 to 31 ms on 2026-10-24 by caching sources per file, a name index per object and a template per type; what remains is QuickJS evaluating bindings. The release build is `opt-level = 3` because the painter runs over every pixel drawn.

**The slow-frame log** (`mayland/slow.rs`): a window's frame longer than 6 ms (a frame and a half at 240 Hz) is reported at most once a second, with its layout, paint and copy times, how much was painted, and whether it was drawn `on the card` or `on the processor`. A reason a rectangle was left to the processor is said once. Read them with `mikudo dmesg -g "slower than"`.

## Debugging

- **Logs.** On MikuOS, `aosh:` lines go to the kernel log (`miku_klog`), because a session program's stderr is nobody's terminal; read them with `dmesg` (see [diagnostics.md](diagnostics.md)). On the host they go to stderr. `console.log` prints; `console.warn`/`error` and load problems are logged.
- **Useful lines.** `<path> is read, and the display server answered`, `<dir> is followed: a shell.qml there takes over`, `<path> reloaded`, `the reload did not load, so what was up stays up: ...`, `there is no type 'X' yet - an inert item stands in for it`, `MultiEffect's blur and shadow are not drawn yet`, `signal '<word>': <error>`, `aosh ipc will not reach this shell: ...`, `drawing on the processor: ...`.
- **Without a window.** `aosh run file.qml` runs timers and processes; `aosh eval file.qml 'expr'` inspects state; `aosh check dir` finds parse and compile errors.
- **Comparing with Qt.** Render the same fixture with `make.sh` and compare (`cargo test-host pixels_are_qts -- --nocapture`).
- **Card vs processor.** `aosh ipc call aosh render cpu` and `gpu` in one boot.

## Known gaps compared with Quickshell

- `PopupWindow` and `FloatingWindow` are not implemented (Mayland popups need an xdg parent; configs draw popups in an overlay `PanelWindow`).
- `State`, `Transition`, `Flickable` and `GridView` are not types; `FocusScope` is not modelled.
- `MultiEffect` blur and shadow are not drawn.
- No text selection in `TextInput`/`TextEdit`; no cursor shapes; `Quickshell.clipboardText` is not the system clipboard.
- Notifications, system tray, MPRIS, PipeWire, UPower, Bluetooth and positioning have nothing behind them (no D-Bus, no audio stack, no notification protocol).
- One screen: `Quickshell.screens` has the one composed output.
- Distance-field text for very thin fonts (which Qt fields at 108 px) and outlined/raised text in DF mode are drawn natively.
- Module imports are not checked: every file sees every type, and singletons are global.

## Code map

| Path | Contents |
|---|---|
| `aosh/src/main.rs`, `shell.rs` | Command line, config choice, the loop, reload |
| `aosh/src/qml/` | `lexer.rs`, `parser.rs`, `ast.rs`, `import.rs` |
| `aosh/src/js/` | `mod.rs` (bridge), `library.rs` (JS imports), `md5.rs`, `prelude/*.js` |
| `aosh/src/object/` | `mod.rs`, `build.rs`, `engine.rs`, `table.rs`, `life.rs`, `hold.rs`, `sync.rs`, `value.rs` |
| `aosh/src/types/` | The type tables: `core`, `item`, `controls`, `effects`, `model`, `anim`, `io`, `shell`, `services`, `hyprland`, `wayland`, `shapes` |
| `aosh/src/model/` | `list.rs`, `loader.rs`, `script.rs` |
| `aosh/src/anim/` | Animator and easing |
| `aosh/src/io/` | `process.rs`, `fileview.rs`, `json.rs`, `kdeglobals.rs`, `xdg.rs`, inotify |
| `aosh/src/input/` | `pointer.rs`, `keyboard.rs`, `keys.rs`, `text.rs` |
| `aosh/src/scene/` | `mod.rs` (layout, list, damage), `paint.rs`, `rectangle.rs`, `text/`, `effect/`, `canvas/`, `shapes/`, `svg/`, `icons.rs`, `picture.rs`, `gpu/` (`raster`, `record`, `device`, `check`) |
| `aosh/src/mayland/` | `mod.rs` (windows), `capture.rs`, `slow.rs` |
| `aosh/src/services/` | `mod.rs`, `apps.rs`, `hyprland.rs`, `capture.rs` |
| `aosh/src/ipc/` | `mod.rs`, `socket.rs` |
| `aosh/src/check.rs`, `shot.rs` | `aosh check`, `aosh shot` and benches |
| `aosh/tests/qtref/` | Qt reference renders |
| `lib/freetype/` | FreeType 2.14.3 and its shim |
| `kernel/drivers/gpu/nvidia/shaders/paint.cu` | The GPU painter |
| `etc/aosh/` | Default config and `negi/` |

Design notes: [../../dev/aosh-architecture.md](../../dev/aosh-architecture.md), [../../dev/aosh-gpu.md](../../dev/aosh-gpu.md), [../../dev/aosh-negi.md](../../dev/aosh-negi.md).
