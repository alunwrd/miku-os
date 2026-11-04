//! Applies a plan and undoes one that stopped halfway
//!
//! The contract:
//!
//! - Every path is relative to the root, using `/`, so `--root` is an ordinary
//!   flag.
//! - Ownership is always set explicitly; the filesystem creates inodes owned by
//!   root.
//! - One file is open at a time, since the vnode table is shared and has no
//!   reclaim path under pressure.
//! - `rename` replaces; the other guarantees depend on it.

#[cfg(feature = "host")]
pub mod host;
#[cfg(feature = "userspace")]
pub mod miku;
pub mod mem;
pub mod recover;
pub mod staged;
pub mod root;
pub mod trigger;
pub mod tx;

use alloc::string::{String, ToString};

pub use root::{Kind, Root, RootError, RootResult, Stat};
pub use tx::{Outcome, TxError};

/// Where the machine's own state lives, relative to the root. `/pkg` holds
/// machine-written state, separate from hand-edited `/etc`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub journal: String,
    pub journal_dir: String,
    pub db: String,
    pub cache: String,
    /// Where the unprivileged half leaves what it fetched and checked.
    /// World-writable: every file is hashed against a signed table before it is
    /// placed, so nothing here is trusted
    pub stage: String,
}

impl Default for Paths {
    fn default() -> Paths {
        Paths {
            journal: "pkg/journal/tx".to_string(),
            journal_dir: "pkg/journal".to_string(),
            db: "pkg/db".to_string(),
            cache: "pkg/cache".to_string(),
            stage: "pkg/stage".to_string(),
        }
    }
}
