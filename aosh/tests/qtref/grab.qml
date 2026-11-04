// A fixture rendered by Qt itself, for aosh's tests to be measured against
//
//     QT_QPA_PLATFORM=offscreen qml6 grab.qml -- fixtures/text.qml out/text.png
//
// The fixture is loaded into a window its own size, given a few frames to
// settle, and grabbed to the PNG. If its root has a `report()` function, what
// it returns is printed as one line, `AOSHREF <json>`, which `make.sh` keeps
// beside the PNG. The fixtures are aosh's own (`fixtures/`): no shell code,
// nothing that starts a program.
import QtQuick
import QtQuick.Window

Window {
    id: win
    visible: true
    color: "transparent"
    readonly property var args: Qt.application.arguments.slice(Qt.application.arguments.indexOf("--") + 1)
    width: Math.max(1, loader.item ? loader.item.width : 1)
    height: Math.max(1, loader.item ? loader.item.height : 1)

    Loader {
        id: loader
        source: Qt.resolvedUrl(win.args[0])
    }

    Timer {
        interval: 400
        running: loader.status === Loader.Ready
        onTriggered: loader.item.grabToImage(result => {
            result.saveToFile(win.args[1]);
            if (typeof loader.item.report === "function")
                console.log("AOSHREF " + JSON.stringify(loader.item.report()));
            Qt.quit();
        })
    }
    Component.onCompleted: if (loader.status === Loader.Error) Qt.exit(1)
}
