//! The miku libraries as build scripts see them
//!
//! | module    | does |
//! |-----------|------|
//! | `libs`    | `libs.list`: names, dependencies, where each library's files are |
//! | `exports` | reads `#[no_mangle]` functions and their signatures from source |
//! | `stubs`   | an empty `.so` per library, for linking against |
//! | `shim`    | the modules one library calls another through |
//! | `decls`   | the `extern` blocks of `lib/sdk/miku` |
//! | `check`   | hand-written declarations against the definitions |

pub mod check;
pub mod decls;
pub mod exports;
pub mod libs;
pub mod shim;
pub mod stubs;
