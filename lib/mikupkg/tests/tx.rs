// The transaction when everything works, and what it refuses before starting.
// `crash.rs` covers power loss

use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::root::{Kind, Root, RootError, RootResult};
use mikupkg::apply::{tx, Paths};
use mikupkg::format::sha256;
use mikupkg::meta::{Arch, PackageMeta, Trigger};
use mikupkg::name::Name;
use mikupkg::plan::conflict::{self, NoOwners, Owners};
use mikupkg::plan::space::{self, Space};
use mikupkg::plan::{ContentKey, Op, Plan, Source};
use mikupkg::version::Version;

fn n(s: &str) -> Name {
    Name::parse(s).unwrap()
}
fn v(s: &str) -> Version {
    Version::parse(s).unwrap()
}

/// Bytes by package and index, as an opened container on the host or a staging
/// directory on the machine
#[derive(Default)]
struct Bytes(Vec<Vec<Vec<u8>>>);

impl Source for Bytes {
    fn content(&self, key: ContentKey) -> RootResult<Vec<u8>> {
        self.0
            .get(key.package)
            .and_then(|p| p.get(key.file))
            .cloned()
            .ok_or(RootError::NotFound { path: "content".into() })
    }
}

fn file_op(path: &str, body: &[u8], pkg: usize, idx: usize, mode: u16) -> Op {
    Op::File {
        path: path.into(),
        mode,
        uid: 0,
        gid: 0,
        key: ContentKey { package: pkg, file: idx },
        size: body.len() as u64,
        sha256: sha256(body),
    }
}

/// A root with `/pkg` already on it, as the image ships
fn root_with_pkg() -> MemRoot {
    let mut r = MemRoot::new();
    r.preset_dir("pkg", 0o755);
    r.preset_dir("bin", 0o755);
    r
}

