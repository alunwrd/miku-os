// Rectangles as end4's widgets draw them: rounded, bordered, in gradients,
// turned and scaled, and inside things that are. One case a cell, 80 by 80,
// on white; nothing but Rectangles, so the picture is the painter's alone
import QtQuick

Rectangle {
    id: root
    width: 650
    height: 250
    color: "white"

    // 0 - no antialiasing, at a fraction of a pixel
    Rectangle { x: 10.3; y: 10.6; width: 50.4; height: 30.2; color: "#3060c0" }
    // 1 - a radius, which turns antialiasing on
    Rectangle { x: 90; y: 10; width: 60; height: 40; radius: 8; color: "#c03060" }
    // 2 - and a border
    Rectangle { x: 170; y: 10; width: 60; height: 40; radius: 8; color: "#f0e0a0"; border.width: 2; border.color: "#204080" }
    // 3 - a border and no fill, square
    Rectangle { x: 250; y: 10; width: 60; height: 40; color: "transparent"; border.width: 1; border.color: "black" }
    // 4 - a border colour alone is a border a pixel wide
    Rectangle { x: 330; y: 10; width: 60; height: 40; color: "#a0d0a0"; border.color: "#d02020" }
    // 5 - corners of their own
    Rectangle { x: 410; y: 10; width: 60; height: 40; topLeftRadius: 14; bottomRightRadius: 5; color: "#6040a0" }
    // 6 - a gradient down, rounded
    Rectangle {
        x: 490; y: 10; width: 60; height: 60; radius: 10
        gradient: Gradient {
            GradientStop { position: 0; color: "#ff0000" }
            GradientStop { position: 0.5; color: "#00ff00" }
            GradientStop { position: 1; color: "#0000ff" }
        }
    }
    // 7 - a gradient across, with a border
    Rectangle {
        x: 570; y: 10; width: 60; height: 40; border.width: 3; border.color: "#303030"
        gradient: Gradient {
            orientation: Gradient.Horizontal
            GradientStop { position: 0.2; color: "#ffcc00" }
            GradientStop { position: 0.8; color: "#0066ff" }
        }
    }

    // 8 - turned
    Rectangle { x: 15; y: 100; width: 50; height: 30; radius: 6; rotation: 30; color: "#208060" }
    // 9 - a quarter turn, square
    Rectangle { x: 95; y: 100; width: 50; height: 30; rotation: 90; color: "#806020" }
    // 10 - scaled about its corner
    Rectangle { x: 170; y: 95; width: 30; height: 20; radius: 5; scale: 1.5; transformOrigin: Item.TopLeft; color: "#2060a0" }
    // 11 - a transform list
    Rectangle {
        x: 255; y: 100; width: 50; height: 30; antialiasing: true; color: "#a02060"
        transform: [
            Rotation { angle: -20; origin.x: 25; origin.y: 15 },
            Translate { x: 5 }
        ]
    }
    // 12 - see-through, in something see-through
    Item {
        x: 330; y: 100; width: 60; height: 40; opacity: 0.5
        Rectangle { width: 50; height: 30; radius: 4; color: "#8040a0ff" }
    }
    // 13 - turned, antialiased, square, a border
    Rectangle { x: 415; y: 100; width: 50; height: 30; rotation: 15; antialiasing: true; color: "#e0e0ff"; border.width: 2; border.color: "#4040a0" }
    // 14 - a pill
    Rectangle { x: 490; y: 105; width: 60; height: 20; radius: 10; color: "#30a0a0" }
    // 15 - a circle
    Rectangle { x: 575; y: 95; width: 31; height: 31; radius: 15.5; color: "#d06000" }

    // 16 - inside something turned
    Item {
        x: 10; y: 170; width: 60; height: 60; rotation: 45
        Rectangle { x: 10; y: 20; width: 40; height: 20; radius: 3; color: "#4080ff" }
    }
    // 17 - a clock's hand, turned about its foot
    Rectangle { x: 120; y: 175; width: 3; height: 36; radius: 1.5; transformOrigin: Item.Bottom; rotation: 37; color: "#202020" }
    // 18 - a tick, turned about a dial's middle
    Rectangle {
        x: 199; y: 172; width: 2; height: 8; color: "#404040"
        transform: Rotation { angle: 120; origin.x: 1; origin.y: 28 }
    }
    // 19 - a ring
    Rectangle { x: 250; y: 170; width: 60; height: 40; radius: 8; color: "transparent"; border.width: 1; border.color: "#205080" }
    // 20 - a border at a fraction of a pixel
    Rectangle { x: 330; y: 170; width: 60; height: 40; radius: 6; color: "#fff0f0"; border.width: 1.5; border.pixelAligned: false; border.color: "#a03030" }
    // 21 - corners of their own and a border
    Rectangle { x: 410; y: 170; width: 60; height: 40; radius: 12; topLeftRadius: 0; color: "#f0f0d0"; border.width: 2; border.color: "#606020" }
    // 22 - a radius past half the size
    Rectangle { x: 490; y: 170; width: 60; height: 60; radius: 30; color: "#9020c0" }
    // 23 - an antialiased border at an opacity
    Rectangle { x: 570; y: 170; width: 60; height: 40; radius: 9; opacity: 0.7; color: "#40c040"; border.width: 2; border.color: "#000000" }
}
