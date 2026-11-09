//! Sets of versions, the resolver's algebra
//!
//! A dependency is a set of acceptable versions. Every operation returns
//! another set with no unrepresentable case, which lets explanations print
//! "every version of menu requires intl <4.0.0".
//!
//! Stored as a sorted list of disjoint, non-adjacent intervals, canonical by
//! construction. The solver uses `PartialEq` to detect what it has already
//! derived, so one set must have one spelling.

use alloc::vec::Vec;
use core::cmp::Ordering;
use core::fmt;

use crate::version::Version;

/// One end of an interval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bound {
    /// No limit in this direction
    Unbounded,
    /// This version is in the set
    Included(Version),
    /// Up to or past this version, excluding it
    Excluded(Version),
}

impl Bound {
    fn version(&self) -> Option<&Version> {
        match self {
            Bound::Unbounded => None,
            Bound::Included(v) | Bound::Excluded(v) => Some(v),
        }
    }
}

/// Compares two lower bounds: which starts first. `Included(v)` precedes
/// `Excluded(v)`
fn cmp_lo(a: &Bound, b: &Bound) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Less,
        (_, Bound::Unbounded) => Ordering::Greater,
        _ => {
            let (x, y) = (a.version().unwrap(), b.version().unwrap());
            match x.cmp(y) {
                Ordering::Equal => match (a, b) {
                    (Bound::Included(_), Bound::Excluded(_)) => Ordering::Less,
                    (Bound::Excluded(_), Bound::Included(_)) => Ordering::Greater,
                    _ => Ordering::Equal,
                },
                other => other,
            }
        }
    }
}

/// Compares two upper bounds: which ends last
fn cmp_hi(a: &Bound, b: &Bound) -> Ordering {
    match (a, b) {
        (Bound::Unbounded, Bound::Unbounded) => Ordering::Equal,
        (Bound::Unbounded, _) => Ordering::Greater,
        (_, Bound::Unbounded) => Ordering::Less,
        _ => {
            let (x, y) = (a.version().unwrap(), b.version().unwrap());
            match x.cmp(y) {
                Ordering::Equal => match (a, b) {
                    (Bound::Included(_), Bound::Excluded(_)) => Ordering::Greater,
                    (Bound::Excluded(_), Bound::Included(_)) => Ordering::Less,
                    _ => Ordering::Equal,
                },
                other => other,
            }
        }
    }
}

/// An interval, half-open or closed at either end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Interval {
    pub lo: Bound,
    pub hi: Bound,
}

impl Interval {
    /// Whether it contains anything. Versions are treated as dense:
    /// `(1.0, 1.1)` is non-empty even with no version between, so the algebra
    /// does not depend on an index
    fn is_inhabited(&self) -> bool {
        match (&self.lo, &self.hi) {
            (Bound::Unbounded, _) | (_, Bound::Unbounded) => true,
            (Bound::Included(a), Bound::Included(b)) => a <= b,
            _ => self.lo.version().unwrap() < self.hi.version().unwrap(),
        }
    }

    fn contains(&self, v: &Version) -> bool {
        let above_lo = match &self.lo {
            Bound::Unbounded => true,
            Bound::Included(a) => v >= a,
            Bound::Excluded(a) => v > a,
        };
        let below_hi = match &self.hi {
            Bound::Unbounded => true,
            Bound::Included(b) => v <= b,
            Bound::Excluded(b) => v < b,
        };
        above_lo && below_hi
    }
}

/// Whether `..hi` and `lo..` join with nothing left out. Two exclusions of one
/// version do not join: `<1.0` and `>1.0` leave `1.0` out
fn joins(hi: &Bound, lo: &Bound) -> bool {
    match (hi, lo) {
        (Bound::Unbounded, _) | (_, Bound::Unbounded) => true,
        (Bound::Excluded(a), Bound::Excluded(b)) => a > b,
        _ => hi.version().unwrap() >= lo.version().unwrap(),
    }
}

/// A set of versions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct VersionSet {
    /// Sorted by `lo`, disjoint, non-adjacent. Every constructor restores this
    parts: Vec<Interval>,
}

impl VersionSet {
    /// The empty set, which signals a conflict
    pub fn empty() -> VersionSet {
        VersionSet { parts: Vec::new() }
    }

    /// Every version there is
    pub fn full() -> VersionSet {
        VersionSet {
            parts: alloc::vec![Interval { lo: Bound::Unbounded, hi: Bound::Unbounded }],
        }
    }

    /// Exactly this version
    pub fn exact(v: Version) -> VersionSet {
        VersionSet {
            parts: alloc::vec![Interval { lo: Bound::Included(v), hi: Bound::Included(v) }],
        }
    }

    pub fn at_least(v: Version) -> VersionSet {
        VersionSet::interval(Bound::Included(v), Bound::Unbounded)
    }
    pub fn greater_than(v: Version) -> VersionSet {
        VersionSet::interval(Bound::Excluded(v), Bound::Unbounded)
    }
    pub fn at_most(v: Version) -> VersionSet {
        VersionSet::interval(Bound::Unbounded, Bound::Included(v))
    }
    pub fn less_than(v: Version) -> VersionSet {
        VersionSet::interval(Bound::Unbounded, Bound::Excluded(v))
    }

