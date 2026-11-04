//! The trust store, against itself. Every root is self-signed, so each is a
//! signature verifiable alone, covering RSA-2048, RSA-4096, and ECDSA on P-256,
//! P-384, and P-521

use mikunet::verify::Trust;
use mikunet::x509::Cert;

fn bundle() -> Vec<u8> {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../etc/ssl/certs/ca-bundle.pem");
    std::fs::read(p).unwrap_or_else(|e| panic!("cannot read {}: {}", p, e))
}

fn store() -> Trust {
    Trust::from_pem(&bundle())
}

#[test]
fn the_bundle_parses_and_is_not_thin() {
    let t = store();
    assert!(t.len() >= 100, "only {} roots came out of the bundle", t.len());
    // The count is not pinned, since the bundle updates from upstream
    assert_eq!(t.roots().count(), t.len());
}

#[test]
fn every_root_is_a_certificate() {
    let t = store();
    let parsed = t.roots().filter(|d| Cert::parse(d).is_some()).count();
    assert_eq!(parsed, t.len(), "{} of {} roots did not parse", t.len() - parsed, t.len());
}

#[test]
fn every_root_verifies_its_own_signature() {
    let t = store();
    let mut checked = 0usize;
    let mut failed: Vec<String> = Vec::new();

    for der in t.roots() {
        let c = Cert::parse(der).expect("checked by the test above");
        // A root is self-signed by definition
        assert_eq!(c.subject, c.issuer, "a root that is not self-issued");
        checked += 1;
        if !mikunet::verify::verify_signature(&c.key, c.sig_alg, c.tbs, c.signature) {
            failed.push(format!("serial {:02x?}", &c.serial[..c.serial.len().min(8)]));
        }
    }

    assert!(checked >= 100, "only {} roots checked", checked);
    assert!(failed.is_empty(), "{} roots failed to verify: {:?}", failed.len(), failed);
}

#[test]
fn a_flipped_bit_anywhere_is_caught() {
    let t = store();
    // All key types, which take different code paths
    let mut tried_rsa = false;
    let mut tried_ec = false;

    for der in t.roots() {
        let c = Cert::parse(der).expect("parses");
        let is_ec = matches!(c.key, mikunet::x509::PublicKey::Ec { .. });
        if is_ec && tried_ec { continue; }
        if !is_ec && tried_rsa { continue; }
        if is_ec { tried_ec = true; } else { tried_rsa = true; }

        assert!(
            mikunet::verify::verify_signature(&c.key, c.sig_alg, c.tbs, c.signature),
            "the untouched certificate must verify first"
        );

        let mut bad_sig = c.signature.to_vec();
        let last = bad_sig.len() - 1;
        bad_sig[last] ^= 0x01;
        assert!(
            !mikunet::verify::verify_signature(&c.key, c.sig_alg, c.tbs, &bad_sig),
            "one bit flipped in the signature was accepted"
        );

        let mut bad_tbs = c.tbs.to_vec();
        let mid = bad_tbs.len() / 2;
        bad_tbs[mid] ^= 0x01;
        assert!(
            !mikunet::verify::verify_signature(&c.key, c.sig_alg, &bad_tbs, c.signature),
            "one bit flipped in the signed body was accepted"
        );

        if tried_rsa && tried_ec { return; }
    }

    assert!(tried_rsa && tried_ec, "the bundle held only one kind of key");
}

#[test]
fn an_empty_store_anchors_nothing() {
    // A missing bundle must refuse chains. `Trust::empty()` stands in for the
    // missing file
    let t = Trust::empty();
    assert_eq!(t.len(), 0);
    assert!(t.is_empty());
    assert!(!t.insecure, "an empty store must not also be a permissive one");
}

#[test]
fn junk_around_the_blocks_is_skipped_not_fatal() {
    // Bundles carry comments, subject lines, and blank space between
    // certificates
    let real = bundle();
    let mut messy = Vec::new();
    messy.extend_from_slice(b"# a comment\nSubject: something\n\n");
    messy.extend_from_slice(&real);
    messy.extend_from_slice(b"\n\ntrailing junk without a block\n");

    let t = Trust::from_pem(&messy);
    assert_eq!(t.len(), store().len());
}
