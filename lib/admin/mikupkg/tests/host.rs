// The build host's filesystem. The last test runs one plan against memory and a
// real directory and requires the same tree

use mikupkg::apply::host::{HostRoot, Ownership};
use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::root::{Kind, Root, RootError, RootResult};
use mikupkg::apply::{tx, Paths};
use mikupkg::format::sha256;
use mikupkg::plan::{ContentKey, Op, Plan, Source};

struct Bytes(Vec<Vec<u8>>);

impl Source for Bytes {
    fn content(&self, key: ContentKey) -> RootResult<Vec<u8>> {
        self.0
            .get(key.file)
            .cloned()
            .ok_or(RootError::NotFound { path: "content".into() })
    }
}

/// A directory of our own, removed on the way out
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Scratch {
        let mut p = std::env::temp_dir();
        p.push(std::format!("mikupkg-host-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn file_op(path: &str, body: &[u8], idx: usize, mode: u16) -> Op {
    Op::File {
        path: path.into(),
        mode,
        uid: 0,
        gid: 0,
        key: ContentKey { package: 0, file: idx },
        size: body.len() as u64,
        sha256: sha256(body),
    }
}

fn a_plan() -> (Plan, Bytes) {
    let prog = b"a program".to_vec();
    let conf = b"font = 12\n".to_vec();
    let secret = b"root:!:x\n".to_vec();

    let mut plan = Plan::new(1);
    plan.ops.push(Op::Dir { path: "bin".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.ops.push(Op::Dir { path: "etc".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.ops.push(file_op("bin/mayterm", &prog, 0, 0o755));
    plan.ops.push(file_op("etc/term.conf", &conf, 1, 0o644));
    // A file whose mode matters
    plan.ops.push(file_op("etc/shadow", &secret, 2, 0o640));
    plan.ops.push(Op::Symlink {
        path: "bin/term".into(),
        target: "mayterm".into(),
        uid: 0,
        gid: 0,
    });
    plan.finish();
    (plan, Bytes(vec![prog, conf, secret]))
}

#[test]
fn a_plan_lands_on_a_real_filesystem() {
    let scratch = Scratch::new("install");
    let (plan, bytes) = a_plan();
    let mut root = HostRoot::new(&scratch.0, Ownership::Record);

    let out = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).expect("it should install");
    assert_eq!(out.placed, 4);

    assert_eq!(std::fs::read(scratch.0.join("bin/mayterm")).unwrap(), b"a program");
    assert_eq!(std::fs::read_link(scratch.0.join("bin/term")).unwrap().to_str(), Some("mayterm"));

    use std::os::unix::fs::PermissionsExt;
    let mode = |p: &str| {
        std::fs::metadata(scratch.0.join(p)).unwrap().permissions().mode() & 0o7777
    };
    assert_eq!(mode("bin/mayterm"), 0o755);
    assert_eq!(mode("etc/term.conf"), 0o644);
    assert_eq!(mode("etc/shadow"), 0o640, "the one mode that is the point of the file");

    // Nothing of the transaction's own is left in the tree
    let strays: Vec<_> = walk(&scratch.0)
        .into_iter()
        .filter(|p| p.contains(".mikupkg-"))
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

/// A build runs as a user and cannot chown to root, so ownership is recorded
/// and the image step applies it. The table holds what the packages declared
#[test]
fn ownership_is_recorded_for_the_step_that_can_apply_it() {
    let scratch = Scratch::new("owner");
    let (plan, bytes) = a_plan();
    let mut root = HostRoot::new(&scratch.0, Ownership::Record);
    tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap();

    let wanted = root.wanted();
    assert_eq!(wanted.get("bin/mayterm"), Some(&(0o755, 0, 0)));
    assert_eq!(wanted.get("etc/shadow"), Some(&(0o640, 0, 0)));
    assert_eq!(wanted.get("bin").map(|w| w.0), Some(0o755));
    // The staging name must not stay in the table after its rename
    assert!(
        !wanted.keys().any(|k| k.contains(".mikupkg-")),
        "the table names a file that no longer exists"
    );
}

/// Same plan, same tree in both backends
#[test]
fn the_same_plan_gives_the_same_tree_in_memory_and_on_disk() {
    let scratch = Scratch::new("agree");
    let (plan, bytes) = a_plan();

    let mut disk = HostRoot::new(&scratch.0, Ownership::Record);
    tx::run(&plan, &mut disk, &bytes, &Paths::default(), 0).unwrap();

    let (plan, bytes) = a_plan();
    let mut memory = MemRoot::new();
    tx::run(&plan, &mut memory, &bytes, &Paths::default(), 0).unwrap();

    let mut from_disk: Vec<(String, Kind, Vec<u8>)> = walk(&scratch.0)
        .into_iter()
        .filter(|p| !p.starts_with("pkg"))
        .map(|p| {
            let st = disk.stat(&p).unwrap();
            let body = match st.kind {
                Kind::File => disk.read(&p).unwrap(),
                Kind::Symlink => disk.read_link(&p).unwrap().into_bytes(),
                Kind::Directory => Vec::new(),
            };
            (p, st.kind, body)
        })
        .collect();

    let mut from_memory: Vec<(String, Kind, Vec<u8>)> = memory
        .snapshot()
        .into_iter()
        .filter(|(p, ..)| !p.starts_with("pkg"))
        .map(|(p, kind, _, _, _, data)| (p, kind, data))
        .collect();

    from_disk.sort();
    from_memory.sort();
    assert_eq!(from_disk, from_memory);
}

/// Crash test against a real directory, so recovery is proven beyond the memory
/// backend
#[test]
fn a_failed_transaction_leaves_a_real_directory_as_it_was() {
    let scratch = Scratch::new("undo");
    std::fs::create_dir_all(scratch.0.join("bin")).unwrap();
    std::fs::write(scratch.0.join("bin/mayterm"), b"version one").unwrap();

    let body = b"version two".to_vec();
    let mut plan = Plan::new(1);
    plan.ops.push(file_op("bin/mayterm", &body, 0, 0o755));
    // An operation fails after the first was placed; rollback must remove the
    // new program
    plan.ops.push(file_op("bin/nope", b"not what the source has", 1, 0o644));
    plan.finish();

    let mut root = HostRoot::new(&scratch.0, Ownership::Record);
    let err = tx::run(&plan, &mut root, &Bytes(vec![body]), &Paths::default(), 0).unwrap_err();
    std::eprintln!("refused with: {err}");

    assert_eq!(
        std::fs::read(scratch.0.join("bin/mayterm")).unwrap(),
        b"version one",
        "the old program must still be there"
    );
    let strays: Vec<_> = walk(&scratch.0)
        .into_iter()
        .filter(|p| p.contains(".mikupkg-"))
        .collect();
    assert!(strays.is_empty(), "left behind: {strays:?}");
}

/// Every path under `base`, relative, in no particular order
fn walk(base: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
            out.push(rel);
            if p.is_dir() && !p.is_symlink() {
                stack.push(p);
            }
        }
    }
    out
}
