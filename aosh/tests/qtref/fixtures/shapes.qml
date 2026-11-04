// QtQuick.Shapes as end4 draws with them: the screen's round corners
// (`RoundCorner`, 23 across, filled black, through the curve renderer in a
// smoothed layer), the same corner through the default renderer, the arcs of a
// `CircularProgress` (2 and 3 wide, round ends), and a polyline stroked; then
// the other pens the curve renderer has: flat and square ends, bevel and mitre
// corners, pens thinner than a pixel. The picture is what is compared
import QtQuick
import QtQuick.Shapes

Rectangle {
    id: root
    width: 650
    height: 255
    color: "white"

    component Corner: Item {
        id: c
        property int corner: 0   // 0 top left, 1 top right, 2 bottom left, 3 bottom right
        property int size: 23
        property bool curve: true
        width: size; height: size
        Shape {
            anchors.fill: parent
            layer.enabled: c.curve
            layer.smooth: true
            preferredRendererType: c.curve ? Shape.CurveRenderer : Shape.UnknownRenderer
            ShapePath {
                id: sp
                strokeWidth: 0
                fillColor: "black"
                startX: (c.corner === 0 || c.corner === 2) ? 0 : c.size
                startY: (c.corner === 0 || c.corner === 1) ? 0 : c.size
                PathAngleArc {
                    moveToStart: false
                    centerX: c.size - sp.startX
                    centerY: c.size - sp.startY
                    radiusX: c.size
                    radiusY: c.size
                    startAngle: [180, -90, 90, 0][c.corner]
                    sweepAngle: 90
                }
                PathLine { x: sp.startX; y: sp.startY }
            }
        }
    }

    // 0 - 3: the four corners; 4: one through the default renderer; 5: a
    // larger one
    Row {
        x: 10; y: 10; spacing: 12
        Corner { corner: 0 }
        Corner { corner: 1 }
        Corner { corner: 2 }
        Corner { corner: 3 }
        Corner { corner: 0; curve: false }
        Corner { corner: 3; size: 41 }
    }

    component Arc: Item {
        id: a
        property real lineWidth: 2
        property real degree: 250
        property real size: 30
        width: size; height: size
        Shape {
            anchors.fill: parent
            layer.enabled: true
            layer.smooth: true
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeColor: "#303060"
                strokeWidth: a.lineWidth
                capStyle: ShapePath.RoundCap
                fillColor: "transparent"
                PathAngleArc {
                    centerX: a.width / 2
                    centerY: a.height / 2
                    radiusX: a.size / 2 - a.lineWidth
                    radiusY: a.size / 2 - a.lineWidth
                    startAngle: -90
                    sweepAngle: a.degree
                }
            }
        }
    }

    // Shapes given no size, which are as big as their paths reach
    function report() {
        return { corner: [ia.implicitWidth, ia.implicitHeight, ia.width, ia.height],
                 line: [ib.implicitWidth, ib.implicitHeight], clear: [ic.implicitWidth, ic.implicitHeight],
                 ring: [id.implicitWidth, id.implicitHeight], behind: [ie.implicitWidth, ie.implicitHeight] }
    }
    Item {
        x: 410; y: 8
        Shape {
            id: ia
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 0; fillColor: "black"; startX: 0; startY: 0
                PathAngleArc { moveToStart: false; centerX: 23; centerY: 23; radiusX: 23; radiusY: 23; startAngle: 180; sweepAngle: 90 }
                PathLine { x: 0; y: 0 }
            }
        }
        Shape {
            id: ib
            x: 30
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 4; strokeColor: "#406020"; fillColor: "transparent"; startX: 10; startY: 15
                PathLine { x: 30; y: 40 }
            }
        }
        Shape {
            id: ic
            x: 70
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 4; strokeColor: "transparent"; fillColor: "#406020"; startX: 10; startY: 15
                PathLine { x: 30; y: 40 }
                PathLine { x: 10; y: 40 }
            }
        }
        Shape {
            id: id
            x: 110
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 3; strokeColor: "#204060"; fillColor: "#a0c0e0"; startX: 10; startY: 15
                PathAngleArc { centerX: 20; centerY: 20; radiusX: 8; radiusY: 8; startAngle: 0; sweepAngle: 360 }
            }
        }
        Shape {
            id: ie
            x: 150
            preferredRendererType: Shape.CurveRenderer
            ShapePath {
                strokeWidth: 0; fillColor: "#602040"; startX: -5; startY: -5
                PathLine { x: 20; y: 12 }
                PathLine { x: 0; y: 12 }
            }
        }
    }

    // 6 - 8: arcs as a CircularProgress has them
    Row {
        x: 10; y: 80; spacing: 14
        Arc { }
        Arc { lineWidth: 3; degree: 120; size: 40 }
        Arc { lineWidth: 2; degree: -300; size: 22 }
    }

    // 9: a polyline, stroked and not filled
    Shape {
        x: 200; y: 80; width: 120; height: 60
        layer.enabled: true
        layer.smooth: true
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: "#a02020"
            strokeWidth: 2.5
            fillColor: "transparent"
            joinStyle: ShapePath.RoundJoin
            capStyle: ShapePath.RoundCap
            PathPolyline { path: [Qt.point(5, 50), Qt.point(30, 10), Qt.point(60, 40), Qt.point(110, 8)] }
        }
    }

    component Pen: Shape {
        id: pn
        property real w: 6
        property int cap: ShapePath.RoundCap
        property int join: ShapePath.BevelJoin
        property var pts: [Qt.point(10, 50), Qt.point(35, 10), Qt.point(60, 50)]
        width: 70; height: 60
        layer.enabled: true
        layer.smooth: true
        preferredRendererType: Shape.CurveRenderer
        ShapePath {
            strokeColor: "#204060"
            strokeWidth: pn.w
            fillColor: "transparent"
            capStyle: pn.cap
            joinStyle: pn.join
            PathPolyline { path: pn.pts }
        }
    }

    // 10 - 16: ends and corners
    Row {
        x: 10; y: 175; spacing: 10
        Pen { cap: ShapePath.FlatCap }
        Pen { cap: ShapePath.SquareCap; join: ShapePath.MiterJoin }
        Pen { join: ShapePath.RoundJoin; w: 4.5 }
        Pen { w: 1 }
        Pen { w: 0.5; pts: [Qt.point(5, 20), Qt.point(60, 32)] }
        Pen { w: 3; pts: [Qt.point(10.3, 40.4), Qt.point(30.7, 12.2), Qt.point(62.1, 20.6)] }
        Pen { cap: ShapePath.FlatCap; join: ShapePath.MiterJoin; pts: [Qt.point(8, 45), Qt.point(35, 30), Qt.point(62, 45)] }
    }
}
