// The interface's pictures, as flattened by the build script
//
// Shared by `#[path]` between the kernel and clients, like `font.rs`.
// `icongen.rs` parses lucide's `d` attributes, flattens curves and arcs, and
// emits polylines in hundredths of the 24-unit box. Drawing scales them into a
// rectangle and strokes them; `Canvas::stroke` has the round caps these paths
// use, which also round the joins

include!(concat!(env!("OUT_DIR"), "/icon_art.rs"));
