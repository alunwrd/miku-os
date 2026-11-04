// Text around the size Qt stops caching glyphs at (64 px): Caveat with its
// weight given and with the application's, and the interface font
import QtQuick

Rectangle {
    width: 650
    height: 255
    color: "white"
    FontLoader { id: caveat; source: Qt.resolvedUrl("../../../../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf") }
    component Big: Text { renderType: Text.NativeRendering; color: "#203040"; text: "alunwrd" }

    // 0 - 63 px, weight 400
    Big { x: 10; y: 5; font { family: caveat.name; pixelSize: 63; weight: 400 } }
    // 1 - 64 px, weight 400
    Big { x: 330; y: 5; font { family: caveat.name; pixelSize: 64; weight: 400 } }
    // 2 - 64 px, the application's weight
    Big { x: 10; y: 90; font { family: caveat.name; pixelSize: 64 } }
    // 3 - 65 px, weight 400
    Big { x: 330; y: 90; font { family: caveat.name; pixelSize: 65; weight: 400 } }
    // 4 - the interface font at 64
    Big { x: 10; y: 175; text: "Ag"; font.pixelSize: 64 }
}
