// Where a centred anchor puts an item: odd and even sizes, fractions, an
// offset, and `alignWhenCentered` off. end4 centres every label in its card
// with `verticalCenter`. The numbers are what `report()` says
import QtQuick

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    component Box: Rectangle {
        property real cw: 20
        property real ch: 20
        width: cw; height: ch
        color: "#4060a0"
    }

    Row {
        x: 5; y: 5
        spacing: 5
        // parent 55 high, children 20, 21, 20.5, 19.4 high
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v0; ch: 20; anchors.verticalCenter: parent.verticalCenter } }
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v1; ch: 21; anchors.verticalCenter: parent.verticalCenter } }
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v2; ch: 20.5; anchors.verticalCenter: parent.verticalCenter } }
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v3; ch: 19.4; anchors.verticalCenter: parent.verticalCenter } }
        // parent 54 high
        Rectangle { width: 30; height: 54; color: "#e0e0e0"; Box { id: v4; ch: 21; anchors.verticalCenter: parent.verticalCenter } }
        Rectangle { width: 30; height: 54; color: "#e0e0e0"; Box { id: v5; ch: 20; anchors.verticalCenter: parent.verticalCenter } }
        // parent 54.6 high, an offset
        Rectangle { width: 30; height: 54.6; color: "#e0e0e0"; Box { id: v6; ch: 21; anchors.verticalCenter: parent.verticalCenter } }
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v7; ch: 21; anchors.verticalCenter: parent.verticalCenter; anchors.verticalCenterOffset: 0.7 } }
        // not aligned
        Rectangle { width: 30; height: 55; color: "#e0e0e0"; Box { id: v8; ch: 21; anchors.verticalCenter: parent.verticalCenter; anchors.alignWhenCentered: false } }
        // across, and centerIn
        Rectangle { width: 55; height: 30; color: "#e0e0e0"; Box { id: h0; cw: 21; anchors.horizontalCenter: parent.horizontalCenter } }
        Rectangle { width: 55; height: 55; color: "#e0e0e0"; Box { id: c0; cw: 21; ch: 20.5; anchors.centerIn: parent } }
        Rectangle { width: 54.5; height: 55; color: "#e0e0e0"; Box { id: c1; cw: 19; ch: 19; anchors.centerIn: parent } }
    }

    // a centred item inside an item that is itself at a fraction
    Item {
        x: 10.3; y: 100.6; width: 55; height: 55
        Box { id: n0; ch: 21; cw: 21; anchors.centerIn: parent }
    }
    // a sibling as the anchor
    Rectangle { id: sib; x: 100; y: 100; width: 41; height: 41; color: "#e0e0e0" }
    Box { id: s0; ch: 20; cw: 20; anchors.verticalCenter: sib.verticalCenter; anchors.horizontalCenter: sib.horizontalCenter }

    function report() {
        const at = (i) => [i.x, i.y];
        return { v0: at(v0), v1: at(v1), v2: at(v2), v3: at(v3), v4: at(v4), v5: at(v5), v6: at(v6), v7: at(v7), v8: at(v8),
                 h0: at(h0), c0: at(c0), c1: at(c1), n0: at(n0), s0: at(s0) };
    }
}
