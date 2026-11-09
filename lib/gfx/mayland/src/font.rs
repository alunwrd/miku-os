// The system font, as rasterized by the build script
//
// Shared by `#[path]` between the kernel and userspace apps. Both build scripts
// run `fontgen.rs` over the same TTF into their own `OUT_DIR`.
//
// A glyph is `CHAR_WIDTH * CHAR_HEIGHT` coverage bytes, 0 for nothing and 255
// for solid, and an advance

include!(concat!(env!("OUT_DIR"), "/font_data.rs"));

// Interface faces beside the console's: several sizes, two weights,
// proportional, ASCII and Cyrillic. `ui.rs` names them
include!(concat!(env!("OUT_DIR"), "/ui_font_data.rs"));
