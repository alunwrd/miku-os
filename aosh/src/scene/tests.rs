// The layout and the painter against a window, on the host

use std::path::Path;

use super::paint::Rect;
use super::text::Fonts;
use super::{Scene, WinScene};
use crate::object::engine::Engine;

fn setup(src: &str) -> (Engine, u32, Scene) {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("test.qml", src, Path::new(".")).unwrap();
    let fonts = Fonts::load(&[Path::new("../assets")]);
    (e, root, Scene::new(fonts))
}

fn n(e: &Engine, oid: u32, expr: &str) -> f64 {
    e.eval(oid, expr).unwrap().parse().unwrap_or_else(|_| panic!("{expr}"))
}

#[test]
fn a_row_puts_its_children_side_by_side() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Row {
                id: row
                spacing: 4
                Rectangle { id: a; width: 10; height: 10 }
                Rectangle { id: b; width: 20; height: 30 }
                Rectangle { id: c; width: 5; height: 5; visible: false }
            }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 40.0));
    assert_eq!(n(&e, w, "b.x"), 14.0);
    assert_eq!(n(&e, w, "row.width"), 34.0);
    assert_eq!(n(&e, w, "row.height"), 30.0);
}

#[test]
fn a_row_layout_fills_and_centres() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            RowLayout {
                id: lay
                anchors.fill: parent
                spacing: 10
                Rectangle { id: a; implicitWidth: 30; implicitHeight: 20 }
                Rectangle { id: gap; Layout.fillWidth: true }
                Rectangle { id: b; Layout.preferredWidth: 40; implicitHeight: 10 }
            }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 48.0));
    assert_eq!(n(&e, w, "lay.width"), 300.0);
    assert_eq!(n(&e, w, "gap.width"), 300.0 - 30.0 - 40.0 - 20.0);
    assert_eq!(n(&e, w, "b.x"), 260.0);
    assert_eq!(n(&e, w, "a.y"), 14.0);
}

#[test]
fn anchors_with_margins() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Rectangle { id: box; anchors.fill: parent; anchors.margins: 4 }
            Rectangle { id: tag; width: 10; height: 10; anchors.right: box.right; anchors.verticalCenter: parent.verticalCenter }
            Rectangle { id: wide; height: 2; anchors.left: parent.left; anchors.right: parent.right; anchors.leftMargin: 7 }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 50.0));
    assert_eq!(n(&e, w, "box.width"), 92.0);
    assert_eq!(n(&e, w, "box.height"), 42.0);
    assert_eq!(n(&e, w, "tag.x"), 86.0);
    assert_eq!(n(&e, w, "tag.y"), 20.0);
    assert_eq!(n(&e, w, "wide.x"), 7.0);
    assert_eq!(n(&e, w, "wide.width"), 93.0);
}

#[test]
fn a_text_is_as_wide_as_its_text_and_bindings_follow() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Text { id: t; text: "hello"; font.pixelSize: 15 }
            Rectangle { id: under; width: t.width + 20; height: 4 }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 40.0));
    let tw = n(&e, w, "t.width");
    assert!(tw > 20.0 && tw < 60.0, "{tw}");
    assert_eq!(n(&e, w, "under.width"), tw + 20.0);
    e.eval(w, "t.text = 'hello, a longer line'").unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 40.0));
    assert!(n(&e, w, "t.width") > tw * 2.0);
    assert_eq!(n(&e, w, "under.width"), n(&e, w, "t.width") + 20.0);
}

#[test]
fn only_what_changed_is_painted() {
    let (e, w, mut sc) = setup(r##"
        PanelWindow {
            color: "#ff101010"
            Rectangle { id: a; x: 10; y: 5; width: 20; height: 10; color: "red" }
            Rectangle { id: b; x: 50; y: 5; width: 20; height: 10; color: "blue" }
        }
    "##);
    let mut ws = WinScene::new(100, 20);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 20.0));
    let first = sc.paint(&e.core, w, &mut ws);
    assert_eq!(first, vec![Rect::new(0, 0, 100, 20)]);
    assert_eq!(ws.image.px[7 * 100 + 15], 0xFFFF0000);
    assert_eq!(sc.paint(&e.core, w, &mut ws), Vec::new(), "nothing changed");
    e.eval(w, "b.color = 'lime'").unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 20.0));
    assert_eq!(sc.paint(&e.core, w, &mut ws), vec![Rect::new(50, 5, 20, 10)]);
    assert_eq!(ws.image.px[7 * 100 + 55], 0xFF00FF00);
}

#[test]
fn an_item_that_comes_or_goes_costs_its_own_box() {
    // A fading state layer adds a list entry without dirtying what follows
    let (e, w, mut sc) = setup(r##"
        PanelWindow {
            color: "#ff101010"
            Rectangle { id: a; x: 10; y: 5; width: 20; height: 10; color: "red" }
            Rectangle { id: hover; x: 40; y: 5; width: 8; height: 8; color: "white"; opacity: 0 }
            Rectangle { id: b; x: 70; y: 5; width: 20; height: 10; color: "blue" }
            Rectangle { id: c; x: 75; y: 8; width: 20; height: 10; color: "yellow" }
        }
    "##);
    let mut ws = WinScene::new(100, 20);
    let mut frame = |e: &Engine, sc: &mut Scene| {
        e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 20.0));
        sc.paint(&e.core, w, &mut ws)
    };
    frame(&e, &mut sc);
    e.eval(w, "hover.opacity = 0.5").unwrap();
    let came = frame(&e, &mut sc);
    e.eval(w, "hover.opacity = 0").unwrap();
    let went = frame(&e, &mut sc);
    // Two overlapping items swapping order repaint their overlap
    e.eval(w, "b.z = 1").unwrap();
    let swapped = frame(&e, &mut sc);
    assert_eq!((came, went), (vec![Rect::new(40, 5, 8, 8)], vec![Rect::new(40, 5, 8, 8)]));
    let s = swapped.iter().fold(Rect::default(), |a, r| a.union(r));
    let overlap = Rect::new(75, 8, 15, 7);
    assert!(s.intersect(&overlap) == Some(overlap) && s.w < 100, "{s:?}");
    assert_eq!(ws.image.px[10 * 100 + 80], 0xFF0000FF, "blue is on top now");
}

#[test]
fn a_growing_selection_costs_its_edges() {
    // A growing flat rectangle damages the new area and its edges, not its
    // middle
    let (e, w, mut sc) = setup(r##"
        PanelWindow {
            color: "#ff101010"
            Rectangle { id: sel; x: 10; y: 10; width: 100; height: 60; color: "#40ffffff"
                        border.width: 1; border.color: "white" }
        }
    "##);
    let mut ws = WinScene::new(200, 120);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 120.0));
    sc.paint(&e.core, w, &mut ws);
    let before = ws.image.px.clone();
    e.eval(w, "(sel.width = 110, sel.height = 66)").unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 120.0));
    let grew = sc.paint(&e.core, w, &mut ws);
    let px: i64 = grew.iter().map(|r| r.area()).sum();
    assert!(px < 110 * 66 / 2, "{px} pixels over {grew:?}");
    assert!(grew.iter().all(|r| r.intersect(&Rect::new(20, 20, 80, 40)).is_none()), "the middle: {grew:?}");
    // And what is on the screen is what painting it all would have given
    let mut whole = WinScene::new(200, 120);
    sc.paint(&e.core, w, &mut whole);
    assert_eq!(ws.image.px, whole.image.px);
    assert_ne!(ws.image.px, before);
}

