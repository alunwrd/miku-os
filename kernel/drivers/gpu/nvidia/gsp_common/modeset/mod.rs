// Modeset: turning an enumerated output into a raster
//
//     RM controls (NV0073_CTRL_*) own the far side of the connector: which SOR drives which display id, and the DisplayPort link itself. GSP-RM runs link training.
//     The core channel's method stream owns the near side: the raster the head generates, which output resource it feeds, how deep the pixels are.
//
// A SOR is assigned before the link can be trained, the link carries a known
// bandwidth before the stream parameters can be computed, and all of it is
// settled before the head produces pixels at that clock.
//
// A head with no window bound scans out black: the panel leaves "no signal" and
// locks to our timings




// Re-exported flat, so every caller says `modeset::X`.

mod mode;
mod dp_rm;
mod dp_train;
mod hdmi;
mod core;
mod window;
mod wimm;
mod cursor;
mod readback;
mod colour;
pub mod assign;
pub mod imp;
pub mod vrr;

pub use colour::{Curve, Depth, Dither, HeadColour, Matrix, Range};
pub use core::{colour_encodings, ocsc0_coefficients};
pub use mode::*;
pub use dp_rm::*;
pub use dp_train::*;
pub use hdmi::*;
pub use core::*;
pub use window::*;
pub use wimm::*;
pub use cursor::*;
pub use readback::*;
