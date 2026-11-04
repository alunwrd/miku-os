// The bar across the top of the screen: 48 high and kept off the work area, a
// card at each end, four above and below, eight down the sides.

import QtQuick
import QtQuick.Layouts
import Quickshell

PanelWindow {
    id: bar

    // Stuck to the top and stretched between the sides, which is what makes
    // it a bar and not a window
    anchors {
        top: true
        left: true
        right: true
    }
    implicitHeight: 48
    // The height it keeps off the work area: what puts windows under the bar
    // rather than behind it
    exclusiveZone: 48
    // Never: the keyboard stays with the focused window
    focusable: false
    aboveWindows: true
    color: "transparent"

    RowLayout {
        anchors.fill: parent
        anchors.leftMargin: 8
        anchors.rightMargin: 8
        anchors.topMargin: 4
        anchors.bottomMargin: 4
        spacing: 12

        // What is open, on the left
        Rectangle {
            Layout.fillHeight: true
            implicitWidth: open.implicitWidth + 20
            radius: 8
            color: Theme.card
            border.width: 1
            border.color: Theme.border

            RowLayout {
                id: open
                anchors.left: parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 10

                // One pill per workspace, whether anything is on it or not, so
                // the numbers do not move under the pointer
                Row {
                    spacing: 4
                    Repeater {
                        model: 9
                        Pill { n: index + 1 }
                    }
                }

                Text {
                    text: Desktop.focusedTitle
                    color: Theme.text
                    font.pixelSize: 15
                    elide: Text.ElideRight
                    Layout.maximumWidth: 420
                }
            }
        }

        // Nothing, with all the room: pushes the right-hand card to the right
        // without either card knowing how wide the screen is
        Item {
            Layout.fillWidth: true
        }

        // What the machine is doing, on the right
        Rectangle {
            Layout.fillHeight: true
            implicitWidth: machine.implicitWidth + 20
            radius: 8
            color: Theme.card
            border.width: 1
            border.color: Theme.border

            RowLayout {
                id: machine
                anchors.left: parent.left
                anchors.leftMargin: 10
                anchors.verticalCenter: parent.verticalCenter
                spacing: 8

                Chip { text: Sys.netIp }
                Chip { text: Sys.memPercent + "% mem" }
                Chip { text: Sys.uptimeText }
                Chip {
                    text: Qt.formatDateTime(clock.date, "HH:mm")
                    strong: true
                }
            }
        }
    }

    SystemClock {
        id: clock
        precision: SystemClock.Minutes
    }
}
