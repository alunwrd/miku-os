// Pictures as Qt's image reader shrinks them to their `sourceSize`, and as
// end4's stickers draw them: decoded at twice the item, drawn at the item
// (the GPU halving it), turned, in a layer whose DropShadow is the white
// outline. `pattern.png` is a picture made for this: hairlines, diagonals,
// a soft disc, an alpha ramp over fine checks
import QtQuick
import Qt5Compat.GraphicalEffects

Rectangle {
    id: root
    width: 650
    height: 510
    color: "white"

    // 0 - 3: shrunk while read by 1.32, 2, 2.6 and 4, drawn as they are
    Row {
        x: 5; y: 5
        spacing: 4
        Image { source: "pattern.png"; sourceSize: Qt.size(227, 227) }
        Image { source: "pattern.png"; sourceSize: Qt.size(150, 150) }
        Image { source: "pattern.png"; sourceSize: Qt.size(115, 115) }
        Image { source: "pattern.png"; sourceSize: Qt.size(75, 75) }
    }

    // 4 - 6: stickers, on the sky
    Rectangle {
        y: 255; width: 650; height: 255
        color: "#5aa0e0"
    }
    component Sticker: Item {
        id: s
        property real size: 120
        width: size; height: size
        Image {
            anchors.fill: parent
            source: "pattern.png"
            fillMode: Image.PreserveAspectFit
            antialiasing: true
            sourceSize.width: parent.width * 2
            sourceSize.height: parent.height * 2
            layer.enabled: true
            layer.effect: DropShadow {
                horizontalOffset: 0
                verticalOffset: 0
                radius: 4
                spread: 8 / 24
                samples: 24
                color: "#ffffff"
                transparentBorder: true
            }
        }
    }
    Sticker { x: 30; y: 300 }
    Sticker { x: 200; y: 300; rotation: -12 }
    Sticker { x: 400; y: 280; size: 180; rotation: 8 }
}
