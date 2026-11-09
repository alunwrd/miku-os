//! The derivation tree as text a person can act on
//!
//! Every fact in the search carries its clause, so a failure arrives as a
//! proof.
//!
//! - The request comes last, so the argument ends where the person started.
//! - Only proof steps appear; another package appears only where a numbered
//!   line names it.
//! - It ends with something to try, worked out from what went wrong.

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::incompat::{Cause, Incompat, Store};
use super::solve::Failure;
use super::term::Term;
use super::universe::Pkg;

/// Render a failure.
pub fn report(failure: &Failure) -> String {
    let mut w = Writer {
        store: &failure.store,
        order: Vec::new(),
        lines: Vec::new(),
    };
    w.walk(failure.root);

    let mut out = String::from("mikupkg: cannot work out what to install.\n\n");
    for (i, line) in w.lines.iter().enumerate() {
        out.push_str(&format!("  {}. {line}\n", i + 1));
    }
    out.push_str(&format!(
        "\nSo, {}, version solving failed.\n",
        request_phrase(&failure.store, failure.root)
    ));

    let hints = hints(&failure.store);
    if !hints.is_empty() {
        out.push('\n');
        for h in hints {
            out.push_str(&format!("Try:  {h}\n"));
        }
    }
    out
}

struct Writer<'a> {
    store: &'a Store,
    /// The incompatibility of each numbered line, so a node reached twice is
    /// cited by number
    order: Vec<usize>,
    lines: Vec<String>,
}

impl Writer<'_> {
    /// Post-order: what a step rests on is numbered first. Returns this
    /// incompatibility's line number
    fn walk(&mut self, id: usize) -> usize {
        if let Some(at) = self.order.iter().position(|x| *x == id) {
            return at + 1;
        }

        let inc = self.store.get(id);
        let line = match &inc.cause {
            Cause::Derived { left, right } => {
                let a = self.walk(*left);
                let b = self.walk(*right);
                format!("because ({a}) and ({b}), {}", statement(self.store, id))
            }
            _ => reason(self.store, id),
        };

        self.order.push(id);
        self.lines.push(line);
        self.lines.len()
    }
}

/// What an incompatibility forbids. Read from the terms, so a learned clause
/// says something true
fn statement(store: &Store, id: usize) -> String {
    let inc = store.get(id);
    if inc.terms.is_empty() {
        return "there is no solution".to_string();
    }

    let (here, elsewhere): (Vec<_>, Vec<_>) =
        inc.terms.iter().partition(|(_, t)| t.positive);

    if here.iter().any(|(p, _)| *p == Pkg::Root) && inc.terms.len() == 1 {
        return "there is no solution".to_string();
    }

    match (here.len(), elsewhere.len()) {
        (0, _) => format!("one of {} is needed", any_of(&elsewhere)),
        (_, 0) if here.len() == 1 => format!("{} cannot be installed", one(&here[0])),
        (_, 0) => format!("{} cannot be installed together", list(&here)),
        (1, 1) => format!("{} requires {}", one(&here[0]), one_negated(&elsewhere[0])),
        (1, _) => format!("{} requires one of {}", one(&here[0]), any_of(&elsewhere)),
        _ => format!(
            "{} together require one of {}",
            list(&here),
            any_of(&elsewhere)
        ),
    }
}

