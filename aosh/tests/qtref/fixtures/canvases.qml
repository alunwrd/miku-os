// Canvases as end4's widgets paint them: Material shapes filled from cubic
// curves under a scale, arcs stroked with round caps, polylines filled and
// stroked, a dashed border, see-through fills. One case a cell, 80 by 80,
// on white; nothing but Canvases
import QtQuick

Rectangle {
    id: root
    width: 650
    height: 170
    color: "white"

    // 0 - a circle, filled
    Canvas {
        x: 10; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.fillStyle = "#3060c0";
            ctx.beginPath();
            ctx.arc(30, 30, 24.5, 0, Math.PI * 2);
            ctx.fill();
        }
        Component.onCompleted: requestPaint()
    }
    // 1 - a resize handle: a quarter arc, 3 wide, round caps
    Canvas {
        x: 90; y: 10; width: 62; height: 62
        onPaint: {
            const ctx = getContext("2d");
            ctx.reset();
            ctx.strokeStyle = "#202020";
            ctx.lineWidth = 3;
            ctx.lineCap = "round";
            ctx.beginPath();
            ctx.arc(width * 0.5, height * 0.5, width * 0.35, 0, Math.PI * 0.5);
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
    // 2 - a shape in unit coordinates, scaled to the canvas, as ShapeCanvas
    // draws a Material cookie
    Canvas {
        x: 170; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.fillStyle = "#c03060";
            ctx.clearRect(0, 0, width, height);
            ctx.save();
            ctx.scale(60, 60);
            ctx.translate(0, 0);
            ctx.beginPath();
            const n = 7;
            const pts = [];
            for (let i = 0; i < n * 2; i++) {
                const a = Math.PI * 2 * i / (n * 2) - Math.PI / 2;
                const r = i % 2 === 0 ? 0.5 : 0.4;
                pts.push([0.5 + r * Math.cos(a), 0.5 + r * Math.sin(a)]);
            }
            ctx.moveTo(pts[0][0], pts[0][1]);
            for (let i = 0; i < pts.length; i++) {
                const p = pts[i], q = pts[(i + 1) % pts.length];
                const m = [(p[0] + q[0]) / 2, (p[1] + q[1]) / 2];
                ctx.bezierCurveTo(p[0] + (m[0] - p[0]) * 0.6, p[1] + (m[1] - p[1]) * 0.6,
                                  q[0] - (q[0] - m[0]) * 0.6, q[1] - (q[1] - m[1]) * 0.6, q[0], q[1]);
            }
            ctx.closePath();
            ctx.fill();
            ctx.restore();
        }
        Component.onCompleted: requestPaint()
    }
    // 3 - a graph: a polyline filled under, and stroked
    Canvas {
        x: 250; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.clearRect(0, 0, width, height);
            const ys = [40, 30, 35, 12, 20, 8, 25, 18];
            ctx.beginPath();
            ctx.moveTo(0, height);
            for (let i = 0; i < ys.length; i++)
                ctx.lineTo(i * width / (ys.length - 1), ys[i]);
            ctx.lineTo(width, height);
            ctx.closePath();
            ctx.fillStyle = "#6040a080";
            ctx.fill();
            ctx.beginPath();
            for (let i = 0; i < ys.length; i++)
                ctx.lineTo(i * width / (ys.length - 1), ys[i]);
            ctx.strokeStyle = "#6040a0";
            ctx.lineWidth = 2;
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
    // 4 - a dashed border
    Canvas {
        x: 330; y: 10; width: 60; height: 40
        onPaint: {
            const ctx = getContext("2d");
            ctx.clearRect(0, 0, width, height);
            ctx.save();
            ctx.strokeStyle = "#208060";
            ctx.lineWidth = 2;
            ctx.setLineDash([6, 4]);
            ctx.strokeRect(1, 1, width - 2, height - 2);
            ctx.restore();
        }
        Component.onCompleted: requestPaint()
    }
    // 5 - thin lines, on and between pixels
    Canvas {
        x: 410; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.strokeStyle = "#000000";
            ctx.lineWidth = 1;
            ctx.beginPath();
            ctx.moveTo(5, 10); ctx.lineTo(55, 10);
            ctx.moveTo(5, 20.5); ctx.lineTo(55, 20.5);
            ctx.moveTo(5, 30); ctx.lineTo(55, 50);
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
    // 6 - see-through fills, and globalAlpha
    Canvas {
        x: 490; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.fillStyle = Qt.rgba(0.9, 0.3, 0.1, 0.6);
            ctx.fillRect(5, 5, 35, 35);
            ctx.globalAlpha = 0.5;
            ctx.fillStyle = "#0040c0";
            ctx.beginPath();
            ctx.arc(38, 38, 17, 0, Math.PI * 2);
            ctx.fill();
        }
        Component.onCompleted: requestPaint()
    }
    // 7 - painted without asking: does a Canvas paint when it first shows?
    Canvas {
        x: 570; y: 10; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.fillStyle = "#a0a020";
            ctx.fillRect(10, 10, 40, 40);
        }
    }

    // 8 - turned, its picture with it
    Canvas {
        x: 15; y: 95; width: 50; height: 50; rotation: 20
        onPaint: {
            const ctx = getContext("2d");
            ctx.fillStyle = "#30a0a0";
            ctx.fillRect(5, 5, 40, 40);
        }
        Component.onCompleted: requestPaint()
    }
    // 9 - an arc the other way round, open, stroked with butt caps
    Canvas {
        x: 90; y: 90; width: 62; height: 62
        onPaint: {
            const ctx = getContext("2d");
            ctx.strokeStyle = "#a03030";
            ctx.lineWidth = 4;
            ctx.beginPath();
            ctx.arc(31, 31, 20, Math.PI * 0.15, Math.PI * 1.75, false);
            ctx.stroke();
            ctx.beginPath();
            ctx.moveTo(40, 12);
            ctx.lineTo(35, 9);
            ctx.moveTo(40, 12);
            ctx.lineTo(37, 17);
            ctx.lineWidth = 3;
            ctx.lineCap = "round";
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
    // 10 - a quadratic curve and a joined polyline, miter and round joins
    Canvas {
        x: 170; y: 90; width: 60; height: 60
        onPaint: {
            const ctx = getContext("2d");
            ctx.lineWidth = 5;
            ctx.strokeStyle = "#404080";
            ctx.beginPath();
            ctx.moveTo(5, 25); ctx.lineTo(20, 8); ctx.lineTo(35, 25);
            ctx.stroke();
            ctx.lineJoin = "round";
            ctx.beginPath();
            ctx.moveTo(5, 50); ctx.quadraticCurveTo(30, 20, 55, 50);
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
    // 11 - a rounded rectangle path, filled and stroked
    Canvas {
        x: 250; y: 95; width: 60; height: 50
        onPaint: {
            const ctx = getContext("2d");
            const r = 10, x = 4, y = 4, w = 52, h = 42;
            ctx.beginPath();
            ctx.moveTo(x + r, y);
            ctx.arcTo(x + w, y, x + w, y + h, r);
            ctx.arcTo(x + w, y + h, x, y + h, r);
            ctx.arcTo(x, y + h, x, y, r);
            ctx.arcTo(x, y, x + w, y, r);
            ctx.closePath();
            ctx.fillStyle = "#f0d0a0";
            ctx.fill();
            ctx.strokeStyle = "#806020";
            ctx.lineWidth = 1.5;
            ctx.stroke();
        }
        Component.onCompleted: requestPaint()
    }
}
