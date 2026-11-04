//! One package's metadata as types. `format/` turns this into a container
//! section and back

pub mod dep;
pub mod path;
pub mod tag;
pub mod trigger;

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

pub use dep::{Dep, DepError, OptDep, Provide};
pub use trigger::Trigger;

use crate::name::Name;
use crate::version::Version;

/// What a package was built for. `Any` covers wallpapers, certificates, and
/// configuration
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Arch {
    Any,
    X86_64,
}

impl Arch {
    pub fn parse(s: &str) -> Option<Arch> {
        match s {
            "any" => Some(Arch::Any),
            "x86_64" => Some(Arch::X86_64),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Arch::Any => "any",
            Arch::X86_64 => "x86_64",
        }
    }

    /// May a machine of `self` install a package built for `other`?
    pub fn accepts(self, other: Arch) -> bool {
        other == Arch::Any || other == self
    }
}

impl fmt::Display for Arch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Everything a package says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageMeta {
    pub name: Name,
    pub version: Version,
    pub arch: Arch,

    pub depends: Vec<Dep>,
    pub provides: Vec<Provide>,
    pub conflicts: Vec<Dep>,
    pub replaces: Vec<Dep>,
    pub optdepends: Vec<OptDep>,
    pub triggers: Vec<Trigger>,

    pub desc: String,
    pub url: String,
    pub license: String,
    pub build_time: u64,
    /// Bytes on disk once installed. Advisory, since the file table is
    /// authoritative, but it lets a plan be weighed against free space before
    /// downloading
    pub installed_size: u64,
}

impl PackageMeta {
    /// The minimal package: a name, a version, and no dependencies
    pub fn new(name: Name, version: Version, arch: Arch) -> PackageMeta {
        PackageMeta {
            name,
            version,
            arch,
            depends: Vec::new(),
            provides: Vec::new(),
            conflicts: Vec::new(),
            replaces: Vec::new(),
            optdepends: Vec::new(),
            triggers: Vec::new(),
            desc: String::new(),
            url: String::new(),
            license: String::new(),
            build_time: 0,
            installed_size: 0,
        }
    }

    /// Whether this package satisfies `want`, by being it or providing it
    pub fn satisfies(&self, want: &Dep) -> bool {
        if want.satisfied_by(&self.name, &self.version) {
            return true;
        }
        self.provides.iter().any(|p| {
            p.name == want.name && want.versions.contains(&p.version_or(self.version))
        })
    }
}

impl fmt::Display for PackageMeta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.name, self.version)
    }
}
