// Text as end4 sets it: Google Sans Flex at the sizes `Appearance` names,
// with the axes StyledText gives it, and Material Symbols by ligature. The
// numbers are the metrics aosh's layout has to come to; the picture is the
// rows, black on white, one per case
import QtQuick

Rectangle {
    id: root
    width: 760
    height: column.height + 8
    color: "white"

    // Google Sans Flex as end4 reaches it: installed in ~/.local/share/fonts
    // and found by fontconfig, named instances and all. A FontLoader would be
    // another font to Qt: one face, the weight not applied
    QtObject { id: sans; readonly property string name: "Google Sans Flex" }
    FontLoader { id: symbols; source: Qt.resolvedUrl("../../../../etc/aosh/negi/assets/fonts/MaterialSymbolsRounded.ttf") }
    FontLoader { id: caveat; source: Qt.resolvedUrl("../../../../etc/aosh/negi/assets/fonts/handwriting/Caveat.ttf") }

    // family, pixel size, weight, axes, text
    readonly property var cases: [
        ["sans", 10, 400, { "wght": 450, "wdth": 100 }, "days hours min"],
        ["sans", 12, 400, { "wght": 450, "wdth": 100 }, "CPU RAM Disk Something"],
        ["sans", 13, 400, { "wght": 450, "wdth": 100 }, "miku os: fix all known bugs"],
        ["sans", 15, 400, { "wght": 450, "wdth": 100 }, "alunwrd@pc Lock Play"],
        ["sans", 16, 400, { "wght": 450, "wdth": 100 }, "Notes"],
        ["sans", 17, 400, { "wght": 450, "wdth": 100 }, "Ag Notes Play"],
        ["sans", 19, 400, { "wght": 450, "wdth": 100 }, "Workspace 5"],
        ["sans", 22, 400, { "wght": 450, "wdth": 100 }, "Notes Ag"],
        ["sans", 23, 700, {}, "29% 85% 2%"],
        ["sans", 15, 400, {}, "0 12 345"],
        ["sans", 80, 700, {}, "09 05"],
        ["symbols", 18, 400, { "FILL": 1, "opsz": 18 }, "planner_review"],
        ["symbols", 24, 400, { "FILL": 0, "opsz": 24 }, "play_arrow skip_next"],
        ["caveat", 40, 400, {}, "alunwrd"],
        // A Text's `style`, as end4's clock draws it: the glyphs again, a
        // pixel down, under them
        ["sans", 23, 400, { "wght": 450, "wdth": 100 }, "09:05 Raised", Text.Raised, "#e05000"],
        ["sans", 17, 400, { "wght": 450, "wdth": 100 }, "Outline Sunken", Text.Outline, "#0060e0"],
    ]

    Column {
        id: column
        x: 4; y: 4
        spacing: 2
        Repeater {
            id: rows
            model: root.cases
            Text {
                id: t
                required property var modelData
                renderType: Text.NativeRendering
                color: "black"
                text: modelData[4]
                font.family: modelData[0] === "sans" ? sans.name : modelData[0] === "symbols" ? symbols.name : caveat.name
                font.pixelSize: modelData[1]
                font.weight: modelData[2]
                font.variableAxes: modelData[3]
                font.hintingPreference: modelData[0] === "symbols" ? Font.PreferNoHinting : Font.PreferDefaultHinting
                style: modelData.length > 5 ? modelData[5] : Text.Normal
                styleColor: modelData.length > 6 ? modelData[6] : "black"
                FontMetrics { id: fm; font: t.font }
                function measure() {
                    return {
                        family: font.family, px: font.pixelSize, weight: font.weight, axes: modelData[3], text: text,
                        implicitWidth: implicitWidth, implicitHeight: implicitHeight,
                        contentWidth: contentWidth, contentHeight: contentHeight, baselineOffset: baselineOffset,
                        ascent: fm.ascent, descent: fm.descent, height: fm.height, leading: fm.leading,
                        lineSpacing: fm.lineSpacing, advance: fm.advanceWidth(text), y: y,
                        style: style, styleColor: String(styleColor),
                    };
                }
            }
        }
    }

    function report() {
        const out = [];
        for (let i = 0; i < rows.count; i++) out.push(rows.itemAt(i).measure());
        return { fonts: [sans.name, symbols.name, caveat.name], rows: out };
    }
}
