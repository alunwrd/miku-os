// Keys, signatures, and two ways an index can mislead without being forged
//
// A signature says who made it, not whether it is the newest. The serial and
// expiry cover that, and freshness has its own answer, including "cannot tell".

use mikupkg::format::index::{Index, IndexEntry};
use mikupkg::format::meta as meta_codec;
use mikupkg::format::writer::Builder;
use mikupkg::format::Package;
use mikupkg::meta::{Arch, PackageMeta};
use mikupkg::name::Name;
use mikupkg::trust::keyring::{Key, KeyId, Keyring, POINT_LEN};
use mikupkg::trust::policy::{self, Freshness, Verdict};
use mikupkg::trust::sig::{Signature, ALG_ECDSA_P256_SHA256};
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

const NOW: u64 = 1_780_000_000;

/// A signer like `builder`: the private key stays on the host and only the
/// public point reaches the machine
struct Signer {
    priv_key: [u8; 32],
    point: [u8; POINT_LEN],
}

impl Signer {
    fn new(seed: u8) -> Signer {
        let mut priv_key = [seed; 32];
        priv_key[31] = 0x42;
        let curve = mikucrypto::ec::Curve::p256();
        let mut point = [0u8; POINT_LEN];
        mikucrypto::ec::ecdsa_public_key(&curve, &priv_key, &mut point).unwrap();
        Signer { priv_key, point }
    }

    fn key(&self, scope: Option<Name>) -> Key {
        Key {
            id: KeyId::of(&self.point),
            alg: ALG_ECDSA_P256_SHA256,
            point: self.point,
            not_before: NOW - 1000,
            not_after: NOW + 1000,
            scope,
        }
    }

    fn sign(&self, digest: &[u8; 32]) -> Vec<u8> {
        let curve = mikucrypto::ec::Curve::p256();
        let mut raw = [0u8; mikucrypto::ec::MAX_SIG_LEN];
        let len = mikucrypto::ec::ecdsa_sign(&curve, &self.priv_key, digest, &mut raw).unwrap();
        Signature {
            key_id: KeyId::of(&self.point),
            alg: ALG_ECDSA_P256_SHA256,
            bytes: raw[..len].to_vec(),
        }
        .encode()
    }
}

fn ring_of(keys: &[Key]) -> Keyring {
    let mut r = Keyring::new();
    for k in keys {
        r.add(k.clone());
    }
    r
}

// A package

fn signed_package(signer: &Signer) -> Vec<u8> {
    let mut b = Builder::new(PackageMeta::new(n("mayterm"), v("0.2.10"), Arch::X86_64));
    b.add_file("bin/mayterm", 0o755, 0, 0, b"a program").unwrap();
    let sig = signer.sign(&b.manifest_hash());
    b.finish(Some(&sig)).unwrap()
}

#[test]
fn a_package_signed_by_a_trusted_key_is_trusted() {
    let signer = Signer::new(0x11);
    let bytes = signed_package(&signer);
    let pkg = Package::open(&bytes).unwrap();
    let ring = ring_of(&[signer.key(None)]);

    let verdict = policy::check(&pkg.manifest_hash(), pkg.signature(), &ring, None, NOW);
    assert!(verdict.is_trusted(), "{verdict}");
}

/// The signature covers metadata and the file table, not the payload. A payload
/// byte changed after signing leaves the signature valid, and the table's
/// SHA-256 catches it. Both checks are in `trust::package`
#[test]
fn a_changed_payload_leaves_the_signature_valid_and_is_caught_by_the_file_hash() {
    let signer = Signer::new(0x11);
    let bytes = signed_package(&signer);
    let ring = ring_of(&[signer.key(None)]);

    let pkg = Package::open(&bytes).unwrap();
    let at = (pkg.header().payload_off + pkg.table().entries[0].payload_rel) as usize;

    let mut tampered = bytes.clone();
    tampered[at] ^= 0x01;
    let pkg = Package::open(&tampered).unwrap();

    // The signature alone passes: it still signs this table
    let verdict = policy::check(&pkg.manifest_hash(), pkg.signature(), &ring, None, NOW);
    assert!(verdict.is_trusted(), "the signature covers the table, and the table is intact");

    // The file's own hash says no
    assert!(pkg.verify_all().is_err());

    // ...and the one call that asks both questions says no
    assert!(
        mikupkg::trust::package(&pkg, &ring, None, NOW).is_err(),
        "the check a caller is meant to use must refuse it"
    );

    // Changing the table breaks the signature
    let mut relabelled = bytes.clone();
    let table_at = pkg.header().table_off as usize + 24; // into the sha256 field
    relabelled[table_at] ^= 0x01;
    let pkg = Package::open(&relabelled).unwrap();
    let verdict = policy::check(&pkg.manifest_hash(), pkg.signature(), &ring, None, NOW);
    assert_eq!(verdict, Verdict::Forged { key: signer.key(None).id }, "{verdict}");
}

