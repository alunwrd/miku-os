// The launcher: the programs /etc/mayland/apps.d describes, found by typing
//
// The numbers are from quickshell's `applauncher/appLauncher.qml`: a card 720
// wide with a radius of 16, a search row 65 tall, a hairline, rows 60 tall four
// apart and eight at most, a tile 40 square with a picture 24 inside it. The
// card's height follows the list and slides.
//
// `bind = $mod, D, signal, launcher toggle` in
// /etc/mayland/default.d/50-keys.conf is `toggle()` below, and `aosh ipc call
// launcher toggle` is the same call from a terminal.

import QtQuick
import QtQuick.Layouts
import QtQuick.Controls
import Quickshell
import Quickshell.Io

PanelWindow {
    id: launcher

    anchors.top: true
    implicitWidth: 720
    // Room for the card at its tallest. The card is as tall as it needs and
    // the rest of the window is see-through: a shape that changes height on
    // a surface that does not
    implicitHeight: 660
    // In front of everything, reserving nothing, with the keyboard to itself
    // while it is up
    WlrLayershell.layer: WlrLayer.Overlay
    WlrLayershell.keyboardFocus: WlrKeyboardFocus.Exclusive
    exclusionMode: ExclusionMode.Ignore
    color: "transparent"
    visible: false

    // What has been typed, and what it finds: by name, ignoring case
    property string query: ""
    readonly property var found: {
        const q = query.toLowerCase();
        return DesktopEntries.applications.values
            .filter(a => !a.noDisplay && a.name.toLowerCase().includes(q));
    }

    // How far the card has come in: 0 to 1 when it opens
    property real intro: 1

    // It always opens empty, on the first row
    function open() {
        search.text = "";
        list.currentIndex = 0;
        visible = true;
        search.forceActiveFocus();
        introAnim.restart();
    }
    function close() { visible = false; }
    function toggle() { if (visible) close(); else open(); }
    // Hidden first and run after: the launcher goes in the frame the program
    // starts, not a frame later when its window is already coming up
    function run(entry) {
        close();
        if (entry)
            entry.execute();
    }

    IpcHandler {
        target: "launcher"
        function toggle(): void { launcher.toggle(); }
        function open(): void { launcher.open(); }
        function close(): void { launcher.close(); }
    }

    NumberAnimation {
        id: introAnim
        target: launcher
        property: "intro"
        from: 0
        to: 1
        duration: 600
        easing.type: Easing.OutExpo
    }

    // A click beside the card closes it
    MouseArea {
        anchors.fill: parent
        onClicked: launcher.close()
    }

    Rectangle {
        id: card

        readonly property real rowHeight: 60
        readonly property real gap: 4
        readonly property int rows: Math.min(launcher.found.length, 8)
        readonly property real listHeight: rows === 0 ? 0 : rows * rowHeight + (rows - 1) * gap
        readonly property real margins: rows > 0 ? 20 : 0
        property real shownListHeight: listHeight
        property real shownMargins: margins
        Behavior on shownListHeight { NumberAnimation { duration: 500; easing.type: Easing.OutExpo } }
        Behavior on shownMargins { NumberAnimation { duration: 500; easing.type: Easing.OutExpo } }

        // Level with the top of a window, from the compositor's own work area,
        // coming down into place as it opens
        y: Desktop.workTop - (1 - launcher.intro) * 60
        opacity: launcher.intro
        width: 720
        height: 65 + 1 + shownMargins + shownListHeight
        radius: 16
        color: Theme.page
        border.width: 1
        border.color: Theme.border
        clip: true

        // The card's own clicks are not "beside the card"
        MouseArea { anchors.fill: parent }

        Column {
            width: parent.width

            Item {
                width: parent.width
                height: 65

                TextField {
                    id: search
                    anchors.fill: parent
                    anchors.leftMargin: 20
                    anchors.rightMargin: 20
                    padding: 0
                    color: Theme.text
                    placeholderText: "Поиск..."
                    placeholderTextColor: Theme.textMuted
                    font.family: "JetBrains Mono"
                    font.pixelSize: 16
                    focus: true

                    onTextChanged: {
                        launcher.query = text;
                        list.currentIndex = 0;
                        list.positionViewAtBeginning();
                    }
                    Keys.onDownPressed: if (list.currentIndex < launcher.found.length - 1) list.currentIndex++
                    Keys.onUpPressed: if (list.currentIndex > 0) list.currentIndex--
                    Keys.onReturnPressed: launcher.run(launcher.found[list.currentIndex])
                    Keys.onEscapePressed: launcher.close()
                }
            }

            Rectangle {
                width: parent.width
                height: 1
                color: Theme.border
            }

            Item {
                width: parent.width
                height: card.shownMargins / 2
            }

            ListView {
                id: list
                x: 10
                width: parent.width - 20
                height: card.shownListHeight
                clip: true
                spacing: card.gap
                model: launcher.found
                currentIndex: 0
                onCurrentIndexChanged: if (currentIndex >= 0) positionViewAtIndex(currentIndex, ListView.Contain)

                delegate: Rectangle {
                    id: row
                    required property var modelData
                    required property int index
                    readonly property bool current: index === list.currentIndex

                    width: list.width
                    height: card.rowHeight
                    radius: 8
                    color: current ? Theme.accent : hover.containsMouse ? Theme.card : "transparent"
                    Behavior on color { ColorAnimation { duration: 150 } }

                    RowLayout {
                        anchors.fill: parent
                        anchors.leftMargin: 12
                        anchors.rightMargin: 12
                        spacing: 15

                        Rectangle {
                            Layout.preferredWidth: 40
                            Layout.preferredHeight: 40
                            radius: 12
                            color: row.current ? Theme.onAccent : Theme.card
                            Behavior on color { ColorAnimation { duration: 150 } }

                            Image {
                                anchors.centerIn: parent
                                width: 24
                                height: 24
                                source: row.modelData.icon
                                fillMode: Image.PreserveAspectFit
                            }
                        }

                        Text {
                            Layout.fillWidth: true
                            text: row.modelData.name
                            font.family: "JetBrains Mono"
                            font.pixelSize: 14
                            font.bold: row.current
                            color: row.current ? Theme.onAccent : Theme.text
                            elide: Text.ElideRight
                        }
                    }

                    MouseArea {
                        id: hover
                        anchors.fill: parent
                        hoverEnabled: true
                        onClicked: launcher.run(row.modelData)
                    }
                }
            }
        }
    }
}
