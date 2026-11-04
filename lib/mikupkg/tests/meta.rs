// What a package says about itself, including the rule that a package may not
// own a file under `conf.d`

use mikupkg::meta::dep::{Dep, DepError, Provide};
use mikupkg::meta::path::{self, PathError, MAX_DEPTH, MAX_PATH};
use mikupkg::meta::{Arch, PackageMeta, Trigger};
use mikupkg::name::Name;
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

// Dependencies

#[test]
fn a_dependency_reads_the_way_it_is_written() {
    let cases = [
        ("libmay", "libmay"),
        ("libmay>=2.0", "libmay >=2.0"),
        ("libmay>2.0", "libmay >2.0"),
        ("libmay<=2.0", "libmay <=2.0"),
        ("libmay<2.0", "libmay <2.0"),
        ("libmay=2.0", "libmay 2.0"),
        ("libmay>=2.0,<3.0", "libmay >=2.0 <3.0"),
    ];
    for (text, shown) in cases {
        let d = Dep::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        assert_eq!(d.to_string(), shown, "{text:?}");
    }
}

/// `>=` must not read as `>` and a version starting with `=`
#[test]
fn two_character_operators_are_read_as_one() {
    let ge = Dep::parse("libmay>=2.0").unwrap();
    assert!(ge.versions.contains(&v("2.0")), ">= includes its own version");
    let gt = Dep::parse("libmay>2.0").unwrap();
    assert!(!gt.versions.contains(&v("2.0")), "> does not");
}

/// `!=` is not an interval, showing the dependency holds a set
#[test]
fn a_not_equal_leaves_a_hole() {
    let d = Dep::parse("libmay!=2.0").unwrap();
    assert!(d.versions.contains(&v("1.9")));
    assert!(!d.versions.contains(&v("2.0")));
    assert!(d.versions.contains(&v("2.0.1")));
}

#[test]
fn commas_intersect() {
    let d = Dep::parse("libmay>=2.0,<3.0,!=2.5").unwrap();
    assert!(!d.versions.contains(&v("1.9")));
    assert!(d.versions.contains(&v("2.0")));
    assert!(!d.versions.contains(&v("2.5")));
    assert!(d.versions.contains(&v("2.9")));
    assert!(!d.versions.contains(&v("3.0")));
}

/// Constraints that cannot all hold are a package error, reported at parse time
#[test]
fn constraints_that_cannot_all_hold_are_refused() {
    assert_eq!(Dep::parse("libmay>2.0,<1.0"), Err(DepError::Impossible));
}

#[test]
fn what_a_dependency_refuses() {
    assert!(matches!(Dep::parse("libmay>="), Err(DepError::MissingVersion { .. })));
    assert!(matches!(Dep::parse("libmay~2.0"), Err(DepError::Name(_))));
    assert!(matches!(Dep::parse("libmay>=x"), Err(DepError::Version { .. })));
    assert!(matches!(Dep::parse("libmay>=1.0 2.0"), Err(DepError::Version { .. })));
}

#[test]
fn a_provide_carries_its_own_version_or_borrows_one() {
    let bare = Provide::parse("editor").unwrap();
    assert_eq!(bare.version, None);
    assert_eq!(bare.version_or(v("1.4")), v("1.4"), "bare means 'as new as me'");

    let stated = Provide::parse("editor=2.0").unwrap();
    assert_eq!(stated.version_or(v("1.4")), v("2.0"));
    assert_eq!(stated.to_string(), "editor=2.0");
}

/// Being a thing and providing it are one question
#[test]
fn a_package_satisfies_by_name_or_by_provide() {
    let mut vi = PackageMeta::new(n("vi"), v("1.4"), Arch::X86_64);
    vi.provides.push(Provide::parse("editor").unwrap());

    assert!(vi.satisfies(&Dep::parse("vi").unwrap()));
    assert!(vi.satisfies(&Dep::parse("vi>=1.0").unwrap()));
    assert!(!vi.satisfies(&Dep::parse("vi>=2.0").unwrap()));
    assert!(vi.satisfies(&Dep::parse("editor").unwrap()));
    assert!(vi.satisfies(&Dep::parse("editor>=1.4").unwrap()), "the provide stands at 1.4");
    assert!(!vi.satisfies(&Dep::parse("editor>=2.0").unwrap()));
    assert!(!vi.satisfies(&Dep::parse("emacs").unwrap()));
}

// Paths

#[test]
fn the_paths_a_package_will_actually_own() {
    for p in [
        "bin/mayterm",
        "etc/mikupkg/default.d/10-repos.conf",
        "etc/mayland/default.d/50-input.conf",
        "etc/ssl/certs/ca-bundle.pem",
        "a",
    ] {
        path::check(p).unwrap_or_else(|e| panic!("{p:?} should be allowed: {e}"));
    }
}

/// `default.d` is what the build ships; `conf.d` belongs to the machine's
/// owner, and nothing is installed there
#[test]
fn a_package_may_not_own_anything_under_conf_d() {
    for p in [
        "etc/mayland/conf.d/mine.conf",
        "etc/mikupkg/conf.d/repos.conf",
        "conf.d/x",
        "a/b/c/conf.d/d",
    ] {
        assert!(
            matches!(path::check(p), Err(PathError::UserConfig { .. })),
            "{p:?} should be refused"
        );
    }
    // The directory it may ship into passes
    assert!(path::check("etc/mayland/default.d/10-x.conf").is_ok());
}