fn simple_plan() -> (Plan, Bytes) {
    let prog = b"a program".to_vec();
    let conf = b"font = 12\n".to_vec();

    let mut plan = Plan::new(1);
    plan.ops.push(Op::Dir { path: "etc".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.ops.push(file_op("bin/mayterm", &prog, 0, 0, 0o755));
    plan.ops.push(file_op("etc/term.conf", &conf, 0, 1, 0o644));
    plan.ops.push(Op::Symlink {
        path: "bin/term".into(),
        target: "mayterm".into(),
        uid: 0,
        gid: 0,
    });
    let mut meta = PackageMeta::new(n("mayterm"), v("0.2.10"), Arch::X86_64);
    meta.triggers.push(Trigger::Services);
    plan.installs.push(meta);
    plan.triggers.push(Trigger::Services);
    plan.finish();

    (plan, Bytes(vec![vec![prog, conf]]))
}

#[test]
fn a_fresh_install_puts_everything_where_it_belongs() {
    let (plan, bytes) = simple_plan();
    let mut root = root_with_pkg();

    let out = tx::run(&plan, &mut root, &bytes, &Paths::default(), 1_780_000_000)
        .expect("it should install");

    assert_eq!(out.placed, 3);
    assert_eq!(out.directories, 1);
    assert_eq!(out.triggers, vec![Trigger::Services]);

    assert_eq!(root.read("bin/mayterm").unwrap(), b"a program");
    assert_eq!(root.read("etc/term.conf").unwrap(), b"font = 12\n");
    assert_eq!(root.read_link("bin/term").unwrap(), "mayterm");
    assert_eq!(root.stat("bin/mayterm").unwrap().mode, 0o755);
    assert_eq!(root.stat("etc/term.conf").unwrap().mode, 0o644);
    assert_eq!(root.stat("etc").unwrap().kind, Kind::Directory);
}

/// Nothing of the transaction's own stays: no `.new` or `.old` in `/bin` after
/// a successful install
#[test]
fn it_leaves_no_working_files_behind() {
    let (plan, bytes) = simple_plan();
    let mut root = root_with_pkg();
    tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap();

    let strays: Vec<&str> = root
        .paths()
        .into_iter()
        .filter(|p| p.contains(".mikupkg-"))
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

#[test]
fn an_upgrade_replaces_what_is_there() {
    let mut root = root_with_pkg();
    root.preset_file("bin/mayterm", b"version one", 0o755, 0, 0);

    let body = b"version two".to_vec();
    let mut plan = Plan::new(2);
    plan.ops.push(file_op("bin/mayterm", &body, 0, 0, 0o755));
    plan.finish();

    tx::run(&plan, &mut root, &Bytes(vec![vec![body]]), &Paths::default(), 0).unwrap();
    assert_eq!(root.read("bin/mayterm").unwrap(), b"version two");
}

#[test]
fn a_removal_takes_things_away_deepest_first() {
    let mut root = root_with_pkg();
    root.preset_dir("bin/old", 0o755);
    root.preset_file("bin/old/thing", b"x", 0o644, 0, 0);

    let mut plan = Plan::new(3);
    plan.ops.push(Op::Remove { path: "bin/old".into() });
    plan.ops.push(Op::Remove { path: "bin/old/thing".into() });
    plan.finish();

    // The deeper path must come first, or the directory cannot go
    assert_eq!(plan.ops[0].path(), "bin/old/thing");

    let out = tx::run(&plan, &mut root, &Bytes::default(), &Paths::default(), 0).unwrap();
    assert_eq!(out.removed, 2);
    assert!(root.stat("bin/old/thing").is_none());
    assert!(root.stat("bin/old").is_none());
}

/// Directories before their contents, removals after everything
#[test]
fn the_order_is_one_a_filesystem_can_carry_out() {
    let mut plan = Plan::new(1);
    plan.ops.push(Op::Remove { path: "bin/gone".into() });
    plan.ops.push(file_op("bin/b/deep", b"x", 0, 0, 0o644));
    plan.ops.push(Op::Dir { path: "bin/b".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.ops.push(file_op("bin/a", b"x", 0, 0, 0o644));
    plan.ops.push(Op::Dir { path: "bin".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.finish();

    let order: Vec<&str> = plan.ops.iter().map(|o| o.path()).collect();
    assert_eq!(order, vec!["bin", "bin/b", "bin/a", "bin/b/deep", "bin/gone"]);
}

/// A directory several packages declare is normal
#[test]
fn a_directory_that_is_already_there_is_not_a_problem() {
    let mut root = root_with_pkg();
    let mut plan = Plan::new(1);
    plan.ops.push(Op::Dir { path: "bin".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.finish();
    let out = tx::run(&plan, &mut root, &Bytes::default(), &Paths::default(), 0).unwrap();
    assert_eq!(out.directories, 0, "it was already there");
}

// What it refuses before writing anything

/// The root half rechecks every file's hash
#[test]
fn bytes_that_do_not_match_the_plan_are_refused_and_nothing_is_written() {
    let mut root = root_with_pkg();
    let before = root.snapshot();

    let mut plan = Plan::new(1);
    plan.ops.push(file_op("bin/mayterm", b"what the plan says", 0, 0, 0o755));
    plan.finish();

    // ...and the source hands over something else
    let err = tx::run(
        &plan,
        &mut root,
        &Bytes(vec![vec![b"what the source has".to_vec()]]),
        &Paths::default(),
        0,
    )
    .unwrap_err();

    assert!(
        matches!(err, tx::TxError::Undone { why: tx::Why::WrongContent { .. }, .. }),
        "{err}"
    );
    assert!(root.stat("bin/mayterm").is_none());
    // The journal directory is the only thing that may have appeared
    let after: Vec<_> = root
        .snapshot()
        .into_iter()
        .filter(|(p, ..)| !p.starts_with("pkg/journal"))
        .collect();
    assert_eq!(after, before, "nothing else may have changed");
}

#[test]
fn a_transaction_that_was_interrupted_blocks_the_next_one() {
    let (plan, bytes) = simple_plan();
    let mut root = root_with_pkg();

    // Stop halfway, leaving an uncommitted journal
    root.crash_after(4);
    let _ = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0);
    root.power_on();

    let err = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap_err();
    assert!(matches!(err, tx::TxError::Unfinished), "{err}");
    assert!(
        std::format!("{err}").contains("tx recover"),
        "the refusal must say what to do: {err}"
    );
}

#[test]
fn a_transaction_that_will_not_fit_is_refused_before_it_starts() {
    let (plan, bytes) = simple_plan();
    let mut root = root_with_pkg();
    root.set_free_space(Some(16));

    let err = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap_err();
    assert!(matches!(err, tx::TxError::NoSpace(Space::Short { .. })), "{err}");
    assert!(
        std::format!("{err}").contains("Nothing has been downloaded"),
        "{err}"
    );
}

/// A filesystem not reporting its room does not count as having room
#[test]
fn a_filesystem_that_will_not_say_is_reported_not_assumed() {
    let (plan, _) = simple_plan();
    let mut root = root_with_pkg();
    root.set_free_space(None);
    assert!(matches!(
        space::check(&plan, &root),
        Space::Unknown { .. }
    ));
}

#[test]
fn a_file_where_a_directory_is_cannot_be_installed() {
    let mut root = root_with_pkg();
    root.preset_dir("bin/mayterm", 0o755);

    let body = b"x".to_vec();
    let mut plan = Plan::new(1);
    plan.ops.push(file_op("bin/mayterm", &body, 0, 0, 0o755));
    plan.finish();

    let err = tx::run(&plan, &mut root, &Bytes(vec![vec![body]]), &Paths::default(), 0)
        .unwrap_err();
    assert!(
        matches!(err, tx::TxError::Undone { why: tx::Why::Impossible { .. }, .. }),
        "{err}"
    );
}

// Conflicts

struct Table(Vec<(&'static str, &'static str)>);

impl Owners for Table {
    fn owner_of(&self, path: &str) -> Option<Name> {
        self.0
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, o)| Name::parse(o).unwrap())
    }
}

#[test]
fn two_packages_may_not_claim_one_path() {
    let mut plan = Plan::new(1);
    plan.ops.push(file_op("bin/msh", b"a", 0, 0, 0o755));
    plan.ops.push(file_op("bin/msh", b"b", 1, 0, 0o755));
    plan.finish();

    let found = conflict::check(&plan, &NoOwners, |_| false);
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0], conflict::Conflict::WithinPlan { .. }));
}

#[test]
fn a_path_somebody_else_owns_is_a_conflict_and_the_message_says_who() {
    let mut plan = Plan::new(1);
    plan.ops.push(file_op("bin/msh", b"a", 0, 0, 0o755));
    plan.installs.push(PackageMeta::new(n("miku-system"), v("1.0"), Arch::X86_64));
    plan.finish();

    let owners = Table(vec![("bin/msh", "miku-base")]);
    let found = conflict::check(&plan, &owners, |_| true);
    assert_eq!(found.len(), 1);
    let said = std::format!("{}", found[0]);
    assert!(said.contains("miku-base"), "{said}");

    // Upgrading the package that owns it is not a conflict
    let mut upgrade = Plan::new(2);
    upgrade.ops.push(file_op("bin/msh", b"a", 0, 0, 0o755));
    upgrade.installs.push(PackageMeta::new(n("miku-base"), v("2.0"), Arch::X86_64));
    upgrade.finish();
    assert!(conflict::check(&upgrade, &owners, |_| true).is_empty());
}

/// A file placed by hand must not be overwritten silently
#[test]
fn a_file_no_package_owns_is_not_quietly_overwritten() {
    let mut plan = Plan::new(1);
    plan.ops.push(file_op("etc/mine.conf", b"a", 0, 0, 0o644));
    plan.finish();

    let found = conflict::check(&plan, &NoOwners, |p| p == "etc/mine.conf");
    assert_eq!(found.len(), 1);
    assert!(matches!(found[0], conflict::Conflict::Unowned { .. }));

    // ...but a path with nothing at it is fine
    assert!(conflict::check(&plan, &NoOwners, |_| false).is_empty());
}

/// `replaces` is a transition on the installed set, not a solver constraint
#[test]
fn replaces_is_worked_out_against_what_is_installed() {
    let mut new = PackageMeta::new(n("mayterm"), v("1.0"), Arch::X86_64);
    new.replaces.push(mikupkg::meta::dep::Dep::parse("mayconsole<2.0").unwrap());

    let taken = conflict::replaced_by(&[new.clone()], |name| {
        if name.as_str() == "mayconsole" {
            Some(v("1.5"))
        } else {
            None
        }
    });
    assert_eq!(taken.len(), 1);
    assert_eq!(taken[0].as_str(), "mayconsole");

    // A version outside the declared range is not replaced
    let untouched = conflict::replaced_by(&[new], |name| {
        if name.as_str() == "mayconsole" {
            Some(v("2.5"))
        } else {
            None
        }
    });
    assert!(untouched.is_empty());
}

// Triggers

#[test]
fn triggers_are_collected_once_and_in_a_fixed_order() {
    use mikupkg::apply::trigger;

    let a = [Trigger::Services, Trigger::Ldconfig];
    let b = [Trigger::Ldconfig];
    let owed = trigger::owed([&a[..], &b[..]].into_iter());
    assert_eq!(
        owed,
        vec![Trigger::Ldconfig, Trigger::Services],
        "declared order must not decide what the machine does"
    );

    // ...and the other declaration order gives the same answer
    let owed2 = trigger::owed([&b[..], &a[..]].into_iter());
    assert_eq!(owed, owed2);
}
