// TextEdit as end4's desktop text widget uses it: Caveat at 64 px, one word,
// centred in a box as big as it and 16 more each side, with a shadow on its
// layer, and beside it a Text and a TextInput with the same text, for what each
// is measured as; and the Controls' TextArea and TextField in the Basic style,
// for their padding. `report()` gives their sizes
import QtQuick
import QtQuick.Controls
import Qt5Compat.GraphicalEffects

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    FontLoader { id: caveat; source: Qt.resolvedUrl("../../../../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf") }

    // 0, 1 - the widget: the box from the editor's implicit size
    Item {
        id: box
        x: 10; y: 10
        implicitWidth: editor.implicitWidth + 32
        implicitHeight: editor.implicitHeight + 32
        width: implicitWidth; height: implicitHeight
        TextEdit {
            id: editor
            anchors.centerIn: parent
            text: "alunwrd"
            textFormat: TextEdit.PlainText
            color: "#203040"
            horizontalAlignment: TextEdit.AlignHCenter
            renderType: Text.NativeRendering
            font { family: caveat.name; pixelSize: 64 }
            layer.enabled: true
            layer.effect: DropShadow {
                radius: 8
                samples: radius * 2 + 1
                verticalOffset: 2
                color: "#4d000000"
                transparentBorder: true
            }
        }
    }

    // 2 - the same as a Text
    Text {
        id: text
        x: 330; y: 10
        text: "alunwrd"
        color: "#203040"
        renderType: Text.NativeRendering
        font { family: caveat.name; pixelSize: 64 }
    }

    // 3 - a TextEdit in the interface font, two lines
    TextEdit {
        id: two
        x: 10; y: 140
        text: "Notes Ag\nsecond line"
        color: "#203040"
        renderType: Text.NativeRendering
        font.pixelSize: 17
    }

    // 4 - a TextEdit wider than its text, the text centred, and one right
    TextEdit {
        id: wide
        x: 170; y: 140; width: 200
        text: "centred"
        color: "#203040"
        horizontalAlignment: TextEdit.AlignHCenter
        renderType: Text.NativeRendering
        font.pixelSize: 17
    }
    TextEdit {
        id: right
        x: 170; y: 180; width: 200
        text: "to the right"
        color: "#203040"
        horizontalAlignment: TextEdit.AlignRight
        renderType: Text.NativeRendering
        font.pixelSize: 17
    }

    // 5 - a TextInput, for its sizes
    TextInput {
        id: input
        x: 420; y: 140
        text: "Notes Ag"
        color: "#203040"
        renderType: Text.NativeRendering
        font.pixelSize: 17
    }

    // 6 - 8: a TextArea as the style pads it, one with no padding (4 on
    // the left still), a TextField with its own
    TextArea {
        id: area
        x: 10; y: 200
        text: "Type"
        color: "#203040"
        renderType: Text.NativeRendering
        font.pixelSize: 17
        background: null
    }
    TextArea {
        id: area0
        x: 120; y: 200
        padding: 0
        text: "Type"
        color: "#203040"
        renderType: Text.NativeRendering
        font.pixelSize: 17
        background: null
    }
    TextField {
        id: field
        x: 230; y: 200
        padding: 2
        text: "field"
        color: "#203040"
        renderType: Text.NativeRendering
        font.pixelSize: 17
        background: null
    }

    function report() {
        const at = (i) => [i.x, i.y, i.width, i.height, i.implicitWidth, i.implicitHeight, i.contentWidth, i.contentHeight];
        const padded = (i) => at(i).concat([i.leftPadding, i.topPadding, i.rightPadding, i.bottomPadding]);
        return { box: at(box), editor: at(editor), text: at(text), two: at(two), wide: at(wide), right: at(right), input: at(input),
                 area: padded(area), area0: padded(area0), field: padded(field) };
    }
}
