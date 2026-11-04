pragma ComponentBehavior: Bound

import QtQuick
import QtQuick.Layouts
import QtQuick.Effects
import Qt5Compat.GraphicalEffects
import Quickshell
import qs.modules.common
import qs.modules.common.widgets
import qs.modules.ii.background.widgets

AbstractBackgroundWidget {
    id: root

    configEntryName: "sticker"
    hoverEnabled: true

    // -1 = the built-in sticker; >= 0 = extra sticker stored in sticker.items[itemIndex]
    property int itemIndex: -1
    readonly property bool isExtra: itemIndex >= 0
    readonly property var stickerConfig: Config.options.background.widgets.sticker

    configEntry: isExtra ? (stickerConfig.items[itemIndex] ?? {}) : stickerConfig
    saveEntryHandler: isExtra ? (fields => root.updateItem(fields)) : null
    deleteHandler: isExtra ? (() => root.removeItem()) : null

    function updateItem(fields) {
        const items = stickerConfig.items.slice();
        items[itemIndex] = Object.assign({}, items[itemIndex], fields);
        stickerConfig.items = items;
    }
    function removeItem() {
        const items = stickerConfig.items.slice();
        items.splice(itemIndex, 1);
        stickerConfig.items = items;
    }

    property string imagePath: configEntry.path ?? ""
    property bool dropHover: false
    property real widgetSize: configEntry.size ?? 200
    property real widgetRotation: configEntry.rotation ?? 0
    property color outlineColor: stickerConfig.outlineColor !== ""
        ? stickerConfig.outlineColor
        : "#ffffff"
    property real outlineWidth: stickerConfig.outlineWidth ?? 8

    implicitWidth: contentItem.implicitWidth
    implicitHeight: contentItem.implicitHeight

    Item {
        id: contentItem
        implicitWidth: root.widgetSize
        implicitHeight: root.widgetSize
        rotation: root.widgetRotation

        Behavior on implicitWidth {
            animation: Appearance.animation.elementResize.numberAnimation.createObject(this)
        }
        Behavior on implicitHeight {
            animation: Appearance.animation.elementResize.numberAnimation.createObject(this)
        }

        StyledImage {
            id: stickerImage
            anchors.fill: parent
            source: root.imagePath !== "" ? root.imagePath : ""
            fillMode: Image.PreserveAspectFit
            cache: false
            antialiasing: true
            sourceSize.width: parent.width * 2
            sourceSize.height: parent.height * 2
            visible: root.imagePath !== ""

            layer.enabled: true
            layer.effect: DropShadow {
                horizontalOffset: 0
                verticalOffset: 0
                radius: 4
                spread: root.outlineWidth / 24 
                samples: 24
                color: root.outlineColor
                transparentBorder: true
            }
        }

        MaterialSymbol {
            anchors.centerIn: parent
            iconSize: contentItem.implicitWidth / 3
            text: root.dropHover ? "download" : "sticker"
            fill: root.dropHover ? 1 : 0
            color: root.dropHover
                ? Appearance.colors.colPrimary
                : Appearance.colors.colOnPrimaryContainer
            visible: root.imagePath === ""
            Behavior on color { animation: Appearance.animation.elementMoveFast.colorAnimation.createObject(this) }
        }

        DropArea {
            anchors.fill: parent
            keys: ["text/uri-list"]
            onEntered: (drag) => {
                drag.accept(Qt.CopyAction)
                root.dropHover = true
            }
            onExited: {
                root.dropHover = false
            }
            onDropped: (drop) => {
                if (drop.hasUrls && drop.urls.length > 0) {
                    var cleanPath = drop.urls[0].toString().replace(/^file:\/\//, "")
                    var ext = cleanPath.split(".").pop().toLowerCase()
                    var accepted = ["png", "svg", "webp", "gif"] 
                    if (accepted.indexOf(ext) !== -1) {
                        root.saveEntry({ path: cleanPath })
                    }
                }
                root.dropHover = false
            }
        }

        ResizeHandler {
            anchorItem: stickerImage
            hoverActive: root.containsMouse
            locked: Config.options.background.widgetsLocked
            currentWidth: root.widgetSize
            resizeMode: "diagonal"
            rotatable: true
            currentRotation: root.widgetRotation
            z: 1
            onResized: (newValue) => {
                root.widgetSize = Math.max(60, newValue)
            }
            onResizeFinished: {
                root.saveEntry({ size: root.widgetSize })
            }
            onRotated: (newAngle) => {
                root.widgetRotation = newAngle
            }
            onRotateFinished: {
                root.saveEntry({ rotation: root.widgetRotation })
            }
        }
    }
}