// Power loss during an install
//
// Fails at operation n for every n in the transaction and asserts that recovery
// restores the tree. The crash lands after the operation, with the write on
// disk and nothing recording it. Sweeping n also covers "before operation n",
// which is "after operation n-1".

use mikupkg::apply::mem::MemRoot;
use mikupkg::apply::recover::{self, Recovery};
use mikupkg::apply::root::{Root, RootError, RootResult};
use mikupkg::apply::{tx, Paths};
use mikupkg::format::journal::{Journal, Record};
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

fn file_op(path: &str, body: &[u8], idx: usize) -> Op {
    Op::File {
        path: path.into(),
        mode: 0o755,
        uid: 0,
        gid: 0,
        key: ContentKey { package: 0, file: idx },
        size: body.len() as u64,
        sha256: sha256(body),
    }
}

/// A machine with a system on it: two programs, a config, a directory to be
/// emptied, and a symlink, covering every operation kind in one transaction
fn before() -> MemRoot {
    let mut r = MemRoot::new();
    r.preset_dir("pkg", 0o755);
    r.preset_dir("bin", 0o755);
    r.preset_dir("etc", 0o755);
    r.preset_file("bin/msh", b"the old shell", 0o755, 0, 0);
    r.preset_file("etc/keep.conf", b"do not touch", 0o644, 0, 0);
    r.preset_dir("bin/legacy", 0o755);
    r.preset_file("bin/legacy/tool", b"an old tool", 0o755, 0, 0);
    r
}

/// An upgrade with every shape: a replacement, new file, new directory,
/// symlink, and a removal emptying a directory
fn upgrade() -> (Plan, Bytes) {
    let shell = b"the new shell".to_vec();
    let term = b"a terminal".to_vec();
    let conf = b"font = 14\n".to_vec();

    let mut plan = Plan::new(7);
    plan.ops.push(Op::Dir { path: "etc/mayland".into(), mode: 0o755, uid: 0, gid: 0 });
    plan.ops.push(file_op("bin/msh", &shell, 0));
    plan.ops.push(file_op("bin/mayterm", &term, 1));
    plan.ops.push(file_op("etc/mayland/term.conf", &conf, 2));
    plan.ops.push(Op::Symlink {
        path: "bin/term".into(),
        target: "mayterm".into(),
        uid: 0,
        gid: 0,
    });
    plan.ops.push(Op::Remove { path: "bin/legacy/tool".into() });
    plan.ops.push(Op::Remove { path: "bin/legacy".into() });
    plan.finish();

    (plan, Bytes(vec![shell, term, conf]))
}

/// The tree, excluding the package manager's state, since the journal is meant
/// to survive
fn tree(root: &MemRoot) -> Vec<(String, mikupkg::apply::Kind, u16, u16, u16, Vec<u8>)> {
    root.snapshot()
        .into_iter()
        .filter(|(p, ..)| !p.starts_with("pkg"))
        .collect()
}

/// How many mutating calls the transaction makes when nothing goes wrong
fn call_count() -> u64 {
    let (plan, bytes) = upgrade();
    let mut root = before();
    tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).expect("the clean run should work");
    root.calls()
}

// The sweep

#[test]
fn a_crash_at_any_point_recovers_to_the_state_before() {
    let want = tree(&before());
    let total = call_count();
    assert!(total > 10, "the transaction should be long enough to be worth sweeping: {total}");

    for n in 1..=total {
        let (plan, bytes) = upgrade();
        let mut root = before();

        root.crash_after(n);
        let result = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0);
        if !root.did_crash() {
            // Past the end of the transaction: it finished first
            assert!(result.is_ok(), "crash {n}: {:?}", result.err());
            continue;
        }

        root.power_on();
        let outcome = recover::recover(&mut root, &Paths::default())
            .unwrap_or_else(|e| panic!("crash {n}: recovery itself failed: {e}"));

        match outcome {
            Recovery::RolledBack { .. } | Recovery::Nothing => {
                assert_eq!(
                    tree(&root),
                    want,
                    "crash after call {n} ({}): the tree was not put back.\n\
                     recovery said: {outcome}\n\
                     what the applier did: {:?}",
                    root.log.get(n as usize - 1).cloned().unwrap_or_default(),
                    root.log
                );
            }
            Recovery::Cleaned { .. } => {
                // Committed before the crash: the new state stands and working
                // files are gone
                assert_eq!(root.read("bin/msh").unwrap(), b"the new shell");
                assert!(
                    !root.paths().iter().any(|p| p.contains(".mikupkg-")),
                    "crash {n}: working files left behind"
                );
            }
        }
    }
}

