//! mikupkg: what a package is and what installing one means. Design in
//! `dev/package-architecture.md`.
//!
//! - `format` decides bytes, `trust` decides trust.
//! - `resolve` asks a `Universe` and knows nothing of files or networks, so
//!   dependency graphs are unit tests.
//! - `apply` reaches the world through the `Root` trait: the builder stages
//!   through `std::fs`, the machine through syscalls, tests through memory that
//!   can fail at step n.
//!
//! Public items are named by `/bin/mikupkg` or `builder`.

#![cfg_attr(not(any(test, feature = "host")), no_std)]
// `deny`, not `forbid`, because of `apply::miku`, the syscall backend. The rest
// is arithmetic over untrusted bytes with no raw pointers
#![deny(unsafe_code)]

extern crate alloc;

/// What a package may be called.
pub mod name;
/// Versions, and the total order over them.
pub mod version;
/// Sets of versions, the resolver's algebra
pub mod range;
/// What a package says about itself: dependencies, provides, conflicts,
/// triggers, and allowed paths
pub mod meta;
/// Bytes on disk: the container, the file table, and their checks. Trust is in
/// `trust`
pub mod format;
/// Whether to believe a set of bytes: keys, signatures, and the rules
/// separating an old index from a stale one
pub mod trust;
/// Machine configuration: repositories, paths, and parser errors
pub mod conf;
/// Working out what to install, and explaining it when there is no answer.
pub mod resolve;
/// The installed set: what is here, why, and who owns each file
pub mod db;
/// Fetches bytes, checks them, and lands them in the cache
pub mod fetch;
/// Turns a solve result into a bounded, ordered list of operations
pub mod plan;
/// `trait Root`, the transaction state machine, and recovery. The only part
/// that touches a filesystem
pub mod apply;

pub use format::{FormatError, Package};
pub use trust::{Key, KeyId, Keyring, Verdict};
pub use meta::{Arch, Dep, PackageMeta, Provide, Trigger};
pub use name::{Name, NameError};
pub use range::VersionSet;
pub use version::{Version, VersionError};
