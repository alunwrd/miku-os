// Models and what they make, on the host

use std::path::Path;

use crate::object::engine::Engine;
use crate::object::value::Value;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

fn ev(e: &Engine, root: u32, expr: &str) -> String {
    e.eval(root, expr).unwrap_or_else(|err| panic!("{expr}: {err}"))
}

#[test]
fn a_repeater_makes_one_per_row_beside_itself() {
    let (e, r) = load(r#"
        Row {
            id: row
            property int n: 3
            Rectangle { id: first; width: 5; height: 5 }
            Repeater {
                id: rep
                model: row.n
                Rectangle { width: 10 + index; height: 10; property int i: index }
            }
            Rectangle { id: last; width: 5; height: 5 }
        }
    "#);
    assert_eq!(ev(&e, r, "rep.count"), "3");
    // In the parent, where the Repeater is written
    assert_eq!(ev(&e, r, "row.children.length"), "6");
    // [first, the Repeater, three delegates, last]
    assert_eq!(ev(&e, r, "row.children[3].width"), "11");
    assert_eq!(ev(&e, r, "row.children[5] === last"), "true");
    assert_eq!(ev(&e, r, "rep.itemAt(2).i"), "2");
    e.set(r, "n", Value::Num(1.0)).unwrap();
    assert_eq!(ev(&e, r, "rep.count"), "1");
    assert_eq!(ev(&e, r, "row.children.length"), "4");
    assert!(e.take_log().is_empty());
}

#[test]
fn an_array_is_modeldata_and_the_delegate_sees_its_file() {
    let (e, r) = load(r#"
        Item {
            id: root
            property color accent: "red"
            property var names: ["a", "b"]
            Item {
                id: holder
                Repeater {
                    model: root.names
                    delegate: Text { text: modelData + index; color: root.accent }
                }
            }
        }
    "#);
    assert_eq!(ev(&e, r, "holder.children[1].text"), "a0");
    assert_eq!(ev(&e, r, "holder.children[2].text"), "b1");
    assert_eq!(ev(&e, r, "holder.children[2].color"), "#ff0000");
    e.eval(r, "root.accent = 'blue'").unwrap();
    assert_eq!(ev(&e, r, "holder.children[1].color"), "#0000ff");
    e.eval(r, "root.names = ['x']").unwrap();
    assert_eq!(ev(&e, r, "holder.children.length"), "2");
    assert_eq!(ev(&e, r, "holder.children[1].text"), "x0");
}

#[test]
fn a_list_model_changes_row_by_row() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int made: 0
            ListModel {
                id: m
                ListElement { name: "one"; big: false }
                ListElement { name: "two"; big: true }
            }
            Column {
                id: col
                Repeater {
                    id: rep
                    model: m
                    Text {
                        text: index + ":" + name + (model.big ? "!" : "")
                        Component.onCompleted: root.made++
                    }
                }
            }
        }
    "#);
    assert_eq!(ev(&e, r, "m.count"), "2");
    assert_eq!(ev(&e, r, "col.children[1].text"), "0:one");
    assert_eq!(ev(&e, r, "col.children[2].text"), "1:two!");
    assert_eq!(ev(&e, r, "made"), "2");
    // Insert at the front creates one delegate and renumbers the rest
    e.eval(r, "m.insert(0, { name: 'zero', big: false })").unwrap();
    assert_eq!(ev(&e, r, "made"), "3");
    assert_eq!(ev(&e, r, "col.children[1].text"), "0:zero");
    assert_eq!(ev(&e, r, "col.children[3].text"), "2:two!");
    // A role set is a store the delegate follows
    e.eval(r, "m.setProperty(1, 'name', 'uno')").unwrap();
    assert_eq!(ev(&e, r, "col.children[2].text"), "1:uno");
    e.eval(r, "m.get(2).big = false").unwrap();
    assert_eq!(ev(&e, r, "col.children[3].text"), "2:two");
    e.eval(r, "m.remove(0)").unwrap();
    assert_eq!(ev(&e, r, "rep.count"), "2");
    assert_eq!(ev(&e, r, "col.children[1].text"), "0:uno");
    e.eval(r, "m.move(0, 1, 1)").unwrap();
    assert_eq!(ev(&e, r, "col.children[1].text"), "0:two");
    e.eval(r, "m.clear()").unwrap();
    assert_eq!(ev(&e, r, "rep.count"), "0");
    e.eval(r, "m.append([{ name: 'p', big: true }, { name: 'q', big: false }])").unwrap();
    assert_eq!(ev(&e, r, "col.children[2].text"), "1:q");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn destroyed_objects_go_and_their_slots_come_back() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int n: 50
            property var kept: null
            Repeater {
                model: root.n
                Item { property int v: index * 2; Timer { running: true; interval: 10 } }
            }
        }
    "#);
    let live = e.core.0.borrow().objs.live();
    for k in 0..20 {
        e.set(r, "n", Value::Num(50.0 + (k % 2) as f64)).unwrap();
    }
    e.set(r, "n", Value::Num(50.0)).unwrap();
    assert_eq!(e.core.0.borrow().objs.live(), live, "a rebuilt list is the same size");
    // A reference to a delegate is null once the delegate is gone
    e.eval(r, "root.kept = root.children[1]").unwrap();
    assert_eq!(ev(&e, r, "root.kept.v"), "0");
    e.set(r, "n", Value::Num(0.0)).unwrap();
    assert_eq!(ev(&e, r, "root.kept === null"), "true");
    assert!(e.core.0.borrow().timers.is_empty());
}

