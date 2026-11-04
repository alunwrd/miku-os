//! Signature schemes, which must not be interchangeable. PKCS#1 v1.5 and PSS
//! have different structure, and passing one as the other weakens the check.
//! Signatures are real ones from trust-store certificates

use mikunet::rsa::{rsa_pkcs1v15_verify, rsa_pss_verify, RsaPublicKey};
use mikunet::verify::Trust;
use mikunet::x509::{Cert, Hash, PublicKey, SigAlg};

fn store() -> Trust {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../etc/ssl/certs/ca-bundle.pem");
    Trust::from_pem(&std::fs::read(p).expect("the bundle"))
}

/// The first root whose signature is RSA PKCS#1 v1.5, with the hash it used
fn an_rsa_pkcs1_root(store: &Trust) -> (Vec<u8>, Hash) {
    for der in store.roots() {
        let c = Cert::parse(der).expect("parses");
        if let (PublicKey::Rsa { .. }, SigAlg::RsaPkcs1(h)) = (&c.key, c.sig_alg) {
            return (der.to_vec(), h);
        }
    }
    panic!("the bundle held no RSA PKCS#1 root");
}

#[test]
fn pkcs1_and_pss_do_not_accept_each_other() {
    let s = store();
    let (der, hash) = an_rsa_pkcs1_root(&s);
    let c = Cert::parse(&der).expect("parses");
    let key = match &c.key {
        PublicKey::Rsa { n, e } => RsaPublicKey::from_parts(n, e).expect("a usable key"),
        _ => unreachable!(),
    };

    assert!(
        rsa_pkcs1v15_verify(&key, hash, c.tbs, c.signature),
        "the signature must verify under the scheme it was made with"
    );
    assert!(
        !rsa_pss_verify(&key, hash, c.tbs, c.signature),
        "a PKCS#1 v1.5 signature was accepted as PSS"
    );
}

#[test]
fn the_wrong_hash_is_not_accepted() {
    // PKCS#1 names the digest in DigestInfo, so another digest must fail
    let s = store();
    let (der, hash) = an_rsa_pkcs1_root(&s);
    let c = Cert::parse(&der).expect("parses");
    let key = match &c.key {
        PublicKey::Rsa { n, e } => RsaPublicKey::from_parts(n, e).expect("a usable key"),
        _ => unreachable!(),
    };

    let other = if hash == Hash::Sha256 { Hash::Sha384 } else { Hash::Sha256 };
    assert!(!rsa_pkcs1v15_verify(&key, other, c.tbs, c.signature));
}

#[test]
fn a_truncated_signature_is_refused() {
    let s = store();
    let (der, hash) = an_rsa_pkcs1_root(&s);
    let c = Cert::parse(&der).expect("parses");
    let key = match &c.key {
        PublicKey::Rsa { n, e } => RsaPublicKey::from_parts(n, e).expect("a usable key"),
        _ => unreachable!(),
    };

    // One byte short of the modulus
    let short = &c.signature[..c.signature.len() - 1];
    assert!(!rsa_pkcs1v15_verify(&key, hash, c.tbs, short));
    assert!(!rsa_pkcs1v15_verify(&key, hash, c.tbs, &[]));
}

#[test]
fn a_key_too_small_or_too_odd_is_refused() {
    // Below 512 bits, or an even exponent
    assert!(RsaPublicKey::from_parts(&[0xff; 32], &[0x01, 0x00, 0x01]).is_none(),
        "a 256-bit modulus must be refused");
    assert!(RsaPublicKey::from_parts(&[0xff; 256], &[0x02]).is_none(),
        "an even exponent must be refused");
    assert!(RsaPublicKey::from_parts(&[0xff; 256], &[0x01]).is_none(),
        "e = 1 is the identity and must be refused");
}

#[test]
fn ecdh_agrees_in_both_directions() {
    // Two keypairs, one shared secret. `trust.rs` also verifies real ECDSA
    // signatures, which catches self-consistent errors
    let (priv_a, pub_a) = mikunet::ec::ecdh_keypair(&[0x11; 32]);
    let (priv_b, pub_b) = mikunet::ec::ecdh_keypair(&[0x22; 32]);

    let ab = mikunet::ec::ecdh_shared(&priv_a, &pub_b).expect("a valid peer point");
    let ba = mikunet::ec::ecdh_shared(&priv_b, &pub_a).expect("a valid peer point");
    assert_eq!(ab, ba, "the two sides derived different secrets");
    assert_ne!(ab, [0u8; 32], "the shared secret is the point at infinity");
}

