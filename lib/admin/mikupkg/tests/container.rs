// The package container
//
// The round trip checks that writer and reader agree. The rest checks what a
// reader does with wrong bytes: truncated, reordered, overlapping, relabeled,
// or altered after signing. Each must be a refusal, not a panic, a silent skip,
// or a read past the end.

use mikupkg::format::container::{FORMAT_VERSION, HEADER_LEN};
use mikupkg::format::meta as meta_codec;
use mikupkg::format::table::MAX_SYMLINK_TARGET;
use mikupkg::format::writer::{BuildError, Builder};
use mikupkg::format::{FileKind, FormatError, Package};
use mikupkg::meta::dep::{Dep, Provide};
use mikupkg::meta::path::PathError;
use mikupkg::meta::tag;
use mikupkg::meta::{Arch, PackageMeta, Trigger};
use mikupkg::name::Name;
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

fn sample_meta() -> PackageMeta {
    let mut m = PackageMeta::new(n("mayterm"), v("0.2.10"), Arch::X86_64);
    m.depends.push(Dep::parse("libmay>=2.0,<3.0").unwrap());
    m.depends.push(Dep::parse("miku-base").unwrap());
    m.conflicts.push(Dep::parse("mayterm-git").unwrap());
    m.replaces.push(Dep::parse("mayconsole").unwrap());
    m.provides.push(Provide::parse("terminal=1.0").unwrap());
    m.triggers.push(Trigger::Services);
    m.desc = "the terminal".into();
    m.url = "https://github.com/alunwrd/miku-os".into();
    m.license = "MIT".into();
    m.build_time = 1_780_000_000;
    m
}

fn sample() -> Vec<u8> {
    let mut b = Builder::new(sample_meta());
    b.add_dir("bin", 0o755, 0, 0).unwrap();
    b.add_file("bin/mayterm", 0o755, 0, 0, &vec![0x7Fu8; 4096]).unwrap();
    b.add_file("etc/mayland/default.d/20-term.conf", 0o644, 0, 0, b"font = 12\n").unwrap();
    b.add_symlink("bin/term", "mayterm", 0, 0).unwrap();
    b.finish(None).unwrap()
}

// The round trip

#[test]
fn what_goes_in_comes_out() {
    let bytes = sample();
    let pkg = Package::open(&bytes).expect("the package should open");

    let mut skipped = Vec::new();
    let meta = pkg.meta(|t, at| skipped.push((t, at))).expect("metadata should read");
    assert!(skipped.is_empty(), "nothing in our own package should be unknown");

    let want = sample_meta();
    assert_eq!(meta.name, want.name);
    assert_eq!(meta.version, want.version);
    assert_eq!(meta.arch, want.arch);
    assert_eq!(meta.depends, want.depends, "dependencies must survive the wire");
    assert_eq!(meta.conflicts, want.conflicts);
    assert_eq!(meta.replaces, want.replaces);
    assert_eq!(meta.provides, want.provides);
    assert_eq!(meta.triggers, want.triggers);
    assert_eq!(meta.desc, want.desc);
    assert_eq!(meta.url, want.url);
    assert_eq!(meta.license, want.license);
    assert_eq!(meta.build_time, want.build_time);
    assert_eq!(
        meta.installed_size,
        4096 + b"font = 12\n".len() as u64,
        "the writer counts the file bytes, so a plan can be weighed against free \
         space before a single byte is downloaded. Directories and symlinks are an \
         inode each and are deliberately not counted - the field is advisory, and \
         the file table is the truth"
    );

    assert_eq!(pkg.paths(), vec![
        "bin",
        "bin/mayterm",
        "bin/term",
        "etc/mayland/default.d/20-term.conf",
    ], "entries are sorted by path, so the output is a function of its inputs");

    pkg.verify_all().expect("every file should hash to what the table says");
    assert_eq!(pkg.content(1).unwrap(), &vec![0x7Fu8; 4096][..]);
    assert_eq!(pkg.content(2).unwrap(), b"mayterm", "a symlink's target is its content");
    assert_eq!(pkg.table().entries[2].kind, FileKind::Symlink);
    assert_eq!(pkg.content(0).unwrap(), b"", "a directory has none");
}

/// A file under `default.d` is marked, so `verify` says "changed" instead of
/// "damaged"
#[test]
fn a_shipped_default_is_marked_as_one() {
    let bytes = sample();
    let pkg = Package::open(&bytes).unwrap();
    let conf = pkg
        .table()
        .entries
        .iter()
        .find(|e| e.path.ends_with("20-term.conf"))
        .unwrap();
    assert!(conf.is_default_config());
    let prog = pkg.table().entries.iter().find(|e| e.path == "bin/mayterm").unwrap();
    assert!(!prog.is_default_config());
}