#[test]
fn the_pointer_finds_the_topmost_area() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            MouseArea { id: under; anchors.fill: parent }
            Rectangle {
                x: 10; y: 10; width: 20; height: 20
                MouseArea { id: over; anchors.fill: parent }
            }
        }
    "#);
    let mut ws = WinScene::new(100, 50);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 50.0));
    sc.paint(&e.core, w, &mut ws);
    let over = e.eval(w, "over").unwrap();
    let hit = sc.hit(&e.core, &ws, 15.0, 15.0).unwrap();
    assert_eq!(e.core.0.borrow().objs[hit].ty, "MouseArea");
    assert!(over.contains("MouseArea"));
    let under = sc.hit(&e.core, &ws, 50.0, 40.0).unwrap();
    assert_ne!(hit, under);
}

/// `/etc/aosh/shell.qml` as it ships, at the numbers `10-bar.conf` had
#[test]
fn the_shipped_bar_is_laid_out_like_mayshells() {
    let mut e = Engine::new().unwrap();
    let services = crate::services::Services::new(&mut e).unwrap();
    let root = e.load_file(Path::new("../etc/aosh/shell.qml")).unwrap();
    let bar = e.core.0.borrow().objs[root].children[0];
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, bar, 1920.0, 48.0));
    assert!(e.take_log().is_empty());

    // Four above and below, eight down the sides, the cards 40 high
    let card = |expr: &str| n(&e, bar, expr);
    assert_eq!(card("open.parent.x + open.parent.parent.x"), 8.0);
    assert_eq!(card("open.parent.parent.y"), 4.0);
    assert_eq!(card("open.parent.height"), 40.0);
    // Nine pills of 26 four apart, the title ten after them
    assert_eq!(card("open.children[0].width"), 9.0 * 26.0 + 8.0 * 4.0);
    assert_eq!(card("open.x"), 10.0);
    assert_eq!(card("open.y"), 7.0);
    assert_eq!(card("open.parent.width"), card("open.implicitWidth") + 20.0);
    // The right-hand card ends eight from the right edge
    assert_eq!(card("machine.parent.x + machine.parent.width + machine.parent.parent.x"), 1912.0);

    // A long title stops at 420 and ends in an ellipsis when painted
    e.with_ctx(|ctx, core| {
        let i = core.prop_idx(services.desktop, "focusedTitle").unwrap();
        core.store(ctx, services.desktop, i, crate::object::value::Value::str(&"a long title ".repeat(60)));
    });
    e.with_ctx(|ctx, core| sc.layout(ctx, core, bar, 1920.0, 48.0));
    assert_eq!(card("open.children[1].width"), 420.0);

    // Active workspace pill uses the accent; switching animates the fill for
    // 250 ms (`Behavior on color`)
    let mut ws = WinScene::new(1920, 48);
    sc.paint(&e.core, bar, &mut ws);
    let colour = |expr: &str| e.eval(bar, expr).unwrap();
    // The Row holds the Repeater, then the nine pills it made
    assert_eq!(card("open.children[0].children.length"), 10.0);
    let pill1 = colour("open.children[0].children[1].color");
    let pill2 = colour("open.children[0].children[2].color");
    assert_ne!(pill1, pill2);
    e.advance(1000);
    e.with_ctx(|ctx, core| core.store_layout(ctx, services.desktop, "workspace", 2.0));
    assert!(e.animating());
    e.advance(1125);
    let half = colour("open.children[0].children[2].color");
    assert!(half != pill1 && half != pill2, "half way: {half}");
    e.advance(1300);
    assert_eq!(colour("open.children[0].children[2].color"), pill1);
    assert_eq!(colour("open.children[0].children[1].color"), pill2);
    assert!(!e.animating());
    e.with_ctx(|ctx, core| sc.layout(ctx, core, bar, 1920.0, 48.0));
    let damage = sc.paint(&e.core, bar, &mut ws).iter().fold(Rect::default(), |a, r| a.union(r));
    assert!(!damage.is_empty(), "the pills changed");
    // Damage covers the pills (26 high) and their antialiasing pixel, not the
    // whole bar
    assert!(damage.w < 200 && damage.h <= 28, "{damage:?}");
}

/// A window opening after startup: `opened` requests the list, then the title
/// appears
#[test]
fn a_window_opened_after_the_shell_reaches_the_bar() {
    use libmay::conn::Event;
    let mut e = Engine::new().unwrap();
    let mut services = crate::services::Services::new(&mut e).unwrap();
    let root = e.load_file(Path::new("../etc/aosh/shell.qml")).unwrap();
    let bar = e.core.0.borrow().objs[root].children[0];
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));

    assert!(services.desk(&e, &Event::DeskOpened { client: 4, surface: 7 }), "the list is asked for again");
    let mut title = [0u8; 32];
    title[..7].copy_from_slice(b"console");
    services.desk(&e, &Event::DeskWindow {
        client: 4, surface: 7, title, title_len: 7, app_id: [0; 32], app_id_len: 0,
        x: 4, y: 52, width: 1912, height: 1024, flags: 0, workspace: 1,
    });
    assert!(services.desk(&e, &Event::DeskFocused { client: 4, surface: 7 }));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, bar, 1920.0, 48.0));
    assert_eq!(e.eval(bar, "open.children[1].text").unwrap(), "console");
    assert!(n(&e, bar, "open.children[1].width") > 30.0);
    // Windows on the first workspace, none on the second [the Repeater, pill 1,
    // pill 2, ...]
    assert_eq!(e.eval(bar, "open.children[0].children[1].busy").unwrap(), "true");
    assert_eq!(e.eval(bar, "open.children[0].children[2].busy").unwrap(), "false");
}

