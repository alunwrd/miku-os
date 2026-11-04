// Qt5Compat's RadialGradient as end4's RippleButton draws its ripple: a disc
// of colour fading out at half its radius, on a button whose background is
// a layer masked to its rounded box. One case a cell, 80 by 80, on white
import QtQuick
import QtQuick.Controls
import Qt5Compat.GraphicalEffects

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    // The ripple's stops, in a colour
    component Ripple: Gradient {
        property color c: "#6a5acd"
        GradientStop { position: 0.0; color: c }
        GradientStop { position: 0.3; color: c }
        GradientStop { position: 0.5; color: Qt.rgba(c.r, c.g, c.b, 0) }
    }

    // 0 - as it comes: white in the middle, black at the edge, an ellipse
    // as wide and high as the item
    RadialGradient { x: 10; y: 10; width: 70; height: 50 }

    // 1 - the ripple's stops, a circle
    RadialGradient { x: 90; y: 10; width: 70; height: 70; gradient: Ripple {} }

    // 2 - its own radii, moved off the middle, turned
    RadialGradient {
        x: 170; y: 10; width: 70; height: 70
        horizontalRadius: 20; verticalRadius: 40
        horizontalOffset: 8; verticalOffset: -6
        angle: 30
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#f0c020" }
            GradientStop { position: 0.6; color: "#e04030" }
            GradientStop { position: 1.0; color: "#203080" }
        }
    }

    // 3 - through a mask: a rounded rectangle that is not shown
    Rectangle { id: m3; width: 70; height: 70; radius: 20; visible: false }
    RadialGradient {
        x: 250; y: 10; width: 70; height: 70
        source: m3
        gradient: Gradient {
            GradientStop { position: 0.0; color: "#30a050" }
            GradientStop { position: 1.0; color: "#3050e0" }
        }
    }

    // 4 - a ripple at half strength, as it fades
    Item {
        x: 330; y: 10; width: 70; height: 70
        opacity: 0.5
        RadialGradient { anchors.fill: parent; gradient: Ripple { c: "#d6cee2" } }
    }

    // 5 - no radius across: the last colour everywhere
    RadialGradient {
        x: 410; y: 10; width: 70; height: 70
        horizontalRadius: 0
        gradient: Ripple {}
    }

    // 6 - a radius past the item: only its middle shows
    RadialGradient {
        x: 490; y: 10; width: 70; height: 70
        horizontalRadius: 140; verticalRadius: 140
    }

    // 7 - turned with its item
    RadialGradient {
        x: 570; y: 15; width: 60; height: 40
        rotation: 25
        gradient: Ripple { c: "#e04030" }
    }

    // 8 - 10: RippleButton as it is pressed: the background a layer masked
    // to its rounded box, the ripple an item in it, centred on the press by
    // a Translate, as big as it has grown
    component Pressed: Button {
        id: b
        property real px
        property real py
        property real grown
        property real strength: 1
        width: 70; height: 34
        contentItem: Item {}
        background: Rectangle {
            id: bg
            radius: 10
            color: "#e5dfed"
            layer.enabled: true
            layer.effect: OpacityMask {
                maskSource: Rectangle { width: bg.width; height: bg.height; radius: 10 }
            }
            Item {
                id: ripple
                x: b.px; y: b.py
                width: b.grown; height: b.grown
                opacity: b.strength
                RadialGradient { anchors.fill: parent; gradient: Ripple { c: "#a090c8" } }
                transform: Translate { x: -ripple.width / 2; y: -ripple.height / 2 }
            }
        }
    }
    Pressed { x: 10; y: 100; px: 20; py: 12; grown: 60 }
    Pressed { x: 90; y: 100; px: 50; py: 30; grown: 140 }
    Pressed { x: 170; y: 100; px: 35; py: 17; grown: 160; strength: 0.4 }
}