#[test]
fn a_point_that_is_not_on_the_curve_is_refused() {
    // Invalid-curve attack: a point on a different, weaker curve must be
    // refused
    let (priv_a, _) = mikunet::ec::ecdh_keypair(&[0x33; 32]);
    let (_, mut pub_b) = mikunet::ec::ecdh_keypair(&[0x44; 32]);

    assert!(mikunet::ec::ecdh_shared(&priv_a, &pub_b).is_some(), "the real point works");

    // Move it off the curve by one bit
    pub_b[64] ^= 0x01;
    assert!(
        mikunet::ec::ecdh_shared(&priv_a, &pub_b).is_none(),
        "a point off the curve was accepted"
    );

    // A point not in uncompressed form
    let mut wrong_form = pub_b;
    wrong_form[0] = 0x02;
    assert!(mikunet::ec::ecdh_shared(&priv_a, &wrong_form).is_none());
}

// Signing
//
// For `builder`, which signs packages on the host; MikuOS only verifies. The
// nonce is RFC 6979 deterministic, since `plat::random` in a non-`userspace`
// build is a deterministic xorshift. The first test compares to RFC 6979
// Appendix A.2.5 vectors, not a round trip

use mikunet::ec::{ecdsa_public_key, ecdsa_sign, ecdsa_verify, Curve, MAX_SIG_LEN};

fn unhex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

/// Extracts `r` and `s` from a DER `SEQUENCE { INTEGER, INTEGER }`, for
/// comparison to the published integers
fn der_pair(sig: &[u8]) -> (Vec<u8>, Vec<u8>) {
    assert_eq!(sig[0], 0x30, "a signature is a SEQUENCE");
    // Short form below 128, long form above
    let mut at = if sig[1] < 0x80 {
        assert_eq!(sig[1] as usize, sig.len() - 2, "whose length covers the rest");
        2usize
    } else {
        assert_eq!(sig[1], 0x81);
        assert_eq!(sig[2] as usize, sig.len() - 3);
        3usize
    };
    let mut out = Vec::new();
    for _ in 0..2 {
        assert_eq!(sig[at], 0x02, "holding INTEGERs");
        let len = sig[at + 1] as usize;
        let mut v = sig[at + 2..at + 2 + len].to_vec();
        // DER prepends a zero to a high bit so the integer stays positive
        if v.first() == Some(&0) && v.len() > 1 {
            v.remove(0);
        }
        while v.first() == Some(&0) && v.len() > 1 {
            v.remove(0);
        }
        out.push(v);
        at += 2 + len;
    }
    assert_eq!(at, sig.len(), "and nothing after them");
    (out.remove(0), out.remove(0))
}

/// RFC 6979 A.2.5: P-256, SHA-256, key C9AF...6721
const RFC6979_KEY: &str = "C9AFA9D845BA75166B5C215767B1D6934E50C3DB36E89B127B8A622B120F6721";
const RFC6979_UX: &str = "60FED4BA255A9D31C961EB74C6356D68C049B8923B61FA6CE669622E60F29FB6";
const RFC6979_UY: &str = "7903FE1008B8BC99A41AE9E95628BC64F2F1B20C2D7E9F5177A3C294D4462299";

#[test]
fn we_reproduce_the_rfc_6979_vectors() {
    let curve = Curve::p256();
    let key = unhex(RFC6979_KEY);

    // message, expected r, expected s: the RFC's two SHA-256 cases
    let cases = [
        (
            "sample",
            "EFD48B2AACB6A8FD1140DD9CD45E81D69D2C877B56AAF991C34D0EA84EAF3716",
            "F7CB1C942D657C41D436C7A1B6E29F65F3E900DBB9AFF4064DC4AB2F843ACDA8",
        ),
        (
            "test",
            "F1ABB023518351CD71D881567B1EA663ED3EFCF6C5132B354F28D3B0B7D38367",
            "019F4113742A2B14BD25926B49C649155F267E60D3814B4C0CC84250E46F0083",
        ),
    ];

    for (msg, want_r, want_s) in cases {
        let hash = mikunet::crypto::sha256(msg.as_bytes());
        let mut sig = [0u8; MAX_SIG_LEN];
        let n = ecdsa_sign(&curve, &key, &hash, &mut sig).expect("signing should work");
        let (r, s) = der_pair(&sig[..n]);

        let strip = |v: Vec<u8>| {
            let mut v = v;
            while v.first() == Some(&0) && v.len() > 1 {
                v.remove(0);
            }
            v
        };
        assert_eq!(
            r,
            strip(unhex(want_r)),
            "{msg:?}: r does not match the published vector"
        );
        assert_eq!(
            s,
            strip(unhex(want_s)),
            "{msg:?}: s does not match the published vector"
        );
    }
}

