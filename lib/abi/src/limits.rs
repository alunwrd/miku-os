//! `<limits.h>`

#![allow(unused_imports)]

use super::prelude::*;

/// The longest path `realpath` (119) and `F_GETPATH` will write, NUL included
pub const PATH_MAX: c_int = 4096;
/// `PIPE_BUF`: a pipe write this size or smaller goes in whole or not at all, so
/// two writers' records never interleave. The ring itself holds 64 KiB
pub const PIPE_BUF: usize = 4096;
pub const NAME_MAX: c_int = 255;
pub const IOV_MAX: c_int = 1024;