/// The whole package, checked as an installer must
#[test]
fn the_one_call_that_asks_both_questions() {
    let signer = Signer::new(0x11);
    let bytes = signed_package(&signer);
    let pkg = Package::open(&bytes).unwrap();

    let ring = ring_of(&[signer.key(None)]);
    assert!(mikupkg::trust::package(&pkg, &ring, None, NOW).unwrap().is_trusted());

    // An unsigned package is a verdict, not an error; installing one is decided
    // elsewhere
    let mut b = Builder::new(PackageMeta::new(n("hello"), v("1.0"), Arch::X86_64));
    b.add_file("bin/hello", 0o755, 0, 0, b"hi").unwrap();
    let plain = b.finish(None).unwrap();
    let pkg = Package::open(&plain).unwrap();
    assert_eq!(
        mikupkg::trust::package(&pkg, &ring, None, NOW).unwrap(),
        Verdict::Unsigned
    );
}

#[test]
fn every_way_a_signature_can_fail_is_a_different_answer() {
    let signer = Signer::new(0x11);
    let stranger = Signer::new(0x22);
    let bytes = signed_package(&signer);
    let pkg = Package::open(&bytes).unwrap();
    let digest = pkg.manifest_hash();
    let mine = signer.key(None);

    // No signature at all
    assert_eq!(
        policy::check(&digest, None, &ring_of(&[mine.clone()]), None, NOW),
        Verdict::Unsigned
    );

    // A key not held here
    assert_eq!(
        policy::check(&digest, pkg.signature(), &ring_of(&[stranger.key(None)]), None, NOW),
        Verdict::UnknownKey { key: mine.id }
    );

    // An empty keyring gives the same answer
    assert_eq!(
        policy::check(&digest, pkg.signature(), &Keyring::new(), None, NOW),
        Verdict::UnknownKey { key: mine.id }
    );

    // A key outside its window
    assert_eq!(
        policy::check(&digest, pkg.signature(), &ring_of(&[mine.clone()]), None, NOW + 100_000),
        Verdict::Expired { key: mine.id }
    );

    // An unset clock is not a pass
    assert_eq!(
        policy::check(&digest, pkg.signature(), &ring_of(&[mine.clone()]), None, 0),
        Verdict::Expired { key: mine.id }
    );

    // A key that may only sign for another repository
    let scoped = signer.key(Some(n("other")));
    assert_eq!(
        policy::check(&digest, pkg.signature(), &ring_of(&[scoped]), Some(&n("core")), NOW),
        Verdict::OutOfScope { key: mine.id }
    );

    // An invalid envelope
    assert_eq!(
        policy::check(&digest, Some(b"not a signature"), &ring_of(&[mine.clone()]), None, NOW),
        Verdict::Malformed
    );

    // An algorithm this build cannot check
    let mut envelope = Signature::parse(pkg.signature().unwrap()).unwrap();
    envelope.alg = 99;
    assert_eq!(
        policy::check(&digest, Some(&envelope.encode()), &ring_of(&[mine]), None, NOW),
        Verdict::UnknownAlgorithm { alg: 99 }
    );
}

/// A signature over one thing must not verify over another. `manifest_hash`
/// provides domain separation
#[test]
fn a_signature_does_not_travel_between_packages() {
    let signer = Signer::new(0x11);
    let ring = ring_of(&[signer.key(None)]);
    let a = signed_package(&signer);
    let sig = Package::open(&a).unwrap().signature().unwrap().to_vec();

    let mut b = Builder::new(PackageMeta::new(n("msh"), v("0.2.10"), Arch::X86_64));
    b.add_file("bin/msh", 0o755, 0, 0, b"a different program").unwrap();
    let other = b.finish(Some(&sig)).unwrap();

    let pkg = Package::open(&other).unwrap();
    let verdict = policy::check(&pkg.manifest_hash(), pkg.signature(), &ring, None, NOW);
    assert!(!verdict.is_trusted(), "{verdict}");
}

// The keyring's own bytes

#[test]
fn a_key_survives_being_written_to_a_file_and_read_back() {
    let signer = Signer::new(0x33);
    for scope in [None, Some(n("core"))] {
        let key = signer.key(scope);
        let back = Key::parse(&key.encode()).expect("a key file should read back");
        assert_eq!(back, key);
    }
}

#[test]
fn a_key_file_that_is_not_one_is_refused() {
    let signer = Signer::new(0x33);
    let good = signer.key(None).encode();

    assert!(Key::parse(&good[..64]).is_err(), "half a record is not a key");
    let mut wrong_magic = good;
    wrong_magic[0] = b'X';
    assert!(Key::parse(&wrong_magic).is_err());

    let mut wrong_version = signer.key(None).encode();
    wrong_version[8..10].copy_from_slice(&99u16.to_le_bytes());
    assert!(Key::parse(&wrong_version).is_err());

    let mut not_a_point = signer.key(None).encode();
    not_a_point[32] = 0x02; // compressed, which this does not accept
    assert!(Key::parse(&not_a_point).is_err());
}

