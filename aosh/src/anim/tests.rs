// Host animations driven by the engine clock

use std::path::Path;

use crate::object::engine::Engine;
use crate::object::value::Value;

fn load(src: &str) -> (Engine, u32) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    (e, root)
}

fn n(e: &Engine, root: u32, expr: &str) -> f64 {
    e.eval(root, expr)
        .unwrap()
        .parse()
        .unwrap_or_else(|_| panic!("{expr}"))
}

#[test]
fn a_behavior_animates_what_is_stored_and_not_the_first_value() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property bool wide: false
            width: wide ? 200 : 100
            Behavior on width { NumberAnimation { duration: 100 } }
        }
    "#,
    );
    // The initial value is ignored / unused
    assert_eq!(n(&e, r, "width"), 100.0);
    assert!(!e.animating());
    e.advance(0);
    e.set(r, "wide", Value::Bool(true)).unwrap();
    assert!(e.animating());
    assert_eq!(
        n(&e, r, "width"),
        100.0,
        "nothing moves until the clock does"
    );
    e.advance(50);
    assert_eq!(n(&e, r, "width"), 150.0, "linear, half way");
    e.advance(100);
    assert_eq!(n(&e, r, "width"), 200.0);
    e.advance(116);
    assert!(!e.animating());
    assert!(e.take_log().is_empty());
}

#[test]
fn what_an_event_starts_after_a_wait_begins_at_the_next_frame() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property bool shown: false
            opacity: shown ? 1 : 0
            Behavior on opacity { NumberAnimation { duration: 300 } }
        }
    "#,
    );
    // Last tick, then wait 1s for a key press
    e.advance(0);
    e.set(r, "shown", Value::Bool(true)).unwrap();
    // Key handler took 40ms; first frame anchored to start
    e.start_new(1040);
    e.advance(1040);
    assert_eq!(n(&e, r, "opacity"), 0.0, "not over before it is seen");
    e.advance(1190);
    assert_eq!(n(&e, r, "opacity"), 0.5);
    // Next key reverses it after 60ms, effective the frame after
    e.set(r, "shown", Value::Bool(false)).unwrap();
    e.start_new(1250);
    e.advance(1250);
    assert_eq!(
        n(&e, r, "opacity"),
        0.5,
        "turning back starts from where it was"
    );
    e.advance(1400);
    assert_eq!(n(&e, r, "opacity"), 0.25);
}

#[test]
fn a_behavior_turns_towards_a_new_value_from_where_it_is() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property real to: 0
            x: to
            Behavior on x { NumberAnimation { duration: 100 } }
        }
    "#,
    );
    e.advance(0);
    e.set(r, "to", Value::Num(100.0)).unwrap();
    e.advance(50);
    assert_eq!(n(&e, r, "x"), 50.0);
    // Plays in reverse from 50 (full duration)
    e.set(r, "to", Value::Num(0.0)).unwrap();
    e.advance(100);
    assert_eq!(n(&e, r, "x"), 25.0);
    e.advance(150);
    assert_eq!(n(&e, r, "x"), 0.0);
}

#[test]
fn colours_move_by_channel_and_a_disabled_behavior_does_nothing() {
    let (e, r) = load(
        r##"
        Rectangle {
            id: root
            property bool on: false
            property bool animate: true
            color: on ? "#ffffffff" : "#ff000000"
            Behavior on color { enabled: root.animate; ColorAnimation { duration: 100 } }
        }
    "##,
    );
    e.advance(0);
    e.set(r, "on", Value::Bool(true)).unwrap();
    e.advance(50);
    assert_eq!(e.eval(r, "color").unwrap(), "#808080");
    e.advance(100);
    assert_eq!(e.eval(r, "color").unwrap(), "#ffffff");
    e.set(r, "animate", Value::Bool(false)).unwrap();
    e.set(r, "on", Value::Bool(false)).unwrap();
    assert_eq!(e.eval(r, "color").unwrap(), "#000000", "at once");
}

/// Disabling a Behavior mid-animation must not write its old target over the
/// new value
#[test]
fn a_behavior_switched_off_mid_flight_does_not_finish_over_the_new_value() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property bool tall: false
            property bool animate: tall
            height: tall ? 200 : 50
            Behavior on height { enabled: root.animate; NumberAnimation { duration: 100 } }
        }
    "#,
    );
    e.advance(0);
    e.set(r, "tall", Value::Bool(true)).unwrap();
    e.advance(50);
    assert_eq!(n(&e, r, "height"), 125.0, "on its way");
    e.set(r, "tall", Value::Bool(false)).unwrap();
    assert_eq!(n(&e, r, "height"), 50.0, "at once");
    e.advance(75);
    e.advance(500);
    assert_eq!(n(&e, r, "height"), 50.0, "and stays");
}

#[test]
fn easing_is_applied() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property real to: 0
            x: to
            Behavior on x { NumberAnimation { duration: 100; easing.type: Easing.OutCubic } }
        }
    "#,
    );
    e.advance(0);
    e.set(r, "to", Value::Num(100.0)).unwrap();
    e.advance(50);
    assert_eq!(n(&e, r, "x"), 87.5, "OutCubic at a half is 0.875");
}