#[test]
fn a_loader_makes_and_unmakes() {
    let (e, r) = load(r#"
        Item {
            id: root
            property bool show: true
            property int gone: 0
            Component {
                id: card
                Rectangle {
                    implicitWidth: 40; implicitHeight: 30
                    Component.onDestruction: root.gone++
                }
            }
            Loader { id: l; active: root.show; sourceComponent: card }
        }
    "#);
    assert_eq!(ev(&e, r, "l.status"), "1");
    assert_eq!(ev(&e, r, "l.item.implicitWidth"), "40");
    assert_eq!(ev(&e, r, "l.item.parent === l"), "true");
    e.set(r, "show", Value::Bool(false)).unwrap();
    assert_eq!(ev(&e, r, "l.item === null"), "true");
    assert_eq!(ev(&e, r, "gone"), "1");
    e.set(r, "show", Value::Bool(true)).unwrap();
    assert_eq!(ev(&e, r, "l.item.implicitHeight"), "30");
    // createObject, and destroy()
    let before: usize = ev(&e, r, "root.children.length").parse().unwrap();
    assert_eq!(ev(&e, r, "card.createObject(root, { implicitWidth: 7 }).implicitWidth"), "7");
    assert_eq!(ev(&e, r, "root.children.length"), (before + 1).to_string());
    assert_eq!(ev(&e, r, "root.children[root.children.length - 1].implicitWidth"), "7");
    e.eval(r, "root.children[root.children.length - 1].destroy()").unwrap();
    assert_eq!(ev(&e, r, "root.children.length"), before.to_string());
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn a_list_view_has_its_delegates_inside_and_knows_the_current_one() {
    let (e, r) = load(r#"
        ListView {
            id: lv
            width: 100; height: 50
            spacing: 2
            model: 5
            delegate: Rectangle {
                width: ListView.view.width; height: 20
                property bool current: ListView.isCurrentItem
            }
        }
    "#);
    assert_eq!(ev(&e, r, "lv.count"), "5");
    assert_eq!(ev(&e, r, "lv.currentIndex"), "0");
    assert_eq!(ev(&e, r, "lv.currentItem === lv.children[0]"), "true");
    assert_eq!(ev(&e, r, "lv.children[0].width"), "100");
    assert_eq!(ev(&e, r, "lv.children[0].current"), "true");
    e.eval(r, "lv.incrementCurrentIndex()").unwrap();
    assert_eq!(ev(&e, r, "lv.children[0].current"), "false");
    assert_eq!(ev(&e, r, "lv.children[1].current"), "true");
}

/// quickshell Variants and QtQml Instantiator create objects without showing
/// them
#[test]
fn variants_and_instantiator_make_objects_per_row() {
    let mut e = Engine::new().unwrap();
    let r = e.load_source("test.qml", r#"
        Item {
            id: root
            property int added: 0
            property bool on: true
            Variants {
                id: per
                model: Quickshell.screens
                QtObject {
                    required property var modelData
                    property int w: modelData.width
                }
            }
            Instantiator {
                id: inst
                active: root.on
                model: ["a", "b"]
                delegate: QtObject { property string name: modelData }
                onObjectAdded: (i, o) => root.added++
            }
        }
    "#, std::path::Path::new(".")).unwrap();
    assert_eq!(e.eval(r, "per.instances.length").unwrap(), "1");
    assert_eq!(e.eval(r, "per.instances[0].w").unwrap(), "0", "a screen of no size until it is known");
    e.set_screen(1920, 1080);
    assert_eq!(e.eval(r, "per.instances[0].w").unwrap(), "1920", "the same instance follows its screen");
    assert_eq!(e.eval(r, "inst.count + ' ' + inst.object.name + ' ' + inst.objectAt(1).name").unwrap(), "2 a b");
    assert_eq!(e.eval(r, "added").unwrap(), "2");
    e.set(r, "on", Value::Bool(false)).unwrap();
    assert_eq!(e.eval(r, "inst.count + ' ' + inst.object").unwrap(), "0 null");
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
}

/// A handler that closes its own Loader can still read its values:
/// `onDestruction` runs immediately, values remain until the handler ends
/// (matches qml6)
#[test]
fn a_handler_that_unloads_itself_reads_on_until_it_returns() {
    let (e, r) = load(r#"
        Item {
            id: root
            property bool open: true
            property var log: []
            Loader {
                id: loader
                active: root.open
                sourceComponent: Item {
                    id: inner
                    property int value: 7
                    Repeater {
                        model: 2
                        Item {
                            id: cell
                            required property int index
                            property int number: index + 10
                            function act() {
                                root.open = false
                                log.push("after close: number=" + cell.number + " value=" + inner.value
                                    + " index=" + index + " loader.item=" + loader.item + " status=" + loader.status)
                            }
                            Component.onDestruction: log.push("destroyed cell " + index)
                        }
                    }
                    Component.onDestruction: log.push("destroyed inner")
                }
            }
            property var first
            function run() {
                for (const c of loader.item.children) if (c.act) { first = c; c.act(); break }
                return log.join("|")
            }
        }
    "#);
    assert_eq!(ev(&e, r, "run()"), [
        "destroyed inner", "destroyed cell 1", "destroyed cell 0",
        "after close: number=10 value=7 index=0 loader.item=null status=0",
    ].join("|"));
    // Once out of the engine they are gone
    assert_eq!(ev(&e, r, "first === null || first === undefined || first.number === undefined"), "true");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// ScriptModel keeps delegates of remaining values in the new order, creates
/// new ones and destroys the rest (cases from quickshell's
/// `src/core/test/scriptmodel.cpp`)
#[test]
fn a_script_model_keeps_the_delegates_of_values_that_stay() {
    let (e, r) = load(r#"
        Item {
            id: root
            property var list: []
            property int made: 0
            property int gone: 0
            Repeater {
                id: rep
                model: ScriptModel { values: root.list }
                Item {
                    required property var modelData
                    required property int index
                    property int born
                    Component.onCompleted: born = ++root.made
                    Component.onDestruction: root.gone++
                }
            }
            // Each delegate as value:birth, in order, and the index each has
            function shown() {
                let out = []
                for (let i = 0; i < rep.count; i++) {
                    const d = rep.itemAt(i)
                    out.push(d.modelData + ":" + d.born + (d.index === i ? "" : "!"))
                }
                return out.join(" ")
            }
            function births() {
                let m = {}
                for (let i = 0; i < rep.count; i++) m[rep.itemAt(i).modelData] = rep.itemAt(i).born
                return m
            }
        }
    "#);
    let cases = [
        ("ABCD", "ABCDEFG"), ("EFG", "ABCDEFG"), ("ABFG", "ABCDEFG"), ("ABCDEFG", "ABCD"),
        ("ABCDEFG", "DEFG"), ("ABCDEFG", "ABFG"), ("ABCDEFG", "AFBCDEG"), ("ABCDEFG", "ADEFBCG"),
        ("ABCDEFG", "EFGABCD"), ("ABCDEFG", "ABDEFCG"), ("ABCDEFG", "ABXXEFG"), ("ABCDEFG", "ABFGE"),
        ("ABCDEFG", "ABEFXYCDG"),
    ];
    for (old, new) in cases {
        let list = |s: &str| format!("[{}]", s.chars().map(|c| format!("{c:?}")).collect::<Vec<_>>().join(","));
        e.eval(r, &format!("root.list = {}", list(old))).unwrap();
        let before = ev(&e, r, "JSON.stringify(births())");
        let made: usize = ev(&e, r, "root.made").parse().unwrap();
        let gone: usize = ev(&e, r, "root.gone").parse().unwrap();
        e.eval(r, &format!("root.list = {}", list(new))).unwrap();
        let before: std::collections::HashMap<String, usize> =
            before.trim_matches(|c| c == '{' || c == '}').split(',').filter(|p| !p.is_empty())
                .map(|p| {
                    let (k, v) = p.split_once(':').unwrap();
                    (k.trim_matches('"').to_string(), v.parse().unwrap())
                })
                .collect();
        // Old delegate if the value was present, otherwise a new one
        let mut next = made;
        let want: Vec<String> = new.chars().map(|c| {
            // "XX" is two equal values, as in quickshell's case
            let born = match before.get(&c.to_string()) {
                Some(&b) if old.contains(c) => b,
                _ => { next += 1; next }
            };
            format!("{c}:{born}")
        }).collect();
        let shown = ev(&e, r, "shown()");
        assert_eq!(shown, want.join(" "), "{old} -> {new}");
        let removed = old.chars().filter(|c| !new.contains(*c)).count();
        assert_eq!(ev(&e, r, "root.gone").parse::<usize>().unwrap() - gone, removed, "{old} -> {new}");
        // Back to nothing, for the next case
        e.eval(r, "root.list = []").unwrap();
    }
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// `objectProp` matches by property: the delegate stays and `modelData`
/// updates. `Array(n)` grows and shrinks at its end
#[test]
fn a_script_model_matches_by_its_object_prop_and_counts_undefined() {
    let (e, r) = load(r#"
        Item {
            id: root
            property var list: [{ key: "a", v: 1 }, { key: "b", v: 2 }]
            property int n: 2
            property int made: 0
            Repeater {
                id: rep
                model: ScriptModel { values: root.list; objectProp: "key" }
                Item { required property var modelData; property int born; Component.onCompleted: born = ++root.made }
            }
            Repeater {
                id: holes
                model: ScriptModel { values: Array(root.n) }
                Item { property int born; Component.onCompleted: born = ++root.made }
            }
            function shown(view) {
                let out = []
                for (let i = 0; i < view.count; i++) {
                    const d = view.itemAt(i)
                    out.push((d.modelData ? d.modelData.key + d.modelData.v : "u") + ":" + d.born)
                }
                return out.join(" ")
            }
        }
    "#);
    assert_eq!(ev(&e, r, "shown(rep)"), "a1:1 b2:2");
    e.eval(r, r#"root.list = [{ key: "b", v: 20 }, { key: "c", v: 3 }, { key: "a", v: 10 }]"#).unwrap();
    assert_eq!(ev(&e, r, "shown(rep)"), "b20:2 c3:5 a10:1");
    assert_eq!(ev(&e, r, "shown(holes)"), "u:3 u:4");
    e.eval(r, "root.n = 4").unwrap();
    assert_eq!(ev(&e, r, "shown(holes)"), "u:3 u:4 u:6 u:7");
    e.eval(r, "root.n = 1").unwrap();
    assert_eq!(ev(&e, r, "shown(holes)"), "u:3");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// Values compare with `QVariant ==`: a rebuilt object with the same contents
/// keeps its delegate, a changed one is a new row, nested arrays compare by
/// contents
#[test]
fn a_script_model_matches_plain_objects_by_what_they_hold() {
    let (e, r) = load(r#"
        Item {
            id: root
            property int w: 1
            property var list: [{ id: 1, at: [0, 0] }, { id: 2, at: [5, 5] }]
            property int made: 0
            Repeater {
                id: rep
                model: ScriptModel { values: root.list }
                Item { required property var modelData; property int born; Component.onCompleted: born = ++root.made }
            }
            function shown() {
                let out = []
                for (let i = 0; i < rep.count; i++) out.push(rep.itemAt(i).modelData.id + ":" + rep.itemAt(i).born)
                return out.join(" ")
            }
        }
    "#);
    assert_eq!(ev(&e, r, "shown()"), "1:1 2:2");
    // Same contents in new objects create nothing
    e.eval(r, "root.list = [{ id: 1, at: [0, 0] }, { id: 2, at: [5, 5] }]").unwrap();
    assert_eq!(ev(&e, r, "shown()"), "1:1 2:2");
    // Second value changed: new row for it, first kept
    e.eval(r, "root.list = [{ id: 1, at: [0, 0] }, { id: 2, at: [6, 5] }]").unwrap();
    assert_eq!(ev(&e, r, "shown()"), "1:1 2:3");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// A type's file can read ids of the file that created it (matches qml6)
#[test]
fn a_type_reads_the_ids_where_it_is_made() {
    let dir = std::env::temp_dir().join(format!("aosh-ctx-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Inner.qml"), r#"
        import QtQuick
        Item {
            property int seen: typeof win !== "undefined" ? win.tag : -1
            property int outerSeen: typeof outer !== "undefined" ? outer.tag : -1
        }
    "#).unwrap();
    std::fs::write(dir.join("main.qml"), r#"
        import QtQuick
        Item {
            id: outer
            property int tag: 5
            Repeater {
                model: 1
                delegate: Inner { id: win; property int tag: 9 }
            }
            Inner { id: direct; property int tag: 3 }
            function seen() {
                let out = []
                for (const c of children) if (c.tag === 9) out.push("delegate " + c.seen + " " + c.outerSeen)
                out.push("direct " + direct.seen + " " + direct.outerSeen)
                return out.join("|")
            }
        }
    "#).unwrap();
    let mut e = Engine::new().unwrap();
    let r = e.load_file(&dir.join("main.qml")).unwrap();
    assert_eq!(ev(&e, r, "seen()"), "delegate 9 5|direct -1 5");
    let _ = std::fs::remove_dir_all(&dir);
}


/// Synchronizer keeps properties equal in both directions (matches qml6 6.12).
/// Initial value comes from the source (alias `source` or `sourceObject`'s
/// `sourceProperty`), otherwise the target
#[test]
fn a_synchronizer_keeps_properties_equal_both_ways() {
    let (e, r) = load(r#"
        import QtQuick
        import Qt.labs.synchronizer
        Item {
            QtObject { id: a; property string text: "from-a" }
            QtObject { id: b; property string text: "from-b"
                Synchronizer on text { property alias source: a.text } }
            QtObject { id: c; property int n: 1; property int m: 5 }
            QtObject { id: d; property int k: 9
                Synchronizer on k { property alias one: c.n; property alias two: c.m } }
            QtObject { id: a6; property int v: 1 }
            Item { id: t4; property int v: 9
                Synchronizer { sourceObject: a6; sourceProperty: "v"; targetObject: t4; targetProperty: "v" } }
            QtObject { id: a9; property string v: "" }
            QtObject { id: t7; property string v: "tgt"; Synchronizer on v { property alias source: a9.v } }
            function start() { return [a.text, b.text, c.n, c.m, d.k, t4.v, a6.v, JSON.stringify(t7.v), JSON.stringify(a9.v)].join(" ") }
            function typed() {
                a.text = "typed"; const one = a.text + "/" + b.text
                b.text = "set-b"; const two = a.text + "/" + b.text
                c.m = 7; const three = c.n + "/" + c.m + "/" + d.k
                a6.v = 3; const four = t4.v
                return [one, two, three, four].join(" ")
            }
        }
    "#);
    assert_eq!(ev(&e, r, "start()"), r#"from-a from-a 9 9 9 1 1 "" """#);
    assert_eq!(ev(&e, r, "typed()"), "typed/typed set-b/set-b 7/7/7 3");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}
