// The root half: `mikupkg-apply`, the one program with every capability. The
// unprivileged half stages bytes and writes the plan, and none of it is
// believed

use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::root::Root;
use mikupkg::apply::staged::{self, StagedError};
use mikupkg::apply::Paths;
use mikupkg::db;
use mikupkg::format::db::{Installed, Reason};
use mikupkg::format::writer::Builder;
use mikupkg::format::{FileKind, Package};
use mikupkg::meta::{Arch, PackageMeta, Trigger};
use mikupkg::name::Name;
use mikupkg::plan::encode::{Item, StagedPlan, Verb};
use mikupkg::trust::keyring::{Key, KeyId, Keyring, POINT_LEN};
use mikupkg::trust::sig::{Signature, ALG_ECDSA_P256_SHA256};
use mikupkg::version::Version;

const NOW: u64 = 1_780_000_000;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// The host's signer. The private key never reaches the machine
struct Signer {
    priv_key: [u8; 32],
    point: [u8; POINT_LEN],
}

impl Signer {
    fn new(seed: u8) -> Signer {
        let mut priv_key = [seed; 32];
        priv_key[31] = 0x11;
        let curve = mikunet::ec::Curve::p256();
        let mut point = [0u8; POINT_LEN];
        mikunet::ec::ecdsa_public_key(&curve, &priv_key, &mut point).unwrap();
        Signer { priv_key, point }
    }
    fn key(&self) -> Key {
        Key {
            id: KeyId::of(&self.point),
            alg: ALG_ECDSA_P256_SHA256,
            point: self.point,
            not_before: 0,
            not_after: 0,
            scope: None,
        }
    }
    fn sign(&self, digest: &[u8; 32]) -> Vec<u8> {
        let curve = mikunet::ec::Curve::p256();
        let mut raw = [0u8; mikunet::ec::MAX_SIG_LEN];
        let len = mikunet::ec::ecdsa_sign(&curve, &self.priv_key, digest, &mut raw).unwrap();
        Signature { key_id: KeyId::of(&self.point), alg: ALG_ECDSA_P256_SHA256, bytes: raw[..len].to_vec() }
            .encode()
    }
}

fn a_package(name: &str, version: &str) -> Vec<u8> {
    let mut meta = PackageMeta::new(n(name), v(version), Arch::X86_64);
    meta.triggers.push(Trigger::Services);
    let mut b = Builder::new(meta);
    b.add_dir("bin", 0o755, 0, 0).unwrap();
    b.add_file("bin/mayterm", 0o755, 0, 0, b"a program").unwrap();
    b.add_file("etc/mayland/default.d/20-term.conf", 0o644, 0, 0, b"font = 12\n").unwrap();
    b.add_symlink("bin/term", "mayterm", 0, 0).unwrap();
    b.finish(None).unwrap()
}

fn signed_package(signer: &Signer, name: &str, version: &str) -> Vec<u8> {
    let mut meta = PackageMeta::new(n(name), v(version), Arch::X86_64);
    meta.triggers.push(Trigger::Services);
    let mut b = Builder::new(meta);
    b.add_dir("bin", 0o755, 0, 0).unwrap();
    b.add_file("bin/mayterm", 0o755, 0, 0, b"a program").unwrap();
    b.add_file("etc/mayland/default.d/20-term.conf", 0o644, 0, 0, b"font = 12\n").unwrap();
    b.add_symlink("bin/term", "mayterm", 0, 0).unwrap();
    let sig = signer.sign(&b.manifest_hash());
    b.finish(Some(&sig)).unwrap()
}

/// The unprivileged half: unpacks every file into a numbered slot and writes
/// the package's evidence
fn stage(root: &mut MemRoot, paths: &Paths, serial: u64, index: usize, bytes: &[u8], verb: Verb) -> Item {
    let pkg = Package::open(bytes).expect("a container we built");
    let meta = pkg.meta(|_, _| {}).unwrap();

    for (file, entry) in pkg.table().entries.iter().enumerate() {
        if entry.kind == FileKind::Directory {
            continue;
        }
        let at = StagedPlan::staged_at(&paths.stage, serial, index, file);
        root.write_new(&at, pkg.content(file).unwrap(), 0o600, 1000, 100)
            .unwrap();
    }

    Item {
        verb,
        reason: Reason::Explicit,
        name: meta.name,
        meta: pkg.meta_bytes().to_vec(),
        table: pkg.table_bytes().to_vec(),
        signature: pkg.signature().map(|s| s.to_vec()).unwrap_or_default(),
        container_sha256: mikupkg::format::sha256(bytes),
    }
}

fn machine() -> MemRoot {
    let mut r = MemRoot::new();
    r.preset_dir("pkg", 0o755);
    r.preset_dir("pkg/db", 0o755);
    r.preset_dir("pkg/stage", 0o1777);
    r.preset_dir("bin", 0o755);
    r
}

fn ring(signer: &Signer) -> Keyring {
    let mut k = Keyring::new();
    k.add(signer.key());
    k
}

