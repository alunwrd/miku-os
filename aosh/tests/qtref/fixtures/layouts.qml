// Where QtQuick.Layouts put items when there is more room than they ask
// for: end4's media card is a ColumnLayout 120 high with two things in it
// that do not fill. The numbers are what `report()` says; the picture is
// each layout's items, coloured (at random: the picture is not compared)
import QtQuick
import QtQuick.Layouts

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    component Box: Rectangle {
        property real ih: 20
        property real iw: 30
        implicitHeight: ih
        implicitWidth: iw
        color: Qt.hsla(Math.random(), 0.5, 0.6, 1)
    }

    // 0 - the media card's: 38 and 42, overlapping by 10, in 120
    ColumnLayout {
        id: l0
        x: 10; y: 10; width: 60; height: 120
        spacing: -10
        Box { id: a0; ih: 38; Layout.fillWidth: true; Layout.alignment: Qt.AlignVCenter }
        Box { id: b0; ih: 42; iw: 40; Layout.alignment: Qt.AlignRight }
    }

    // 1 - three, no spacing, in 150
    ColumnLayout {
        id: l1
        x: 90; y: 10; width: 60; height: 150
        spacing: 0
        Box { id: a1; ih: 20 }
        Box { id: b1; ih: 30 }
        Box { id: c1; ih: 10 }
    }

    // 2 - one fills
    ColumnLayout {
        id: l2
        x: 170; y: 10; width: 60; height: 150
        spacing: 5
        Box { id: a2; ih: 20 }
        Box { id: b2; ih: 30; Layout.fillHeight: true }
        Box { id: c2; ih: 10 }
    }

    // 3 - aligned to the top and to the bottom of their cells
    ColumnLayout {
        id: l3
        x: 250; y: 10; width: 60; height: 150
        spacing: 4
        Box { id: a3; ih: 20; Layout.alignment: Qt.AlignTop }
        Box { id: b3; ih: 30; Layout.alignment: Qt.AlignBottom }
    }

    // 4 - one that may grow to 50, and one to 100
    ColumnLayout {
        id: l4
        x: 330; y: 10; width: 60; height: 150
        spacing: 0
        Box { id: a4; ih: 20; Layout.maximumHeight: 50 }
        Box { id: b4; ih: 30; Layout.maximumHeight: 100 }
    }

    // 5 - a RowLayout the same way: 30 and 40 across 200
    RowLayout {
        id: l5
        x: 410; y: 10; width: 200; height: 40
        spacing: 6
        Box { id: a5; iw: 30 }
        Box { id: b5; iw: 40 }
    }

    // 6 - a filler Item that fills, as end4 pushes things apart
    ColumnLayout {
        id: l6
        x: 410; y: 70; width: 60; height: 150
        spacing: 4
        Box { id: a6; ih: 20 }
        Item { id: f6; Layout.fillHeight: true }
        Box { id: b6; ih: 30 }
    }

    // 7 - margins on one that does not fill
    ColumnLayout {
        id: l7
        x: 490; y: 70; width: 60; height: 150
        spacing: 0
        Box { id: a7; ih: 20; Layout.topMargin: 10; Layout.bottomMargin: 6 }
        Box { id: b7; ih: 30 }
    }

    // 8 - preferred heights given
    ColumnLayout {
        id: l8
        x: 570; y: 70; width: 60; height: 150
        spacing: 0
        Box { id: a8; Layout.preferredHeight: 25 }
        Box { id: b8; ih: 10; Layout.minimumHeight: 15 }
    }

    // 9 - 13: sizes of a fraction of a pixel, rounded up; a filler; an
    // item centred and one at the far end of its cell; too little room
    Column {
        x: 10; y: 175
        RowLayout { id: r9; width: 200; spacing: 4; Box { iw: 30.3 } Item { Layout.fillWidth: true } Box { iw: 40.6 } }
        RowLayout { id: r10; width: 200; spacing: 4; Box { iw: 30.3 } Box { iw: 40.6 } }
        RowLayout { id: r11; width: 200.5; spacing: 3; Box { iw: 30.3; Layout.fillWidth: true } Box { iw: 40.6 } }
        RowLayout { id: r12; width: 200; spacing: 0; Box { iw: 30.7; Layout.alignment: Qt.AlignHCenter } Box { iw: 40.2; Layout.alignment: Qt.AlignRight } }
    }
    RowLayout { id: r13; x: 250; y: 175; width: 100; spacing: 0; Box { iw: 70.4 } Box { iw: 50.2 } }

    // 14 - 17: GridLayouts one line long, as end4's bar has them: a row with
    // `columns: -1` and the tray's negative spacing, the same centred in a
    // bigger box, a column with `columns: 1`, and `rows: 1`
    GridLayout { id: g14; x: 370; y: 175; columns: -1; columnSpacing: -6; rowSpacing: 4; Box { iw: 24 } Box { iw: 30.4 } Box { iw: 20 } }
    Item {
        x: 370; y: 205; width: 120; height: 40
        GridLayout { id: g15; anchors.centerIn: parent; columns: -1; columnSpacing: 0; rowSpacing: 0; Box { iw: 26; ih: 26 } Box { iw: 33; ih: 21 } }
    }
    GridLayout { id: g16; x: 500; y: 175; width: 40; columns: 1; rowSpacing: 3; columnSpacing: 9; Box { ih: 12 } Box { ih: 15.5; Layout.fillWidth: true } }
    GridLayout { id: g17; x: 550; y: 175; width: 90; height: 30; rows: 1; columnSpacing: 2; Box { iw: 20 } Box { iw: 25; Layout.fillWidth: true } }

    // 18, 19: Flows: one line with an item of no width passed over, as the
    // bar's system icons have one, and one 60 wide that wraps
    Flow { id: f18; x: 370; y: 225; spacing: 10; Box { iw: 18; ih: 18 } Box { iw: 0 } Box { iw: 22.5; ih: 14 } }
    Flow { id: f19; x: 500; y: 215; width: 60; spacing: 4; Box { iw: 30; ih: 10 } Box { iw: 20; ih: 14 } Box { iw: 25; ih: 8 } Box { iw: 50; ih: 6 } }

    function report() {
        const at = (i) => [i.x, i.y, i.width, i.height];
        return {
            l0: [at(a0), at(b0)], l1: [at(a1), at(b1), at(c1)], l2: [at(a2), at(b2), at(c2)], l3: [at(a3), at(b3)],
            l4: [at(a4), at(b4)], l5: [at(a5), at(b5)], l6: [at(a6), at(f6), at(b6)], l7: [at(a7), at(b7)],
            l8: [at(a8), at(b8)],
            r9: Array.from(r9.children).map(at), r10: Array.from(r10.children).map(at),
            r11: Array.from(r11.children).map(at), r12: Array.from(r12.children).map(at),
            r13: Array.from(r13.children).map(at),
            g14: [at(g14)].concat(Array.from(g14.children).map(at)), g15: [at(g15)].concat(Array.from(g15.children).map(at)),
            g16: [at(g16)].concat(Array.from(g16.children).map(at)), g17: [at(g17)].concat(Array.from(g17.children).map(at)),
            f18: [at(f18)].concat(Array.from(f18.children).map(at)), f19: [at(f19)].concat(Array.from(f19.children).map(at)),
        };
    }
}
