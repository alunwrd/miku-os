//! What has been decided, what follows, and when
//!
//! A decision is a guess the solver can take back; a derivation follows from an
//! incompatibility and cannot. Backjumping discards every assignment above a
//! decision level, and each derivation remembers its clause.
//!
//! Each assignment stores the accumulated term for its package, so finding the
//! satisfier is a scan.

use alloc::vec::Vec;

use super::term::{relation, Relation, Term};
use super::universe::Pkg;
use crate::range::VersionSet;
use crate::version::Version;

#[derive(Debug, Clone)]
pub struct Assignment {
    pub pkg: Pkg,
    /// What this assignment added
    pub term: Term,
    /// Everything known about this package once it had been added
    pub accumulated: Term,
    pub level: usize,
    /// `None` for a decision
    pub cause: Option<usize>,
}

impl Assignment {
    pub fn is_decision(&self) -> bool {
        self.cause.is_none()
    }
}

#[derive(Debug, Clone, Default)]
pub struct Partial {
    assignments: Vec<Assignment>,
    level: usize,
}

impl Partial {
    pub fn new() -> Partial {
        Partial { assignments: Vec::new(), level: 0 }
    }

    pub fn level(&self) -> usize {
        self.level
    }

    pub fn assignments(&self) -> &[Assignment] {
        &self.assignments
    }

    /// Everything known about this package, or nothing at all
    pub fn term_for(&self, pkg: &Pkg) -> Option<Term> {
        self.assignments
            .iter()
            .rev()
            .find(|a| a.pkg == *pkg)
            .map(|a| a.accumulated.clone())
    }

    pub fn relation(&self, pkg: &Pkg, term: &Term) -> Relation {
        match self.term_for(pkg) {
            None => Relation::Inconclusive,
            Some(known) => relation(&known, term),
        }
    }

    /// A guess: this package and version, at a new decision level
    pub fn decide(&mut self, pkg: Pkg, version: Version) {
        self.level += 1;
        let term = Term::positive(VersionSet::exact(version));
        self.push(pkg, term, None);
    }

    /// A fact, at the current level, with the clause that produced it
    pub fn derive(&mut self, pkg: Pkg, term: Term, cause: usize) {
        self.push(pkg, term, Some(cause));
    }

    fn push(&mut self, pkg: Pkg, term: Term, cause: Option<usize>) {
        let accumulated = match self.term_for(&pkg) {
            Some(known) => known.intersect(&term),
            None => term.clone(),
        };
        self.assignments.push(Assignment {
            pkg,
            term,
            accumulated,
            level: self.level,
            cause,
        });
    }

    /// Discards everything above `level`; the decision at that level stays
    pub fn backtrack(&mut self, level: usize) {
        self.assignments.retain(|a| a.level <= level);
        self.level = level;
    }

    /// First assignment after which `term` is satisfied for `pkg`, as an index
    /// into `assignments`. `None` means not yet satisfied
    pub fn satisfier(&self, pkg: &Pkg, term: &Term) -> Option<usize> {
        self.assignments
            .iter()
            .position(|a| a.pkg == *pkg && relation(&a.accumulated, term) == Relation::Satisfied)
    }

    /// Every package with a positive term and no decision yet.
    pub fn undecided(&self) -> Vec<(Pkg, Term)> {
        let mut out: Vec<(Pkg, Term)> = Vec::new();
        for a in &self.assignments {
            if a.accumulated.positive
                && !out.iter().any(|(p, _)| *p == a.pkg)
                && !self.assignments.iter().any(|d| d.pkg == a.pkg && d.is_decision())
            {
                out.push((a.pkg, a.accumulated.clone()));
            }
        }
        // Rebuilt with the latest accumulated term for each, since the loop
        // takes the first mention
        for entry in out.iter_mut() {
            if let Some(t) = self.term_for(&entry.0) {
                entry.1 = t;
            }
        }
        out
    }

    /// The answer, once nothing is undecided.
    pub fn decisions(&self) -> Vec<(Pkg, Version)> {
        let mut out = Vec::new();
        for a in &self.assignments {
            if a.is_decision() {
                if let Some(v) = only_version(&a.term) {
                    out.push((a.pkg, v));
                }
            }
        }
        out
    }
}

/// A decision's term is one version; this extracts it
fn only_version(term: &Term) -> Option<Version> {
    if !term.positive {
        return None;
    }
    let parts = term.set.intervals();
    if parts.len() != 1 {
        return None;
    }
    match (parts[0].lo, parts[0].hi) {
        (crate::range::Bound::Included(a), crate::range::Bound::Included(b)) if a == b => Some(a),
        _ => None,
    }
}
