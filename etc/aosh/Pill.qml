// One workspace on the bar: its number, filled with the accent when it is the
// one on the screen, and a click away from being it

import QtQuick

Rectangle {
    id: pill

    property int n: 1
    readonly property bool active: Desktop.workspace === n
    // Whether anything is open on it: a dimmer number when nothing is
    readonly property bool busy: Desktop.windows ? Desktop.windows.some(w => w.workspace === n) : false

    width: 26
    height: 26
    radius: 13
    color: active ? Theme.accent : Theme.page
    // The accent moves to the workspace that is up rather than jumping there,
    // in a quarter of a second, as quickshell's TopBar does
    Behavior on color { ColorAnimation { duration: 250 } }

    Text {
        anchors.centerIn: parent
        text: pill.n
        font.pixelSize: 15
        font.bold: pill.active
        color: pill.active ? Theme.onAccent : pill.busy ? Theme.text : Theme.textMuted
        Behavior on color { ColorAnimation { duration: 250 } }
    }

    MouseArea {
        anchors.fill: parent
        onClicked: Desktop.switchTo(pill.n)
    }
}
