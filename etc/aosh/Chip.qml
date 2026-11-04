// One reading on the right of the bar (the address, the memory, the uptime, the
// time) in a rounded box of its own

import QtQuick

Rectangle {
    property alias text: label.text
    // The time is in bold
    property bool strong: false

    implicitWidth: label.implicitWidth + 20
    implicitHeight: 24
    radius: 12
    color: Theme.page

    Text {
        id: label
        anchors.centerIn: parent
        color: parent.strong ? Theme.text : Theme.textMuted
        font.pixelSize: parent.strong ? 15 : 13
        font.bold: parent.strong
    }
}
