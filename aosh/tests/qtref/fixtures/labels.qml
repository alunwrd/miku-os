// Labels as end4's StyledText makes them, centred in a card: Google Sans
// Flex at 450 and 100 wide, native, centred down, one line cut short. The
// notes widget's cards are 55 high; the other sizes are its neighbours'.
// `report()` gives each label's place and size
import QtQuick

Rectangle {
    id: root
    width: 650
    height: 375
    color: "white"

    component Label: Text {
        renderType: Text.NativeRendering
        verticalAlignment: Text.AlignVCenter
        color: "#203040"
        font.family: "Google Sans Flex"
        font.hintingPreference: Font.PreferDefaultHinting
        font.pixelSize: 15
        font.variableAxes: ({ "wght": 450, "wdth": 100 })
    }
    component Card: Rectangle {
        width: 200; height: 55
        radius: 12
        color: "#a0d8f0"
    }

    Column {
        x: 5; y: 5
        spacing: 6
        // 0 - a note: left and right, centred down, cut short
        Card { Label { id: l0; anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: 12; rightMargin: 12 }
                       text: "miku os: fix all known bugs, compile the world"; elide: Text.ElideRight; maximumLineCount: 1 } }
        // 1 - the same, short enough
        Card { Label { id: l1; anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: 12; rightMargin: 12 }
                       text: "miku: change TCBAIN; TRAI."; elide: Text.ElideRight; maximumLineCount: 1 } }
        // 2 - centred, nothing else
        Card { Label { id: l2; anchors.centerIn: parent; text: "Notes Ag" } }
    }
    Column {
        x: 220; y: 5
        spacing: 6
        // 3 - 5: 14, 16 and 17 pixels
        Card { Label { id: l3; font.pixelSize: 14; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "fourteen Ag" } }
        Card { Label { id: l4; font.pixelSize: 16; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "sixteen Ag" } }
        Card { Label { id: l5; font.pixelSize: 17; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "seventeen Ag" } }
    }
    Column {
        x: 435; y: 5
        spacing: 6
        // 6 - the title weight
        Card { Label { id: l6; font.variableAxes: ({ "wght": 550 }); font.pixelSize: 23; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "Notes" } }
        // 7 - a fixed height, the text centred in it
        Card { Label { id: l7; height: 30; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "thirty high" } }
        // 8 - left and right, no cut
        Card { Label { id: l8; anchors { left: parent.left; right: parent.right; verticalCenter: parent.verticalCenter; leftMargin: 12; rightMargin: 12 }
                       text: "wide label" } }
        // 9, 10 - the bar's clock: figures all one width (`tnum`), and without
        Card { Label { id: l9; font.features: { "tnum": 1 }; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "Tue, 13/10 • 21:11" } }
        Card { Label { id: l10; anchors.verticalCenter: parent.verticalCenter; x: 12; text: "Tue, 13/10 • 21:11" } }
    }

    function report() {
        const at = (i) => [i.x, i.y, i.width, i.height, i.implicitHeight, i.contentHeight, i.baselineOffset];
        return { l0: at(l0), l1: at(l1), l2: at(l2), l3: at(l3), l4: at(l4), l5: at(l5), l6: at(l6), l7: at(l7), l8: at(l8),
                 l9: at(l9).concat([l9.implicitWidth]), l10: at(l10).concat([l10.implicitWidth]) };
    }
}
