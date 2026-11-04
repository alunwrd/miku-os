//! Header: `unistd.h`
//!
//! The three descriptor numbers. `kernel/syscall/mod.rs` reserves 0, 1 and 2
//! before any other descriptor is handed out.

pub use crate::new::common::posix::unistd::{STDERR_FILENO, STDIN_FILENO, STDOUT_FILENO};
