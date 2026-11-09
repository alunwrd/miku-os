//! Repositories and where the machine's state lives
//!
//! - Nothing is dropped silently: an unknown line is reported with file and
//!   line number (`lib/gfx/mayland/src/conf.rs`).
//! - A parse error in anything deciding trust denies everything
//!   (`lib/admin/mikuauth/src/conf.rs`). `key =` says whose signatures this machine
//!   believes.

pub mod grammar;
pub mod layers;
pub mod note;

use alloc::string::{String, ToString};
use alloc::vec::Vec;

pub use layers::{load, Files};
pub use note::{Complaint, Note};

use crate::name::Name;
use crate::trust::keyring::KeyId;

/// Largest configuration file
pub const MAX_BYTES: usize = 64 * 1024;
pub const MAX_REPOS: usize = 64;
pub const MAX_MIRRORS: usize = 16;
pub const MAX_IGNORES: usize = 256;

/// Where configuration lives. Absolute, passed to the machine's `open`, not to
/// a `Root`
pub const ETC: &str = "/etc/mikupkg";

/// Somewhere to fetch packages from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub name: Name,
    pub url: String,
    /// Key allowed to sign this repository's index. `None` leaves it to the
    /// keyring, which is weaker
    pub key: Option<KeyId>,
    /// Higher wins when two repositories offer the same package
    pub priority: i32,
    pub mirrors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub cache: String,
    /// Where the unprivileged half leaves what it checked. World-writable;
    /// nothing in it is trusted
    pub stage: String,
    pub db: String,
    pub journal: String,
    pub keyring: String,
    /// An index older than this is stale even if not expired, in case a
    /// repository stops publishing its expiry
    pub max_index_age: u64,
    /// Whether a package may be replaced by an older one. Off by default
    pub allow_downgrade: bool,
    /// Packages left alone
    pub ignore: Vec<Name>,
    pub repos: Vec<Repo>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            cache: "pkg/cache".to_string(),
            stage: "pkg/stage".to_string(),
            db: "pkg/db".to_string(),
            journal: "pkg/journal".to_string(),
            keyring: "etc/mikupkg/keys".to_string(),
            max_index_age: 7 * 86_400,
            allow_downgrade: false,
            ignore: Vec::new(),
            repos: Vec::new(),
        }
    }
}

impl Config {
    /// Applier paths, relative: the leading slash is trimmed here. A person
    /// writes `/pkg/db` and a syscall takes that, but a `Root` path is relative
    /// to its root. Mixing them gives `//pkg/db`, a path with an empty
    /// component
    pub fn paths(&self) -> crate::apply::Paths {
        let rel = |p: &str| p.trim_start_matches('/').to_string();
        crate::apply::Paths {
            journal: alloc::format!("{}/tx", rel(&self.journal)),
            journal_dir: rel(&self.journal),
            db: rel(&self.db),
            cache: rel(&self.cache),
            stage: rel(&self.stage),
        }
    }

    pub fn repo(&self, name: &Name) -> Option<&Repo> {
        self.repos.iter().find(|r| r.name == *name)
    }

    pub fn is_ignored(&self, name: &Name) -> bool {
        self.ignore.contains(name)
    }
}
