// Controls as end4's widgets make them, in the Basic style quickshell runs
// (`QT_QUICK_CONTROLS_STYLE=Basic`): buttons with a background and a content
// item of their own, a FAB whose content is anchored, delegates in a list,
// paddings and insets. What the picture shows and where each part was put
// (`report()`)
import QtQuick
import QtQuick.Controls

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    component Label: Text {
        font.pixelSize: 15
        renderType: Text.NativeRendering
        color: "#203040"
    }

    // 0 - a RippleButton's shape: a background 30 high, text, the style's
    // padding (6, 8 across)
    Button {
        id: b0
        x: 10; y: 10
        background: Rectangle { implicitHeight: 30; radius: 8; color: "#c0d8f0" }
        contentItem: Label { text: "Lock" }
    }

    // 1 - its own padding, and wider than it would be
    Button {
        id: b1
        x: 90; y: 10; width: 70
        padding: 2
        background: Rectangle { implicitHeight: 30; radius: 15; color: "#f0c0d0" }
        contentItem: Label { text: "Play"; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
    }

    // 2 - a FAB: the content a Row anchored to the button's left and middle
    Button {
        id: b2
        x: 180; y: 10
        implicitWidth: 38; implicitHeight: 38
        background: Rectangle { radius: 10; color: "#5a4a8a" }
        contentItem: Row {
            anchors { verticalCenter: parent?.verticalCenter; left: parent?.left; leftMargin: 11 }
            Rectangle { width: 16; height: 16; color: "white" }
        }
    }

    // 3 - insets: the background inside them, the content inside the padding
    Control {
        id: c3
        x: 250; y: 10; width: 70; height: 60
        topInset: 4; leftInset: 6; rightInset: 2; bottomInset: 8
        padding: 10; leftPadding: 14
        background: Rectangle { color: "#a0d0a0" }
        contentItem: Rectangle { color: "#306030" }
    }

    // 4 - no size given: as big as its content and padding
    Control {
        id: c4
        x: 340; y: 10
        padding: 5; horizontalPadding: 9
        background: Rectangle { color: "#e0e0a0"; implicitWidth: 20; implicitHeight: 20 }
        contentItem: Rectangle { color: "#806020"; implicitWidth: 30; implicitHeight: 12 }
    }

    // 5 - a ScrollBar's shape, on its own
    ScrollBar {
        id: s5
        x: 420; y: 10; height: 60
        policy: ScrollBar.AlwaysOn
        size: 0.4
        position: 0.2
        contentItem: Rectangle { implicitWidth: 4; implicitHeight: 20; radius: 2; color: "#404040" }
    }

    // 6, 7 - notes: SwipeDelegates in a list, no padding, no background,
    // the content a card as wide as the delegate
    ListView {
        id: list
        x: 90; y: 95; width: 200; height: 150
        spacing: 6
        clip: true
        model: ["miku os: fix all known bugs, compile the world", "miku: change TCBAIN; TRAI.", "third"]
        delegate: SwipeDelegate {
            id: card
            required property var modelData
            required property int index
            width: list.width
            implicitHeight: 55
            padding: 0
            background: null
            clip: true
            contentItem: Rectangle {
                radius: 12
                color: card.index % 2 ? "#b0c8e0" : "#a0d8f0"
                width: parent.width - Math.abs(card.swipe.position) * 6
                Label {
                    anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: 12; rightMargin: 12 }
                    text: card.modelData
                    elide: Text.ElideRight
                    maximumLineCount: 1
                }
            }
            swipe.right: Rectangle {
                width: 64; height: parent.height
                anchors.right: parent.right
                color: "red"
            }
        }
    }

    // 8 - a button whose background has a size of its own
    Button {
        id: b8
        x: 330; y: 100
        background: Rectangle { width: 40; height: 20; color: "#d0a0f0" }
        contentItem: Label { text: "Own" }
    }

    // 9 - a button with no content, its background's implicit size only
    Button {
        id: b9
        x: 420; y: 100
        background: Rectangle { implicitWidth: 44; implicitHeight: 22; radius: 11; color: "#f0b080" }
        contentItem: Item {}
    }

    function report() {
        const at = (i) => ({ x: i.x, y: i.y, w: i.width, h: i.height, iw: i.implicitWidth, ih: i.implicitHeight });
        const parts = (c) => ({
            self: at(c),
            bg: c.background ? at(c.background) : null,
            content: c.contentItem ? at(c.contentItem) : null,
            pad: [c.leftPadding, c.topPadding, c.rightPadding, c.bottomPadding],
            avail: [c.availableWidth, c.availableHeight],
        });
        const cards = [];
        for (let i = 0; i < list.count; i++) {
            const d = list.itemAtIndex(i);
            if (d) cards.push(parts(d));
        }
        return { b0: parts(b0), b1: parts(b1), b2: parts(b2), c3: parts(c3), c4: parts(c4), s5: parts(s5),
                 b8: parts(b8), b9: parts(b9), cards: cards };
    }
}