#[test]
fn sequential_parallel_pause_and_loops() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property real a: 0
            property real b: 0
            property int done: 0
            SequentialAnimation {
                id: seq
                loops: 2
                NumberAnimation { target: root; property: "a"; from: 0; to: 10; duration: 100 }
                PauseAnimation { duration: 50 }
                ParallelAnimation {
                    NumberAnimation { target: root; property: "b"; from: 0; to: 4; duration: 40 }
                    NumberAnimation { target: root; property: "a"; to: 0; duration: 20 }
                }
                onFinished: root.done++
            }
        }
    "#,
    );
    e.advance(1000);
    e.eval(r, "seq.start()").unwrap();
    assert_eq!(e.eval(r, "seq.running").unwrap(), "true");
    e.advance(1050);
    assert_eq!(n(&e, r, "a"), 5.0);
    e.advance(1120);
    assert_eq!(
        n(&e, r, "a"),
        10.0,
        "the first part is over and stays at its end"
    );
    e.advance(1170);
    assert_eq!(n(&e, r, "b"), 2.0);
    e.advance(1190);
    assert_eq!(n(&e, r, "b"), 4.0);
    assert_eq!(n(&e, r, "a"), 0.0);
    // Two loops of 190ms each
    e.advance(1240);
    assert_eq!(n(&e, r, "a"), 5.0, "the second loop");
    assert_eq!(n(&e, r, "done"), 0.0);
    e.advance(1400);
    assert_eq!(n(&e, r, "done"), 1.0);
    assert_eq!(e.eval(r, "seq.running").unwrap(), "false");
    assert!(e.take_log().is_empty());
}

#[test]
fn a_value_source_runs_by_itself_and_forever() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            NumberAnimation on rotation { from: 0; to: 360; duration: 1000; loops: Animation.Infinite }
        }
    "#,
    );
    e.advance(0);
    e.advance(250);
    assert_eq!(n(&e, r, "rotation"), 90.0);
    e.advance(1500);
    assert_eq!(n(&e, r, "rotation"), 180.0);
    assert!(e.animating());
}

#[test]
fn running_follows_a_binding_and_stop_holds_the_value() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property bool go: false
            property real v: 0
            NumberAnimation { id: a; target: root; property: "v"; to: 100; duration: 100; running: root.go }
        }
    "#,
    );
    e.advance(0);
    e.set(r, "go", Value::Bool(true)).unwrap();
    e.advance(40);
    assert_eq!(n(&e, r, "v"), 40.0);
    e.set(r, "go", Value::Bool(false)).unwrap();
    e.advance(80);
    assert_eq!(n(&e, r, "v"), 40.0, "stopped where it was");
    assert!(!e.animating());
}

/// Standard components generated by `kata` across animations: a `BezierSpline`
/// curve,
/// a `RotationAnimation`, a sequential `PropertyAction`/`ScriptAction` pair,
/// and
/// a `Behavior` receiving its animation via alias
#[test]
fn end4s_animations() {
    let (e, r) = load(
        r#"
        Item {
            id: root
            property real a: 0
            property real angle: 350
            property bool shown: false
            property int scripts: 0
            property list<real> curve: [1 / 3, 1 / 3, 2 / 3, 2 / 3, 1, 1]
            property alias anim: behave.animation
            property real w: 0
            anim: NumberAnimation { duration: 100 }
            Behavior on w { id: behave }
            SequentialAnimation {
                id: seq
                loops: 2
                PropertyAction { target: root; property: "shown"; value: true }
                NumberAnimation { target: root; property: "a"; to: 100; duration: 100
                                  easing.type: Easing.BezierSpline; easing.bezierCurve: root.curve }
                ScriptAction { script: root.scripts++ }
                RotationAnimation { target: root; property: "angle"; to: 10; duration: 100
                                    direction: RotationAnimation.Shortest }
            }
        }
    "#,
    );
    e.advance(1000);
    e.eval(r, "seq.start()").unwrap();
    e.advance(1000);
    assert_eq!(
        e.eval(r, "shown").unwrap(),
        "true",
        "the PropertyAction is at once"
    );
    assert_eq!(n(&e, r, "scripts"), 0.0);
    e.advance(1050);
    assert!(
        (n(&e, r, "a") - 50.0).abs() < 1e-3,
        "a straight-line spline is linear: {}",
        n(&e, r, "a")
    );
    e.advance(1150);
    assert_eq!(n(&e, r, "scripts"), 1.0, "the script ran once");
    // 350° to 10° wraps around 0°/360° (+20°), not backward via 180° (-340°).
    assert_eq!(n(&e, r, "angle"), 360.0);
    e.advance(1199);
    assert_eq!(n(&e, r, "scripts"), 1.0, "and not again in the same loop");
    e.advance(1350);
    assert_eq!(n(&e, r, "scripts"), 2.0, "the second loop runs it again");
    // Aliased animation is owned by the Behavior
    e.set(r, "w", Value::Num(100.0)).unwrap();
    e.advance(1400);
    assert_eq!(n(&e, r, "w"), 50.0);
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
}