fn apply(
    root: &mut MemRoot,
    plan: &StagedPlan,
    keyring: &Keyring,
    allow_unsigned: bool,
) -> Result<mikupkg::apply::staged::Applied, StagedError> {
    let paths = Paths::default();
    let reader = root.clone();
    staged::run(plan, root, &reader, keyring, &paths, &paths.stage, allow_unsigned, NOW)
}

// Installing

#[test]
fn a_signed_package_is_placed_and_recorded_in_one_transaction() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };

    let done = apply(&mut root, &plan, &ring(&signer), false).expect("it should install");
    assert_eq!(done.installed, vec![n("mayterm")]);
    assert_eq!(done.triggers, vec![Trigger::Services]);

    assert_eq!(root.read("bin/mayterm").unwrap(), b"a program");
    assert_eq!(root.stat("bin/mayterm").unwrap().mode, 0o755);
    assert_eq!(root.read_link("bin/term").unwrap(), "mayterm");
    assert_eq!(
        root.read("etc/mayland/default.d/20-term.conf").unwrap(),
        b"font = 12\n"
    );

    // The record went in with the files
    let raw = root.read(&db::record_path(&paths.db, &n("mayterm"))).unwrap();
    let rec = Installed::parse(&raw).unwrap();
    assert_eq!(rec.reason, Reason::Explicit);
    assert_eq!(rec.install_time, NOW);
    assert!(rec.files.entries.iter().any(|e| e.path == "bin/mayterm"));
}

/// Staged files belong to whoever staged them; installed files belong to what
/// the package says
#[test]
fn what_is_placed_is_owned_by_what_the_package_says() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);

    // ...and the staged copy really is owned by the caller
    let at = StagedPlan::staged_at(&paths.stage, 1, 0, 1);
    assert_eq!(root.stat(&at).unwrap().uid, 1000);

    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };
    apply(&mut root, &plan, &ring(&signer), false).unwrap();
    assert_eq!(root.stat("bin/mayterm").unwrap().uid, 0);
    assert_eq!(root.stat("bin/mayterm").unwrap().gid, 0);
}

// Refusing

/// Editing staged bytes cannot make the root half write them
#[test]
fn a_staged_file_that_does_not_match_the_signed_table_is_refused() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);

    // Swap the staged program for something else, leaving the plan untouched
    let at = StagedPlan::staged_at(&paths.stage, 1, 0, 1);
    root.preset_file(&at, b"a program of my own", 0o600, 1000, 100);

    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };
    let err = apply(&mut root, &plan, &ring(&signer), false).unwrap_err();
    assert!(matches!(err, StagedError::Tx(_)), "{err}");
    assert!(root.stat("bin/mayterm").is_none(), "and nothing was written");
    assert!(root.stat(&db::record_path(&paths.db, &n("mayterm"))).is_none());
}

/// Nor can editing the table, which the signature covers
#[test]
fn a_table_edited_after_signing_is_refused() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let mut item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);

    // Points an entry at a different hash
    item.table[24] ^= 0x01;

    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };
    let err = apply(&mut root, &plan, &ring(&signer), false).unwrap_err();
    assert!(
        matches!(err, StagedError::Untrusted { .. }),
        "the signature must stop covering it: {err}"
    );
}

#[test]
fn a_package_signed_by_nobody_here_is_refused() {
    let signer = Signer::new(0x11);
    let stranger = Signer::new(0x22);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&stranger, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };

    let err = apply(&mut root, &plan, &ring(&signer), false).unwrap_err();
    assert!(matches!(err, StagedError::Untrusted { .. }), "{err}");
    // Allowing unsigned packages does not allow wrongly signed ones
    let err = apply(&mut root, &plan, &ring(&signer), true).unwrap_err();
    assert!(matches!(err, StagedError::Untrusted { .. }), "{err}");
}

/// Unsigned is legal; whoever runs the command decides on installing it
#[test]
fn an_unsigned_package_is_refused_unless_it_was_allowed() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = a_package("mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };

    let err = apply(&mut root, &plan, &ring(&signer), false).unwrap_err();
    assert!(
        matches!(err, StagedError::Untrusted { verdict: mikupkg::trust::Verdict::Unsigned, .. }),
        "{err}"
    );
    apply(&mut root, &plan, &ring(&signer), true).expect("allowed, so installed");
    assert_eq!(root.read("bin/mayterm").unwrap(), b"a program");
}

#[test]
fn a_plan_that_calls_a_package_something_else_is_refused() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let mut item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    item.name = n("msh");

    let plan = StagedPlan { serial: 1, time: NOW, items: vec![item] };
    let err = apply(&mut root, &plan, &ring(&signer), false).unwrap_err();
    assert!(matches!(err, StagedError::NameMismatch { .. }), "{err}");
}

// Removing

