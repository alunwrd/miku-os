//! The solver's only source of answers: versions, dependencies, conflicts, and
//! what is installed. A repository index or a written-out table implements it
//! the same way

use alloc::vec::Vec;
use core::fmt;

use crate::meta::dep::{Dep, Provide};
use crate::name::Name;
use crate::version::Version;

/// Which package a term is about. `Root` is the request, a variant since a
/// reserved name could be taken
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Pkg {
    Root,
    Named(Name),
}

impl Pkg {
    pub fn name(&self) -> Option<&Name> {
        match self {
            Pkg::Root => None,
            Pkg::Named(n) => Some(n),
        }
    }
}

impl fmt::Display for Pkg {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Pkg::Root => f.write_str("what you asked for"),
            Pkg::Named(n) => write!(f, "{n}"),
        }
    }
}

/// What a repository and an installed set look like to the solver.
pub trait Universe {
    /// Every name known, real or virtual. Used once, at setup, to find which
    /// are provided
    fn names(&self) -> Vec<Name>;

    /// Versions of a real package; empty for a purely virtual name
    fn versions(&self, pkg: &Name) -> Vec<Version>;

    fn depends(&self, pkg: &Name, v: &Version) -> Vec<Dep>;

    fn conflicts(&self, pkg: &Name, v: &Version) -> Vec<Dep>;

    fn provides(&self, pkg: &Name, v: &Version) -> Vec<Provide>;

    /// What is on the machine now. A preference, not a constraint: trying the
    /// installed version first lets the explanation say "bar will be upgraded"
    /// and keeps an ordinary install from rewriting the machine
    fn installed(&self, pkg: &Name) -> Option<Version>;
}
