// Text as Qt draws it by default, its distance-field way (`renderType`
// Text.QtRendering, which a shell gets unless it asks for native text):
// end4's desktop signature is a TextEdit like this, Caveat at 64 in a layer
// with a shadow. Each case is in a layer, where Qt draws such text grey.
// `report()` gives the sizes, which are not native text's
import QtQuick
import Qt5Compat.GraphicalEffects

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    FontLoader { id: caveat; source: Qt.resolvedUrl("../../../../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf") }

    component Layered: Item {
        layer.enabled: true
    }

    // 0, 1 - the signature, and in a shadow as the widget has it
    Layered {
        x: 10; y: 5; width: 300; height: 110
        TextEdit { id: t0; x: 16; y: 16; text: "alunwrd"; textFormat: TextEdit.PlainText; color: "#0d2a3a"; font { family: caveat.name; pixelSize: 64 } }
    }
    Item {
        x: 330; y: 5
        width: t1.implicitWidth + 32; height: t1.implicitHeight + 32
        TextEdit {
            id: t1
            anchors.centerIn: parent
            text: "alunwrd"; textFormat: TextEdit.PlainText; color: "#0d2a3a"
            horizontalAlignment: TextEdit.AlignHCenter
            font { family: caveat.name; pixelSize: 64 }
            layer.enabled: true
            layer.effect: DropShadow { radius: 8; samples: radius * 2 + 1; verticalOffset: 2; color: "#4d000000"; transparentBorder: true }
        }
    }

    // 2 - 5: smaller, a Text, the interface font
    Layered {
        x: 10; y: 125; width: 640; height: 125
        Text { id: t2; x: 6; y: 0; text: "alunwrd 40"; color: "#0d2a3a"; font { family: caveat.name; pixelSize: 40 } }
        Text { id: t3; x: 6; y: 55; text: "Notes Ag 20"; color: "#203040"; font { family: "Google Sans Flex"; pixelSize: 20 } }
        Text { id: t4; x: 6; y: 90; text: "Small text 15, quick fox"; color: "#203040"; font { family: "Google Sans Flex"; pixelSize: 15 } }
        Text { id: t5; x: 330; y: 10; text: "Big 54"; color: "#203040"; font { family: "Google Sans Flex"; pixelSize: 54 } }
    }

    function report() {
        const at = (i) => [i.x, i.y, i.width, i.height, i.implicitWidth, i.implicitHeight];
        return { t0: at(t0), t1: at(t1), t2: at(t2), t3: at(t3), t4: at(t4), t5: at(t5) };
    }
}
