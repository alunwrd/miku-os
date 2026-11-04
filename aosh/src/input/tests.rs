// Focus, key, and editing tests: keycodes in, QML state out

use std::path::Path;

use crate::object::engine::Engine;

const ESC: u32 = 1;
const BACKSPACE: u32 = 14;
const ENTER: u32 = 28;
const UP: u32 = 103;
const DOWN: u32 = 108;
const LEFT: u32 = 105;
const HOME: u32 = 102;
const END: u32 = 107;
const DELETE: u32 = 111;
const CTRL: u32 = 1 << 2;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

fn ev(e: &Engine, root: u32, expr: &str) -> String {
    e.eval(root, expr)
        .unwrap_or_else(|err| panic!("{expr}: {err}"))
}

fn typed(e: &Engine, s: &str) {
    for c in s.chars() {
        e.key(0, Some(c), 0);
    }
}

#[test]
fn focus_goes_to_the_last_item_given_it_and_is_active_with_the_keyboard() {
    let (e, w) = load(
        r#"
        PanelWindow {
            id: win
            TextInput { id: a; focus: true }
            TextInput { id: b; focus: true }
            Item { id: c }
        }
    "#,
    );
    assert_eq!(
        ev(&e, w, "[a.focus, b.focus].join()"),
        "false,true",
        "the last written wins"
    );
    assert_eq!(
        ev(&e, w, "b.activeFocus"),
        "false",
        "not until the window has the keyboard"
    );
    e.keyboard(Some(w));
    assert_eq!(ev(&e, w, "b.activeFocus"), "true");
    ev(&e, w, "a.forceActiveFocus()");
    assert_eq!(
        ev(
            &e,
            w,
            "[a.focus, a.activeFocus, b.focus, b.activeFocus].join()"
        ),
        "true,true,false,false"
    );
    ev(&e, w, "c.focus = true");
    assert_eq!(
        ev(&e, w, "[a.activeFocus, c.activeFocus].join()"),
        "false,true"
    );
    e.keyboard(None);
    assert_eq!(ev(&e, w, "c.activeFocus"), "false");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn a_key_goes_up_the_parents_until_one_accepts_it() {
    let (e, w) = load(
        r#"
        PanelWindow {
            id: win
            property var heard: []
            Keys.onEscapePressed: heard.push("window esc")
            Item {
                id: outer
                Keys.onPressed: (event) => { heard.push("outer " + event.key); event.accepted = event.key === Qt.Key_A }
                Item {
                    id: inner
                    focus: true
                    Keys.onReturnPressed: (event) => heard.push("inner return")
                    Keys.onUpPressed: (event) => { heard.push("inner up"); event.accepted = false }
                }
            }
        }
    "#,
    );
    e.keyboard(Some(w));
    assert!(
        e.key(ENTER, Some('\r'), 0),
        "a handler for the key accepts by default"
    );
    assert!(e.key(30, Some('a'), 0), "onPressed accepts when it says so");
    assert!(!e.key(UP, None, 0), "declined all the way up");
    assert!(e.key(ESC, None, 0));
    assert_eq!(
        ev(&e, w, "heard.join('|')"),
        format!(
            "inner return|outer {}|inner up|outer {}|outer {}|window esc",
            0x41, 0x0100_0013, 0x0100_0000
        )
    );
}

#[test]
fn a_text_input_edits_and_says_so() {
    let (e, w) = load(
        r#"
        PanelWindow {
            id: win
            property int edits: 0
            property int accepts: 0
            property string last: ""
            TextField {
                id: field
                focus: true
                maximumLength: 8
                onTextEdited: win.edits++
                onAccepted: win.accepts++
                onTextChanged: win.last = text
            }
        }
    "#,
    );
    e.keyboard(Some(w));
    typed(&e, "хелло");
    assert_eq!(ev(&e, w, "field.text"), "хелло");
    assert_eq!(
        ev(&e, w, "field.cursorPosition"),
        "5",
        "characters, not bytes"
    );
    e.key(LEFT, None, 0);
    e.key(LEFT, None, 0);
    e.key(BACKSPACE, None, 0);
    assert_eq!(
        ev(&e, w, "field.text + ' ' + field.cursorPosition"),
        "хело 2"
    );
    e.key(HOME, None, 0);
    e.key(DELETE, None, 0);
    typed(&e, "Х");
    assert_eq!(ev(&e, w, "field.text"), "Хело");
    e.key(END, None, 0);
    typed(&e, " world");
    assert_eq!(ev(&e, w, "field.text"), "Хело wor", "maximumLength");
    e.key(BACKSPACE, None, CTRL);
    assert_eq!(
        ev(&e, w, "field.text"),
        "Хело ",
        "Ctrl+Backspace takes a word"
    );
    assert!(
        !e.key(ENTER, Some('\r'), 0),
        "Return goes on to the parents"
    );
    assert_eq!(
        ev(&e, w, "[accepts, last, length = field.length].join()"),
        "1,Хело ,5"
    );
    assert_eq!(ev(&e, w, "edits > 0"), "true");
    // External text updates move the cursor to the end and do not count as
    // edits
    let edits = ev(&e, w, "edits");
    ev(&e, w, "field.text = 'abc'");
    assert_eq!(
        ev(&e, w, "field.cursorPosition + ' ' + edits"),
        format!("3 {edits}")
    );
    ev(&e, w, "field.insert(1, 'XY')");
    assert_eq!(ev(&e, w, "field.text"), "aXYbc");
    ev(&e, w, "field.remove(0, 2)");
    assert_eq!(ev(&e, w, "field.text + field.getText(1, 3)"), "Ybcbc");
    ev(&e, w, "field.clear()");
    assert_eq!(ev(&e, w, "field.text.length"), "0");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn a_password_shows_dots_and_read_only_moves_only_the_cursor() {
    let (e, w) = load(
        r#"
        PanelWindow {
            id: win
            TextInput { id: pw; focus: true; echoMode: TextInput.Password }
            TextInput { id: ro; readOnly: true; text: "fixed" }
        }
    "#,
    );
    e.keyboard(Some(w));
    typed(&e, "miku");
    assert_eq!(ev(&e, w, "pw.text + ' ' + pw.displayText"), "miku ••••");
    ev(&e, w, "ro.forceActiveFocus()");
    e.key(HOME, None, 0);
    typed(&e, "x");
    e.key(BACKSPACE, None, 0);
    assert_eq!(ev(&e, w, "ro.text + ' ' + ro.cursorPosition"), "fixed 0");
}

#[test]
fn the_launchers_keys_walk_a_list_and_a_list_with_focus_walks_itself() {
    let (e, w) = load(
        r#"
        PanelWindow {
            id: win
            property string ran: ""
            TextField {
                id: search
                focus: true
                Keys.onDownPressed: if (list.currentIndex < list.count - 1) list.currentIndex++
                Keys.onUpPressed: if (list.currentIndex > 0) list.currentIndex--
                Keys.onReturnPressed: win.ran = list.model[list.currentIndex]
            }
            ListView {
                id: list
                height: 100
                model: ["a", "b", "c"]
                currentIndex: 0
                delegate: Item { height: 20; width: 10 }
            }
        }
    "#,
    );
    e.keyboard(Some(w));
    e.key(DOWN, None, 0);
    e.key(DOWN, None, 0);
    e.key(DOWN, None, 0);
    e.key(UP, None, 0);
    e.key(ENTER, Some('\r'), 0);
    assert_eq!(ev(&e, w, "ran + list.currentIndex"), "b1");
    ev(&e, w, "list.forceActiveFocus()");
    e.key(DOWN, None, 0);
    assert_eq!(ev(&e, w, "list.currentIndex"), "2", "keyNavigationEnabled");
}

#[test]
fn the_caret_blinks_only_while_typing_is_possible() {
    let (e, w) = load(
        r#"
        PanelWindow { TextInput { id: t; focus: true } }
    "#,
    );
    e.advance(1000);
    assert_eq!(e.next_due(), None, "no keyboard, no caret");
    e.keyboard(Some(w));
    let due = e.next_due().unwrap();
    assert!(e.core.0.borrow().keys.caret_on);
    e.advance(due);
    assert!(
        !e.core.0.borrow().keys.caret_on,
        "half a blink later it is off"
    );
    typed(&e, "a");
    assert!(e.core.0.borrow().keys.caret_on, "typing shows it at once");
    e.keyboard(None);
    e.advance(due + 5000);
    assert_eq!(e.next_due(), None);
}

/// Simulates a click via consecutive press and release events, matching QML6
/// ordering
///
/// Required by components like kata's bar (which triggers actions on
/// `onPressed`)
/// and `RippleButton` (which self-triggers via an overlay `MouseArea`)
#[test]
fn a_click_is_a_press_and_a_release_in_qts_order() {
    let (e, root) = load(
        r#"
        import QtQuick
        import QtQuick.Controls
        Item {
            property var log: []
            Button {
                id: a
                onPressed: log.push("a.pressed p=" + pressed + " d=" + down)
                onReleased: log.push("a.released p=" + pressed + " d=" + down)
                onClicked: log.push("a.clicked")
                onToggled: log.push("a.toggled")
                onPressedChanged: log.push("a.pressedChanged " + pressed)
                onDownChanged: log.push("a.downChanged " + down)
                onCheckedChanged: log.push("a.checkedChanged " + checked)
            }
            Button {
                id: b
                checkable: true
                onPressed: log.push("b.pressed c=" + checked)
                onReleased: log.push("b.released c=" + checked)
                onClicked: log.push("b.clicked c=" + checked)
                onToggled: log.push("b.toggled c=" + checked)
                onCheckedChanged: log.push("b.checkedChanged " + checked)
            }
            Button {
                id: c
                enabled: false
                onPressed: log.push("c.pressed")
                onClicked: log.push("c.clicked")
            }
            Button {
                id: d
                down: false
                onPressed: log.push("d.pressed d=" + down)
                onReleased: log.push("d.released d=" + down)
                onClicked: log.push("d.clicked")
                onDownChanged: log.push("d.downChanged " + down)
            }
            function run() {
                a.click(); log.push("--"); b.click(); log.push("--"); c.click(); log.push("--");
                d.click(); log.push("--"); a.toggle();
                return log.join("|");
            }
        }
    "#,
    );
    assert_eq!(
        ev(&e, root, "run()"),
        [
            "a.pressedChanged true",
            "a.downChanged true",
            "a.pressed p=true d=true",
            "a.pressedChanged false",
            "a.downChanged false",
            "a.released p=false d=false",
            "a.clicked",
            "--",
            "b.pressed c=false",
            "b.checkedChanged true",
            "b.toggled c=true",
            "b.released c=true",
            "b.clicked c=true",
            "--",
            "--",
            "d.pressed d=false",
            "d.released d=false",
            "d.clicked",
            "--",
            "a.checkedChanged true",
        ]
        .join("|")
    );
}