/// The same key's public point, as a keyring would hold it
#[test]
fn the_public_point_matches_the_published_one() {
    let curve = Curve::p256();
    let mut pub_key = [0u8; 65];
    let n = ecdsa_public_key(&curve, &unhex(RFC6979_KEY), &mut pub_key).unwrap();
    assert_eq!(n, 65);
    assert_eq!(pub_key[0], 0x04, "uncompressed");
    assert_eq!(&pub_key[1..33], &unhex(RFC6979_UX)[..]);
    assert_eq!(&pub_key[33..65], &unhex(RFC6979_UY)[..]);
}

/// Same key and message give the same bytes
#[test]
fn signing_is_deterministic() {
    let curve = Curve::p256();
    let key = unhex(RFC6979_KEY);
    let hash = mikunet::crypto::sha256(b"mikupkg");

    let mut a = [0u8; MAX_SIG_LEN];
    let mut b = [0u8; MAX_SIG_LEN];
    let na = ecdsa_sign(&curve, &key, &hash, &mut a).unwrap();
    let nb = ecdsa_sign(&curve, &key, &hash, &mut b).unwrap();
    assert_eq!(&a[..na], &b[..nb]);

    // A different message gives different bytes
    let other = mikunet::crypto::sha256(b"mikupkg ");
    let mut c = [0u8; MAX_SIG_LEN];
    let nc = ecdsa_sign(&curve, &key, &other, &mut c).unwrap();
    assert_ne!(&a[..na], &c[..nc]);
}

/// Verified by the same verifier that checks certificates
#[test]
fn what_we_sign_the_existing_verifier_accepts() {
    let curve = Curve::p256();
    let key = unhex(RFC6979_KEY);
    let mut pub_key = [0u8; 65];
    ecdsa_public_key(&curve, &key, &mut pub_key).unwrap();

    let hash = mikunet::crypto::sha256(b"a package manifest");
    let mut sig = [0u8; MAX_SIG_LEN];
    let n = ecdsa_sign(&curve, &key, &hash, &mut sig).unwrap();

    assert!(ecdsa_verify(&curve, &pub_key, &hash, &sig[..n]));

    // Fails for a different message
    let other = mikunet::crypto::sha256(b"a package manifest.");
    assert!(!ecdsa_verify(&curve, &pub_key, &other, &sig[..n]));

    // ...nor under a different key
    let mut other_key = unhex(RFC6979_KEY);
    other_key[31] ^= 1;
    let mut other_pub = [0u8; 65];
    ecdsa_public_key(&curve, &other_key, &mut other_pub).unwrap();
    assert!(!ecdsa_verify(&curve, &other_pub, &hash, &sig[..n]));

    // ...nor with a bit moved in the signature itself
    for at in [3usize, 10, n - 1] {
        let mut broken = sig;
        broken[at] ^= 0x01;
        assert!(
            !ecdsa_verify(&curve, &pub_key, &hash, &broken[..n]),
            "a signature with byte {at} changed must not verify"
        );
    }
}

#[test]
fn a_key_that_is_not_a_scalar_is_refused() {
    use mikunet::ec::SignError;
    let curve = Curve::p256();
    let hash = [0u8; 32];
    let mut sig = [0u8; MAX_SIG_LEN];

    assert_eq!(
        ecdsa_sign(&curve, &[0u8; 32], &hash, &mut sig),
        Err(SignError::BadPrivateKey),
        "zero is not a private key"
    );
    // The group order itself, out of range by one
    let n = unhex("FFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551");
    assert_eq!(
        ecdsa_sign(&curve, &n, &hash, &mut sig),
        Err(SignError::BadPrivateKey)
    );
}

/// P-521's order is not a whole number of bytes, so `bits2int` must work in
/// bits
#[test]
fn the_wider_curves_sign_and_verify_too() {
    for (curve, keylen) in [(Curve::p384(), 48usize), (Curve::p521(), 66)] {
        let mut key = vec![0x11u8; keylen];
        key[keylen - 1] = 0x23;
        // P-521's top byte must leave the scalar below the order
        key[0] = 0x00;

        let mut pub_key = [0u8; 1 + 2 * 66];
        let plen = ecdsa_public_key(&curve, &key, &mut pub_key).unwrap();

        let hash = mikunet::crypto::sha256(b"wider");
        let mut sig = [0u8; MAX_SIG_LEN];
        let n = ecdsa_sign(&curve, &key, &hash, &mut sig).unwrap();
        assert!(
            ecdsa_verify(&curve, &pub_key[..plen], &hash, &sig[..n]),
            "a {keylen}-byte curve must round trip"
        );
    }
}
