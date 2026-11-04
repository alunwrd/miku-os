//! Unit propagation, decision, conflict resolution, backjumping
//!
//! PubGrub: conflict-driven clause learning, specialized to version solving.
//! Every fact has a recorded cause, so a failure arrives as a derivation tree
//! for `explain.rs`.
//!
//! 1. **Propagate.** Walk clauses mentioning the changed package. A clause with
//!    one undetermined term forces its negation, recorded with the clause. A
//!    clause with none is a conflict.
//! 2. **Resolve.** Find the assignment that made the conflicting clause true,
//!    then backjump past the decision that caused it, or resolve against that
//!    assignment's cause and repeat. If the result is the request itself, there
//!    is no answer.
//! 3. **Decide.** With nothing to propagate, guess a version for an undecided
//!    package, add its dependencies, and go to 1.

use alloc::vec::Vec;

use super::incompat::{Cause, Incompat, Store};
use super::partial::Partial;
use super::term::{Relation, Term};
use super::universe::{Pkg, Universe};
use crate::meta::dep::Dep;
use crate::name::Name;
use crate::range::VersionSet;
use crate::version::Version;

/// What the solver decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Solution {
    /// Every real package and its version. Virtual names are resolved away
    pub selected: Vec<(Name, Version)>,
}

/// No answer, with the reasoning
#[derive(Debug, Clone)]
pub struct Failure {
    pub store: Store,
    /// The clause that turned out to be the request itself, contradicted
    pub root: usize,
}

/// A bound on the search; past it something is wrong with the graph
const MAX_STEPS: usize = 20_000;

pub fn solve<U: Universe + ?Sized>(
    universe: &U,
    request: &[Dep],
) -> Result<Solution, Failure> {
    Solver::new(universe, request).run()
}

struct Solver<'u, U: Universe + ?Sized> {
    universe: &'u U,
    store: Store,
    partial: Partial,
    request: Vec<Dep>,
    /// Versions a name has only through a provider
    virtual_versions: Vec<(Name, Vec<Version>)>,
}

enum Propagated {
    Nothing,
    Conflict,
    Derived(Pkg),
}

