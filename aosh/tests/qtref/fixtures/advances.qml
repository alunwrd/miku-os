// One character at a time: how Qt's advances come out for Google Sans Flex
// at a weight with no axes, at `wght 450`, and bold - whether they are
// rounded, and which instance a weight draws
import QtQuick

Item {
    id: root
    width: 10
    height: 10
    // Google Sans Flex as end4 reaches it: installed in ~/.local/share/fonts
    // and found by fontconfig, named instances and all. A FontLoader would be
    // another font to Qt: one face, the weight not applied
    QtObject { id: sans; readonly property string name: "Google Sans Flex" }
    readonly property var chars: ["0", "2", "%", " ", "A", "N", "o", "29% 85% 2%", "0 12 345"]
    readonly property var configs: [[15, 400, {}], [15, 700, {}], [23, 700, {}], [23, 400, {}],
                                    [15, 400, { "wght": 450, "wdth": 100 }], [100, 400, {}], [100, 700, {}],
                                    [15, 400, { "wght": 400 }], [15, 400, { "wdth": 100 }], [15, 400, { "opsz": 18 }],
                                    [15, 700, { "wght": 700 }], [15, 400, { "GRAD": 0 }]]
    Repeater {
        id: rep
        model: root.configs.length * root.chars.length
        Text {
            required property int index
            readonly property var cfg: root.configs[Math.floor(index / root.chars.length)]
            renderType: Text.NativeRendering
            text: root.chars[index % root.chars.length]
            font.family: sans.name
            font.pixelSize: cfg[0]
            font.weight: cfg[1]
            font.variableAxes: cfg[2]
        }
    }
    function report() {
        const out = [];
        for (let i = 0; i < rep.count; i++) {
            const t = rep.itemAt(i);
            out.push({ px: t.cfg[0], weight: t.cfg[1], axes: t.cfg[2], text: t.text, width: t.implicitWidth, adv: t.contentWidth });
        }
        return { rows: out };
    }
}
