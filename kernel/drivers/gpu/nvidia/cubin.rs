// The cubin reader, as the kernel sees it
//
// `lib/buildgen/cubin.rs` is included here by `#[path]` and by
// `lib/buildgen/shadergen.rs` the same way, so the parser of what `nvcc`
// produced at build time also reads what a ring-3 program hands to
// `gpu_program`. Here the input comes from a process, so every bound is
// checked; see the header of the shared file.

#[path = "../../../../lib/buildgen/cubin.rs"]
mod reader;

pub use reader::{kernels, Bad};
