// MultiEffect as end4's bar uses it: an app's icon coloured toward the bar's
// text through a circle (Colorizer), the active workspace's number cut from its
// indicator (Colorizer masked by text), the occupied workspaces' pill through
// the pills (MaskMultiEffect); and the rest of its colour and mask: contrast,
// saturation, a see-through colour, both thresholds, inverted.
// One case a cell, 80 by 80, on white
import QtQuick
import QtQuick.Effects

Rectangle {
    id: root
    width: 650
    height: 170
    color: "white"

    // A picture with edges and colours in it
    component Pattern: Item {
        width: 40
        height: 40
        Rectangle { x: 0; y: 0; width: 20; height: 20; color: "#e04030" }
        Rectangle { x: 20; y: 0; width: 20; height: 20; color: "#30a050" }
        Rectangle { x: 0; y: 20; width: 20; height: 20; color: "#3050e0" }
        Rectangle { x: 20; y: 20; width: 20; height: 20; color: "#f0c020" }
        Rectangle { x: 10; y: 10; width: 20; height: 20; radius: 10; color: "#ffffff" }
    }
    component Disc: Rectangle {
        width: 40; height: 40; radius: 20; visible: false
        layer.enabled: true
    }

    // 0 - an app icon in the bar: coloured half way to the text, cut to a circle
    Pattern { id: p0; x: 20; y: 20; visible: false }
    Disc { id: d0; x: 20; y: 20 }
    MultiEffect {
        x: 20; y: 20; width: 40; height: 40
        source: p0
        colorization: 0.5
        colorizationColor: "#c1c7ce"
        brightness: 0
        maskEnabled: true; maskSource: d0
        maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 1 - monochrome, 0.8 of the way, from a picture
    Image { id: p1; x: 100; y: 20; width: 40; height: 40; source: "pattern.png"; visible: false }
    Disc { id: d1; x: 100; y: 20 }
    MultiEffect {
        x: 100; y: 20; width: 40; height: 40
        source: p1
        colorization: 0.8
        colorizationColor: "#c1c7ce"
        maskEnabled: true; maskSource: d1
        maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 2 - the active workspace's number: the indicator, its colour turned to
    // the text's, where the numbers are
    Rectangle { id: ind2; x: 170; y: 25; width: 60; height: 30; radius: 15; color: "#90caf3"; visible: false }
    Row {
        id: num2; x: 170; y: 25; width: 60; height: 30; visible: false
        layer.enabled: true
        Repeater {
            model: 3
            Text { width: 20; height: 30; text: index + 4; font.pixelSize: 15; horizontalAlignment: Text.AlignHCenter; verticalAlignment: Text.AlignVCenter }
        }
    }
    MultiEffect {
        x: 170; y: 25; width: 60; height: 30
        source: ind2
        colorization: 1
        colorizationColor: "#00344f"
        brightness: 1 - 0.75
        maskEnabled: true; maskSource: num2
        maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 3 - the occupied workspaces' pill, through pills that meet
    Rectangle { id: bg3; x: 250; y: 30; width: 60; height: 20; color: "#99404850"; visible: false }
    Item {
        id: pills3; x: 250; y: 30; width: 60; height: 20; visible: false
        layer.enabled: true
        Rectangle { x: 0; width: 30; height: 20; radius: 10 }
        Rectangle { x: 15; width: 30; height: 20; radius: 10 }
        Rectangle { x: 42; width: 18; height: 20; radius: 9 }
    }
    MultiEffect {
        x: 250; y: 30; width: 60; height: 20
        source: bg3
        maskEnabled: true; maskSource: pills3
        maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 4 - inverted, softly, through a disc
    Pattern { id: p4; x: 340; y: 20; visible: false }
    Disc { id: d4; x: 340; y: 20 }
    MultiEffect {
        x: 340; y: 20; width: 40; height: 40
        source: p4
        maskEnabled: true; maskSource: d4; maskInverted: true
        maskThresholdMin: 0.3; maskSpreadAtMin: 0.4
    }

    // 5 - contrast, brightness and saturation
    Pattern { id: p5; x: 420; y: 20; visible: false }
    MultiEffect {
        x: 420; y: 20; width: 40; height: 40
        source: p5
        contrast: 0.4; brightness: -0.2; saturation: -0.6
    }

    // 6 - a see-through colour mixed in, stretched to twice the source
    Pattern { id: p6; x: 490; y: 10; visible: false }
    MultiEffect {
        x: 490; y: 10; width: 60; height: 60
        source: p6
        colorization: 0.7
        colorizationColor: "#8020a0f0"
        saturation: 0.5
    }

    // 7 - both thresholds, the mask a ramp of see-through
    Pattern { id: p7; x: 580; y: 20; visible: false }
    Rectangle {
        id: r7; x: 580; y: 20; width: 40; height: 40; visible: false
        layer.enabled: true
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0; color: "#00000000" }
            GradientStop { position: 1; color: "#ff000000" }
        }
    }
    MultiEffect {
        x: 580; y: 20; width: 40; height: 40
        source: p7
        maskEnabled: true; maskSource: r7
        maskThresholdMin: 0.2; maskSpreadAtMin: 0.3
        maskThresholdMax: 0.8; maskSpreadAtMax: 0.2
    }

    // 8 - a Colorizer as end4 writes it, on a shrunk and faded icon, as a
    // workspace's is while Super is held
    Pattern { id: p8; x: 20; y: 100; visible: false }
    Disc { id: d8; x: 20; y: 100 }
    MultiEffect {
        x: 20; y: 100; width: 40; height: 40
        opacity: 0.7
        scale: 0.8
        source: p8
        colorization: 1
        colorizationColor: "#a0d0f0"
        brightness: 1 - 0.4
        maskEnabled: true; maskSource: d8
        maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 9 - nothing asked of it but its source: the source as it is
    Pattern { id: p9; x: 100; y: 100; visible: false }
    MultiEffect { x: 100; y: 100; width: 40; height: 40; source: p9 }

    // 11, 12 - case 8 at its own size, and only faded
    Pattern { id: p11; x: 260; y: 100; visible: false }
    Disc { id: d11; x: 260; y: 100 }
    MultiEffect {
        x: 260; y: 100; width: 40; height: 40
        source: p11
        colorization: 1; colorizationColor: "#a0d0f0"; brightness: 1 - 0.4
        maskEnabled: true; maskSource: d11; maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }
    Pattern { id: p12; x: 340; y: 100; visible: false }
    Disc { id: d12; x: 340; y: 100 }
    MultiEffect {
        x: 340; y: 100; width: 40; height: 40
        scale: 0.8
        source: p12
        colorization: 1; colorizationColor: "#a0d0f0"; brightness: 1 - 0.4
        maskEnabled: true; maskSource: d12; maskThresholdMin: 0.5; maskSpreadAtMin: 1
    }

    // 10 - turned
    Pattern { id: p10; x: 180; y: 100; visible: false }
    MultiEffect { x: 180; y: 100; width: 40; height: 40; rotation: 20; source: p10; colorization: 0.5 }
}