/// `Quickshell.Hyprland`: monitor, workspaces, windows, keyboard `rawEvent`,
/// with stable objects, and workspace switching through `dispatch`
#[test]
fn the_desktop_reads_as_hyprland() {
    use libmay::conn::Event;
    let mut e = Engine::new().unwrap();
    let mut services = crate::services::Services::new(&mut e).unwrap();
    let root = e.load_source("hypr.qml", r#"
        import QtQuick
        import Quickshell
        import Quickshell.Hyprland
        Item {
            property HyprlandMonitor monitor: Hyprland.monitorFor(Quickshell.screens[0])
            property list<HyprlandWorkspace> mine: Hyprland.workspaces.values.filter(w => w.monitor && w.monitor.name == monitor.name)
            property HyprlandWorkspace shown: Hyprland.focusedWorkspace
            property int open: shown ? shown.toplevels.values.length : -1
            property string title: Hyprland.activeToplevel?.title ?? ""
            property bool fullscreen: shown.toplevels.values.some(t => t.wayland?.fullscreen)
            property string layout
            property var events: []
            Connections {
                target: Hyprland
                function onRawEvent(event) {
                    if (event.name == "activelayout") layout = event.parse(2)[1]
                    events = events.concat([event.name])
                }
            }
            // quickshell's Wayland side of the same windows, as end4's bar
            // reads it
            property Toplevel active: ToplevelManager.activeToplevel
            property string activeAddress: active?.HyprlandToplevel?.address ? `0x${active.HyprlandToplevel.address}` : ""
            property var clients: Hyprland.toplevels.values.map(t => t.lastIpcObject)
        }
    "#, Path::new(".")).unwrap();
    // Only the shown workspace, with no windows
    assert_eq!(e.eval(root, "mine.length").unwrap(), "1");
    assert_eq!(e.eval(root, "monitor === Hyprland.focusedMonitor").unwrap(), "true");
    assert_eq!((e.eval(root, "shown.id").unwrap(), e.eval(root, "open").unwrap()), ("1".into(), "0".into()));

    let window = |client: u32, name: &[u8], workspace: u32| {
        let mut title = [0u8; 32];
        title[..name.len()].copy_from_slice(name);
        Event::DeskWindow {
            client, surface: 1, title, title_len: name.len(), app_id: [0; 32], app_id_len: 0,
            x: 0, y: 0, width: 100, height: 100, flags: 0, workspace,
        }
    };
    services.desk(&e, &window(4, b"console", 1));
    services.desk(&e, &window(5, b"zed", 2));
    services.desk(&e, &Event::DeskFocused { client: 4, surface: 1 });
    assert_eq!((e.eval(root, "open").unwrap(), e.eval(root, "title").unwrap()), ("1".into(), "console".into()));
    assert_eq!(e.eval(root, "fullscreen").unwrap(), "false");
    let toplevel = e.eval(root, "String(Hyprland.toplevels.values.length)").unwrap();
    assert_eq!(toplevel, "2");
    // The same window both ways, its client JSON as `hyprctl clients -j`
    assert_eq!(e.eval(root, "String(ToplevelManager.toplevels.values.length)").unwrap(), "2");
    assert_eq!(e.eval(root, "active.title + ' ' + active.activated").unwrap(), "console true");
    assert_eq!(e.eval(root, "activeAddress").unwrap(), "0x400000001");
    assert_eq!(e.eval(root, "active.HyprlandToplevel.wayland === active").unwrap(), "true");
    assert_eq!(e.eval(root, "JSON.stringify(clients.map(c => [c.address, c.size, c.workspace.id, c.title]))").unwrap(),
               r#"[["0x400000001",[100,100],1,"console"],["0x500000001",[100,100],2,"zed"]]"#);
    assert_eq!(e.eval(root, "Hyprland.workspaces.values[1].lastIpcObject.windows").unwrap(), "1");
    assert_eq!(e.eval(root, "events.filter(n => n == 'openwindow').length + ' ' + events.includes('activewindowv2')").unwrap(), "2 true");

    assert_eq!(e.eval(root, "JSON.stringify(mine.map(w => w.id))").unwrap(), "[1,2]");
    services.desk(&e, &Event::DeskWorkspace { index: 2 });
    assert_eq!((e.eval(root, "shown.id").unwrap(), e.eval(root, "open").unwrap()), ("2".into(), "1".into()));
    assert_eq!(e.eval(root, "mine[1] === shown && mine[1].active && !mine[0].active").unwrap(), "true");

    services.desk(&e, &Event::DeskClosed { client: 5, surface: 1 });
    assert_eq!(e.eval(root, "open").unwrap(), "0");
    assert_eq!(e.eval(root, "events.includes('closewindow') + ' ' + ToplevelManager.toplevels.values.length").unwrap(), "true 1");

    let (mut code, mut name) = ([0u8; 32], [0u8; 32]);
    code[..2].copy_from_slice(b"ru");
    name[..7].copy_from_slice(b"Russian");
    services.desk(&e, &Event::DeskKeyboardLayout { group: 1, groups: 2, code, code_len: 2, name, name_len: 7 });
    assert_eq!(e.eval(root, "layout").unwrap(), "Russian");

    e.eval(root, "Hyprland.dispatch('hl.dsp.focus({ workspace = 3 })')").unwrap();
    e.eval(root, "active.activate()").unwrap();
    assert_eq!(e.core.0.borrow().actions, vec![crate::object::Action::Workspace(3), crate::object::Action::Focus(4, 1)]);
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// An unset corner follows `radius`; a set one is independent. A binding on it
/// follows too (matches qml6)
#[test]
fn a_corner_left_alone_reads_as_the_radius() {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("radii.qml", r#"
        import QtQuick
        import QtQuick.Effects
        Item {
            property alias a: ra
            property alias b: rb
            property alias c: rc
            property alias d: rd
            property real copied: ra.topLeftRadius
            Rectangle { id: ra; radius: 11 }
            Rectangle { id: rb; radius: 11; topLeftRadius: 3 }
            Rectangle { id: rc }
            RectangularShadow { id: rd; radius: 7 }
        }
    "#, Path::new(".")).unwrap();
    let ev = |x: &str| e.eval(root, x).unwrap();
    assert_eq!(ev("[a.topLeftRadius, a.bottomRightRadius, copied].join(' ')"), "11 11 11");
    ev("a.radius = 5");
    assert_eq!(ev("[a.topLeftRadius, copied].join(' ')"), "5 5");
    assert_eq!(ev("[b.topLeftRadius, b.topRightRadius].join(' ')"), "3 11");
    ev("b.radius = 20");
    assert_eq!(ev("[b.topLeftRadius, b.topRightRadius].join(' ')"), "3 20");
    assert_eq!(ev("[c.topLeftRadius, d.topLeftRadius].join(' ')"), "0 7");
}

/// `centerIn` keeps the item's size whatever lines are set, and `fill` beats
/// `centerIn` (matches qml6)
#[test]
fn a_centred_item_keeps_its_size_whatever_its_lines() {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("anchors.qml", r#"
        import QtQuick
        Item {
            width: 200; height: 40
            property alias a: ia
            property alias b: ib
            property alias c: ic
            property alias d: id_
            Item { id: ia; anchors.top: parent.top; anchors.bottom: parent.bottom; anchors.centerIn: parent; implicitWidth: 50; implicitHeight: 26 }
            Item { id: ib; anchors.fill: parent; anchors.centerIn: parent; implicitWidth: 50; implicitHeight: 26 }
            Item {
                id: ic
                anchors.left: parent.left; anchors.right: parent.right; anchors.top: parent.top; anchors.bottom: parent.bottom
                anchors.centerIn: parent
                implicitWidth: 50; implicitHeight: 26
            }
            Item { id: id_; anchors.centerIn: parent; anchors.top: parent.top; implicitWidth: 50; implicitHeight: 26 }
        }
    "#, Path::new(".")).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 200.0, 40.0));
    let geo = |n: &str| e.eval(root, &format!("[{n}.x, {n}.y, {n}.width, {n}.height].join(' ')")).unwrap();
    assert_eq!((geo("a"), geo("b"), geo("c"), geo("d")),
               ("75 7 50 26".into(), "0 0 200 40".into(), "75 7 50 26".into(), "75 7 50 26".into()));
}

/// ScreencopyView requests pixels when given a Toplevel, reports them
/// (`hasContent`, `sourceSize`), and draws them stretched. Removing the source
/// requests again, releasing the copy
#[test]
fn a_screencopy_view_shows_its_windows_pixels() {
    use crate::object::Action;
    use libmay::conn::Event;
    let mut e = Engine::new().unwrap();
    let mut services = crate::services::Services::new(&mut e).unwrap();
    let root = e.load_source("copy.qml", r#"
        import QtQuick
        import Quickshell.Wayland
        Item {
            width: 40; height: 20
            property bool open: false
            ScreencopyView {
                id: view
                anchors.fill: parent
                captureSource: open ? ToplevelManager.toplevels.values[0] : null
            }
            property alias view: view
        }
    "#, Path::new(".")).unwrap();
    let mut title = [0u8; 32];
    title[..3].copy_from_slice(b"zed");
    services.desk(&e, &Event::DeskWindow {
        client: 4, surface: 7, title, title_len: 3, app_id: [0; 32], app_id_len: 0,
        x: 0, y: 0, width: 4, height: 2, flags: 0, workspace: 1,
    });
    let view = e.eval(root, "String(view.hasContent)").unwrap();
    assert_eq!(view, "false");
    e.core.0.borrow_mut().actions.clear();
    e.eval(root, "open = true").unwrap();
    let asked: Vec<Action> = std::mem::take(&mut e.core.0.borrow_mut().actions);
    let Some(Action::Capture(v)) = asked.first().cloned() else { panic!("no capture asked for: {asked:?}") };
    assert_eq!(crate::services::capture::window(&e.core, v), Some((4, 7)));

    // A two-pixel window, red then blue, drawn over 40
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    let px = vec![0xFFFF_0000, 0xFF00_00FF, 0xFFFF_0000, 0xFF00_00FF];
    sc.set_capture(&e.core, v, Some(std::rc::Rc::new(super::picture::Picture::new(2, 2, px))));
    e.with_ctx(|ctx, core| crate::services::capture::took(ctx, core, v, 2, 2));
    assert_eq!(e.eval(root, "view.hasContent + ' ' + view.sourceSize.width + 'x' + view.sourceSize.height").unwrap(), "true 2x2");
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 40.0, 20.0));
    let mut ws = WinScene::new(40, 20);
    sc.paint(&e.core, root, &mut ws);
    let at = |x: i32| ws.image.px[(10 * 40 + x) as usize];
    assert_eq!((at(2), at(37)), (0xFFFF_0000, 0xFF00_00FF));
    // Nearest sampling, as quickshell 7511545 draws it
    assert_eq!((at(19), at(20)), (0xFFFF_0000, 0xFF00_00FF));

    e.eval(root, "open = false").unwrap();
    assert_eq!(e.eval(root, "String(view.hasContent)").unwrap(), "false");
    assert!(matches!(e.core.0.borrow().actions.last(), Some(Action::Capture(x)) if *x == v));
}

/// A window's `mask` as the input region: an item's box with a corner
/// subtracted, nothing for a null item, the whole window with no mask
#[test]
fn a_mask_is_where_the_pointer_reaches() {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("mask.qml", r#"
        import QtQuick
        import Quickshell
        Item {
            property bool open: true
            PanelWindow {
                id: win
                width: 300; height: 100
                mask: Region {
                    item: open ? band : null
                    Region { intersection: Intersection.Subtract; x: 10; y: 20; width: 5; height: 5 }
                    Region { intersection: Intersection.Subtract; width: 0; height: 0 }
                }
                Item { id: band; x: 10; y: 20; width: 200; height: 30 }
            }
            PanelWindow { id: bare; width: 50; height: 50 }
            property var w: win
            property var b: bare
        }
    "#, Path::new(".")).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    let win = e.core.peek_named(root, "w").and_then(|v| v.as_obj()).unwrap();
    let bare = e.core.peek_named(root, "b").and_then(|v| v.as_obj()).unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, win, 300.0, 100.0));
    assert_eq!(crate::mayland::input_of(&e.core, win), Some(vec![(true, 10, 20, 200, 30), (false, 10, 20, 5, 5)]));
    e.set(root, "open", crate::object::value::Value::Bool(false)).unwrap();
    assert_eq!(crate::mayland::input_of(&e.core, win), Some(vec![(false, 10, 20, 5, 5)]));
    assert_eq!(crate::mayland::input_of(&e.core, bare), None);
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// Minimal QOI file: header, QOI_OP_RGBA pixels, end marker
fn qoi(w: u32, h: u32, px: &[u32]) -> Vec<u8> {
    let mut out = b"qoif".to_vec();
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.extend_from_slice(&[4, 0]);
    for &p in px {
        out.extend_from_slice(&[0xFF, (p >> 16) as u8, (p >> 8) as u8, p as u8, (p >> 24) as u8]);
    }
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out
}

#[test]
fn an_image_is_its_picture_and_fits_its_box() {
    let dir = std::env::temp_dir().join(format!("aosh-img-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Four by two, red on the left half, blue on the right
    let px = [0xFFFF0000u32, 0xFFFF0000, 0xFF0000FF, 0xFF0000FF, 0xFFFF0000, 0xFFFF0000, 0xFF0000FF, 0xFF0000FF];
    std::fs::write(dir.join("pic.qoi"), qoi(4, 2, &px)).unwrap();
    let src = r#"
        PanelWindow {
            Image { id: natural; source: "pic.qoi" }
            Image { id: fit; y: 10; width: 40; height: 40; source: "pic.qoi"; fillMode: Image.PreserveAspectFit }
        }
    "#;
    let mut e = Engine::new().unwrap();
    let w = e.load_source(&dir.join("test.qml").display().to_string(), src, &dir).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 100.0, 60.0));
    assert_eq!(n(&e, w, "natural.width"), 4.0);
    assert_eq!(n(&e, w, "natural.height"), 2.0);
    assert_eq!(n(&e, w, "natural.status"), 1.0);
    let mut ws = WinScene::new(100, 60);
    sc.paint(&e.core, w, &mut ws);
    // Fitted: 40 wide, 20 high, centered in the 40x40 box (y 20..40)
    let at = |x: i32, y: i32| super::paint::to_straight(ws.image.px[(y * 100 + x) as usize]);
    assert_eq!(at(5, 30), 0xFFFF0000);
    assert_eq!(at(35, 30), 0xFF0000FF);
    assert_eq!(at(20, 15) >> 24, 0, "above the fitted picture is clear");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_repeater_in_a_row_takes_no_room_and_a_list_view_scrolls() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Row {
                id: row
                spacing: 4
                Repeater { model: 3; Rectangle { width: 10; height: 10 } }
                Rectangle { id: after; width: 5; height: 5 }
            }
            ListView {
                id: lv
                y: 20; width: 50; height: 30
                spacing: 5
                model: 4
                delegate: Rectangle { width: 50; height: 20 }
            }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 60.0));
    // Three of ten with gaps of four, then the last; the Repeater has no
    // position in the row
    assert_eq!(n(&e, w, "after.x"), 3.0 * 14.0);
    assert_eq!(n(&e, w, "row.width"), 3.0 * 14.0 + 5.0);
    assert_eq!(n(&e, w, "lv.contentHeight"), 4.0 * 20.0 + 3.0 * 5.0);
    assert_eq!(n(&e, w, "lv.children[1].y"), 25.0);
    e.eval(w, "lv.positionViewAtEnd()").unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 60.0));
    assert_eq!(n(&e, w, "lv.contentY"), 95.0 - 30.0);
    assert_eq!(n(&e, w, "lv.children[1].y"), 25.0 - 65.0);
}