/// The same inputs give the same bytes, so a rebuild verifies against the first
/// signature
#[test]
fn building_it_twice_produces_the_same_bytes() {
    assert_eq!(sample(), sample());

    // ...including when the entries are added in a different order
    let mut a = Builder::new(sample_meta());
    a.add_file("bin/b", 0o755, 0, 0, b"b").unwrap();
    a.add_file("bin/a", 0o755, 0, 0, b"a").unwrap();
    let mut b = Builder::new(sample_meta());
    b.add_file("bin/a", 0o755, 0, 0, b"a").unwrap();
    b.add_file("bin/b", 0o755, 0, 0, b"b").unwrap();
    assert_eq!(a.finish(None).unwrap(), b.finish(None).unwrap());
}

/// A signer needs the digest before the container exists, and it must match
/// what ships
#[test]
fn the_hash_a_signer_signs_is_the_hash_the_package_has() {
    let mut b = Builder::new(sample_meta());
    b.add_file("bin/mayterm", 0o755, 0, 0, b"x").unwrap();
    let before = b.manifest_hash();
    let bytes = b.finish(None).unwrap();
    assert_eq!(before, Package::open(&bytes).unwrap().manifest_hash());
}

#[test]
fn a_signature_rides_along_and_is_found_again() {
    let mut b = Builder::new(sample_meta());
    b.add_file("bin/mayterm", 0o755, 0, 0, b"x").unwrap();
    let sig = [0xABu8; 72];
    let bytes = b.finish(Some(&sig)).unwrap();

    let pkg = Package::open(&bytes).unwrap();
    assert_eq!(pkg.signature(), Some(&sig[..]));
    // An unsigned package has no signature, not an empty one
    assert_eq!(Package::open(&sample()).unwrap().signature(), None);
}

// Wrong bytes

/// Truncation at every length
#[test]
fn a_truncated_package_is_refused_at_every_length() {
    let bytes = sample();
    for cut in 0..bytes.len() {
        let got = Package::open(&bytes[..cut]);
        assert!(
            got.is_err(),
            "{cut} bytes of a {} byte package should not open",
            bytes.len()
        );
    }
    assert!(Package::open(&bytes).is_ok(), "...and the whole of it should");
}

#[test]
fn something_that_is_not_a_package_is_refused_as_such() {
    assert_eq!(Package::open(b"").unwrap_err(), FormatError::Truncated { need: HEADER_LEN, have: 0 });
    assert_eq!(Package::open(&[0u8; 512]).unwrap_err(), FormatError::BadMagic);
    let mut tar = vec![0u8; 1024];
    tar[257..262].copy_from_slice(b"ustar");
    assert_eq!(Package::open(&tar).unwrap_err(), FormatError::BadMagic);
}

/// A future format is refused by number
#[test]
fn a_newer_format_is_refused_by_number() {
    let mut bytes = sample();
    bytes[8..10].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    assert_eq!(
        Package::open(&bytes).unwrap_err(),
        FormatError::UnsupportedVersion { found: FORMAT_VERSION + 1, understood: FORMAT_VERSION }
    );
}

/// Each section length in turn, made impossible
#[test]
fn a_section_that_does_not_fit_is_refused() {
    let offsets = [
        (16, "metadata length"),
        (24, "table length"),
        (32, "signature length"),
        (12, "metadata offset"),
        (20, "table offset"),
        (28, "signature offset"),
    ];
    for (at, what) in offsets {
        let mut bytes = sample();
        bytes[at..at + 4].copy_from_slice(&0xFFFF_FFF0u32.to_le_bytes());
        assert!(
            Package::open(&bytes).is_err(),
            "an impossible {what} should be refused"
        );
    }
    for at in [36usize, 44] {
        let mut bytes = sample();
        bytes[at..at + 8].copy_from_slice(&u64::MAX.to_le_bytes());
        assert!(Package::open(&bytes).is_err(), "an impossible payload extent");
    }
}

/// Sections must follow the declared order. Bounds alone allow an overlapping
/// pair, letting one signature cover something else
#[test]
fn sections_may_not_be_reordered_or_overlap() {
    let mut bytes = sample();
    // Points the table back at the metadata, both still inside the file
    let meta_off = u32::from_le_bytes(bytes[12..16].try_into().unwrap());
    bytes[20..24].copy_from_slice(&meta_off.to_le_bytes());
    assert!(
        matches!(Package::open(&bytes), Err(FormatError::SectionsOutOfOrder { .. }))
            || Package::open(&bytes).is_err()
    );
}

