// fshost: what a filesystem needs underneath it
//
// `BlockHost` (sectors in and out, plus a clock), a test double, and a
// checksum.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub use miku_base::crc32c;
pub mod host;

pub use host::{BlockHost, HostError, MemDisk};
