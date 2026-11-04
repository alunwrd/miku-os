//! MikuOS-specific extensions to primitives in the [`std::ffi`] module.
//!
//! [`std::ffi`]: crate::ffi
//!
//! An `OsStr` on MikuOS is bytes: a filesystem name is up to 255 bytes and need
//! not be UTF-8. The same two traits as `std::os::unix::ffi`, over the same
//! `sys::os_str::Buf`, re-exported from there.

#![stable(feature = "rust1", since = "1.0.0")]
// Stable: these items are reached through `std::os::unix::{ffi,fs,process}`,
// which is stable on every Unix, and crates on stable Rust (`rustix`, and
// through it `polling`, `async-io`, `smol`) cannot name an unstable item.
// MikuOS-only additions carry `#[unstable]` themselves.

#[path = "../unix/ffi/os_str.rs"]
mod os_str;

#[stable(feature = "rust1", since = "1.0.0")]
pub use self::os_str::{OsStrExt, OsStringExt};
