// The version-set algebra, tested by its laws. If union, intersection, and
// complement obey them, every expression built from them is right

use mikupkg::range::{Bound, VersionSet};
use mikupkg::version::Version;

fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}
fn ge(s: &str) -> VersionSet {
    VersionSet::at_least(v(s))
}
fn lt(s: &str) -> VersionSet {
    VersionSet::less_than(v(s))
}
fn exact(s: &str) -> VersionSet {
    VersionSet::exact(v(s))
}
fn between(a: &str, b: &str) -> VersionSet {
    VersionSet::interval(Bound::Included(v(a)), Bound::Excluded(v(b)))
}

/// Sets for the laws: empty, full, both half-lines, a closed interval, a point,
/// and a union with a hole
fn corpus() -> Vec<VersionSet> {
    vec![
        VersionSet::empty(),
        VersionSet::full(),
        ge("1.0"),
        lt("2.0"),
        between("1.0", "2.0"),
        exact("1.5"),
        lt("1.0").union(&ge("2.0")),
        VersionSet::greater_than(v("1.0")),
        VersionSet::at_most(v("1.0")),
        between("0.1", "0.2").union(&between("3.0", "4.0")),
    ]
}

// Membership: the ground truth for every law

#[test]
fn membership_is_what_the_bounds_say() {
    let s = between("1.0", "2.0");
    assert!(s.contains(&v("1.0")), "an included low bound is in");
    assert!(s.contains(&v("1.9.9")));
    assert!(!s.contains(&v("2.0")), "an excluded high bound is out");
    assert!(!s.contains(&v("0.9")));

    // A pre-release is below its release, so >=1.0 does not contain 1.0-rc
    assert!(!ge("1.0").contains(&v("1.0-rc")));
    assert!(ge("1.0").contains(&v("1.0")));
    assert!(lt("1.0").contains(&v("1.0-rc")));
}

// The laws

#[test]
fn complement_is_its_own_inverse() {
    for s in corpus() {
        assert_eq!(s.complement().complement(), s, "¬¬{s} should be {s}");
    }
}

#[test]
fn a_set_and_its_complement_partition_everything() {
    for s in corpus() {
        assert!(
            s.intersection(&s.complement()).is_empty(),
            "{s} overlaps its own complement"
        );
        assert!(
            s.union(&s.complement()).is_full(),
            "{s} and its complement do not cover everything"
        );
    }
}

#[test]
fn de_morgan_holds() {
    for a in corpus() {
        for b in corpus() {
            assert_eq!(
                a.union(&b).complement(),
                a.complement().intersection(&b.complement()),
                "¬({a} ∪ {b})"
            );
            assert_eq!(
                a.intersection(&b).complement(),
                a.complement().union(&b.complement()),
                "¬({a} ∩ {b})"
            );
        }
    }
}

#[test]
fn union_and_intersection_commute_and_absorb() {
    for a in corpus() {
        for b in corpus() {
            assert_eq!(a.union(&b), b.union(&a), "{a} ∪ {b}");
            assert_eq!(a.intersection(&b), b.intersection(&a), "{a} ∩ {b}");
            assert_eq!(a.union(&a.intersection(&b)), a, "absorption on {a}");
            assert_eq!(a.intersection(&a.union(&b)), a, "absorption on {a}");
        }
    }
}

#[test]
fn the_operations_agree_with_membership() {
    let probes: Vec<Version> = ["0.1", "0.15", "1.0-rc", "1.0", "1.5", "1.9", "2.0", "3.5", "9.9"]
        .iter()
        .map(|s| v(s))
        .collect();

    for a in corpus() {
        for b in corpus() {
            let u = a.union(&b);
            let i = a.intersection(&b);
            let c = a.complement();
            for p in &probes {
                assert_eq!(u.contains(p), a.contains(p) || b.contains(p), "{p} in {a} ∪ {b}");
                assert_eq!(i.contains(p), a.contains(p) && b.contains(p), "{p} in {a} ∩ {b}");
                assert_eq!(c.contains(p), !a.contains(p), "{p} in ¬{a}");
            }
        }
    }
}

#[test]
fn subset_means_what_it_says() {
    assert!(between("1.0", "2.0").is_subset_of(&ge("1.0")));
    assert!(exact("1.5").is_subset_of(&between("1.0", "2.0")));
    assert!(!ge("1.0").is_subset_of(&between("1.0", "2.0")));
    for s in corpus() {
        assert!(VersionSet::empty().is_subset_of(&s));
        assert!(s.is_subset_of(&VersionSet::full()));
        assert!(s.is_subset_of(&s));
    }
}

// Canonical form

/// Two spellings of one set are one value; the resolver relies on equality to
/// detect derived facts
#[test]
fn one_set_has_one_spelling() {
    assert_eq!(
        between("1.0", "2.0").union(&between("2.0", "3.0")),
        between("1.0", "3.0"),
        "adjacent intervals must merge"
    );
    assert_eq!(
        between("1.0", "2.0").union(&between("1.5", "3.0")),
        between("1.0", "3.0"),
        "overlapping intervals must merge"
    );
    assert_eq!(
        ge("1.0").union(&ge("1.0")),
        ge("1.0"),
        "a union with itself changes nothing"
    );
    assert_eq!(
        VersionSet::interval(Bound::Included(v("2.0")), Bound::Excluded(v("1.0"))),
        VersionSet::empty(),
        "an interval that ends before it begins is empty, not backwards"
    );
}

/// A real hole survives: `<1.0` and `>1.0` leave 1.0 out
#[test]
fn a_hole_of_exactly_one_version_is_not_closed_over() {
    let holed = lt("1.0").union(&VersionSet::greater_than(v("1.0")));
    assert!(!holed.is_full(), "1.0 is in neither half");
    assert!(!holed.contains(&v("1.0")));
    assert!(holed.contains(&v("0.9")));
    assert!(holed.contains(&v("1.0.1")));
    assert_eq!(holed.complement(), exact("1.0"));
}

// How it reads

/// The explanation prints these, so printing is part of the contract
#[test]
fn a_set_prints_the_way_the_explanation_needs_it() {
    assert_eq!(VersionSet::empty().to_string(), "no version");
    assert_eq!(VersionSet::full().to_string(), "any version");
    assert_eq!(ge("1.0").to_string(), ">=1.0");
    assert_eq!(lt("2.0").to_string(), "<2.0");
    assert_eq!(VersionSet::greater_than(v("1.0")).to_string(), ">1.0");
    assert_eq!(VersionSet::at_most(v("1.0")).to_string(), "<=1.0");
    assert_eq!(exact("1.5").to_string(), "1.5");
    assert_eq!(between("1.0", "2.0").to_string(), ">=1.0 <2.0");
    assert_eq!(
        lt("1.0").union(&ge("2.0")).to_string(),
        "<1.0 or >=2.0"
    );
}
