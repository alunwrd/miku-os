//! Which names a certificate covers
//!
//! A wildcard must not match a dot, a CN must not be read when a SAN is
//! present, and comparison is case-insensitive

use mikunet::x509::Cert;

/// CN=test.miku.local, SAN dNSName test.miku.local and *.wild.local
const NAMES: &[u8] = include_bytes!("../testdata/names.der");
/// SAN iPAddress 10.0.2.2 and dNSName bench.local
const IPSAN: &[u8] = include_bytes!("../testdata/ipsan.der");

fn cert(der: &[u8]) -> Cert<'_> {
    Cert::parse(der).expect("test certificate parses")
}

#[test]
fn an_exact_name_matches() {
    let c = cert(NAMES);
    assert!(c.matches_host("test.miku.local"));
}

#[test]
fn matching_ignores_case() {
    // DNS names are case-insensitive
    let c = cert(NAMES);
    assert!(c.matches_host("TEST.MIKU.LOCAL"));
    assert!(c.matches_host("Test.Miku.Local"));
}

#[test]
fn a_wildcard_covers_one_label_and_no_more() {
    let c = cert(NAMES);
    assert!(c.matches_host("anything.wild.local"), "one label must match");
    assert!(
        !c.matches_host("two.labels.wild.local"),
        "a wildcard must not cross a dot - this is the whole point of the rule"
    );
    assert!(!c.matches_host("wild.local"), "the label is not optional");
}

#[test]
fn a_name_the_certificate_does_not_carry_is_refused() {
    let c = cert(NAMES);
    assert!(!c.matches_host("other.miku.local"));
    assert!(!c.matches_host("test.miku.local.evil.com"));
    assert!(!c.matches_host(""));
}

#[test]
fn a_suffix_is_not_a_match() {
    // "miku.local" ends the same way and is a different host
    let c = cert(NAMES);
    assert!(!c.matches_host("miku.local"));
    assert!(!c.matches_host("nottest.miku.local"));
}

#[test]
fn an_address_certificate_matches_the_address_it_names() {
    let c = cert(IPSAN);
    assert!(c.matches_host("10.0.2.2"));
    assert!(c.matches_host("bench.local"));
}

#[test]
fn an_address_certificate_matches_nothing_else() {
    let c = cert(IPSAN);
    assert!(!c.matches_host("10.0.2.3"));
    assert!(!c.matches_host("10.0.2.22"));
    // A literal not in the SAN
    assert!(!c.matches_host("010.0.2.2"));
}
