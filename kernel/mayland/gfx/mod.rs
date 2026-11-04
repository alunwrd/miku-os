// Drawing, shared with every client that draws
//
// Not the protocol and not the compositor: the toolkit both ends use for a
// rectangle of pixels. A client writing a letter and a console writing one
// produce the same letter, and a button drawn by the installer and by a
// userspace app is one button, so these files, like the protocol's, come from
// `libmay`, the one crate both halves of the machine link.
//
//      font   the bitmaps, where the framebuffer keeps them
//      paint  a canvas: rectangles, blending, rounded corners
//      text   glyphs onto a canvas, and the cell metrics the console agrees to
//      ui     the widgets: buttons, rows, selects
//
// `text` finds its `font` inside the crate; the re-export below is for the
// kernel's own callers.

pub use crate::io::framebuffer::font;

pub use libmay::paint;
pub use libmay::text;
pub use libmay::ui;
