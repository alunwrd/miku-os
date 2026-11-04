//! A term: what is known or required about one package
//!
//! - `Positive(s)`: this package is selected, at a version in `s`.
//! - `Negative(s)`: if selected, its version is not in `s`. An absent package
//!   satisfies every negative term.
//!
//! Both shapes over sets keep every operation total.

use core::fmt;

use crate::range::VersionSet;
use crate::version::Version;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Term {
    pub set: VersionSet,
    pub positive: bool,
}

impl Term {
    pub fn positive(set: VersionSet) -> Term {
        Term { set, positive: true }
    }

    pub fn negative(set: VersionSet) -> Term {
        Term { set, positive: false }
    }

    /// "This package is not here at all"
    pub fn absent() -> Term {
        Term::negative(VersionSet::full())
    }

    /// "Anything, or nothing": says nothing
    pub fn anything() -> Term {
        Term::negative(VersionSet::empty())
    }

    pub fn negate(&self) -> Term {
        Term { set: self.set.clone(), positive: !self.positive }
    }

    /// Nothing satisfies it. Only a positive term can be impossible
    pub fn is_impossible(&self) -> bool {
        self.positive && self.set.is_empty()
    }

    /// Everything satisfies it
    pub fn is_vacuous(&self) -> bool {
        !self.positive && self.set.is_empty()
    }

    pub fn allows(&self, v: &Version) -> bool {
        if self.positive {
            self.set.contains(v)
        } else {
            !self.set.contains(v)
        }
    }

    /// Everything satisfying both. "In a" and "not in b" gives "in a minus b"
    pub fn intersect(&self, other: &Term) -> Term {
        match (self.positive, other.positive) {
            (true, true) => Term::positive(self.set.intersection(&other.set)),
            (true, false) => Term::positive(self.set.intersection(&other.set.complement())),
            (false, true) => Term::positive(other.set.intersection(&self.set.complement())),
            (false, false) => Term::negative(self.set.union(&other.set)),
        }
    }

    /// Whether everything satisfying `self` also satisfies `other`. A negative
    /// term is never a subset of a positive one
    pub fn subset_of(&self, other: &Term) -> bool {
        match (self.positive, other.positive) {
            (true, true) => self.set.is_subset_of(&other.set),
            (true, false) => !self.set.intersects(&other.set),
            (false, true) => false,
            (false, false) => other.set.is_subset_of(&self.set),
        }
    }
}

/// How a term stands against what is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Relation {
    /// What is known already implies it
    Satisfied,
    /// What is known already rules it out
    Contradicted,
    /// Neither, yet
    Inconclusive,
}

/// `known` is what the partial solution concluded about this package, `term`
/// what an incompatibility says
pub fn relation(known: &Term, term: &Term) -> Relation {
    if known.subset_of(term) {
        Relation::Satisfied
    } else if known.intersect(term).is_impossible() {
        Relation::Contradicted
    } else {
        Relation::Inconclusive
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.positive {
            write!(f, "{}", self.set)
        } else if self.set.is_full() {
            write!(f, "not installed")
        } else {
            write!(f, "not {}", self.set)
        }
    }
}
