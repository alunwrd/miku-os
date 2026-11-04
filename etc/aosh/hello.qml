// A file for aosh with no window: `aosh run /etc/aosh/hello.qml`
//
// Exercises the half of the shell that has no pixels: QuickJS, bindings that
// follow what they read, a signal and its handler, a Timer on the real clock, a
// Process, a ListModel and an animation (`dev/aosh-architecture.md`). `aosh
// eval /etc/aosh/hello.qml 'doubled'` asks it one question instead

import QtQuick

Item {
    id: root

    property int ticks: 0
    property int doubled: ticks * 2
    property string said: "ticks " + ticks + ", doubled " + doubled

    signal done(string why)

    onDone: (why) => console.log("done:", why)
    onSaidChanged: console.log(said)

    Timer {
        interval: 250
        running: true
        repeat: true
        onTriggered: {
            root.ticks++
            if (root.ticks === 4) {
                running = false
                root.done(Qt.formatDateTime(new Date(), "HH:mm:ss"))
            }
        }
    }

    // A program, and what it wrote
    Process {
        command: ["echo", "hello from a process"]
        running: true
        stdout: StdioCollector { onStreamFinished: console.log("process said:", text.trim()) }
        onExited: (exitCode) => console.log("process exited with", exitCode)
    }

    // Rows that are objects, and a delegate per row
    ListModel {
        id: fruit
        ListElement { name: "apple" }
        ListElement { name: "pear" }
    }
    Repeater {
        id: rows
        model: fruit
        Item { Component.onCompleted: console.log("row", index, name) }
    }

    // A number on the clock
    property real slide: 0
    NumberAnimation on slide {
        from: 0; to: 100; duration: 300; easing.type: Easing.OutCubic
        onFinished: console.log("animated to", root.slide)
    }

    Component.onCompleted: {
        console.log("aosh is up:", [1, 2, 3].map(x => x * 2).join())
        fruit.append({ name: "plum" })
        console.log("rows now", rows.count)
    }
}
