// Versions and their order, written down as assertions

use mikupkg::version::{PreId, Version, VersionError, MAX_TEXT};

fn v(s: &str) -> Version {
    Version::parse(s).unwrap_or_else(|e| panic!("{s:?} should parse: {e}"))
}

// Reading

#[test]
fn the_shapes_this_tree_actually_uses() {
    for s in ["1", "0.2.10", "1.0.0", "0.2.7-rc", "2:1.0", "1.4.2_3", "0.1.0-alpha.1"] {
        let parsed = v(s);
        assert_eq!(
            parsed.to_string(),
            s,
            "a version must print as what it was read from"
        );
    }
}

#[test]
fn the_parts_land_where_they_belong() {
    let parsed = v("3:1.2.3.4-rc.2_7");
    assert_eq!(parsed.epoch(), 3);
    assert_eq!(parsed.numbers(), &[1, 2, 3, 4]);
    assert_eq!(parsed.release(), 7);
    assert!(parsed.is_pre_release());
    assert_eq!(parsed.pre_release().len(), 2);
    assert_eq!(parsed.pre_release()[1], PreId::Num(2));
}

#[test]
fn what_is_refused_and_where() {
    let cases: &[(&str, VersionError)] = &[
        ("", VersionError::Empty),
        ("1.", VersionError::Trailing { at: 1 }),
        ("1..2", VersionError::BadNumber { at: 2 }),
        ("1.2.3.4.5", VersionError::TooManyNumbers { at: 8 }),
        ("1-", VersionError::Trailing { at: 1 }),
        ("1-rc.", VersionError::Trailing { at: 4 }),
        ("1_", VersionError::BadNumber { at: 2 }),
        ("v1.0", VersionError::BadNumber { at: 0 }),
        ("1.0 ", VersionError::BadByte { at: 3, byte: b' ' }),
        ("1.0/etc", VersionError::BadByte { at: 3, byte: b'/' }),
        ("99999999999", VersionError::BadNumber { at: 0 }),
        ("1-a.b.c.d.e", VersionError::TooManyPreIds { at: 10 }),
        ("1-abcdefghijklmnop", VersionError::PreIdTooLong { at: 2 }),
    ];
    for (text, want) in cases {
        let got = Version::parse(text);
        assert_eq!(
            got,
            Err(*want),
            "{text:?} should be refused as {want:?}, got {got:?}"
        );
    }

    let long = "1".repeat(MAX_TEXT + 1);
    assert_eq!(
        Version::parse(&long),
        Err(VersionError::TooLong { len: MAX_TEXT + 1 })
    );
}

/// Every refusal gives an offset
#[test]
fn every_refusal_names_a_position_or_a_length() {
    for text in ["", "1.", "1..2", "x", "1.0!", "1-rc."] {
        if let Err(e) = Version::parse(text) {
            let said = std::format!("{e}");
            assert!(
                said.contains("byte ") || said.contains("empty") || said.contains("at most"),
                "{text:?}: {said:?} does not say where"
            );
        }
    }
}

// Ordering

/// Each is strictly less than the next. Every pair is checked, not only
/// neighbours
#[test]
fn the_order_in_one_chain() {
    let chain = [
        "0.1.0-alpha",
        "0.1.0-alpha.1",
        "0.1.0-alpha.2",
        "0.1.0-alpha.10",
        "0.1.0-beta",
        "0.1.0-rc",
        "0.1.0-rc.1",
        "0.1.0",
        "0.1.0_1",
        "0.1.0_2",
        "0.1.1",
        "0.2.7-rc",
        "0.2.7",
        "0.2.10",
        "1.0.0",
        "1:0.0.1",
        "2:0.0.1",
    ];
    for (i, a) in chain.iter().enumerate() {
        for b in &chain[i + 1..] {
            assert!(
                v(a) < v(b),
                "{a} should sort before {b}"
            );
            assert!(v(b) > v(a), "and {b} after {a}");
        }
    }
}

/// 0.2.10 is newer than 0.2.7; a string comparison says the opposite
#[test]
fn ten_is_after_seven() {
    assert!(v("0.2.10") > v("0.2.7"));
    assert!(v("0.2.10") > v("0.2.9"));
    assert!(v("1.10.0") > v("1.9.0"));
}

/// A missing release number is zero, so these are one version
#[test]
fn a_missing_number_is_zero() {
    assert_eq!(v("1.2"), v("1.2.0"));
    assert_eq!(v("1.2"), v("1.2.0.0"));
    assert_eq!(v("1"), v("1.0.0.0"));
    assert!(v("1.2") < v("1.2.1"));
}

/// Equal versions hash equally, or a set could hold one twice and the resolver
/// never ends
#[test]
fn versions_that_compare_equal_hash_equal() {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    fn h(x: &Version) -> u64 {
        let mut s = DefaultHasher::new();
        x.hash(&mut s);
        s.finish()
    }

    assert_eq!(h(&v("1.2")), h(&v("1.2.0")));
    assert_eq!(h(&v("1.2")), h(&v("1.2.0.0")));
    assert_ne!(h(&v("1.2")), h(&v("1.2.1")));
    assert_ne!(h(&v("1.2")), h(&v("1.2-rc")));
}

/// An epoch outranks everything below it
#[test]
fn an_epoch_overrules_the_rest() {
    assert!(v("1:0.0.1") > v("99.99.99.99"));
    assert!(v("1:0.0.1-alpha") > v("0:99.0.0"));
}

/// A numeric pre-release identifier ranks below an alphanumeric one, a prefix
/// below its extension: semver 11.4
#[test]
fn the_pre_release_rules_are_semvers() {
    assert!(v("1.0-1") < v("1.0-alpha"));
    assert!(v("1.0-rc") < v("1.0-rc.1"));
    assert!(v("1.0-alpha.9") < v("1.0-alpha.10"));
    assert!(v("1.0-alpha") < v("1.0-beta"));
    // Anything with one precedes the release
    assert!(v("1.0-zzz") < v("1.0"));
}

/// The packaging revision decides only which build of a version is newer
#[test]
fn the_release_number_is_the_last_word() {
    assert!(v("1.0") < v("1.0_1"));
    assert!(v("1.0_1") < v("1.0_2"));
    assert!(v("1.0_9") < v("1.0.1"));
}

#[test]
fn the_order_is_total() {
    let all = ["1", "1.0-rc", "2:1", "1.0_3", "0.9.9", "1.0.0.1"];
    for a in all {
        for b in all {
            let (x, y) = (v(a), v(b));
            assert_eq!(
                x < y,
                y > x,
                "{a} vs {b}: the comparison must be antisymmetric"
            );
            assert_eq!(x == y, x.cmp(&y) == std::cmp::Ordering::Equal);
        }
    }
}
