// Effects as end4's widgets use them: a card's glass (a layer masked to the
// card, the wallpaper under it blurred and tinted), shadows behind cards,
// stickers and text, masks. One case a cell, 80 by 80, on white
import QtQuick
import QtQuick.Effects
import Qt5Compat.GraphicalEffects

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    // A picture with edges in it: four squares and a disc
    component Pattern: Item {
        width: 60
        height: 60
        Rectangle { x: 0; y: 0; width: 30; height: 30; color: "#e04030" }
        Rectangle { x: 30; y: 0; width: 30; height: 30; color: "#30a050" }
        Rectangle { x: 0; y: 30; width: 30; height: 30; color: "#3050e0" }
        Rectangle { x: 30; y: 30; width: 30; height: 30; color: "#f0c020" }
        Rectangle { x: 15; y: 15; width: 30; height: 30; radius: 15; color: "#ffffff" }
    }

    // 0 - OpacityMask: the pattern through a rounded rectangle
    Pattern { id: p0; x: 10; y: 10; visible: false }
    Rectangle { id: m0; width: 60; height: 60; radius: 18; visible: false }
    OpacityMask { x: 10; y: 10; width: 60; height: 60; source: p0; maskSource: m0 }

    // 1 - inverted, through a disc
    Pattern { id: p1; x: 90; y: 10; visible: false }
    Rectangle { id: m1; width: 60; height: 60; radius: 30; visible: false }
    OpacityMask { x: 90; y: 10; width: 60; height: 60; source: p1; maskSource: m1; invert: true }

    // 2 - FastBlur, radius 32, as the cards' glass blurs
    Pattern { id: p2; x: 170; y: 10; visible: false }
    FastBlur { x: 170; y: 10; width: 60; height: 60; source: p2; radius: 32 }

    // 3 - radius 8
    Pattern { id: p3; x: 250; y: 10; visible: false }
    FastBlur { x: 250; y: 10; width: 60; height: 60; source: p3; radius: 8 }

    // 4 - radius 16, the blur let out past the item
    Rectangle { id: p4; x: 340; y: 20; width: 40; height: 40; radius: 6; color: "#3050e0"; visible: false }
    FastBlur { x: 340; y: 20; width: 40; height: 40; source: p4; radius: 16; transparentBorder: true }

    // 5 - a card's DropShadow (StyledDropShadow): radius 8, 17 samples,
    // behind the card it is the shadow of
    DropShadow {
        anchors.fill: t5
        source: t5
        radius: 8
        samples: radius * 2 + 1
        color: "#4d000000"
        transparentBorder: true
    }
    Rectangle { id: t5; x: 420; y: 20; width: 40; height: 40; radius: 10; color: "#f0e0a0" }

    // 6 - moved, spread, darker
    DropShadow {
        anchors.fill: t6
        source: t6
        radius: 6
        samples: 13
        horizontalOffset: 3
        verticalOffset: 4
        spread: 0.3
        color: "#a0203080"
        transparentBorder: true
    }
    Rectangle { id: t6; x: 500; y: 18; width: 36; height: 36; radius: 4; color: "#c0e0f0" }

    // 7 - a card's RectangularShadow (StyledRectangularShadow)
    RectangularShadow {
        anchors.fill: t7
        radius: t7.radius
        blur: 9
        offset: Qt.vector2d(0.0, 1.0)
        spread: 1
        color: "#4d000000"
        cached: true
    }
    Rectangle { id: t7; x: 580; y: 20; width: 40; height: 40; radius: 12; color: "#f8f8ff" }

    // 8 - a square one, moved
    RectangularShadow {
        x: 20; y: 105; width: 40; height: 40
        blur: 4
        offset: Qt.vector2d(3.0, 3.0)
        color: "#80203080"
    }

    // 9 - a layer masked to a card: what is in it, cut to the card's corners
    Item {
        x: 90; y: 95; width: 60; height: 60
        layer.enabled: true
        layer.effect: OpacityMask {
            maskSource: Rectangle { width: 60; height: 60; radius: 16 }
        }
        Pattern {}
        Rectangle { anchors.fill: parent; color: "white"; opacity: 0.3 }
    }

    // 10 - a layer seen through: its children's overlaps are not
    Item {
        x: 170; y: 95; width: 60; height: 60
        opacity: 0.5
        layer.enabled: true
        Rectangle { x: 0; y: 0; width: 40; height: 40; color: "#e04030" }
        Rectangle { x: 20; y: 20; width: 40; height: 40; color: "#3050e0" }
    }

    // 11 - a sticker: turned, its outline a DropShadow on its layer
    Rectangle {
        x: 260; y: 105; width: 40; height: 40
        rotation: -12
        radius: 8
        color: "#e04080"
        layer.enabled: true
        layer.effect: DropShadow {
            radius: 4
            spread: 8 / 24
            samples: 24
            color: "#ffffff"
            transparentBorder: true
        }
    }

    // 12 - a ShaderEffectSource shown: part of a pattern, at its own size
    Pattern { id: p12; x: 330; y: 95; visible: false }
    ShaderEffectSource {
        x: 330; y: 95; width: 60; height: 60
        sourceItem: p12
        sourceRect: Qt.rect(10, 10, 40, 40)
    }

    // 13 - text with a shadow on its layer, as a custom text widget's
    Text {
        x: 415; y: 105
        text: "Ag"
        font.pixelSize: 32
        color: "#304060"
        renderType: Text.NativeRendering
        layer.enabled: true
        layer.effect: DropShadow {
            radius: 8
            samples: radius * 2 + 1
            verticalOffset: 2
            color: "#4d000000"
            transparentBorder: true
        }
    }

    // 14, 15 - a card's glass (FastBlurred) over a wallpaper: the wallpaper
    // under the card, blurred, tinted, cut to the card
    Item {
        id: wall
        x: 490; y: 90; width: 150; height: 160
        Repeater {
            model: 25
            Rectangle {
                x: (index % 5) * 30; y: Math.floor(index / 5) * 32; width: 30; height: 32
                color: Qt.hsla((index * 0.37) % 1, 0.7, 0.55, 1)
            }
        }
    }
    Item {
        id: glass
        x: 520; y: 120; width: 90; height: 100
        readonly property real oversample: 32 * 1.5
        layer.enabled: true
        layer.effect: OpacityMask {
            maskSource: Rectangle { width: glass.width; height: glass.height; radius: 20 }
        }
        FastBlur {
            id: blur
            x: -glass.oversample; y: -glass.oversample
            width: glass.width + glass.oversample * 2
            height: glass.height + glass.oversample * 2
            radius: 32
            source: ShaderEffectSource {
                sourceItem: wall
                sourceRect: Qt.rect(glass.x - wall.x - glass.oversample, glass.y - wall.y - glass.oversample,
                                    blur.width, blur.height)
            }
        }
        Rectangle { anchors.fill: parent; radius: 20; color: "white"; opacity: 0.15 }
    }

    // 16 - a turned layer with a mask, at a fraction of a pixel
    Item {
        x: 20.4; y: 185.3; width: 50; height: 50
        rotation: 15
        layer.enabled: true
        layer.effect: OpacityMask {
            maskSource: Rectangle { width: 50; height: 50; radius: 25 }
        }
        Rectangle { anchors.fill: parent; color: "#30a050" }
        Rectangle { width: 25; height: 50; color: "#f0c020" }
    }

    // 18, 19 - the same text, drawn and through a layer
    Text { x: 175; y: 190; text: "Ag"; font.pixelSize: 32; color: "#304060"; renderType: Text.NativeRendering }
    Text { x: 255; y: 190; text: "Ag"; font.pixelSize: 32; color: "#304060"; renderType: Text.NativeRendering; layer.enabled: true }

    // 17 - FastBlur, radius 64
    Pattern { id: p17; x: 90; y: 180; visible: false }
    FastBlur { x: 90; y: 180; width: 60; height: 60; source: p17; radius: 64 }
}
