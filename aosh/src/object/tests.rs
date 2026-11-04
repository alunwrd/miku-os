// Object model tests against QML's rules, on the host. Each case is a small
// file and its expected result

use std::path::Path;

use super::engine::Engine;
use super::value::Value;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

fn ev(e: &Engine, root: u32, expr: &str) -> String {
    e.eval(root, expr).unwrap_or_else(|err| panic!("{expr}: {err}"))
}

/// `...args` is an array of all remaining arguments
#[test]
fn a_rest_parameter_is_the_rest_of_the_arguments() {
    let (e, r) = load(r#"
        Item {
            function count(first, ...rest) { return rest.length }
            function pass(...args) { return [].concat(...args).join("+") }
        }
    "#);
    assert_eq!(ev(&e, r, "count(1, 2, 3)"), "2");
    assert_eq!(ev(&e, r, "pass('ss', ['a', 'b'])"), "ss+a+b");
}

#[test]
fn bindings_follow_what_they_read() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int x: 2
            property int y: x * 10
            property string label: "x is " + x
            width: y + 1
        }
    "#);
    assert_eq!(ev(&e, r, "y"), "20");
    assert_eq!(ev(&e, r, "label"), "x is 2");
    assert_eq!(ev(&e, r, "width"), "21");
    e.set(r, "x", Value::Num(5.0)).unwrap();
    assert_eq!(ev(&e, r, "y"), "50");
    assert_eq!(ev(&e, r, "width"), "51");
    assert_eq!(ev(&e, r, "label"), "x is 5");
    assert!(e.take_log().is_empty());
}

/// Delegate properties are bare names inside it, followed by the file root's
/// (context object, then outer context)
#[test]
fn a_delegates_root_is_its_scope() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int shared: 7
            property int seen: 0
            Repeater {
                model: 2
                delegate: Item {
                    id: d
                    required property int index
                    property bool mine: index == 1
                    property int shared: 100
                    Item {
                        property int got: (mine ? 10 : 0) + shared + seenOuter
                        property int seenOuter: root.seen
                        Component.onCompleted: root.seen += got
                    }
                }
            }
        }
    "#);
    // 100 + 0, then 10 + 100 + 100: the delegate's `shared` hides the file's
    assert_eq!(ev(&e, r, "seen"), "310");
}