    pub fn interval(lo: Bound, hi: Bound) -> VersionSet {
        let i = Interval { lo, hi };
        if i.is_inhabited() {
            VersionSet { parts: alloc::vec![i] }
        } else {
            VersionSet::empty()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    pub fn is_full(&self) -> bool {
        self.parts.len() == 1
            && self.parts[0].lo == Bound::Unbounded
            && self.parts[0].hi == Bound::Unbounded
    }

    pub fn intervals(&self) -> &[Interval] {
        &self.parts
    }

    pub fn contains(&self, v: &Version) -> bool {
        self.parts.iter().any(|i| i.contains(v))
    }

    /// Everything in either.
    pub fn union(&self, other: &VersionSet) -> VersionSet {
        let mut all: Vec<Interval> = Vec::with_capacity(self.parts.len() + other.parts.len());
        all.extend_from_slice(&self.parts);
        all.extend_from_slice(&other.parts);
        VersionSet::canonical(all)
    }

    /// Everything in both.
    pub fn intersection(&self, other: &VersionSet) -> VersionSet {
        let mut out: Vec<Interval> = Vec::new();
        let (mut i, mut j) = (0usize, 0usize);
        while i < self.parts.len() && j < other.parts.len() {
            let a = self.parts[i];
            let b = other.parts[j];
            let lo = if cmp_lo(&a.lo, &b.lo) == Ordering::Greater { a.lo } else { b.lo };
            let hi = if cmp_hi(&a.hi, &b.hi) == Ordering::Less { a.hi } else { b.hi };
            let cut = Interval { lo, hi };
            if cut.is_inhabited() {
                out.push(cut);
            }
            // Retires whichever ends first; the other may still meet the next
            if cmp_hi(&a.hi, &b.hi) == Ordering::Less {
                i += 1;
            } else {
                j += 1;
            }
        }
        VersionSet { parts: out }
    }

    /// Everything not in it
    pub fn complement(&self) -> VersionSet {
        if self.parts.is_empty() {
            return VersionSet::full();
        }
        let mut out: Vec<Interval> = Vec::new();
        let mut cursor = Bound::Unbounded;

        for part in &self.parts {
            let hi = match part.lo {
                Bound::Unbounded => None,
                // The gap ends where this interval begins, and the bounds
                // mirror: everything below an included 1.0 is `< 1.0`
                Bound::Included(v) => Some(Bound::Excluded(v)),
                Bound::Excluded(v) => Some(Bound::Included(v)),
            };
            if let Some(hi) = hi {
                let gap = Interval { lo: cursor, hi };
                if gap.is_inhabited() {
                    out.push(gap);
                }
            }
            cursor = match part.hi {
                Bound::Unbounded => {
                    // Nothing above it, since the parts are sorted and disjoint
                    return VersionSet { parts: out };
                }
                Bound::Included(v) => Bound::Excluded(v),
                Bound::Excluded(v) => Bound::Included(v),
            };
        }

        let tail = Interval { lo: cursor, hi: Bound::Unbounded };
        if tail.is_inhabited() {
            out.push(tail);
        }
        VersionSet { parts: out }
    }

    /// Whether everything here is also there: nothing here is outside there
    pub fn is_subset_of(&self, other: &VersionSet) -> bool {
        self.intersection(&other.complement()).is_empty()
    }

    pub fn intersects(&self, other: &VersionSet) -> bool {
        !self.intersection(other).is_empty()
    }

    /// Sorts, drops empties, merges what touches. The only constructor from
    /// loose intervals
    fn canonical(mut parts: Vec<Interval>) -> VersionSet {
        parts.retain(|i| i.is_inhabited());
        parts.sort_by(|a, b| match cmp_lo(&a.lo, &b.lo) {
            Ordering::Equal => cmp_hi(&a.hi, &b.hi),
            other => other,
        });

        let mut out: Vec<Interval> = Vec::with_capacity(parts.len());
        for part in parts {
            match out.last_mut() {
                Some(last) if joins(&last.hi, &part.lo) => {
                    if cmp_hi(&part.hi, &last.hi) == Ordering::Greater {
                        last.hi = part.hi;
                    }
                }
                _ => out.push(part),
            }
        }
        VersionSet { parts: out }
    }
}

impl fmt::Display for VersionSet {
    /// Printed for the explanation, its only user
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.parts.is_empty() {
            return f.write_str("no version");
        }
        if self.is_full() {
            return f.write_str("any version");
        }
        for (k, part) in self.parts.iter().enumerate() {
            if k > 0 {
                f.write_str(" or ")?;
            }
            match (&part.lo, &part.hi) {
                (Bound::Included(a), Bound::Included(b)) if a == b => write!(f, "{a}")?,
                (Bound::Unbounded, Bound::Included(b)) => write!(f, "<={b}")?,
                (Bound::Unbounded, Bound::Excluded(b)) => write!(f, "<{b}")?,
                (Bound::Included(a), Bound::Unbounded) => write!(f, ">={a}")?,
                (Bound::Excluded(a), Bound::Unbounded) => write!(f, ">{a}")?,
                (lo, hi) => {
                    match lo {
                        Bound::Included(a) => write!(f, ">={a}")?,
                        Bound::Excluded(a) => write!(f, ">{a}")?,
                        Bound::Unbounded => {}
                    }
                    // A space, not a comma: `>=1.0 <2.0 or >3.0` is one set
                    // with a hole
                    f.write_str(" ")?;
                    match hi {
                        Bound::Included(b) => write!(f, "<={b}")?,
                        Bound::Excluded(b) => write!(f, "<{b}")?,
                        Bound::Unbounded => {}
                    }
                }
            }
        }
        Ok(())
    }
}