/// A package altered after building. The per-file hash makes one signature
/// cover every payload byte
#[test]
fn a_changed_file_does_not_hash_to_what_the_table_says() {
    let bytes = sample();
    let pkg = Package::open(&bytes).unwrap();
    let e = &pkg.table().entries[1];
    let at = (pkg.header().payload_off + e.payload_rel) as usize;

    let mut tampered = bytes.clone();
    tampered[at] ^= 0x01;
    let pkg = Package::open(&tampered).expect("the shape is still fine, which is the point");
    assert_eq!(pkg.content(1).unwrap_err(), FormatError::WrongHash { index: 1 });
    assert_eq!(pkg.verify_all().unwrap_err(), FormatError::WrongHash { index: 1 });
}

/// So is a changed symlink target
#[test]
fn a_changed_symlink_target_is_caught_too() {
    let bytes = sample();
    let pkg = Package::open(&bytes).unwrap();
    let idx = pkg.table().entries.iter().position(|e| e.kind == FileKind::Symlink).unwrap();
    let at = (pkg.header().payload_off + pkg.table().entries[idx].payload_rel) as usize;

    let mut tampered = bytes.clone();
    tampered[at] = b'X';
    let pkg = Package::open(&tampered).unwrap();
    assert_eq!(pkg.content(idx).unwrap_err(), FormatError::WrongHash { index: idx });
}

/// Changing the metadata changes what a signature would have to cover
#[test]
fn altering_the_metadata_changes_the_manifest_hash() {
    let a = sample();
    let mut m = sample_meta();
    m.version = v("1.0.0");
    let mut b = Builder::new(m);
    b.add_dir("bin", 0o755, 0, 0).unwrap();
    b.add_file("bin/mayterm", 0o755, 0, 0, &vec![0x7Fu8; 4096]).unwrap();
    b.add_file("etc/mayland/default.d/20-term.conf", 0o644, 0, 0, b"font = 12\n").unwrap();
    b.add_symlink("bin/term", "mayterm", 0, 0).unwrap();
    let b = b.finish(None).unwrap();

    assert_ne!(
        Package::open(&a).unwrap().manifest_hash(),
        Package::open(&b).unwrap().manifest_hash()
    );
}

// The critical bit

/// Splits a metadata section into records, so a test can add or remove one
fn records(bytes: &[u8]) -> Vec<(u16, Vec<u8>)> {
    let mut out = Vec::new();
    let mut at = 0;
    while at + 6 <= bytes.len() {
        let t = u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap());
        let len = u32::from_le_bytes(bytes[at + 2..at + 6].try_into().unwrap()) as usize;
        out.push((t, bytes[at + 6..at + 6 + len].to_vec()));
        at += 6 + len;
    }
    out
}

