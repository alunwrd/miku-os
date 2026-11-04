// The colour pipeline
//
// What happens to a pixel between the window fetching it and the panel showing
// it. `dev/colour-pipeline.md` has the ten stages in hardware order. The code
// is cut along NVKMS's four seams:
//
//   state.rs    what the head should be doing: gamma, range, dither, depth.
//               Validated, comparable, defaulting to what the hardware does by
//               itself. NVKMS calls these attributes
//   curve.rs    ramps: a request turned into the numbers a LUT takes
//   matrix.rs   colour space conversion: a 3x4 matrix in the hardware's own
//               fixed-point format
//   (methods)   `modeset/core.rs` and `window.rs`, the only place a method
//               number appears
//
// Arithmetic never writes a method, and a method never computes a value.

mod curve;
// The CSC stages are not wired yet: OCSC0 in the head and the six in the window
// are disabled. See `dev/colour-pipeline.md`
#[allow(dead_code)]
mod matrix;
mod state;

pub use curve::Curve;
pub use matrix::{Matrix, Range};
pub use state::{Dither, Depth, HeadColour};