/// A package may own the `conf.d` directory and nothing inside it, so an image
/// can ship an empty `/etc/mayland/conf.d`
#[test]
fn a_package_may_own_the_conf_d_directory_and_nothing_in_it() {
    assert!(path::check_dir("etc/mayland/conf.d").is_ok());
    assert!(path::check_dir("etc/mikupkg/conf.d").is_ok());

    // A file of that name would sit where the directory belongs
    assert!(matches!(
        path::check("etc/mayland/conf.d"),
        Err(PathError::UserConfig { .. })
    ));
    // Nothing may be inside it, directory or not
    assert!(matches!(
        path::check_dir("etc/mayland/conf.d/sub"),
        Err(PathError::UserConfig { .. })
    ));
    assert!(matches!(
        path::check("etc/mayland/conf.d/README"),
        Err(PathError::UserConfig { .. })
    ));
}

#[test]
fn nothing_escapes_the_root_it_is_installed_into() {
    assert_eq!(path::check("/etc/passwd"), Err(PathError::Absolute));
    assert!(matches!(path::check("../etc/passwd"), Err(PathError::BadComponent { .. })));
    assert!(matches!(path::check("bin/../../etc"), Err(PathError::BadComponent { .. })));
    assert!(matches!(path::check("bin//msh"), Err(PathError::BadComponent { .. })));
    assert!(matches!(path::check("./bin"), Err(PathError::BadComponent { .. })));
    assert!(matches!(path::check("bin\\msh"), Err(PathError::BadByte { .. })));
    assert!(matches!(path::check("bin/m\nsh"), Err(PathError::BadByte { .. })));
}

#[test]
fn the_limits_are_the_filesystems_own() {
    assert_eq!(path::check("").unwrap_err(), PathError::Empty);

    // A name may be as long as the filesystem allows; negi has a 37-byte one
    let long_name = "etc/aosh/negi/modules/common/widgets/MaterialShapeWrappedMaterialSymbol.qml";
    assert_eq!(path::check(long_name).unwrap(), 7);
    let one_name = "a".repeat(MAX_PATH);
    assert_eq!(path::check(&one_name).unwrap(), 1);

    let deep: String = (0..MAX_DEPTH + 1).map(|_| "a/").collect::<String>() + "x";
    assert!(matches!(path::check(&deep), Err(PathError::TooDeep { .. })));

    let wide = std::format!("{}/x", "a".repeat(MAX_PATH));
    assert!(matches!(path::check(&wide), Err(PathError::TooLong { .. })));

    assert_eq!(path::check("a/b/c").unwrap(), 3, "the depth comes back");
}

// Triggers and architecture

/// An unknown trigger is refused by name
#[test]
fn a_trigger_is_one_of_a_known_few() {
    assert_eq!(Trigger::parse("ldconfig"), Some(Trigger::Ldconfig));
    assert_eq!(Trigger::parse("services"), Some(Trigger::Services));
    assert_eq!(Trigger::parse("rm -rf /"), None);
    assert_eq!(Trigger::parse("postinstall.sh"), None);

    for t in Trigger::ALL {
        assert_eq!(Trigger::from_code(t.code()), Some(*t), "codes round-trip");
        assert_eq!(Trigger::parse(t.as_str()), Some(*t), "and so do names");
    }
    assert_eq!(Trigger::from_code(999), None);
}

#[test]
fn an_any_package_installs_anywhere_and_a_foreign_one_does_not() {
    assert!(Arch::X86_64.accepts(Arch::Any));
    assert!(Arch::X86_64.accepts(Arch::X86_64));
    assert!(Arch::Any.accepts(Arch::Any));
    assert!(!Arch::Any.accepts(Arch::X86_64));
    assert_eq!(Arch::parse("x86_64"), Some(Arch::X86_64));
    assert_eq!(Arch::parse("aarch64"), None);
}

/// The wire form must be the inverse of reading. It is written back on rebuild
/// and in the installed database. `!=` needs union in the grammar: its set is
/// two intervals
#[test]
fn a_dependency_survives_being_written_back() {
    for text in [
        "libmay",
        "libmay>=2.0",
        "libmay>2.0",
        "libmay<=2.0",
        "libmay<2.0",
        "libmay=2.0",
        "libmay!=2.0",
        "libmay>=2.0,<3.0",
        "libmay>=2.0,<3.0,!=2.5",
        "libmay>=1.0-rc,<2.0_3",
    ] {
        let once = Dep::parse(text).unwrap_or_else(|e| panic!("{text:?}: {e}"));
        let wire = once.to_wire();
        let twice = Dep::parse(&wire)
            .unwrap_or_else(|e| panic!("{text:?} was written as {wire:?}, which does not read back: {e}"));
        assert_eq!(once, twice, "{text:?} -> {wire:?}");
        assert_eq!(twice.to_wire(), wire, "and writing it again changes nothing");
    }
}

/// A bar unions, a comma intersects; together they spell any set
#[test]
fn bars_union_and_commas_intersect() {
    let d = Dep::parse("libmay<1.0|>=2.0,<3.0").unwrap();
    assert!(d.versions.contains(&v("0.9")));
    assert!(!d.versions.contains(&v("1.5")));
    assert!(d.versions.contains(&v("2.5")));
    assert!(!d.versions.contains(&v("3.0")));

    assert_eq!(
        Dep::parse("libmay!=2.0").unwrap(),
        Dep::parse("libmay<2.0|>2.0").unwrap(),
        "the two spellings are one dependency"
    );
}