/// Recovery can be interrupted too, so undoing must be idempotent
#[test]
fn recovery_can_itself_be_interrupted_and_run_again() {
    let want = tree(&before());
    let total = call_count();

    for n in 1..=total {
        let (plan, bytes) = upgrade();
        let mut root = before();
        root.crash_after(n);
        let _ = tx::run(&plan, &mut root, &bytes, &Paths::default(), 0);
        if !root.did_crash() {
            continue;
        }
        root.power_on();

        // Dies part way through restoring, then retries
        for kill in 1..=3u64 {
            root.crash_after(kill);
            let _ = recover::recover(&mut root, &Paths::default());
            root.power_on();
        }
        let outcome = recover::recover(&mut root, &Paths::default()).unwrap();

        if !matches!(outcome, Recovery::Cleaned { .. }) {
            assert_eq!(
                tree(&root),
                want,
                "crash {n}, then interrupted recovery: the tree was not put back"
            );
        }
    }
}

/// A committed transaction stays committed even if the machine dies during
/// cleanup
#[test]
fn a_committed_transaction_is_never_rolled_back() {
    let (plan, bytes) = upgrade();
    let mut root = before();
    tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap();
    let after = tree(&root);

    // Recovery again, and again, changes nothing
    for _ in 0..3 {
        let outcome = recover::recover(&mut root, &Paths::default()).unwrap();
        assert!(
            matches!(outcome, Recovery::Nothing | Recovery::Cleaned { .. }),
            "{outcome}"
        );
        assert_eq!(tree(&root), after);
    }
}

// The journal's own bytes

/// The last record may be half-written, and must not be undone as though it had
/// happened
#[test]
fn a_half_written_record_is_detected_rather_than_believed() {
    let records = [
        Record::Begin { serial: 1, time: 5, ops: 2 },
        Record::Backup { dest: "bin/msh".into(), backup: "pkg/journal/1/0000".into() },
        Record::Placed { dest: "bin/msh".into() },
    ];
    let mut whole = Vec::new();
    for r in &records {
        whole.extend_from_slice(&r.encode());
    }

    let sound = Journal::parse(&whole);
    assert!(!sound.torn);
    assert_eq!(sound.records.len(), 3);

    // Record ends. A file cut at one is a complete, short journal; cut
    // elsewhere it is torn
    let mut boundaries = alloc_boundaries(&records);
    boundaries.insert(0, 0);

    for cut in 0..=whole.len() {
        let j = Journal::parse(&whole[..cut]);
        for (i, r) in j.records.iter().enumerate() {
            assert_eq!(&records[i], r, "{cut} bytes: record {i} is not what was written");
        }
        let at_boundary = boundaries.contains(&cut);
        assert_eq!(
            j.torn, !at_boundary,
            "{cut} bytes: torn={} and it {} a record boundary",
            j.torn,
            if at_boundary { "is" } else { "is not" }
        );
        if at_boundary {
            let expect = boundaries.iter().position(|b| *b == cut).unwrap();
            assert_eq!(j.records.len(), expect, "{cut} bytes: wrong number of records");
        }
    }
}

/// A changed record fails its four bytes of SHA-256
#[test]
fn a_damaged_record_is_not_read() {
    let rec = Record::Backup { dest: "bin/msh".into(), backup: "pkg/journal/1/0000".into() };
    let good = rec.encode();

    for at in 0..good.len() {
        let mut bad = good.clone();
        bad[at] ^= 0x01;
        let j = Journal::parse(&bad);
        if !j.records.is_empty() {
            assert_ne!(
                j.records[0], rec,
                "byte {at} changed and the record still read back as itself"
            );
        }
    }
}

#[test]
fn a_journal_holds_its_history_and_recovery_reads_only_the_last_transaction() {
    let (plan, bytes) = upgrade();
    let mut root = before();
    tx::run(&plan, &mut root, &bytes, &Paths::default(), 0).unwrap();

    let raw = root.read(&Paths::default().journal).unwrap();
    let j = Journal::parse(&raw);
    assert!(j.committed(), "the finished transaction is in there");
    assert!(
        matches!(j.last_transaction().first(), Some(Record::Begin { serial: 7, .. })),
        "and the last transaction starts at its own Begin"
    );

    // A second transaction appends, so `history` has something to show
    let body = b"third".to_vec();
    let mut second = Plan::new(8);
    second.ops.push(file_op("bin/msh", &body, 0));
    second.finish();
    tx::run(&second, &mut root, &Bytes(vec![body]), &Paths::default(), 0).unwrap();

    let raw = root.read(&Paths::default().journal).unwrap();
    let j = Journal::parse(&raw);
    assert!(
        j.records.iter().filter(|r| matches!(r, Record::Begin { .. })).count() >= 2,
        "both transactions should be in the log"
    );
    assert!(matches!(
        j.last_transaction().first(),
        Some(Record::Begin { serial: 8, .. })
    ));
}

/// Cumulative record ends, for the test above
fn alloc_boundaries(records: &[Record]) -> Vec<usize> {
    let mut at = 0usize;
    let mut out = Vec::new();
    for r in records {
        at += r.encode().len();
        out.push(at);
    }
    out
}
