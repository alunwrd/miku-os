//! Path conflicts between packages, and with installed packages' paths
//!
//! Found before anything is written. `replaces` is handled here, not in the
//! resolver, as a transition: the old package's files stop being its own before
//! the solve.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::{Op, Plan};
use crate::name::Name;

/// Who owns what. A trait, so the planner can be tested against a table
pub trait Owners {
    /// Which installed package owns this path, if any
    fn owner_of(&self, path: &str) -> Option<Name>;
}

/// Nothing is installed. The state a fresh image is built from
pub struct NoOwners;

impl Owners for NoOwners {
    fn owner_of(&self, _path: &str) -> Option<Name> {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Conflict {
    /// Two operations in one transaction claim one path
    WithinPlan { path: String },
    /// An installed package owns the path, and this transaction does not
    /// replace it
    WithInstalled { path: String, owner: Name },
    /// A file no package owns; overwriting it would lose local changes
    Unowned { path: String },
}

impl fmt::Display for Conflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Conflict::WithinPlan { path } => write!(
                f,
                "{path}: two packages in this transaction both want to own it"
            ),
            Conflict::WithInstalled { path, owner } => write!(
                f,
                "{path}: {owner} already owns it. Remove {owner} first, or install \
                 something that declares it replaces it"
            ),
            Conflict::Unowned { path } => write!(
                f,
                "{path}: something is already there that no package owns. It was put \
                 there by hand, and installing over it would lose it"
            ),
        }
    }
}

/// Every conflict at once, as a list, so a person fixes them in one round trip
pub fn check<O: Owners + ?Sized>(
    plan: &Plan,
    owners: &O,
    exists: impl Fn(&str) -> bool,
) -> Vec<Conflict> {
    let mut out = Vec::new();

    // Packages being placed and removed own their paths: a file moving between
    // packages in one transaction is an upgrade
    let mine: Vec<&Name> = plan
        .installs
        .iter()
        .map(|m| &m.name)
        .chain(plan.removals.iter())
        .collect();

    let mut seen: Vec<&str> = Vec::with_capacity(plan.ops.len());
    for op in &plan.ops {
        // Directories are shared: several packages declare `bin`
        if matches!(op, Op::Dir { .. }) || op.is_removal() {
            continue;
        }
        let path = op.path();

        if seen.contains(&path) {
            out.push(Conflict::WithinPlan { path: String::from(path) });
            continue;
        }
        seen.push(path);

        match owners.owner_of(path) {
            Some(owner) => {
                if !mine.iter().any(|n| **n == owner) {
                    out.push(Conflict::WithInstalled {
                        path: String::from(path),
                        owner,
                    });
                }
            }
            None => {
                if exists(path) {
                    out.push(Conflict::Unowned { path: String::from(path) });
                }
            }
        }
    }

    out
}

/// Installed packages this transaction replaces. Applied before the solve, so
/// they are absent when anything is decided
pub fn replaced_by<'a>(
    installs: &'a [crate::meta::PackageMeta],
    installed: impl Fn(&Name) -> Option<crate::version::Version> + 'a,
) -> Vec<Name> {
    let mut out: Vec<Name> = Vec::new();
    for m in installs {
        for r in &m.replaces {
            if let Some(v) = installed(&r.name) {
                if r.versions.contains(&v) && !out.contains(&r.name) {
                    out.push(r.name);
                }
            }
        }
    }
    out
}