/// ListView with top and bottom margins: items start below the top one, the end
/// is past the bottom one
#[test]
fn a_list_views_margins_are_room_before_and_after() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            ListView {
                id: lv
                width: 50; height: 30
                topMargin: 10; bottomMargin: 6
                spacing: 5
                model: 4
                delegate: Rectangle { width: 50; height: 20 }
            }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 60.0));
    assert_eq!(n(&e, w, "lv.contentHeight"), 95.0, "the margins are not content");
    assert_eq!(n(&e, w, "lv.children[0].y"), 10.0);
    assert_eq!(n(&e, w, "lv.children[1].y"), 35.0);
    e.eval(w, "lv.positionViewAtEnd()").unwrap();
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 60.0));
    assert_eq!(n(&e, w, "lv.contentY"), 10.0 + 95.0 + 6.0 - 30.0);
    assert_eq!(n(&e, w, "lv.children[3].y"), 85.0 - 81.0);
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

#[test]
fn a_loader_is_its_items_size_unless_it_has_one() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Loader { id: free; sourceComponent: Rectangle { implicitWidth: 30; implicitHeight: 12 } }
            Loader { id: sized; width: 70; height: 8; sourceComponent: Rectangle { implicitWidth: 30; implicitHeight: 12 } }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 200.0, 60.0));
    assert_eq!(n(&e, w, "free.width"), 30.0);
    assert_eq!(n(&e, w, "free.height"), 12.0);
    assert_eq!(n(&e, w, "sized.item.width"), 70.0);
    assert_eq!(n(&e, w, "sized.item.height"), 8.0);
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// Reload without a display: a save is noticed, the new tree is built beside
/// the old, and the old is destroyed, or stays if the new one fails
#[test]
fn a_saved_file_reloads_into_a_new_tree() {
    let dir = std::env::temp_dir().join(format!("aosh-reload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("shell.qml");
    std::fs::write(&file, "Item { property int version: 1 }").unwrap();
    let mut e = Engine::new().unwrap();
    let old = e.load_file(&file).unwrap();
    crate::io::watch_config(&e.core, &[dir.canonicalize().unwrap()]);
    let live = e.core.0.borrow().objs.live();

    std::fs::write(&file, "Item { property int version: 2 }").unwrap();
    let start = std::time::Instant::now();
    while e.core.0.borrow().io.reload_due.is_none() && start.elapsed().as_secs() < 2 {
        for (_, f) in e.io_fds() {
            e.io_ready(f);
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(e.core.0.borrow().io.reload_due.is_some(), "the save was noticed");

    e.core.0.borrow().docs.borrow_mut().clear();
    let new = e.load_file(&file).unwrap();
    e.destroy(old);
    assert_eq!(e.eval(new, "version").unwrap(), "2");
    assert!(!e.core.0.borrow().objs.is_live(old));
    assert_eq!(e.core.0.borrow().objs.live(), live, "the old tree is gone, all of it");

    // A load error keeps the current tree
    std::fs::write(&file, "Item { property int version: }").unwrap();
    e.core.0.borrow().docs.borrow_mut().clear();
    assert!(e.load_file(&file).is_err());
    assert_eq!(e.eval(new, "version").unwrap(), "2");

    // An emptied component file: the using file still loads without that part,
    // but the reload keeps the old tree (`shell.rs`). Used twice, counted
    // twice, logged once
    std::fs::write(dir.join("Part.qml"), "").unwrap();
    std::fs::write(&file, "Item {\n property int version: 3\n Part {}\n Part {}\n}").unwrap();
    e.core.0.borrow().docs.borrow_mut().clear();
    let _ = e.take_log();
    let before = e.core.0.borrow().load_errors;
    let partial = e.load_file(&file).unwrap();
    assert_eq!(e.core.0.borrow().load_errors, before + 2, "the empty file is counted");
    let said: Vec<String> = e.take_log().into_iter().filter(|l| l.contains("Part.qml")).collect();
    assert_eq!(said.len(), 1, "{said:?}");
    e.destroy(partial);
    let _ = std::fs::remove_dir_all(&dir);
}

/// TextField: Basic style size, placeholder while empty, caret only with the
/// keyboard
#[test]
fn a_text_field_draws_its_placeholder_and_its_caret() {
    let (e, w, mut sc) = setup(r##"
        PanelWindow {
            color: "#ff000000"
            TextField {
                id: f
                placeholderText: "Поиск..."
                placeholderTextColor: "#ff808080"
                color: "#ffffffff"
                font.pixelSize: 16
                focus: true
            }
        }
    "##);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 60.0));
    assert_eq!((n(&e, w, "f.width"), n(&e, w, "f.height")), (200.0, 40.0));
    let mut ws = WinScene::new(300, 60);
    sc.paint(&e.core, w, &mut ws);
    let lit = |ws: &WinScene, x0: usize, x1: usize, want: u32| {
        (10..30).any(|y| (x0..x1).any(|x| ws.image.px[y * 300 + x] & 0x00ff_ffff == want & 0x00ff_ffff && ws.image.px[y * 300 + x] >> 24 > 0x80))
    };
    let grey = |ws: &WinScene| (10..30).any(|y| (10..120).any(|x| {
        let p = ws.image.px[y * 300 + x];
        let (r, g) = ((p >> 16) & 0xff, (p >> 8) & 0xff);
        r > 0x40 && r == g
    }));
    assert!(grey(&ws), "the placeholder is drawn");
    assert!(!lit(&ws, 9, 12, 0xffffffff), "no caret without the keyboard");

    e.keyboard(Some(w));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 60.0));
    sc.paint(&e.core, w, &mut ws);
    assert!(lit(&ws, 9, 12, 0xffffffff), "the caret, at the left padding");

    e.key(0, Some('W'), 0);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 60.0));
    sc.paint(&e.core, w, &mut ws);
    assert!(lit(&ws, 10, 30, 0xffffffff), "the letter in the text's colour");
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}

/// `/etc/aosh/Launcher.qml`: opened by its word, typed into, navigated, run,
/// and closed
#[test]
fn the_shipped_launcher_opens_filters_runs_and_closes() {
    let mut e = Engine::new().unwrap();
    let services = crate::services::Services::new(&mut e).unwrap();
    let dir = std::env::temp_dir().join(format!("aosh-launcher-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("10-terminal.conf"), "app {\n    name = Терминал\n    exec = true\n}\n").unwrap();
    std::fs::write(dir.join("20-zed.conf"), "app {\n    name = Zed\n    exec = true --zed\n}\n").unwrap();
    e.with_ctx(|ctx, core| crate::services::apps::publish(ctx, core, services.apps, &dir));
    let root = e.load_file(Path::new("../etc/aosh/shell.qml")).unwrap();
    let said = e.take_log();
    assert!(said.is_empty(), "{said:?}");
    let win = e.core.0.borrow().objs[root].children[1];
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    let settle = |e: &Engine, sc: &mut Scene, t: u64| {
        e.advance(t);
        e.with_ctx(|ctx, core| sc.layout(ctx, core, win, 720.0, 660.0));
    };
    assert_eq!(e.eval(win, "visible").unwrap(), "false");

    // The chord's word opens it, empty, on the first row
    assert!(e.with_ctx(|ctx, core| crate::ipc::word(ctx, core, "launcher toggle")));
    assert_eq!(e.eval(win, "visible").unwrap(), "true");
    e.keyboard(Some(win));
    assert_eq!(e.eval(win, "search.activeFocus").unwrap(), "true");
    settle(&e, &mut sc, 1000);
    settle(&e, &mut sc, 2000);
    assert_eq!(n(&e, win, "list.count"), 2.0);
    assert_eq!(n(&e, win, "card.height"), 65.0 + 1.0 + 20.0 + 2.0 * 60.0 + 4.0, "the card fits two rows");

    // Typing filters, case-insensitive; the card shrinks to one row
    for c in "zE".chars() {
        e.key(0, Some(c), 0);
    }
    settle(&e, &mut sc, 3000);
    settle(&e, &mut sc, 4000);
    assert_eq!(n(&e, win, "list.count"), 1.0);
    assert_eq!(e.eval(win, "launcher.found[0].name").unwrap(), "Zed");
    assert_eq!(n(&e, win, "card.height"), 65.0 + 1.0 + 20.0 + 60.0);

    // Back to everything; Down navigates, Return runs and closes
    for _ in 0..2 {
        e.key(14, None, 0);
    }
    e.key(108, None, 0);
    e.key(108, None, 0);
    assert_eq!(n(&e, win, "list.currentIndex"), 1.0, "and stops at the end");
    let before = e.core.0.borrow().children.len();
    e.key(28, Some('\r'), 0);
    assert_eq!(e.eval(win, "visible").unwrap(), "false");
    assert_eq!(e.core.0.borrow().children.len(), before + 1, "the program was started");

    // Reopened empty; Escape closes
    e.with_ctx(|ctx, core| crate::ipc::call(ctx, core, "launcher", "open", &[])).unwrap();
    assert_eq!(e.eval(win, "search.text + list.currentIndex").unwrap(), "0");
    e.key(1, None, 0);
    assert_eq!(e.eval(win, "visible").unwrap(), "false");
    let said = e.take_log();
    assert!(said.is_empty(), "{said:?}");
    for mut c in std::mem::take(&mut e.core.0.borrow_mut().children) {
        let _ = c.wait();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Grid three across, or one across and three down
#[test]
fn a_grid_lays_out_cells() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Grid {
                id: g
                columns: 3
                spacing: 10
                Rectangle { id: a; width: 50; height: 20 }
                Rectangle { id: b; width: 30; height: 40 }
                Rectangle { id: c; width: 20; height: 20 }
                Rectangle { id: d; width: 10; height: 10 }
            }
            RowLayout {
                id: lay
                y: 100
                width: 200
                Rectangle { id: m; implicitWidth: 20; implicitHeight: 10; Layout.leftMargin: 7; Layout.topMargin: 4 }
                Rectangle { id: f; Layout.fillWidth: true; implicitHeight: 10; Layout.margins: 3 }
            }
        }
    "#);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 400.0, 300.0));
    // A layout's margins are room around the item, inside its cell
    assert_eq!(n(&e, w, "m.x"), 7.0);
    assert_eq!(n(&e, w, "f.x"), 7.0 + 20.0 + 5.0 + 3.0);
    assert_eq!(n(&e, w, "f.width"), 200.0 - 27.0 - 5.0 - 6.0);
    assert_eq!(n(&e, w, "b.x") + n(&e, w, "c.x"), 60.0 + 100.0);
    assert_eq!(n(&e, w, "d.x") + n(&e, w, "d.y"), 0.0 + 50.0, "the second row is below the tallest of the first");
    assert_eq!(n(&e, w, "g.width") * 1000.0 + n(&e, w, "g.height"), (50.0 + 30.0 + 20.0 + 20.0) * 1000.0 + 60.0);
}

/// Loads and paints a `tests/qtref/fixtures/` fixture against Qt's render
/// (`tests/qtref/make.sh`): each case's cell, eight to a row, per channel.
/// Returns the worst cell's mean difference, the largest anywhere, and a line
/// per cell; None without a render. Writes both to
/// `target/qtref/NAME-aosh.png`.
/// Numbers in JSON text by path (`cards[2].self.y`), to read a fixture's
/// `report()`
fn flat_json(s: &str) -> Vec<(String, f64)> {
    fn value(b: &[u8], i: &mut usize, path: &str, out: &mut Vec<(String, f64)>) {
        let skip = |i: &mut usize| while *i < b.len() && b[*i].is_ascii_whitespace() { *i += 1 };
        skip(i);
        match b[*i] {
            b'{' => {
                *i += 1;
                loop {
                    skip(i);
                    if b[*i] == b'}' { *i += 1; return; }
                    if b[*i] == b',' { *i += 1; continue; }
                    let start = *i + 1;
                    let end = start + b[start..].iter().position(|&c| c == b'"').unwrap();
                    let key = std::str::from_utf8(&b[start..end]).unwrap();
                    *i = end + 1;
                    skip(i);
                    *i += 1; // the colon
                    let p = if path.is_empty() { key.to_string() } else { format!("{path}.{key}") };
                    value(b, i, &p, out);
                }
            }
            b'[' => {
                *i += 1;
                let mut k = 0;
                loop {
                    skip(i);
                    if b[*i] == b']' { *i += 1; return; }
                    if b[*i] == b',' { *i += 1; k += 1; continue; }
                    value(b, i, &format!("{path}[{k}]"), out);
                }
            }
            b'"' => {
                let end = *i + 1 + b[*i + 1..].iter().position(|&c| c == b'"').unwrap();
                *i = end + 1;
            }
            _ => {
                let start = *i;
                while *i < b.len() && !matches!(b[*i], b',' | b'}' | b']') && !b[*i].is_ascii_whitespace() {
                    *i += 1;
                }
                if let Ok(v) = std::str::from_utf8(&b[start..*i]).unwrap().parse::<f64>() {
                    out.push((path.to_string(), v));
                }
            }
        }
    }
    let mut out = Vec::new();
    value(s.trim().as_bytes(), &mut 0, "", &mut out);
    out
}

fn against_qt(name: &str, rows: &[(i32, i32)]) -> Option<(f64, i32, String)> {
    let qt = image::open(format!("../target/qtref/{name}.png")).ok()?.to_rgba8();
    let (w, h) = (qt.width() as i32, qt.height() as i32);
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new(&format!("tests/qtref/fixtures/{name}.qml"))).unwrap();
    // Fonts qml6 had, including the application font
    let fonts = super::text::tests::reference_fonts().unwrap_or_else(|| Fonts::load(&[Path::new("../assets")]));
    let mut sc = Scene::new(fonts);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, w as f64, h as f64));
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
    let mut ws = WinScene::new(w, h);
    sc.paint(&e.core, root, &mut ws);
    let ours = |x: i32, y: i32| -> [i32; 3] {
        let c = super::paint::to_straight(ws.image.px[(y * w + x) as usize]);
        [(c >> 16 & 0xFF) as i32, (c >> 8 & 0xFF) as i32, (c & 0xFF) as i32]
    };
    let theirs = |x: i32, y: i32| -> [i32; 3] {
        let c = qt.get_pixel(x as u32, y as u32).0;
        [c[0] as i32, c[1] as i32, c[2] as i32]
    };
    let mut report = Vec::new();
    let (mut worst, mut furthest) = (0.0f64, 0);
    // Cells 80 wide from x 5, as many as the fixture is wide
    let cols = ((w - 10) / 80).max(1);
    for (r, &(y0, y1)) in rows.iter().enumerate() {
        for col in 0..cols {
            let (x0, x1) = (col * 80 + 5, (col * 80 + 85).min(w));
            let (mut sum, mut ink, mut max) = (0i64, 0i64, 0);
            for y in y0..y1.min(h) {
                for x in x0..x1 {
                    let (a, b) = (ours(x, y), theirs(x, y));
                    if a.iter().chain(&b).any(|&v| v < 255) {
                        let d = (0..3).map(|k| (a[k] - b[k]).abs()).max().unwrap_or(0);
                        sum += d as i64;
                        ink += 1;
                        max = max.max(d);
                    }
                }
            }
            let mean = sum as f64 / ink.max(1) as f64;
            worst = worst.max(mean);
            furthest = furthest.max(max);
            report.push(format!("case {:>2}: mean {mean:6.2} max {max:3} over {ink} px", r * cols as usize + col as usize));
        }
    }
    let mut both = image::RgbaImage::from_pixel(w as u32, (2 * h + 4) as u32, image::Rgba([255, 0, 0, 255]));
    for y in 0..h {
        for x in 0..w {
            both.put_pixel(x as u32, y as u32, *qt.get_pixel(x as u32, y as u32));
            let v = ours(x, y);
            both.put_pixel(x as u32, (y + h + 4) as u32, image::Rgba([v[0] as u8, v[1] as u8, v[2] as u8, 255]));
        }
    }
    let _ = both.save(format!("../target/qtref/{name}-aosh.png"));
    Some((worst, furthest, report.join("\n")))
}

