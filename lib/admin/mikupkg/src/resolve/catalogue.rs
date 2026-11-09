//! Everything installable and everything installed
//!
//! The `Universe` the solver asks, built from verified repository indexes and
//! the installed database. Every index entry carries the package's metadata, so
//! a solve needs no network; only `sync` does.
//!
//! Where two repositories offer one package, the higher priority wins, then
//! configuration order. Version does not decide, which could take packages from
//! an unintended repository.

use alloc::vec::Vec;

use super::universe::Universe;
use crate::format::index::Index;
use crate::format::meta as meta_codec;
use crate::meta::dep::{Dep, Provide};
use crate::meta::PackageMeta;
use crate::name::Name;
use crate::version::Version;

/// One package on offer, with where it came from.
pub struct Offer {
    pub repo: Name,
    pub priority: i32,
    pub meta: PackageMeta,
    /// Where to fetch it, relative to the repository's url, and its expected
    /// hash
    pub path: alloc::string::String,
    pub sha256: [u8; 32],
    pub size: u64,
}

#[derive(Default)]
pub struct Catalogue {
    offers: Vec<Offer>,
    installed: Vec<(Name, Version)>,
}

impl Catalogue {
    pub fn new() -> Catalogue {
        Catalogue::default()
    }

    /// Everything a verified index offers. A record with unreadable metadata is
    /// skipped and named
    pub fn add_index(
        &mut self,
        index: &Index,
        priority: i32,
        mut note: impl FnMut(&str, crate::format::FormatError),
    ) {
        for entry in &index.entries {
            match meta_codec::decode(&entry.meta, |_, _| {}) {
                Ok(meta) => self.offers.push(Offer {
                    repo: index.repo,
                    priority,
                    meta,
                    path: entry.path.clone(),
                    sha256: entry.sha256,
                    size: entry.size,
                }),
                Err(why) => note(&entry.path, why),
            }
        }
    }

    /// A package given as a file, solved against installed and offered packages
    /// like any other
    pub fn add_local(&mut self, meta: PackageMeta, path: &str, sha256: [u8; 32], size: u64) {
        self.offers.push(Offer {
            repo: Name::parse("local").expect("a legal name"),
            // Above every repository, since it was named directly
            priority: i32::MAX,
            meta,
            path: alloc::string::String::from(path),
            sha256,
            size,
        });
    }

    pub fn add_installed(&mut self, name: Name, version: Version) {
        self.installed.push((name, version));
    }

    pub fn offers(&self) -> &[Offer] {
        &self.offers
    }

    /// Which offer to take for a solved name and version: highest priority,
    /// then configuration order, never by version
    pub fn best(&self, name: &Name, version: &Version) -> Option<&Offer> {
        self.offers
            .iter()
            .filter(|o| o.meta.name == *name && o.meta.version == *version)
            .max_by_key(|o| o.priority)
    }

    fn of<'a>(&'a self, pkg: &Name, v: &Version) -> Option<&'a PackageMeta> {
        self.offers
            .iter()
            .find(|o| o.meta.name == *pkg && o.meta.version == *v)
            .map(|o| &o.meta)
    }
}

impl Universe for Catalogue {
    fn names(&self) -> Vec<Name> {
        let mut out: Vec<Name> = self.offers.iter().map(|o| o.meta.name).collect();
        out.sort();
        out.dedup();
        out
    }

    fn versions(&self, pkg: &Name) -> Vec<Version> {
        let mut out: Vec<Version> = self
            .offers
            .iter()
            .filter(|o| o.meta.name == *pkg)
            .map(|o| o.meta.version)
            .collect();
        out.sort();
        out.dedup();
        out
    }

    fn depends(&self, pkg: &Name, v: &Version) -> Vec<Dep> {
        self.of(pkg, v).map(|m| m.depends.clone()).unwrap_or_default()
    }

    fn conflicts(&self, pkg: &Name, v: &Version) -> Vec<Dep> {
        self.of(pkg, v).map(|m| m.conflicts.clone()).unwrap_or_default()
    }

    fn provides(&self, pkg: &Name, v: &Version) -> Vec<Provide> {
        self.of(pkg, v).map(|m| m.provides.clone()).unwrap_or_default()
    }

    fn installed(&self, pkg: &Name) -> Option<Version> {
        self.installed.iter().find(|(n, _)| n == pkg).map(|(_, v)| *v)
    }
}
