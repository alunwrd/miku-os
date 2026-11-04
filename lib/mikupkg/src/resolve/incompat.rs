//! An incompatibility: a set of terms that cannot all hold at once
//!
//! Everything the solver knows is one of these. `foo 1.0 depends on bar >=2` is
//! `{foo 1.0, not bar >=2}`, `foo conflicts with baz` is `{foo, baz}`, "you
//! asked for foo" is `{not foo}`.
//!
//! Each carries its cause. The cause chain is a DAG, and `explain.rs` walks it
//! to produce the explanation.

use alloc::vec::Vec;

use super::term::Term;
use super::universe::Pkg;
use crate::version::Version;

/// Why an incompatibility exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cause {
    /// The request: what you asked for must be present
    Root,
    /// `of` at `version` depends on the other term
    Dependency { of: Pkg, version: Version },
    /// Nothing matches. `unknown` separates a name no repository has (a typo or
    /// a missing `sync`) from one at versions the request rules out
    NoVersions { pkg: Pkg, unknown: bool },
    /// `of` at `version` says it cannot be installed beside the other term
    Conflict { of: Pkg, version: Version },
    /// A provider implies its virtual name, or a virtual name implies one of
    /// its providers
    Provides,
    /// Resolved from two others during conflict resolution
    Derived { left: usize, right: usize },
}

/// A set of terms that cannot all be satisfied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Incompat {
    /// At most one term per package; two are intersected, or the satisfier
    /// search could find the wrong one
    pub terms: Vec<(Pkg, Term)>,
    pub cause: Cause,
}

impl Incompat {
    pub fn new(terms: Vec<(Pkg, Term)>, cause: Cause) -> Incompat {
        let mut merged: Vec<(Pkg, Term)> = Vec::with_capacity(terms.len());
        for (pkg, term) in terms {
            match merged.iter_mut().find(|(p, _)| *p == pkg) {
                Some((_, existing)) => *existing = existing.intersect(&term),
                None => merged.push((pkg, term)),
            }
        }
        // A vacuous term changes no answer
        merged.retain(|(_, t)| !t.is_vacuous());
        Incompat { terms: merged, cause }
    }

    pub fn term_for(&self, pkg: &Pkg) -> Option<&Term> {
        self.terms.iter().find(|(p, _)| p == pkg).map(|(_, t)| t)
    }

    pub fn mentions(&self, pkg: &Pkg) -> bool {
        self.terms.iter().any(|(p, _)| p == pkg)
    }

    /// The incompatibility meaning there is no answer: an empty clause, or
    /// `{not root}`, the request contradicted
    pub fn is_failure(&self) -> bool {
        self.terms.is_empty() || (self.terms.len() == 1 && self.terms[0].0 == Pkg::Root)
    }
}

/// Every incompatibility, by index. Indices because a cause is two others, and
/// owned trees would copy the derivation at each step
#[derive(Debug, Clone, Default)]
pub struct Store {
    items: Vec<Incompat>,
}

impl Store {
    pub fn new() -> Store {
        Store { items: Vec::new() }
    }

    pub fn add(&mut self, inc: Incompat) -> usize {
        self.items.push(inc);
        self.items.len() - 1
    }

    pub fn get(&self, id: usize) -> &Incompat {
        &self.items[id]
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Incompatibilities mentioning this package, newest first: a learned
    /// clause is the most likely to propagate next
    pub fn mentioning(&self, pkg: &Pkg) -> Vec<usize> {
        let mut out: Vec<usize> = (0..self.items.len())
            .filter(|i| self.items[*i].mentions(pkg))
            .collect();
        out.reverse();
        out
    }
}
