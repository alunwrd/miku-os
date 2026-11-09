// Working out what to install
//
// The search: finding an answer, the right one, and backing out of a wrong
// guess. The explanation: what a person reads when there is no answer.

use mikupkg::meta::dep::{Dep, Provide};
use mikupkg::name::Name;
use mikupkg::resolve::universe::Universe;
use mikupkg::resolve::{report, solve};
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}
fn deps(list: &[&str]) -> Vec<Dep> {
    list.iter().map(|d| Dep::parse(d).unwrap()).collect()
}

/// A universe written out, indistinguishable from a repository index to the
/// solver
#[derive(Default)]
struct Table {
    entries: Vec<Entry>,
    installed: Vec<(Name, Version)>,
}

struct Entry {
    name: Name,
    version: Version,
    depends: Vec<Dep>,
    conflicts: Vec<Dep>,
    provides: Vec<Provide>,
}

impl Table {
    fn add(&mut self, name: &str, version: &str, depends: &[&str]) -> &mut Table {
        self.entries.push(Entry {
            name: n(name),
            version: v(version),
            depends: deps(depends),
            conflicts: Vec::new(),
            provides: Vec::new(),
        });
        self
    }

    fn conflicting(&mut self, with: &[&str]) -> &mut Table {
        self.entries.last_mut().unwrap().conflicts = deps(with);
        self
    }

    fn providing(&mut self, what: &[&str]) -> &mut Table {
        self.entries.last_mut().unwrap().provides =
            what.iter().map(|p| Provide::parse(p).unwrap()).collect();
        self
    }

    fn already_has(&mut self, name: &str, version: &str) -> &mut Table {
        self.installed.push((n(name), v(version)));
        self
    }

    fn find(&self, name: &Name, version: &Version) -> Option<&Entry> {
        self.entries
            .iter()
            .find(|e| e.name == *name && e.version == *version)
    }
}

impl Universe for Table {
    fn names(&self) -> Vec<Name> {
        let mut out: Vec<Name> = self.entries.iter().map(|e| e.name).collect();
        out.sort();
        out.dedup();
        out
    }
    fn versions(&self, pkg: &Name) -> Vec<Version> {
        self.entries
            .iter()
            .filter(|e| e.name == *pkg)
            .map(|e| e.version)
            .collect()
    }
    fn depends(&self, pkg: &Name, ver: &Version) -> Vec<Dep> {
        self.find(pkg, ver).map(|e| e.depends.clone()).unwrap_or_default()
    }
    fn conflicts(&self, pkg: &Name, ver: &Version) -> Vec<Dep> {
        self.find(pkg, ver).map(|e| e.conflicts.clone()).unwrap_or_default()
    }
    fn provides(&self, pkg: &Name, ver: &Version) -> Vec<Provide> {
        self.find(pkg, ver).map(|e| e.provides.clone()).unwrap_or_default()
    }
    fn installed(&self, pkg: &Name) -> Option<Version> {
        self.installed.iter().find(|(p, _)| p == pkg).map(|(_, v)| *v)
    }
}

fn solved(t: &Table, request: &[&str]) -> Vec<(String, String)> {
    match solve(t, &deps(request)) {
        Ok(s) => s
            .selected
            .iter()
            .map(|(n, v)| (n.to_string(), v.to_string()))
            .collect(),
        Err(f) => panic!("expected a solution:\n{}", report(&f)),
    }
}

fn failed(t: &Table, request: &[&str]) -> String {
    match solve(t, &deps(request)) {
        Ok(s) => panic!("expected no solution, got {:?}", s.selected),
        Err(f) => report(&f),
    }
}

// Finding an answer

#[test]
fn one_package_with_nothing_to_say() {
    let mut t = Table::default();
    t.add("hello", "1.0", &[]);
    assert_eq!(solved(&t, &["hello"]), vec![("hello".into(), "1.0".into())]);
}

#[test]
fn a_dependency_is_brought_in() {
    let mut t = Table::default();
    t.add("mayterm", "0.2.10", &["libmay>=2.0"]);
    t.add("libmay", "2.1", &["miku-base"]);
    t.add("miku-base", "1.0", &[]);

    assert_eq!(
        solved(&t, &["mayterm"]),
        vec![
            ("libmay".into(), "2.1".into()),
            ("mayterm".into(), "0.2.10".into()),
            ("miku-base".into(), "1.0".into()),
        ]
    );
}