#[test]
fn a_keyring_holds_one_entry_per_key() {
    let signer = Signer::new(0x44);
    let mut ring = Keyring::new();
    ring.add(signer.key(None));
    ring.add(signer.key(None));
    assert_eq!(ring.len(), 1, "one key, one entry");
    assert!(ring.find(&KeyId::of(&signer.point)).is_some());
}

// An index, and the two attacks a signature does not cover

fn an_index(serial: u64, not_after: u64) -> Index {
    let meta = meta_codec::encode(&PackageMeta::new(n("mayterm"), v("0.2.10"), Arch::X86_64));
    Index {
        repo: n("core"),
        serial,
        not_before: NOW - 1000,
        not_after,
        entries: vec![IndexEntry {
            sha256: [7u8; 32],
            size: 4096,
            path: "mayterm-0.2.10.mpk".into(),
            meta,
        }],
        signature: None,
    }
}

#[test]
fn an_index_survives_the_wire() {
    let mut idx = an_index(7, NOW + 1000);
    idx.entries.push(IndexEntry {
        sha256: [9u8; 32],
        size: 1,
        path: "msh-1.0.mpk".into(),
        meta: meta_codec::encode(&PackageMeta::new(n("msh"), v("1.0"), Arch::X86_64)),
    });

    let back = Index::parse(&idx.encode()).expect("an index should read back");
    assert_eq!(back, idx);

    // Its metadata is the package's own, so a solve needs no container
    let carried = meta_codec::decode(&back.entries[0].meta, |_, _| {}).unwrap();
    assert_eq!(carried.name.as_str(), "mayterm");
}

#[test]
fn a_signed_index_is_trusted_and_a_changed_one_is_not() {
    let signer = Signer::new(0x55);
    let ring = ring_of(&[signer.key(Some(n("core")))]);

    let mut idx = an_index(7, NOW + 1000);
    idx.signature = Some(signer.sign(&idx.digest()));
    let bytes = idx.encode();

    let back = Index::parse(&bytes).unwrap();
    let verdict = policy::check(
        &back.digest(),
        back.signature.as_deref(),
        &ring,
        Some(&n("core")),
        NOW,
    );
    assert!(verdict.is_trusted(), "{verdict}");

    // Bumps the serial without re-signing, so the signature no longer matches
    let mut forged = back.clone();
    forged.serial = 9;
    let verdict = policy::check(
        &forged.digest(),
        forged.signature.as_deref(),
        &ring,
        Some(&n("core")),
        NOW,
    );
    assert!(!verdict.is_trusted(), "{verdict}");

    // So does adding a package
    let mut forged = back.clone();
    forged.entries[0].sha256 = [8u8; 32];
    let verdict = policy::check(
        &forged.digest(),
        forged.signature.as_deref(),
        &ring,
        Some(&n("core")),
        NOW,
    );
    assert!(!verdict.is_trusted(), "{verdict}");
}

/// An old index, correctly signed
#[test]
fn an_index_may_not_go_backwards() {
    assert_eq!(
        policy::freshness(9, 7, NOW + 1000, NOW),
        Freshness::Rollback { held: 9, offered: 7 }
    );
    assert!(policy::freshness(9, 9, NOW + 1000, NOW).is_current());
    assert!(policy::freshness(9, 10, NOW + 1000, NOW).is_current());
}

/// A correct index, served forever
#[test]
fn an_index_past_its_expiry_is_refused() {
    assert_eq!(
        policy::freshness(0, 1, NOW - 1, NOW),
        Freshness::Expired { expired_at: NOW - 1, now: NOW }
    );
    assert!(policy::freshness(0, 1, 0, NOW).is_current(), "zero means no expiry");
}

/// "Cannot tell" is an answer, and not "fine"
#[test]
fn an_unset_clock_cannot_judge_freshness() {
    assert_eq!(policy::freshness(0, 1, NOW + 1000, 0), Freshness::UnknownClock);
    assert!(!policy::freshness(0, 1, NOW + 1000, 0).is_current());

    // A rollback is visible without a clock, by comparison with what is held
    assert_eq!(
        policy::freshness(9, 7, NOW + 1000, 0),
        Freshness::Rollback { held: 9, offered: 7 }
    );

    let said = std::format!("{}", Freshness::UnknownClock);
    assert!(said.contains("ntpdate") || said.contains("clock"), "{said}");
}

#[test]
fn an_index_that_is_not_one_is_refused() {
    let good = an_index(1, NOW + 1000).encode();
    for cut in 0..good.len() {
        assert!(Index::parse(&good[..cut]).is_err(), "{cut} bytes should not parse");
    }
    let mut count_too_high = good.clone();
    count_too_high[12..16].copy_from_slice(&99_999u32.to_le_bytes());
    assert!(Index::parse(&count_too_high).is_err());

    let mut lying_count = good.clone();
    lying_count[12..16].copy_from_slice(&2u32.to_le_bytes());
    assert!(Index::parse(&lying_count).is_err(), "an entry that is not there");
}
