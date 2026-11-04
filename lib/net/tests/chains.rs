//! Whole chains, accepted and refused
//!
//! Certificates are in `testdata/`, whose note gives each one's source. Their
//! verdicts do not change with the date, except the good chain, which is
//! checked at a pinned instant inside its window.

use mikunet::verify::{verify_chain, Trust, VerifyError};

const GOOD_LEAF: &[u8] = include_bytes!("../testdata/good-leaf.der");
const GOOD_CA: &[u8] = include_bytes!("../testdata/good-ca.der");
const EXPIRED_LEAF: &[u8] = include_bytes!("../testdata/expired-leaf.der");
const EXPIRED_CA: &[u8] = include_bytes!("../testdata/expired-ca.der");
const SELF_SIGNED: &[u8] = include_bytes!("../testdata/self-signed.der");

/// Inside the good leaf's validity window, and fixed
const AT: u64 = 1_787_443_200; // 2026-08-23

fn store() -> Trust {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../etc/ssl/certs/ca-bundle.pem");
    Trust::from_pem(&std::fs::read(p).expect("the bundle"))
}

fn chain(parts: &[&[u8]]) -> Vec<Vec<u8>> {
    parts.iter().map(|p| p.to_vec()).collect()
}

#[test]
fn a_real_chain_is_accepted() {
    let t = store();
    let c = chain(&[GOOD_LEAF, GOOD_CA]);
    let r = verify_chain(&c, "www.google.com", AT, &t);
    assert!(r.is_ok(), "the positive control failed: {:?}", r);
}

#[test]
fn the_leaf_alone_is_not_enough() {
    // Without the intermediate, no root signed the leaf
    let t = store();
    let c = chain(&[GOOD_LEAF]);
    assert_eq!(verify_chain(&c, "www.google.com", AT, &t), Err(VerifyError::UnknownCa));
}

#[test]
fn the_wrong_host_is_refused_even_with_a_perfect_chain() {
    let t = store();
    let c = chain(&[GOOD_LEAF, GOOD_CA]);
    assert_eq!(
        verify_chain(&c, "evil.example", AT, &t),
        Err(VerifyError::NameMismatch)
    );
}

#[test]
fn an_expired_chain_is_refused() {
    let t = store();
    let c = chain(&[EXPIRED_LEAF, EXPIRED_CA]);
    assert_eq!(
        verify_chain(&c, "expired.badssl.com", AT, &t),
        Err(VerifyError::Expired)
    );
}

#[test]
fn a_certificate_that_signed_itself_anchors_nothing() {
    let t = store();
    let c = chain(&[SELF_SIGNED]);
    assert_eq!(
        verify_chain(&c, "self-signed.badssl.com", AT, &t),
        Err(VerifyError::UnknownCa)
    );
}

#[test]
fn a_corrupted_signature_breaks_the_chain() {
    let t = store();
    let mut bad = GOOD_LEAF.to_vec();
    // The signature is the certificate's last field, so the tail is inside it
    let last = bad.len() - 1;
    bad[last] ^= 0x01;
    let c = vec![bad, GOOD_CA.to_vec()];
    assert!(verify_chain(&c, "www.google.com", AT, &t).is_err());
}

#[test]
fn an_empty_chain_is_refused() {
    let t = store();
    assert_eq!(verify_chain(&[], "www.google.com", AT, &t), Err(VerifyError::Empty));
}

#[test]
fn a_chain_that_anchors_nowhere_is_refused_however_valid_it_looks() {
    // The good chain against a store with no roots: refused, as with no
    // /etc/ssl/certs
    let t = Trust::empty();
    let c = chain(&[GOOD_LEAF, GOOD_CA]);
    assert_eq!(verify_chain(&c, "www.google.com", AT, &t), Err(VerifyError::UnknownCa));
}

#[test]
fn a_valid_chain_before_its_time_is_refused() {
    let t = store();
    let c = chain(&[GOOD_LEAF, GOOD_CA]);
    // 2020, before this leaf was issued
    assert_eq!(
        verify_chain(&c, "www.google.com", 1_577_836_800, &t),
        Err(VerifyError::NotYetValid)
    );
}