/// Owned files come from the machine's record, not the caller
#[test]
fn a_removal_takes_what_the_record_says_and_nothing_else() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();

    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    apply(&mut root, &StagedPlan { serial: 1, time: NOW, items: vec![item] }, &ring(&signer), false)
        .unwrap();
    root.preset_file("bin/somebody-elses", b"not ours", 0o755, 0, 0);

    let removal = Item {
        verb: Verb::Remove,
        reason: Reason::Explicit,
        name: n("mayterm"),
        meta: Vec::new(),
        table: Vec::new(),
        signature: Vec::new(),
        container_sha256: [0u8; 32],
    };
    let done = apply(
        &mut root,
        &StagedPlan { serial: 2, time: NOW, items: vec![removal] },
        &ring(&signer),
        false,
    )
    .expect("it should remove");

    assert_eq!(done.removed, vec![n("mayterm")]);
    assert!(root.stat("bin/mayterm").is_none());
    assert!(root.stat("bin/term").is_none());
    assert!(root.stat(&db::record_path(&paths.db, &n("mayterm"))).is_none());
    assert_eq!(
        root.read("bin/somebody-elses").unwrap(),
        b"not ours",
        "nothing else was touched"
    );
}

#[test]
fn removing_something_that_is_not_installed_says_so() {
    let signer = Signer::new(0x11);
    let mut root = machine();
    let removal = Item {
        verb: Verb::Remove,
        reason: Reason::Explicit,
        name: n("nothing"),
        meta: Vec::new(),
        table: Vec::new(),
        signature: Vec::new(),
        container_sha256: [0u8; 32],
    };
    let err = apply(
        &mut root,
        &StagedPlan { serial: 1, time: NOW, items: vec![removal] },
        &ring(&signer),
        false,
    )
    .unwrap_err();
    assert!(matches!(err, StagedError::NotInstalled { .. }), "{err}");
}

// The plan's own bytes

#[test]
fn a_plan_survives_the_round_trip() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let a = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    let b = Item {
        verb: Verb::Remove,
        reason: Reason::Dependency,
        name: n("mayconsole"),
        meta: Vec::new(),
        table: Vec::new(),
        signature: Vec::new(),
        container_sha256: [3u8; 32],
    };
    let plan = StagedPlan { serial: 9, time: NOW, items: vec![a, b] };

    let back = StagedPlan::parse(&plan.encode()).expect("it should read back");
    assert_eq!(back, plan);
}

/// The only thing a root process parses, so each length is checked
#[test]
fn a_plan_that_is_not_one_is_refused() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    let good = StagedPlan { serial: 1, time: NOW, items: vec![item] }.encode();

    for cut in 0..good.len() {
        assert!(
            StagedPlan::parse(&good[..cut]).is_err(),
            "{cut} bytes of a plan should not parse"
        );
    }

    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    assert!(StagedPlan::parse(&wrong_magic).is_err());

    let mut wrong_version = good.clone();
    wrong_version[8..10].copy_from_slice(&99u16.to_le_bytes());
    assert!(StagedPlan::parse(&wrong_version).is_err());

    let mut lying_count = good.clone();
    lying_count[28..32].copy_from_slice(&9u32.to_le_bytes());
    assert!(StagedPlan::parse(&lying_count).is_err(), "items that are not there");

    let mut too_many = good.clone();
    too_many[28..32].copy_from_slice(&999_999u32.to_le_bytes());
    assert!(StagedPlan::parse(&too_many).is_err());

    let mut trailing = good;
    trailing.push(0);
    assert!(
        StagedPlan::parse(&trailing).is_err(),
        "bytes nobody accounted for at the end of what a root process reads"
    );
}

/// Only an install needs a key. A removal's authority is the machine's own
/// database, so a machine with no keys can still uninstall
#[test]
fn a_removal_needs_no_keyring() {
    let signer = Signer::new(0x11);
    let paths = Paths::default();
    let mut root = machine();

    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 1, 0, &bytes, Verb::Install);
    apply(&mut root, &StagedPlan { serial: 1, time: NOW, items: vec![item] }, &ring(&signer), false)
        .unwrap();

    let removal = Item {
        verb: Verb::Remove,
        reason: Reason::Explicit,
        name: n("mayterm"),
        meta: Vec::new(),
        table: Vec::new(),
        signature: Vec::new(),
        container_sha256: [0u8; 32],
    };
    let empty = Keyring::new();
    let done = apply(
        &mut root,
        &StagedPlan { serial: 2, time: NOW, items: vec![removal] },
        &empty,
        false,
    )
    .expect("a removal is not a question about keys");

    assert_eq!(done.removed, vec![n("mayterm")]);
    assert!(root.stat("bin/mayterm").is_none());

    // ...and an install with an empty keyring still is
    let bytes = signed_package(&signer, "mayterm", "0.2.10");
    let item = stage(&mut root, &paths, 3, 0, &bytes, Verb::Install);
    let err = apply(
        &mut root,
        &StagedPlan { serial: 3, time: NOW, items: vec![item] },
        &empty,
        false,
    )
    .unwrap_err();
    assert!(matches!(err, StagedError::Untrusted { .. }), "{err}");
}