/// `rects.qml`: rounded, bordered, gradient, and rotated Rectangles
#[test]
fn rectangles_are_qts() {
    let Some((worst, furthest, report)) = against_qt("rects", &[(0, 85), (85, 165), (165, 250)]) else { return };
    eprintln!("{report}");
    // Within 1/255 where Mesa's reference blends in 8 bits, and 2/255 at the
    // corner of case 5 where a square corner meets round ones
    assert!(furthest <= 2 && worst < 1.0, "aosh's rectangles are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `canvases.qml`: filled and stroked Canvas paths
#[test]
fn canvases_are_qts() {
    let Some((worst, furthest, report)) = against_qt("canvases", &[(0, 85), (85, 170)]) else { return };
    eprintln!("{report}");
    // Within 2/255, except round caps on slanted lines in case 9 (up to 5/255
    // at a few pixels)
    assert!(furthest <= 5 && worst < 1.0, "aosh's canvases are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `effects.qml`: masks, blurs, shadows, and layers
#[test]
fn effects_are_qts() {
    let Some((worst, furthest, report)) = against_qt("effects", &[(0, 85), (85, 170), (170, 255)]) else { return };
    eprintln!("{report}");
    assert!(furthest <= 2 && worst < 1.0, "aosh's effects are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `multieffect.qml`: MultiEffect color and mask
#[test]
fn multieffects_are_qts() {
    let Some((worst, _, report)) = against_qt("multieffect", &[(0, 85), (85, 170)]) else { return };
    eprintln!("{report}");
    // Case 2's mask is text, whose distance fields differ at a few tight curves
    // (`distance_field_text_is_qts`)
    let off = |line: &str| line.split("max").nth(1).and_then(|m| m.split_whitespace().next()?.parse::<i32>().ok());
    for (case, line) in report.lines().enumerate() {
        let allowed = if case == 2 { 32 } else { 3 };
        assert!(off(line).is_some_and(|m| m <= allowed) && worst < 1.5, "aosh's MultiEffect is not Qt's:\n{report}");
    }
}

/// `gradients.qml`: RadialGradient alone, masked, rotated, and as
/// RippleButton's ripple
#[test]
fn gradients_are_qts() {
    let Some((worst, furthest, report)) = against_qt("gradients", &[(0, 90), (90, 170)]) else { return };
    eprintln!("{report}");
    assert!(furthest <= 2 && worst < 1.0, "aosh's gradients are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `shapes.qml`: screen corners, CircularProgress arcs, a polyline, and other
/// curve renderer ends and corners
#[test]
fn shapes_are_qts() {
    let Some((worst, _, report)) = against_qt("shapes", &[(0, 70), (70, 160), (160, 255)]) else { return };
    eprintln!("{report}");
    // Within 3/255 except where Qt's triangulation decides: three pixels at
    // case 1's bottom right corner and one inside a sharp fill corner in case 7
    let off = |line: &str| line.split("max").nth(1).and_then(|m| m.split_whitespace().next()?.parse::<i32>().ok());
    for (case, line) in report.lines().enumerate() {
        let allowed = match case { 1 => 42, 7 => 16, _ => 3 };
        assert!(off(line).is_some_and(|m| m <= allowed) && worst < 1.0, "aosh's shapes are not Qt's:\n{report}");
    }
}

/// `shapes.qml`: sizes of Shapes with no explicit size
#[test]
fn shapes_are_as_big_as_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/shapes.json") else { return };
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/shapes.qml")).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "Shapes are not Qt's size:\n{}", wrong.join("\n"));
}

/// `centring.qml`: centered anchors with odd, even, and fractional sizes, an
/// offset, `alignWhenCentered` off, and a sibling
#[test]
fn centring_is_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/centring.json") else { return };
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/centring.qml")).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "centred items are not where Qt puts them:\n{}", wrong.join("\n"));
}

/// `labels.qml`: StyledText centered in a card: position, size, pixels
#[test]
fn labels_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/labels.json") else { return };
    let Some((worst, furthest, report)) = against_qt("labels", &[(0, 70), (70, 130), (130, 190), (190, 250), (250, 310)]) else { return };
    eprintln!("{report}");
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/labels.qml")).unwrap();
    let fonts = super::text::tests::reference_fonts().unwrap_or_else(|| Fonts::load(&[Path::new("../assets")]));
    let mut sc = Scene::new(fonts);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 375.0));
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "the labels are not where Qt puts them:\n{}", wrong.join("\n"));
    assert!(furthest <= 2 && worst < 1.0, "aosh's labels are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `dftext.qml`: distance-field text (Qt's default) in a layer, in a shadow,
/// and at smaller sizes
#[test]
fn distance_field_text_is_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/dftext.json") else { return };
    let Some((worst, furthest, report)) = against_qt("dftext", &[(0, 120), (120, 255)]) else { return };
    eprintln!("{report}");
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/dftext.qml")).unwrap();
    let fonts = super::text::tests::reference_fonts().unwrap_or_else(|| Fonts::load(&[Path::new("../assets")]));
    let mut sc = Scene::new(fonts);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "the text is not measured as Qt's:\n{}", wrong.join("\n"));
    // Remaining difference is at tight curves, flattened differently by Qt
    // (`text/field.rs`): 0.1 to 0.25/255 per text, a few dozen edge pixels over
    // 10
    assert!(worst < 2.5 && furthest <= 64, "aosh's distance-field text is not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `pictures.qml`: pictures scaled to `sourceSize` as Qt's reader does, decoded
/// at twice the item, rotated, outlined by a DropShadow in a layer
#[test]
fn pictures_are_qts() {
    let Some((worst, furthest, report)) = against_qt("pictures", &[(0, 255), (255, 510)]) else { return };
    eprintln!("{report}");
    assert!(furthest <= 4 && worst < 1.0, "aosh's pictures are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `controls.qml`: Basic style Controls: part positions from `report()`, and
/// pixels
#[test]
fn controls_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/controls.json") else { return };
    let Some((worst, furthest, report)) = against_qt("controls", &[(0, 85), (85, 170), (170, 255)]) else { return };
    eprintln!("{report}");
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/controls.qml")).unwrap();
    let fonts = super::text::tests::reference_fonts().unwrap_or_else(|| Fonts::load(&[Path::new("../assets")]));
    let mut sc = Scene::new(fonts);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let ours = e.eval(root, "JSON.stringify(report())").unwrap();
    let (theirs, ours) = (flat_json(&json), flat_json(&ours));
    let mut wrong = Vec::new();
    for (at, v) in &theirs {
        // The Basic style's ScrollBar background shows only in high contrast
        // and is not created
        if at.starts_with("s5.bg") {
            continue;
        }
        match ours.iter().find(|(a, _)| a == at) {
            Some((_, o)) if (o - v).abs() < 0.01 => {}
            Some((_, o)) => wrong.push(format!("{at}: Qt {v}, aosh {o}")),
            None => wrong.push(format!("{at}: Qt {v}, aosh nothing")),
        }
    }
    assert!(wrong.is_empty(), "the Controls are not where Qt put them:\n{}", wrong.join("\n"));
    assert!(furthest <= 2 && worst < 1.0, "aosh's controls are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `layouts.qml`: RowLayout and ColumnLayout with spare and short space and
/// fractional sizes
#[test]
fn layouts_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/layouts.json") else { return };
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/layouts.qml")).unwrap();
    let mut sc = Scene::new(Fonts::load(&[Path::new("../assets")]));
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let log = e.take_log();
    assert!(log.is_empty(), "{log:?}");
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "the layouts are not Qt's:\n{}", wrong.join("\n"));
}

/// `textedit.qml`: TextEdit metrics and pixels against a Text and a TextInput
#[test]
fn text_edits_are_qts() {
    let Ok(json) = std::fs::read_to_string("../target/qtref/textedit.json") else { return };
    let Some((worst, furthest, report)) = against_qt("textedit", &[(0, 130), (130, 255)]) else { return };
    eprintln!("{report}");
    let mut e = Engine::new().unwrap();
    let root = e.load_file(Path::new("tests/qtref/fixtures/textedit.qml")).unwrap();
    let fonts = super::text::tests::reference_fonts().unwrap_or_else(|| Fonts::load(&[Path::new("../assets")]));
    let mut sc = Scene::new(fonts);
    e.with_ctx(|ctx, core| sc.layout(ctx, core, root, 650.0, 255.0));
    let ours = flat_json(&e.eval(root, "JSON.stringify(report())").unwrap());
    let wrong: Vec<String> = flat_json(&json).iter().filter_map(|(at, v)| match ours.iter().find(|(a, _)| a == at) {
        Some((_, o)) if (o - v).abs() < 0.01 => None,
        o => Some(format!("{at}: Qt {v}, aosh {:?}", o.map(|o| o.1))),
    }).collect();
    assert!(wrong.is_empty(), "the text edits are not measured as Qt's:\n{}", wrong.join("\n"));
    assert!(furthest <= 2 && worst < 1.0, "aosh's text edits are not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// `bigtext.qml`: text at the size where Qt stops caching glyphs
#[test]
fn big_text_is_qts() {
    let Some((worst, furthest, report)) = against_qt("bigtext", &[(0, 85), (85, 170), (170, 255)]) else { return };
    eprintln!("{report}");
    assert!(worst < 1.0, "aosh's big text is not Qt's, by {furthest}/255 at most, {worst:.2} in a cell:\n{report}");
}

/// A Loader filling its parent sizes its item, and item bindings on the
/// parent's size follow
#[test]
fn a_loaded_items_size_reaches_the_bindings_inside_it() {
    let (e, w, mut sc) = setup(r#"
        PanelWindow {
            Item {
                id: top
                width: 230; height: 230
                Loader {
                    id: ld
                    anchors.fill: parent
                    sourceComponent: Item {
                        anchors.fill: parent
                        rotation: 30
                        Rectangle {
                            anchors.verticalCenter: parent.verticalCenter
                            x: parent.width / 2 - 6
                            width: 95; height: 12
                            Behavior on x { NumberAnimation { duration: 200 } }
                        }
                    }
                }
            }
        }
    "#);
    for _ in 0..30 {
        e.with_ctx(|ctx, core| sc.layout(ctx, core, w, 300.0, 300.0));
        let now = e.core.0.borrow().now_ms;
        e.advance(now + 16);
    }
    assert_eq!(n(&e, w, "ld.item.width"), 230.0);
    assert_eq!(n(&e, w, "ld.item.children[0].y"), 109.0);
    assert_eq!(n(&e, w, "ld.item.children[0].x"), 109.0);
}



/// A focus grab admits windows from a JavaScript array of pushed items
#[test]
fn a_focus_grab_reaches_the_windows_it_lists() {
    let mut e = Engine::new().unwrap();
    let root = e.load_source("grab.qml", r#"
        import QtQuick
        import Quickshell
        import Quickshell.Hyprland
        Item {
            id: root
            property list<var> persistent: []
            property list<var> dismissable: []
            PanelWindow { id: bar; width: 300; height: 30 }
            PanelWindow { id: overview; width: 300; height: 300 }
            HyprlandFocusGrab {
                id: grab
                windows: root.dismissable.every(w => !w?.focusable) ? [...root.dismissable, ...root.persistent] : [...root.dismissable]
                active: root.dismissable.length > 0
            }
            property var g: grab
            property var b: bar
            property var o: overview
            function open() { root.persistent.push(bar); root.dismissable.push(overview) }
            function holed() { grab.windows = [overview, null] }
        }
    "#, Path::new(".")).unwrap();
    let get = |n: &str| e.core.peek_named(root, n).and_then(|v| v.as_obj()).unwrap();
    let (grab, bar, overview) = (get("g"), get("b"), get("o"));
    assert!(crate::mayland::grab_windows(&e, grab).is_empty());
    e.eval(root, "open()").unwrap();
    assert!(e.core.flag(grab, "active"));
    assert_eq!(crate::mayland::grab_windows(&e, grab), vec![overview, bar]);
    e.eval(root, "holed()").unwrap();
    assert_eq!(crate::mayland::grab_windows(&e, grab), vec![overview]);
    assert!(e.take_log().is_empty(), "{:?}", e.take_log());
}
