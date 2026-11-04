//! HTTP
//!
//! `wire` is bytes to bytes and compiles with `crypto`, so `cargo test` reaches
//! the header parser and chunked decoder on the build host. `fetch` needs a
//! resolver, TLS, and a socket, and is `userspace`-gated.

pub mod wire;
pub use wire::*;

#[cfg(feature = "userspace")]
pub mod fetch;
#[cfg(feature = "userspace")]
pub use fetch::*;