#[test]
fn the_newest_version_that_fits_is_chosen() {
    let mut t = Table::default();
    t.add("libmay", "1.0", &[]);
    t.add("libmay", "2.0", &[]);
    t.add("libmay", "3.0", &[]);

    assert_eq!(solved(&t, &["libmay"]), vec![("libmay".into(), "3.0".into())]);
    assert_eq!(
        solved(&t, &["libmay<3.0"]),
        vec![("libmay".into(), "2.0".into())]
    );
    assert_eq!(
        solved(&t, &["libmay>=1.0,<2.0"]),
        vec![("libmay".into(), "1.0".into())]
    );
}

/// What is installed is a preference, not a constraint, so an ordinary install
/// leaves the system alone
#[test]
fn what_is_installed_is_kept_when_it_will_do() {
    let mut t = Table::default();
    t.add("libmay", "1.0", &[]);
    t.add("libmay", "2.0", &[]);
    t.already_has("libmay", "1.0");

    assert_eq!(
        solved(&t, &["libmay"]),
        vec![("libmay".into(), "1.0".into())],
        "the installed version satisfies the request, so it stays"
    );
    assert_eq!(
        solved(&t, &["libmay>=2.0"]),
        vec![("libmay".into(), "2.0".into())],
        "...and is upgraded when it does not"
    );
}

/// The obvious first guess is wrong, and the solver must find which decision to
/// take back
#[test]
fn a_wrong_guess_is_backed_out_of() {
    let mut t = Table::default();
    t.add("menu", "1.0.0", &["dropdown>=1.0.0,<2.0.0"]);
    t.add("menu", "1.1.0", &["dropdown>=2.0.0"]);
    t.add("dropdown", "1.8.0", &["intl<4.0.0"]);
    t.add("dropdown", "2.0.0", &["icons>=2.0.0"]);
    t.add("icons", "1.0.0", &[]);
    t.add("icons", "2.0.0", &[]);
    t.add("intl", "3.0.0", &[]);
    t.add("intl", "5.0.0", &[]);

    // menu 1.1.0 leads to icons 2.0.0, which the request forbids. The answer is
    // menu 1.0.0, dropdown 1.8.0, intl 3.0.0
    let got = solved(&t, &["menu>=1.0.0", "icons<2.0.0"]);
    assert_eq!(
        got,
        vec![
            ("dropdown".into(), "1.8.0".into()),
            ("icons".into(), "1.0.0".into()),
            ("intl".into(), "3.0.0".into()),
            ("menu".into(), "1.0.0".into()),
        ]
    );
}

#[test]
fn a_declared_conflict_is_honoured_in_both_directions() {
    let mut t = Table::default();
    t.add("mayterm", "1.0", &[]).conflicting(&["mayconsole"]);
    t.add("mayconsole", "1.0", &[]);

    assert_eq!(solved(&t, &["mayterm"]), vec![("mayterm".into(), "1.0".into())]);

    // Declared on mayterm only and still applies with the request reversed, or
    // index order would decide
    let said = failed(&t, &["mayterm", "mayconsole"]);
    assert!(said.contains("mayterm"), "{said}");
    assert!(said.contains("mayconsole"), "{said}");
}

/// A virtual name is an ordinary package to the solver
#[test]
fn a_virtual_name_is_satisfied_by_whoever_provides_it() {
    let mut t = Table::default();
    t.add("vi", "1.4", &[]).providing(&["editor=1.0"]);
    t.add("mayedit", "2.0", &[]).providing(&["editor=2.0"]);
    t.add("notes", "1.0", &["editor>=2.0"]);

    let got = solved(&t, &["notes"]);
    assert_eq!(
        got,
        vec![("mayedit".into(), "2.0".into()), ("notes".into(), "1.0".into())],
        "only mayedit provides editor at 2.0, and the virtual name is not installed"
    );

    // ...and a request for the virtual name itself picks a provider
    let got = solved(&t, &["editor"]);
    assert_eq!(got.len(), 1, "one provider, and not the virtual name: {got:?}");
    assert_eq!(got[0].0, "mayedit");
}

// When there is no answer

/// The wording is asserted, so a change to it is deliberate
#[test]
fn a_package_that_is_not_there_is_named() {
    let mut t = Table::default();
    t.add("mayterm", "0.2.10", &[]);

    assert_eq!(
        failed(&t, &["nosuchthing"]),
        "\
mikupkg: cannot work out what to install.

  1. there is no package called nosuchthing in any repository this machine knows
  2. you asked for nosuchthing
  3. because (1) and (2), there is no solution

So, because you asked for nosuchthing, version solving failed.

Try:  mikupkg sync              no repository here offers nosuchthing
"
    );
}