/// A clause from a package directly, not from resolution
fn reason(store: &Store, id: usize) -> String {
    let inc = store.get(id);
    match &inc.cause {
        Cause::Root => "you asked for something to be installed".to_string(),
        Cause::Dependency { of, version } => {
            let want = inc
                .terms
                .iter()
                .find(|(p, t)| !(p == of && t.positive))
                .map(one_negated)
                .unwrap_or_else(|| "something".to_string());
            match of {
                Pkg::Root => format!("you asked for {want}"),
                _ => format!("{of} {version} depends on {want}"),
            }
        }
        Cause::NoVersions { pkg, unknown } => {
            if *unknown {
                format!("there is no package called {pkg} in any repository this machine knows")
            } else {
                let want = inc
                    .term_for(pkg)
                    .map(|t| t.set.to_string())
                    .unwrap_or_else(|| "that".to_string());
                format!("no version of {pkg} matches {want}")
            }
        }
        Cause::Conflict { of, version } => {
            let other = inc
                .terms
                .iter()
                .find(|(p, _)| p != of)
                .map(one)
                .unwrap_or_else(|| "something".to_string());
            format!("{of} {version} cannot be installed beside {other}")
        }
        Cause::Provides => statement(store, id),
        Cause::Derived { .. } => statement(store, id),
    }
}

fn one((pkg, term): &(Pkg, Term)) -> String {
    // The request has no version to print
    if *pkg == Pkg::Root || term.set.is_full() {
        format!("{pkg}")
    } else {
        format!("{pkg} {}", term.set)
    }
}

/// A negative term read as a requirement. In a clause `{P, not X in S}` it is
/// what `P` requires: X, at a version in S. A full set reads as "any version of
/// X"; `one` handles the conflict reading of a positive term
fn one_negated((pkg, term): &(Pkg, Term)) -> String {
    if term.positive {
        return one(&(*pkg, term.clone()));
    }
    if *pkg == Pkg::Root || term.set.is_full() {
        return format!("{pkg}");
    }
    format!("{pkg} {}", term.set)
}

fn list(items: &[&(Pkg, Term)]) -> String {
    join(items.iter().map(|i| one(i)).collect())
}

/// "one of a or b", since "one of a and b" asks for both
fn any_of(items: &[&(Pkg, Term)]) -> String {
    let parts: Vec<String> = items.iter().map(|i| one_negated(i)).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        _ => {
            let last = parts.last().unwrap().clone();
            let head = parts[..parts.len() - 1].join(", ");
            format!("{head} or {last}")
        }
    }
}

fn join(parts: Vec<String>) -> String {
    match parts.len() {
        0 => String::new(),
        1 => parts[0].clone(),
        _ => {
            let last = parts.last().unwrap().clone();
            let head = parts[..parts.len() - 1].join(", ");
            format!("{head} and {last}")
        }
    }
}

/// What the person actually asked for, for the closing sentence.
fn request_phrase(store: &Store, root: usize) -> String {
    let mut asked: Vec<String> = Vec::new();
    collect_requests(store, root, &mut asked, &mut Vec::new());
    if asked.is_empty() {
        return "because of what you asked for".to_string();
    }
    format!("because you asked for {}", join(asked))
}

fn collect_requests(store: &Store, id: usize, out: &mut Vec<String>, seen: &mut Vec<usize>) {
    if seen.contains(&id) {
        return;
    }
    seen.push(id);
    let inc: &Incompat = store.get(id);
    if let Cause::Dependency { of: Pkg::Root, .. } = inc.cause {
        if let Some(t) = inc.terms.iter().find(|(p, _)| *p != Pkg::Root) {
            let said = one_negated(t);
            if !out.contains(&said) {
                out.push(said);
            }
        }
    }
    if let Cause::Derived { left, right } = inc.cause {
        collect_requests(store, left, out, seen);
        collect_requests(store, right, out, seen);
    }
}

/// Something to try, worked out from the failure
fn hints(store: &Store) -> Vec<String> {
    let mut out = Vec::new();
    let mut missing: Vec<String> = Vec::new();
    for id in 0..store.len() {
        if let Cause::NoVersions { pkg, unknown: true } = &store.get(id).cause {
            let said = pkg.to_string();
            if !missing.contains(&said) {
                missing.push(said);
            }
        }
    }
    // Only for an unknown name; a sync would not help a range that did not fit
    if !missing.is_empty() {
        out.push(format!(
            "mikupkg sync              no repository here offers {}",
            join(missing)
        ));
    }
    out
}
