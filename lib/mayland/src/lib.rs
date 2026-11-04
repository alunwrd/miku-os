// libmay: the Mayland display protocol
//
// Linked by the kernel's display server and every program that draws, so there
// is one encoding.
//
//      wire        the byte format
//      protocol    interfaces, opcodes, the numbers both sides name
//      requests    one method per request, as a trait over a byte sink
//      conn        the client's side: ids, versions, and decoded events
//      keymap      scancodes to characters, and the table the server sends
//      conf        the config-file grammar
//      manifest    which files a config is made of, and in what order
//      qoi         the one image format decoded at runtime
//      font        glyph tables, rasterized by this crate's build script
//      paint       a canvas: rectangles, blends, rounded corners
//      text        strings onto a canvas
//      ui          widgets over `paint`
//
// The desktop's screens are QML, drawn by aosh (`dev/aosh-architecture.md`).
//
// `core` only: clients have no allocator and the kernel's allocations are its
// own.
//
// Modules refer to each other with `super::`, which works unchanged at a
// crate's top level.

#![cfg_attr(not(test), no_std)]
#![allow(dead_code)]

pub mod wire;
pub mod protocol;
pub mod requests;
pub mod conn;
pub mod keymap;
pub mod conf;
pub mod monitor;
pub mod manifest;
pub mod spawn;
pub mod qoi;
pub mod font;
pub mod icons;
pub mod ease;
pub mod paint;
pub mod shape;
pub mod text;
pub mod ui;
