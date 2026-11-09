// The installed database: one file per package, with the container's metadata
// and file table verbatim. `mikupkg files` prints that table, and
// `mikupkg verify` compares against its hashes

use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::root::Root;
use mikupkg::db::{self, Db, OwnerIndex};
use mikupkg::format::db::{Installed, Reason};
use mikupkg::format::meta as meta_codec;
use mikupkg::format::writer::Builder;
use mikupkg::format::Package;
use mikupkg::meta::{Arch, PackageMeta};
use mikupkg::name::Name;
use mikupkg::plan::conflict::Owners;
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// A record as an install makes one, from the container
fn record_for(name: &str, version: &str, files: &[(&str, &[u8])]) -> Installed {
    let mut b = Builder::new(PackageMeta::new(n(name), v(version), Arch::X86_64));
    for (path, body) in files {
        b.add_file(path, 0o755, 0, 0, body).unwrap();
    }
    let bytes = b.finish(None).unwrap();
    let pkg = Package::open(&bytes).unwrap();

    Installed {
        meta: pkg.meta_bytes().to_vec(),
        files: pkg.table().clone(),
        reason: Reason::Explicit,
        install_time: 1_780_000_000,
        container_sha256: mikupkg::format::sha256(&bytes),
    }
}

#[test]
fn a_record_survives_the_round_trip() {
    let rec = record_for("mayterm", "0.2.10", &[("bin/mayterm", b"a program")]);
    let back = Installed::parse(&rec.encode()).expect("a record should read back");
    assert_eq!(back, rec);

    // Its metadata is the package's own, read by the container's code
    let meta = meta_codec::decode(&back.meta, |_, _| {}).unwrap();
    assert_eq!(meta.name.as_str(), "mayterm");
    assert_eq!(meta.version, v("0.2.10"));
}

#[test]
fn the_database_is_written_and_read_through_the_same_root() {
    let mut root = MemRoot::new();
    db::create(&mut root, "pkg/db").unwrap();

    let rec = record_for("mayterm", "0.2.10", &[("bin/mayterm", b"a program")]);
    db::write(&mut root, "pkg/db", &n("mayterm"), &rec).unwrap();

    let d = Db::open(&root, "pkg/db");
    assert!(d.is_installed(&n("mayterm")));
    assert!(!d.is_installed(&n("msh")));
    assert_eq!(d.get(&n("mayterm")).unwrap(), rec);
    assert_eq!(d.meta(&n("mayterm")).unwrap().version, v("0.2.10"));

    // Written beside, then renamed over
    let newer = record_for("mayterm", "0.3.0", &[("bin/mayterm", b"newer")]);
    db::write(&mut root, "pkg/db", &n("mayterm"), &newer).unwrap();
    assert_eq!(Db::open(&root, "pkg/db").meta(&n("mayterm")).unwrap().version, v("0.3.0"));
    assert!(
        !root.paths().iter().any(|p| p.contains(".mikupkg-")),
        "the staging file must not survive"
    );

    db::forget(&mut root, "pkg/db", &n("mayterm")).unwrap();
    assert!(!Db::open(&root, "pkg/db").is_installed(&n("mayterm")));
    // Forgetting something absent is not an error
    db::forget(&mut root, "pkg/db", &n("mayterm")).unwrap();
}

/// The image ships a database describing itself, for the first upgrade
#[test]
fn a_fresh_database_says_what_format_it_is() {
    let mut root = MemRoot::new();
    db::create(&mut root, "pkg/db").unwrap();
    assert_eq!(root.read("pkg/db/FORMAT").unwrap(), b"1\n");
    assert!(root.stat("pkg").is_some(), "the parent is made too");
}

#[test]
fn who_owns_a_path() {
    let a = record_for("miku-base", "1.0", &[("bin/msh", b"shell"), ("bin/ls", b"ls")]);
    let b = record_for("mayterm", "0.2.10", &[("bin/mayterm", b"term")]);

    let index = OwnerIndex::build(&[(n("miku-base"), a), (n("mayterm"), b)]);
    assert_eq!(index.len(), 3);
    assert_eq!(index.owner_of("bin/msh").unwrap().as_str(), "miku-base");
    assert_eq!(index.owner_of("bin/mayterm").unwrap().as_str(), "mayterm");
    assert!(index.owner_of("bin/nothing").is_none());
}

#[test]
fn a_record_that_is_not_one_is_refused() {
    let good = record_for("mayterm", "0.2.10", &[("bin/mayterm", b"x")]).encode();

    for cut in 0..good.len() {
        assert!(
            Installed::parse(&good[..cut]).is_err(),
            "{cut} bytes of a record should not parse"
        );
    }

    let mut wrong_magic = good.clone();
    wrong_magic[0] = b'X';
    assert!(Installed::parse(&wrong_magic).is_err());

    let mut wrong_version = good.clone();
    wrong_version[8..10].copy_from_slice(&99u16.to_le_bytes());
    assert!(Installed::parse(&wrong_version).is_err());

    let mut wrong_reason = good;
    wrong_reason[10] = 7;
    assert!(Installed::parse(&wrong_reason).is_err());
}

/// Tracks why a package is present, so `remove` can tell an unneeded dependency
/// from an explicit install
#[test]
fn a_record_remembers_why_it_is_here() {
    for reason in [Reason::Explicit, Reason::Dependency] {
        let mut rec = record_for("libmay", "2.0", &[("bin/x", b"x")]);
        rec.reason = reason;
        assert_eq!(Installed::parse(&rec.encode()).unwrap().reason, reason);
    }
}