impl<'u, U: Universe + ?Sized> Solver<'u, U> {
    fn new(universe: &'u U, request: &[Dep]) -> Solver<'u, U> {
        Solver {
            universe,
            store: Store::new(),
            partial: Partial::new(),
            request: request.to_vec(),
            virtual_versions: Vec::new(),
        }
    }

    fn run(mut self) -> Result<Solution, Failure> {
        // "What you asked for cannot be absent": contradicting this means no
        // answer
        self.store.add(Incompat::new(
            alloc::vec![(Pkg::Root, Term::absent())],
            Cause::Root,
        ));
        self.add_provider_clauses();

        let mut next = Pkg::Root;
        for _ in 0..MAX_STEPS {
            if let Err(root) = self.propagate(next) {
                return Err(Failure { store: self.store, root });
            }
            match self.choose() {
                None => return Ok(self.finish()),
                Some(pkg) => next = pkg,
            }
        }
        // Reaching here is a bug in this file
        panic!("the solver did not settle in {MAX_STEPS} steps");
    }

    /// The two halves of `provides` as clauses: a provider implies the name,
    /// and the name implies some provider. A virtual name is an ordinary
    /// package to the solver
    fn add_provider_clauses(&mut self) {
        let mut by_name: Vec<(Name, Vec<(Version, Vec<(Name, Version)>)>)> = Vec::new();

        for name in self.universe.names() {
            for v in self.universe.versions(&name) {
                for p in self.universe.provides(&name, &v) {
                    let provided = p.version_or(v);
                    // provider@v -> virtual@provided
                    self.store.add(Incompat::new(
                        alloc::vec![
                            (Pkg::Named(name), Term::positive(VersionSet::exact(v))),
                            (
                                Pkg::Named(p.name),
                                Term::negative(VersionSet::exact(provided))
                            ),
                        ],
                        Cause::Provides,
                    ));

                    let slot = match by_name.iter_mut().find(|(n, _)| *n == p.name) {
                        Some(s) => &mut s.1,
                        None => {
                            by_name.push((p.name, Vec::new()));
                            &mut by_name.last_mut().unwrap().1
                        }
                    };
                    match slot.iter_mut().find(|(ver, _)| *ver == provided) {
                        Some((_, list)) => list.push((name, v)),
                        None => slot.push((provided, alloc::vec![(name, v)])),
                    }
                }
            }
        }

        for (vname, versions) in &by_name {
            let mut all: Vec<Version> = Vec::new();
            for (provided, providers) in versions {
                all.push(*provided);
                // virtual@provided -> one of the providers
                let mut terms =
                    alloc::vec![(Pkg::Named(*vname), Term::positive(VersionSet::exact(*provided)))];
                for (p, pv) in providers {
                    terms.push((Pkg::Named(*p), Term::negative(VersionSet::exact(*pv))));
                }
                self.store.add(Incompat::new(terms, Cause::Provides));
            }
            all.sort();
            all.dedup();
            self.virtual_versions.push((*vname, all));
        }
    }

    // propagation

    fn propagate(&mut self, start: Pkg) -> Result<(), usize> {
        let mut changed = alloc::vec![start];
        while let Some(pkg) = changed.pop() {
            for id in self.store.mentioning(&pkg) {
                match self.propagate_one(id) {
                    Propagated::Nothing => {}
                    Propagated::Derived(p) => changed.push(p),
                    Propagated::Conflict => {
                        let learned = self.resolve_conflict(id)?;
                        changed.clear();
                        // The learned clause is unit by construction, so
                        // propagating it once gives the next package
                        if let Propagated::Derived(p) = self.propagate_one(learned) {
                            changed.push(p);
                        }
                        break;
                    }
                }
            }
        }
        Ok(())
    }

    fn propagate_one(&mut self, id: usize) -> Propagated {
        let inc = self.store.get(id).clone();
        let mut undetermined: Option<(Pkg, Term)> = None;

        for (pkg, term) in &inc.terms {
            match self.partial.relation(pkg, term) {
                // One false term makes the clause harmless
                Relation::Contradicted => return Propagated::Nothing,
                Relation::Inconclusive => {
                    if undetermined.is_some() {
                        // Two loose ends: nothing follows yet
                        return Propagated::Nothing;
                    }
                    undetermined = Some((*pkg, term.clone()));
                }
                Relation::Satisfied => {}
            }
        }

        match undetermined {
            None => Propagated::Conflict,
            Some((pkg, term)) => {
                self.partial.derive(pkg, term.negate(), id);
                Propagated::Derived(pkg)
            }
        }
    }

    // conflict resolution

    fn resolve_conflict(&mut self, mut id: usize) -> Result<usize, usize> {
        loop {
            let inc = self.store.get(id).clone();
            if inc.is_failure() {
                return Err(id);
            }

            let mut recent_term: Option<(Pkg, Term)> = None;
            let mut recent_at: Option<usize> = None;
            let mut previous_level = 1usize;
            let mut difference: Option<(Pkg, Term)> = None;

            for (pkg, term) in &inc.terms {
                let at = match self.partial.satisfier(pkg, term) {
                    Some(a) => a,
                    // Only if the clause was not really satisfied, a bug above
                    None => return Err(id),
                };

                match recent_at {
                    None => {
                        recent_term = Some((*pkg, term.clone()));
                        recent_at = Some(at);
                    }
                    Some(cur) if cur < at => {
                        previous_level =
                            previous_level.max(self.partial.assignments()[cur].level);
                        recent_term = Some((*pkg, term.clone()));
                        recent_at = Some(at);
                        difference = None;
                    }
                    Some(_) => {
                        previous_level = previous_level.max(self.partial.assignments()[at].level);
                    }
                }

                // What the satisfying assignment adds beyond this term. It goes
                // in the learned clause to keep the reason the assignment was
                // more specific
                if recent_term.as_ref().map(|(p, _)| *p) == Some(*pkg) && recent_at == Some(at) {
                    let assignment = self.partial.assignments()[at].clone();
                    let diff = assignment.term.intersect(&term.negate());
                    if !diff.is_impossible() {
                        if let Some(d_at) = self.partial.satisfier(pkg, &diff.negate()) {
                            previous_level =
                                previous_level.max(self.partial.assignments()[d_at].level);
                        }
                        difference = Some((*pkg, diff));
                    } else {
                        difference = None;
                    }
                }
            }

            let at = recent_at.expect("a satisfied clause has a satisfier");
            let satisfier = self.partial.assignments()[at].clone();

            // The conflict is older than the decision exposing it, so
            // backjumping is enough; or the satisfier was a guess, with nothing
            // further to resolve against
            if previous_level < satisfier.level || satisfier.is_decision() {
                self.partial.backtrack(previous_level);
                return Ok(id);
            }

            let cause_id = satisfier.cause.expect("checked just above");
            let cause = self.store.get(cause_id).clone();
            let (recent_pkg, _) = recent_term.expect("set with recent_at");

            let mut terms: Vec<(Pkg, Term)> = inc
                .terms
                .iter()
                .filter(|(p, _)| *p != recent_pkg)
                .cloned()
                .collect();
            terms.extend(
                cause
                    .terms
                    .iter()
                    .filter(|(p, _)| *p != satisfier.pkg)
                    .cloned(),
            );
            if let Some((d_pkg, diff)) = difference {
                terms.push((d_pkg, diff.negate()));
            }

            id = self.store.add(Incompat::new(
                terms,
                Cause::Derived { left: id, right: cause_id },
            ));
        }
    }

    // decisions

    fn versions_of(&self, pkg: &Pkg) -> Vec<Version> {
        match pkg {
            Pkg::Root => alloc::vec![Version::ZERO],
            Pkg::Named(n) => {
                let mut out = self.universe.versions(n);
                if let Some((_, virt)) = self.virtual_versions.iter().find(|(v, _)| v == n) {
                    for v in virt {
                        if !out.contains(v) {
                            out.push(*v);
                        }
                    }
                }
                out.sort();
                out
            }
        }
    }

    fn choose(&mut self) -> Option<Pkg> {
        let undecided = self.partial.undecided();
        if undecided.is_empty() {
            return None;
        }

        // Fewest candidates first: finds a conflict early and keeps
        // explanations short
        let mut best: Option<(Pkg, Term, usize)> = None;
        for (pkg, term) in &undecided {
            let count = self
                .versions_of(pkg)
                .into_iter()
                .filter(|v| term.allows(v))
                .count();
            if best.as_ref().map(|(_, _, c)| count < *c).unwrap_or(true) {
                best = Some((*pkg, term.clone(), count));
            }
        }
        let (pkg, term, _) = best.expect("undecided is not empty");

        let candidates: Vec<Version> = self
            .versions_of(&pkg)
            .into_iter()
            .filter(|v| term.allows(v))
            .collect();

        // The installed version first, if it will do, so an ordinary install
        // leaves the rest of the machine alone
        let installed = pkg.name().and_then(|n| self.universe.installed(n));
        let chosen = installed
            .filter(|v| candidates.contains(v))
            .or_else(|| candidates.iter().max().copied());

        let version = match chosen {
            None => {
                // Nothing matches. Recorded as a clause with the reason,
                // separating an unknown name from a range nothing fits
                let unknown = self.versions_of(&pkg).is_empty();
                self.store.add(Incompat::new(
                    alloc::vec![(pkg, Term::positive(term.set.clone()))],
                    Cause::NoVersions { pkg, unknown },
                ));
                return Some(pkg);
            }
            Some(v) => v,
        };

        let mut conflict = false;
        for inc in self.clauses_for(&pkg, &version) {
            let id = self.store.add(inc);
            let clause = self.store.get(id).clone();
            // A clause already satisfied apart from the term about this package
            // means this version is ruled out
            conflict = conflict
                || clause.terms.iter().all(|(p, t)| {
                    *p == pkg || self.partial.relation(p, t) == Relation::Satisfied
                });
        }
        if !conflict {
            self.partial.decide(pkg, version);
        }
        Some(pkg)
    }

    /// What follows from having this package at this version.
    fn clauses_for(&self, pkg: &Pkg, version: &Version) -> Vec<Incompat> {
        let here = (*pkg, Term::positive(VersionSet::exact(*version)));
        let mut out = Vec::new();

        let (depends, conflicts) = match pkg {
            Pkg::Root => (self.request.clone(), Vec::new()),
            Pkg::Named(n) => (
                self.universe.depends(n, version),
                self.universe.conflicts(n, version),
            ),
        };

        for d in depends {
            out.push(Incompat::new(
                alloc::vec![
                    here.clone(),
                    (Pkg::Named(d.name), Term::negative(d.versions.clone())),
                ],
                Cause::Dependency { of: *pkg, version: *version },
            ));
        }
        for c in conflicts {
            // Symmetric, so index order does not decide
            out.push(Incompat::new(
                alloc::vec![
                    here.clone(),
                    (Pkg::Named(c.name), Term::positive(c.versions.clone())),
                ],
                Cause::Conflict { of: *pkg, version: *version },
            ));
        }
        out
    }

    fn finish(self) -> Solution {
        let mut selected: Vec<(Name, Version)> = Vec::new();
        for (pkg, version) in self.partial.decisions() {
            if let Pkg::Named(n) = pkg {
                // A virtual name is not installed; its provider is in the list
                let is_real = self.universe.versions(&n).contains(&version);
                if is_real {
                    selected.push((n, version));
                }
            }
        }
        selected.sort();
        Solution { selected }
    }
}