/// A package present at a version the request rules out is a different failure
/// with different advice
#[test]
fn a_version_that_is_not_there_is_not_confused_with_a_package_that_is_not() {
    let mut t = Table::default();
    t.add("libmay", "1.4", &[]);

    let said = failed(&t, &["libmay>=2.0"]);
    assert!(said.contains("no version of libmay matches >=2.0"), "{said}");
    assert!(!said.contains("no package called"), "{said}");
    assert!(!said.contains("mikupkg sync"), "a sync cannot help here: {said}");
}

#[test]
fn a_dependency_that_cannot_be_met_is_explained_as_a_proof() {
    let mut t = Table::default();
    t.add("mayterm", "0.3", &["libmay>=2.0"]);
    t.add("libmay", "1.4", &[]);

    assert_eq!(
        failed(&t, &["mayterm>=0.3"]),
        "\
mikupkg: cannot work out what to install.

  1. no version of mayterm matches >0.3
  2. mayterm 0.3 depends on libmay >=2.0
  3. because (1) and (2), mayterm >=0.3 requires libmay >=2.0
  4. no version of libmay matches >=2.0
  5. because (3) and (4), mayterm >=0.3 cannot be installed
  6. you asked for mayterm >=0.3
  7. because (5) and (6), there is no solution

So, because you asked for mayterm >=0.3, version solving failed.
"
    );
}

#[test]
fn the_classic_three_step_conflict_reads_as_one() {
    let mut t = Table::default();
    t.add("menu", "1.0.0", &["dropdown>=1.0.0,<2.0.0"]);
    t.add("menu", "1.1.0", &["dropdown>=2.0.0"]);
    t.add("dropdown", "1.8.0", &["intl<4.0.0"]);
    t.add("dropdown", "2.0.0", &["icons>=2.0.0"]);
    t.add("icons", "1.0.0", &[]);
    t.add("icons", "2.0.0", &[]);
    t.add("intl", "3.0.0", &[]);
    t.add("intl", "5.0.0", &[]);

    // Something impossible: menu, icons <2, and intl >=5
    let said = failed(&t, &["menu>=1.0.0", "icons<2.0.0", "intl>=5.0.0"]);

    // Checks the shape, not all nineteen lines of proof
    assert!(
        said.contains("dropdown >=2.0.0 requires icons >=2.0.0"),
        "the step everybody quotes about this example: {said}"
    );
    assert!(
        said.contains("requires one of intl <4.0.0 or icons >=2.0.0"),
        "a disjunction reads as one *or* the other: {said}"
    );
    assert!(
        !said.contains("what you asked for 0"),
        "the request has no version to print: {said}"
    );
    for must in ["menu", "dropdown", "icons", "intl"] {
        assert!(said.contains(must), "{must} is part of the proof: {said}");
    }
    assert!(said.starts_with("mikupkg: cannot work out what to install."), "{said}");
    // Every numbered line is numbered, in order, with no gaps
    let numbers: Vec<usize> = said
        .lines()
        .filter_map(|l| l.trim().split('.').next().and_then(|p| p.trim().parse().ok()))
        .collect();
    assert_eq!(
        numbers,
        (1..=numbers.len()).collect::<Vec<_>>(),
        "the steps are numbered in order: {said}"
    );
}

/// A conclusion used twice is cited by number, keeping the text from growing
/// exponentially with the graph
#[test]
fn a_step_reached_twice_is_numbered_once() {
    let mut t = Table::default();
    t.add("a", "1.0", &["shared>=2.0"]);
    t.add("b", "1.0", &["shared>=2.0"]);
    t.add("shared", "1.0", &[]);

    let said = failed(&t, &["a", "b"]);
    let mentions = said.matches("no version of shared matches").count();
    assert_eq!(mentions, 1, "the same fact should be stated once: {said}");
}

// The solver asks only the universe

/// Long chains and wide fan-out need no special case
#[test]
fn a_deep_chain_resolves() {
    let mut t = Table::default();
    for i in 0..40 {
        let me = std::format!("p{i}");
        let next = std::format!("p{}", i + 1);
        if i < 39 {
            t.add(&me, "1.0", &[&next]);
        } else {
            t.add(&me, "1.0", &[]);
        }
    }
    let got = solved(&t, &["p0"]);
    assert_eq!(got.len(), 40);
}