fn assemble(recs: &[(u16, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (t, v) in recs {
        out.extend_from_slice(&t.to_le_bytes());
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        out.extend_from_slice(v);
    }
    out
}

/// Inserts a record at its sorted position. Tags must not decrease, and
/// critical ones have the high bit and sort after ordinary fields
fn meta_with(extra_tag: u16, value: &[u8]) -> Vec<u8> {
    let mut recs = records(&meta_codec::encode(&sample_meta()));
    let at = recs.iter().position(|(t, _)| *t > extra_tag).unwrap_or(recs.len());
    recs.insert(at, (extra_tag, value.to_vec()));
    assemble(&recs)
}

/// A field from a newer mikupkg that an older one may skip
#[test]
fn an_unknown_ordinary_tag_is_reported_and_skipped() {
    let bytes = meta_with(0x7FFF, b"something from the future");
    let mut noted = Vec::new();
    let meta = meta_codec::decode(&bytes, |t, at| noted.push((t, at)))
        .expect("a package must not be refused for a field that costs nothing");
    assert_eq!(meta.name.as_str(), "mayterm");
    assert_eq!(noted.len(), 1, "and it must be reported, not dropped in silence");
    assert_eq!(noted[0].0, 0x7FFF);
}

/// A field changing what gets installed is refused, by number
#[test]
fn an_unknown_critical_tag_refuses_the_package() {
    let bytes = meta_with(tag::CRITICAL | 0x7F, b"a constraint we cannot honour");
    let err = meta_codec::decode(&bytes, |_, _| {}).unwrap_err();
    assert!(
        matches!(err, FormatError::UnknownCriticalTag { tag, .. } if tag == (tag::CRITICAL | 0x7F)),
        "got {err:?}"
    );
    assert!(std::format!("{err}").contains("critical"), "and it says so in words");
}

/// An unknown trigger is the same case
#[test]
fn an_unknown_trigger_refuses_the_package() {
    let mut m = sample_meta();
    m.triggers.clear();
    let mut bytes = meta_codec::encode(&m);
    let mut rec = Vec::new();
    rec.extend_from_slice(&tag::TRIGGER.to_le_bytes());
    rec.extend_from_slice(&2u32.to_le_bytes());
    rec.extend_from_slice(&999u16.to_le_bytes());
    // TRIGGER sorts after every other critical tag written, so appending keeps
    // order
    bytes.extend_from_slice(&rec);
    assert!(matches!(
        meta_codec::decode(&bytes, |_, _| {}),
        Err(FormatError::UnknownCriticalTag { .. })
    ));
}

#[test]
fn metadata_that_is_missing_what_it_must_have_is_refused() {
    for missing in [tag::NAME, tag::VERSION, tag::ARCH] {
        let mut recs = records(&meta_codec::encode(&sample_meta()));
        recs.retain(|(t, _)| *t != missing);
        assert_eq!(
            meta_codec::decode(&assemble(&recs), |_, _| {}).unwrap_err(),
            FormatError::MissingTag { tag: missing },
            "a package with no {} is not a package",
            tag::tag_name(missing).unwrap()
        );
    }
    assert!(meta_codec::decode(&[], |_, _| {}).is_err());
}

/// Out of order is refused, not sorted, since the signature covers the bytes
#[test]
fn records_out_of_order_are_refused() {
    // Critical tags have the high bit and sort after ordinary fields, so a DESC
    // at the end decreases
    let mut recs = records(&meta_codec::encode(&sample_meta()));
    recs.push((tag::DESC, b"x".to_vec()));
    let bytes = assemble(&recs);
    assert!(matches!(
        meta_codec::decode(&bytes, |_, _| {}),
        Err(FormatError::BadRecord { .. })
    ));
}

#[test]
fn a_record_running_past_the_section_is_refused() {
    let mut bytes = meta_codec::encode(&sample_meta());
    // Long enough to overrun, short enough to pass the per-field ceiling: this
    // refusal is about the section
    bytes[2..6].copy_from_slice(&4000u32.to_le_bytes());
    assert!(matches!(
        meta_codec::decode(&bytes, |_, _| {}),
        Err(FormatError::BadRecord { .. })
    ));
}

// What the writer refuses

/// Enforced on the build host
#[test]
fn a_package_with_an_illegal_path_cannot_be_built() {
    let mut b = Builder::new(sample_meta());
    assert!(matches!(
        b.add_file("../etc/passwd", 0o644, 0, 0, b"x"),
        Err(BuildError::Path { why: PathError::BadComponent { .. }, .. })
    ));
    assert!(matches!(
        b.add_file("/etc/passwd", 0o644, 0, 0, b"x"),
        Err(BuildError::Path { why: PathError::Absolute, .. })
    ));
    assert!(matches!(
        b.add_file("etc/mayland/conf.d/mine.conf", 0o644, 0, 0, b"x"),
        Err(BuildError::Path { why: PathError::UserConfig { .. }, .. })
    ));
    // The directory itself is allowed, for the machine's owner to write in
    b.add_dir("etc/mayland/conf.d", 0o755, 0, 0).unwrap();

    // The last refusal explains the rule
    let said = std::format!(
        "{}",
        b.add_file("etc/mayland/conf.d/mine.conf", 0o644, 0, 0, b"x").unwrap_err()
    );
    assert!(said.contains("default.d"), "{said:?}");
}

#[test]
fn two_entries_may_not_claim_one_path() {
    let mut b = Builder::new(sample_meta());
    b.add_file("bin/msh", 0o755, 0, 0, b"a").unwrap();
    assert!(matches!(
        b.add_file("bin/msh", 0o755, 0, 0, b"b"),
        Err(BuildError::Duplicate { .. })
    ));
}

/// The kernel stores 64 bytes of a symlink target on the vnode; longer would
/// truncate
#[test]
fn a_symlink_longer_than_the_kernel_stores_cannot_be_built() {
    let mut b = Builder::new(sample_meta());
    let ok = "a".repeat(MAX_SYMLINK_TARGET);
    b.add_symlink("bin/x", &ok, 0, 0).unwrap();

    let too_long = "a".repeat(MAX_SYMLINK_TARGET + 1);
    let err = b.add_symlink("bin/y", &too_long, 0, 0).unwrap_err();
    assert!(matches!(err, BuildError::SymlinkTooLong { .. }));
    let said = std::format!("{err}");
    assert!(said.contains("vnode") || said.contains("truncated"), "{said:?}");
}