/// What a Loader made is asked what it has, as end4's bar asks
#[test]
fn an_object_owns_its_properties() {
    let (e, r) = load(r#"
        Item {
            id: root
            property bool mirrored
            function f() {}
        }
    "#);
    assert_eq!(ev(&e, r, "root.hasOwnProperty('mirrored')"), "true");
    assert_eq!(ev(&e, r, "root.hasOwnProperty('width')"), "true");
    assert_eq!(ev(&e, r, "root.hasOwnProperty('f')"), "true");
    assert_eq!(ev(&e, r, "root.hasOwnProperty('nope')"), "false");
}

/// An Item's `children` lists only Items, not Behaviors
#[test]
fn children_are_the_items_held() {
    let (e, r) = load(r#"
        Item {
            id: root
            Behavior on width { NumberAnimation {} }
            Timer {}
            Item { id: inner; implicitWidth: 19 }
            property real first: children.length > 0 ? children[0].implicitWidth : 0
        }
    "#);
    assert_eq!(ev(&e, r, "root.children.length"), "1");
    assert_eq!(ev(&e, r, "root.first"), "19");
    assert_eq!(ev(&e, r, "root.data.length > root.children.length"), "true");
    assert_eq!(ev(&e, r, "root.resources.indexOf(inner)"), "-1");
}

/// Attached `QsWindow`: `root.QsWindow.window?.screen`
#[test]
fn an_item_knows_its_window() {
    let (e, r) = load(r#"
        import Quickshell
        Item {
            PanelWindow {
                id: win
                Item { Item { id: deep } }
            }
            Item { id: loose }
            property bool found: deep.QsWindow.window === win
            property bool none: loose.QsWindow.window === null
        }
    "#);
    assert_eq!(ev(&e, r, "found"), "true");
    assert_eq!(ev(&e, r, "none"), "true");
}

/// A file's `property real scale` is its own number; the item draws at its own
/// scale
#[test]
fn a_declared_property_shadows_the_types() {
    let (e, r) = load(r#"
        Item {
            id: root
            property real scale: 0.18
            property real twice: scale * 2
            Item { id: inner; scale: root.scale }
        }
    "#);
    assert_eq!(ev(&e, r, "root.scale"), "0.18");
    assert_eq!(ev(&e, r, "root.twice"), "0.36");
    assert_eq!(e.core.num(r, "scale"), 1.0, "the item is drawn at its own scale");
    assert_eq!(ev(&e, r, "inner.scale"), "0.18", "a child that sets its own scale from it has it");
}

/// Pushing to a list property counts as a change
#[test]
fn a_list_pushed_onto_is_written_back() {
    let (e, r) = load(r#"
        Item {
            id: root
            property list<var> dismissable: []
            property bool any: dismissable.length > 0
            function add(w) { if (root.dismissable.indexOf(w) === -1) root.dismissable.push(w) }
            function drop(w) { const i = root.dismissable.indexOf(w); if (i !== -1) root.dismissable.splice(i, 1) }
        }
    "#);
    ev(&e, r, "root.add(root)");
    assert_eq!(ev(&e, r, "root.dismissable.length + ' ' + root.any"), "1 true");
    ev(&e, r, "root.add(root)");
    assert_eq!(ev(&e, r, "root.dismissable.length"), "1");
    ev(&e, r, "root.drop(root)");
    assert_eq!(ev(&e, r, "root.dismissable.length + ' ' + root.any"), "0 false");
}

#[test]
fn a_condition_changes_what_is_read() {
    let (e, r) = load(r#"
        Item {
            property bool useA: true
            property int a: 1
            property int b: 2
            property int pick: useA ? a : b
        }
    "#);
    assert_eq!(ev(&e, r, "pick"), "1");
    e.set(r, "b", Value::Num(20.0)).unwrap();
    assert_eq!(ev(&e, r, "pick"), "1");
    e.set(r, "useA", Value::Bool(false)).unwrap();
    assert_eq!(ev(&e, r, "pick"), "20");
    e.set(r, "b", Value::Num(30.0)).unwrap();
    assert_eq!(ev(&e, r, "pick"), "30");
}

#[test]
fn ids_children_and_parent() {
    let (e, r) = load(r##"
        Item {
            id: root
            width: 100
            Rectangle {
                id: inner
                width: parent.width / 2
                color: "#f00"
            }
            property real innerWidth: inner.width
        }
    "##);
    assert_eq!(ev(&e, r, "innerWidth"), "50");
    assert_eq!(ev(&e, r, "inner.color"), "#ff0000");
    e.set(r, "width", Value::Num(300.0)).unwrap();
    assert_eq!(ev(&e, r, "innerWidth"), "150");
    assert_eq!(ev(&e, r, "children.length"), "1");
}

#[test]
fn an_assignment_breaks_the_binding() {
    let (e, r) = load(r#"
        Item {
            property int a: 1
            property int b: a + 1
            function setB() { b = 100 }
        }
    "#);
    assert_eq!(ev(&e, r, "b"), "2");
    ev(&e, r, "setB()");
    assert_eq!(ev(&e, r, "b"), "100");
    e.set(r, "a", Value::Num(7.0)).unwrap();
    assert_eq!(ev(&e, r, "b"), "100");
}

#[test]
fn signals_handlers_and_change_handlers() {
    let (e, r) = load(r#"
        Item {
            property int count: 0
            property int seen: 0
            property string last: ""
            signal picked(int index, string what)
            onPicked: { count += index; last = what }
            onCountChanged: seen++
        }
    "#);
    ev(&e, r, "picked(2, 'a')");
    ev(&e, r, "picked(3, 'b')");
    assert_eq!(ev(&e, r, "count"), "5");
    assert_eq!(ev(&e, r, "last"), "b");
    assert_eq!(ev(&e, r, "seen"), "2");
}

#[test]
fn a_parameter_is_not_hidden_by_a_property() {
    let (e, r) = load(r#"
        Item {
            property int index: 99
            property int got: 0
            signal picked(int index)
            onPicked: got = index
        }
    "#);
    ev(&e, r, "picked(4)");
    assert_eq!(ev(&e, r, "got"), "4");
}

#[test]
fn arrow_handlers_and_completed() {
    let (e, r) = load(r#"
        Item {
            property int v: 0
            property bool done: false
            signal go(int n)
            onGo: (n) => { v = n * 2 }
            Component.onCompleted: done = true
        }
    "#);
    assert_eq!(ev(&e, r, "done"), "true");
    ev(&e, r, "go(21)");
    assert_eq!(ev(&e, r, "v"), "42");
}

#[test]
fn groups_and_attached() {
    let (e, r) = load(r#"
        Item {
            id: root
            width: 40
            Text {
                id: t
                font.pixelSize: root.width / 2
                anchors.left: parent.left
                Layout.fillWidth: true
            }
            property int size: t.font.pixelSize
        }
    "#);
    assert_eq!(ev(&e, r, "size"), "20");
    assert_eq!(ev(&e, r, "t.Layout.fillWidth"), "true");
    e.set(r, "width", Value::Num(60.0)).unwrap();
    assert_eq!(ev(&e, r, "size"), "30");
    ev(&e, r, "t.font.pixelSize = 9");
    assert_eq!(ev(&e, r, "size"), "9");
}

#[test]
fn a_binding_loop_is_named_not_spun() {
    let (e, _r) = load(r#"
        Item {
            property int a: b + 1
            property int b: a + 1
        }
    "#);
    let log = e.take_log();
    assert!(log.iter().any(|l| l.contains("binding loop")), "{log:?}");
}

#[test]
fn aliases_read_and_write_through() {
    let (e, r) = load(r#"
        Item {
            property alias label: t.text
            property int changes: 0
            onLabelChanged: changes++
            Text { id: t; text: "a" }
        }
    "#);
    assert_eq!(ev(&e, r, "label"), "a");
    ev(&e, r, "label = 'b'");
    assert_eq!(ev(&e, r, "t.text"), "b");
    ev(&e, r, "t.text = 'c'");
    assert_eq!(ev(&e, r, "label"), "c");
    assert_eq!(ev(&e, r, "changes"), "2");
}

#[test]
fn timers_run_on_the_engines_clock() {
    let (e, r) = load(r#"
        Item {
            property int ticks: 0
            property int once: 0
            Timer { interval: 100; running: true; repeat: true; onTriggered: ticks++ }
            Timer { id: single; interval: 50; onTriggered: once++ }
            Component.onCompleted: single.start()
        }
    "#);
    e.advance(49);
    assert_eq!(ev(&e, r, "once"), "0");
    e.advance(50);
    assert_eq!(ev(&e, r, "once"), "1");
    assert_eq!(ev(&e, r, "single.running"), "false");
    // A jump past several intervals fires a repeating timer once, as QTimer
    // does
    for t in [150, 250, 350] {
        e.advance(t);
    }
    assert_eq!(ev(&e, r, "ticks"), "3");
}

/// A Timer started by a key counts from the key, not the loop's last step
#[test]
fn a_timer_an_event_starts_counts_from_when_the_loop_woke() {
    let (e, r) = load(r#"
        Item {
            property int once: 0
            Timer { id: single; interval: 300; onTriggered: once++ }
        }
    "#);
    e.advance(0);
    e.woke(1000);
    e.eval(r, "single.start()").unwrap();
    e.advance(1010);
    assert_eq!(ev(&e, r, "once"), "0", "not due at once");
    e.advance(1299);
    assert_eq!(ev(&e, r, "once"), "0");
    e.advance(1300);
    assert_eq!(ev(&e, r, "once"), "1");
}

#[test]
fn connections_follow_their_target() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int heard: 0
            property var src: a
            Item { id: a; signal ping(int n) }
            Item { id: b; signal ping(int n) }
            Connections {
                target: root.src
                function onPing(n) { root.heard += n }
            }
        }
    "#);
    ev(&e, r, "a.ping(1)");
    assert_eq!(ev(&e, r, "heard"), "1");
    ev(&e, r, "src = b");
    ev(&e, r, "a.ping(10)");
    ev(&e, r, "b.ping(100)");
    assert_eq!(ev(&e, r, "heard"), "101");
}

#[test]
fn components_from_files() {
    let dir = std::env::temp_dir().join(format!("aosh-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Pill.qml"), r#"
        Rectangle {
            id: pill
            property string label: "?"
            property int presses: 0
            width: 10
            radius: width / 2
            function press() { presses++ }
        }
    "#).unwrap();
    let mut e = Engine::new().unwrap();
    let r = e.load_source("shell.qml", r#"
        Item {
            Pill { id: one; label: "one"; width: 30 }
            Pill { id: two }
            property real r1: one.radius
            property string l2: two.label
        }
    "#, &dir).unwrap();
    assert_eq!(ev(&e, r, "r1"), "15");
    assert_eq!(ev(&e, r, "l2"), "?");
    ev(&e, r, "(one.press(), one.press(), two.press())");
    assert_eq!(ev(&e, r, "one.presses + ',' + two.presses"), "2,1");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn mistakes_are_reported_with_a_place() {
    let (e, r) = load("Item {\n  nope: 1\n  property int x: undefinedThing.y\n  onFoo: 1\n}\n");
    let log = e.take_log();
    assert!(log.iter().any(|l| l.contains("test.qml:2") && l.contains("nope")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("test.qml:3") && l.contains("undefinedThing")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("test.qml:4") && l.contains("foo")), "{log:?}");
    // And the object is there anyway
    assert_eq!(ev(&e, r, "x"), "0");
}

#[test]
fn qt_helpers() {
    let (e, r) = load("Item { property color c: Qt.rgba(1, 0, 0, 0.5) }");
    assert_eq!(ev(&e, r, "c"), "#80ff0000");
    assert_eq!(ev(&e, r, "Qt.formatDateTime(new Date(2026, 9, 6, 14, 5, 9), 'yyyy-MM-dd HH:mm:ss')"), "2026-10-06 14:05:09");
    assert_eq!(ev(&e, r, "Qt.alpha('#ff0000', 0)"), "#00ff0000");
}

#[test]
fn an_object_literal_is_a_value() {
    let (e, r) = load(r#"
        Item {
            property var data: { "title": "x", "n": 2 }
            property var nothing: {}
            property int computed: { if (data.n > 1) return 10; return 0 }
        }
    "#);
    assert_eq!(ev(&e, r, "data.title + data.n"), "x2");
    assert_eq!(ev(&e, r, "nothing === undefined"), "true");
    assert_eq!(ev(&e, r, "computed"), "10");
}

#[test]
fn statement_bindings_are_worth_their_value() {
    // A `switch` that returns and an `if` that uses its completion value, both
    // tracking dependencies
    let (e, r) = load(r#"
        Item {
            property int sides: 4
            property bool wide: false
            property string shape: switch (sides) {
                case 4: return "cookie4";
                default: return "star" + sides;
            }
            property int w: if (wide) 200; else 100
            property int hovered: 3
            signal gone()
            onGone: if (hovered === 3)
                        hovered = -1
        }
    "#);
    assert_eq!(ev(&e, r, "shape"), "cookie4");
    assert_eq!(ev(&e, r, "w"), "100");
    e.set(r, "sides", Value::Num(14.0)).unwrap();
    e.set(r, "wide", Value::Bool(true)).unwrap();
    assert_eq!(ev(&e, r, "shape"), "star14");
    assert_eq!(ev(&e, r, "w"), "200");
    ev(&e, r, "gone()");
    assert_eq!(ev(&e, r, "hovered"), "-1");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// Shell directory layout on disk: `qs` modules, a singleton, an aliased
/// module, an inline component, an enum, a default alias, a JS library
/// importing another, and an unknown type
#[test]
fn an_end4_shaped_shell_loads() {
    let root = std::env::temp_dir().join(format!("aosh-qs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let put = |name: &str, src: &str| {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    };
    put("modules/common/Appearance.qml", r#"
        pragma Singleton
        import QtQuick
        QtObject {
            property int rounding: 17
            property int doubled: Twice.of(rounding)
        }
    "#);
    put("modules/common/functions/Twice.qml", r#"
        pragma Singleton
        QtObject { function of(x) { return x * 2 } }
    "#);
    put("modules/common/widgets/Card.qml", r#"
        import QtQuick
        Item {
            id: card
            enum Kind { Plain, Fancy = 4, Fancier }
            default property alias data: box.data
            property int kind: Card.Fancy
            Item { id: box }
            property int inside: box.children.length
        }
    "#);
    put("modules/common/widgets/shapes/offset.js", ".pragma library\n\nclass Offset { constructor(x, y) { this.x = x; this.y = y; } }\nvar made = 0\nfunction make(x, y) { made++; return new Offset(x, y); }\n");
    put("modules/common/widgets/shapes/shapes.js", ".pragma library\n.import \"offset.js\" as Offset\nfunction origin() { return Offset.make(0, 0); }\n");
    put("shell.qml", r#"
        import QtQuick
        import qs.modules.common
        import qs.modules.common.widgets
        import qs.modules.common.functions as CF
        import "modules/common/widgets/shapes/shapes.js" as Shapes
        Item {
            id: root
            property int r: Appearance.rounding
            property int d: Appearance.doubled
            property int viaAlias: CF.Twice.of(5)
            property int fancier: Card.Fancier
            property int kindKind: Card.Kind.Fancy
            property var o: Shapes.origin()
            Card { id: card; Item {} Item {} }
            Pill { id: pill; label: "x" }
            Gizmo { id: g; size: 3 }
            component Pill: Item { property string label: "" }
        }
    "#);
    let mut e = Engine::new().unwrap();
    let r = e.load_file(&root.join("shell.qml")).unwrap();
    assert_eq!(ev(&e, r, "r"), "17");
    assert_eq!(ev(&e, r, "d"), "34");
    assert_eq!(ev(&e, r, "viaAlias"), "10");
    assert_eq!(ev(&e, r, "fancier"), "5");
    assert_eq!(ev(&e, r, "kindKind"), "4");
    assert_eq!(ev(&e, r, "card.kind"), "4");
    assert_eq!(ev(&e, r, "o.x + ',' + o.y"), "0,0");
    // The two children written inside `Card` went into its `box`
    assert_eq!(ev(&e, r, "card.inside"), "2");
    assert_eq!(ev(&e, r, "pill.label"), "x");
    // A stand-in for an unknown type keeps what it was given
    assert_eq!(ev(&e, r, "g.size"), "3");
    let log = e.take_log();
    assert!(log.len() == 1 && log[0].contains("'Gizmo'"), "{log:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// `Qt` and `Quickshell` as services read them: `Qt.locale().toString(...)`,
/// `Qt.resolvedUrl`, `Quickshell.shellPath`, `Qt.binding`, a Singleton root
#[test]
fn qt_and_quickshell_as_end4_reads_them() {
    let root = std::env::temp_dir().join(format!("aosh-qt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let put = |name: &str, src: &str| {
        let p = root.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    };
    put("services/DateTime.qml", r#"
        pragma Singleton
        import Quickshell
        Singleton {
            property var clock: SystemClock { id: clock; precision: SystemClock.Minutes }
            readonly property int hour24: clock.date.getHours()
            property string page: Qt.resolvedUrl("pages/About.qml")
            property string up: Qt.resolvedUrl("../x/./y.png")
        }
    "#);
    put("shell.qml", r#"
        import QtQuick
        import qs.services
        Item {
            id: root
            property var when: new Date(2026, 9, 7, 9, 5, 3)
            property string longDate: Qt.locale().toString(when, "dddd, dd/MM hh:mm AP 'at' yy ''")
            property string month: when.toLocaleDateString(Qt.locale(), "MMM").toUpperCase()
            property string assets: Quickshell.shellPath("assets")
            property int hour: DateTime.hour24
            property int base: 2
            property int derived: 0
            property string page: Qt.resolvedUrl("pages/About.qml")
        }
    "#);
    let mut e = Engine::new().unwrap();
    let r = e.load_file(&root.join("shell.qml")).unwrap();
    let dir = root.canonicalize().unwrap();
    assert_eq!(ev(&e, r, "longDate"), "Wednesday, 07/10 09:05 AM at 26 '");
    assert_eq!(ev(&e, r, "month"), "OCT");
    assert_eq!(ev(&e, r, "Qt.formatDateTime(when, 'HH:mm:ss')"), "09:05:03");
    assert_eq!(ev(&e, r, "Qt.locale().toString(1234.5, 'f', 2)"), "1,234.50");
    assert_eq!(ev(&e, r, "assets"), format!("{}/assets", dir.display()));
    // A SystemClock has the time as soon as it exists
    assert_eq!(ev(&e, r, "hour >= 0 && hour < 24"), "true");
    // Each file resolves against its own directory
    assert_eq!(ev(&e, r, "page"), format!("file://{}/pages/About.qml", dir.display()));
    assert_eq!(ev(&e, r, "DateTime.page"), format!("file://{}/services/pages/About.qml", dir.display()));
    assert_eq!(ev(&e, r, "DateTime.up"), format!("file://{}/x/y.png", dir.display()));
    assert_eq!(ev(&e, r, "Qt.resolvedUrl('image://icon/x')"), "image://icon/x");
    // `x = Qt.binding(f)` tracks what it reads
    e.eval(r, "derived = Qt.binding(() => base * 10)").unwrap();
    assert_eq!(ev(&e, r, "derived"), "20");
    e.set(r, "base", Value::Num(3.0)).unwrap();
    assert_eq!(ev(&e, r, "derived"), "30");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// A value type set whole (`sourceSize: Qt.size(w, h)`, `font: other.font`)
/// binds each part
#[test]
fn a_value_type_is_given_whole() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int side: 10
            Image { id: img; sourceSize: Qt.size(root.side, root.side * 2) }
            Text { id: a; font.pixelSize: 20; font.family: "Space Grotesk" }
            Text { id: b; font: a.font }
            Text { id: c; font: ({ pixelSize: 9 }) }
        }
    "#);
    assert_eq!(ev(&e, r, "img.sourceSize.width + 'x' + img.sourceSize.height"), "10x20");
    e.set(r, "side", Value::Num(4.0)).unwrap();
    assert_eq!(ev(&e, r, "img.sourceSize.width + 'x' + img.sourceSize.height"), "4x8");
    assert_eq!(ev(&e, r, "b.font.family + ' ' + b.font.pixelSize"), "Space Grotesk 20");
    e.eval(r, "a.font.pixelSize = 30").unwrap();
    assert_eq!(ev(&e, r, "b.font.pixelSize"), "30", "it follows the font it was given");
    // A part missing from the value keeps its own
    assert_eq!(ev(&e, r, "c.font.pixelSize + ' ' + c.font.weight"), "9 400");
    e.eval(r, "img.sourceSize = Qt.size(7, 3)").unwrap();
    assert_eq!(ev(&e, r, "img.sourceSize.width + 'x' + img.sourceSize.height"), "7x3");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
}

/// QtQml Binding for drags, with the default restore of the property's own
/// binding
#[test]
fn a_binding_object_holds_and_lets_go() {
    let (e, r) = load(r#"
        Item {
            id: root
            property bool dragging: false
            property real proxy: 5
            property real base: 1
            property real held: base * 100
            property real kept: 0
            Binding { target: root; property: "held"; value: root.proxy; when: root.dragging }
            Binding { target: root; property: "kept"; value: root.proxy; when: root.dragging
                      restoreMode: Binding.RestoreNone }
            Item { id: shown; property bool v: true; Binding on v { value: false; when: root.dragging } }
        }
    "#);
    assert_eq!(ev(&e, r, "held"), "100");
    e.set(r, "dragging", Value::Bool(true)).unwrap();
    assert_eq!(ev(&e, r, "held + ' ' + kept + ' ' + shown.v"), "5 5 false");
    e.set(r, "proxy", Value::Num(7.0)).unwrap();
    assert_eq!(ev(&e, r, "held + ' ' + kept"), "7 7", "the value is followed");
    e.set(r, "base", Value::Num(2.0)).unwrap();
    assert_eq!(ev(&e, r, "held"), "7", "the property's own binding is held meanwhile");
    e.set(r, "dragging", Value::Bool(false)).unwrap();
    assert_eq!(ev(&e, r, "held + ' ' + kept + ' ' + shown.v"), "200 7 true",
        "its binding back; RestoreNone keeps the value; the value back where there was no binding");
    e.set(r, "base", Value::Num(3.0)).unwrap();
    assert_eq!(ev(&e, r, "held"), "300");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
}

/// `a.mapToItem(b, x, y)` and a signal's `connect`
#[test]
fn points_map_between_items_and_signals_connect() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int heard: 0
            signal reset(int n)
            Item { id: a; x: 10; y: 20; Item { id: inner; x: 5; y: 5 } }
            Item { id: b; x: 100; y: 50 }
            function hear(n) { heard += n }
        }
    "#);
    assert_eq!(ev(&e, r, "const p = inner.mapToItem(b, 1, 2); p.x + ',' + p.y"), "-84,-23");
    assert_eq!(ev(&e, r, "const p = b.mapFromItem(inner, 1, 2); p.x + ',' + p.y"), "-84,-23");
    assert_eq!(ev(&e, r, "const p = inner.mapToItem(null, 0, 0); p.x + ',' + p.y"), "15,25");
    assert_eq!(ev(&e, r, "const p = a.mapToItem(b, Qt.rect(0, 0, 4, 3)); p.width + 'x' + p.height"), "4x3");
    e.eval(r, "(reset.connect(hear), reset(2), reset(3))").unwrap();
    assert_eq!(ev(&e, r, "heard"), "5");
    e.eval(r, "(reset.disconnect(hear), reset(10))").unwrap();
    assert_eq!(ev(&e, r, "heard"), "5", "disconnected");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
}

/// Setting an alias replaces the target's value and binding; a value type alias
/// (`property alias font: symbol.font`) is one alias per part
#[test]
fn an_alias_is_given_what_its_target_is_given() {
    let dir = std::env::temp_dir().join(format!("aosh-alias-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Styled.qml"), r#"
        Text {
            font { family: "A"; pixelSize: 15 }
        }
    "#).unwrap();
    std::fs::write(dir.join("Sym.qml"), r#"
        Styled {
            id: root
            property real base: 16
            property real iconSize: base - 1
            font { family: "B"; pixelSize: iconSize }
        }
    "#).unwrap();
    std::fs::write(dir.join("Wrap.qml"), r#"
        Item {
            property alias iconSize: symbol.iconSize
            property alias font: symbol.font
            Sym { id: symbol }
        }
    "#).unwrap();
    let mut e = Engine::new().unwrap();
    let r = e.load_source("shell.qml", r#"
        Item {
            id: top
            property real big: 20
            Wrap { id: plain }
            Wrap { id: given; iconSize: 18 }
            Wrap { id: bound; iconSize: top.big + 2 }
            Wrap { id: styled; font.family: "C"; font.weight: Font.Bold }
            Text { id: other; font.pixelSize: 9; font.family: "D" }
            Wrap { id: whole; font: other.font }
        }
    "#, &dir).unwrap();
    assert_eq!(ev(&e, r, "plain.iconSize + ' ' + plain.font.pixelSize"), "15 15");
    assert_eq!(ev(&e, r, "given.iconSize + ' ' + given.font.pixelSize"), "18 18");
    assert_eq!(ev(&e, r, "bound.iconSize"), "22");
    e.set(r, "big", Value::Num(30.0)).unwrap();
    assert_eq!(ev(&e, r, "bound.iconSize + ' ' + bound.font.pixelSize"), "32 32", "the binding given follows");
    assert_eq!(ev(&e, r, "styled.font.family + ' ' + styled.font.weight + ' ' + styled.font.pixelSize"), "C 700 15");
    assert_eq!(ev(&e, r, "whole.font.family + ' ' + whole.font.pixelSize"), "D 9");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An instance's value replaces the one its file's root gave, binding or not
#[test]
fn an_instance_replaces_its_files_own_binding() {
    let dir = std::env::temp_dir().join(format!("aosh-over-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Fade.qml"), r#"
        Item {
            property real base: 1
            z: base + 1
            width: 7
        }
    "#).unwrap();
    let mut e = Engine::new().unwrap();
    let r = e.load_source("shell.qml", r#"
        Item {
            id: top
            property real far: 10
            Fade { id: lit; z: 4 }
            Fade { id: bound; z: top.far * 2; width: top.far }
            Fade { id: own }
        }
    "#, &dir).unwrap();
    assert_eq!(ev(&e, r, "lit.z + ' ' + bound.z + ' ' + own.z"), "4 20 2");
    assert_eq!(ev(&e, r, "bound.width + ' ' + own.width"), "10 7");
    let _ = std::fs::remove_dir_all(&dir);
}
