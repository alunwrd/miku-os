//! The transaction: stage, record, place, commit
//!
//! Per file:
//!
//! 1. Write the new bytes beside the destination and make them durable. A
//!    rename is atomic only within one filesystem, and must not reach disk
//!    before the bytes.
//! 2. Copy what is there to a backup and record it, before the rename. After
//!    it, the old content exists nowhere else.
//! 3. Rename over the destination.
//!
//! Every record reaches the journal before its operation, so recovery undoes
//! what the log says may have happened.
//!
//! On failure the transaction undoes itself through `recover::roll_back`, the
//! same code a crash uses.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::recover;
use super::root::{Kind, Root, RootError, RootResult};
use super::Paths;
use crate::format::journal::Record;
use crate::format::sha256;
use crate::meta::Trigger;
use crate::plan::conflict::Conflict;
use crate::plan::space::Space;
use crate::plan::{Op, Plan, Source};

/// What a transaction did.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    pub placed: usize,
    pub removed: usize,
    pub directories: usize,
    /// Owed now that everything is placed; the root half runs them
    pub triggers: Vec<Trigger>,
}

/// What went wrong, separate from what happened to the machine
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Why {
    /// The filesystem refused
    Root(RootError),
    /// The source's bytes are not the ones the plan describes. Every hash is
    /// rechecked here
    WrongContent { path: String },
    /// Cannot be done to this tree, such as a file where a directory is
    Impossible { path: String, why: &'static str },
}

impl fmt::Display for Why {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Why::Root(e) => write!(f, "{e}"),
            Why::WrongContent { path } => write!(
                f,
                "{path}: the bytes offered do not hash to what the plan says they should"
            ),
            Why::Impossible { path, why } => write!(f, "{path}: {why}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TxError {
    /// An interrupted transaction is not yet dealt with. Nothing starts until
    /// it is, and no self-repair happens unasked
    Unfinished,
    /// Refused before anything was written or downloaded
    NoSpace(Space),
    /// Refused before anything was written
    Conflicts(Vec<Conflict>),
    /// Failed part way, and the tree was restored
    Undone { why: Why, undone: usize },
    /// Failed, and restoring failed too. The one outcome needing a person
    Stuck { why: Why, then: RootError },
}

impl fmt::Display for TxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TxError::Unfinished => write!(
                f,
                "a previous transaction was interrupted and has not been dealt with. \
                 Run `mikupkg tx recover` - nothing here repairs itself behind you"
            ),
            TxError::NoSpace(s) => write!(f, "{s}"),
            TxError::Conflicts(c) => {
                for (i, one) in c.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{one}")?;
                }
                Ok(())
            }
            TxError::Undone { why, undone } => write!(
                f,
                "{why}. The transaction was undone - {undone} path(s) put back, and the \
                 machine is as it was"
            ),
            TxError::Stuck { why, then } => write!(
                f,
                "{why} - and undoing it failed too: {then}. The machine is part way \
                 through a transaction and needs looking at"
            ),
        }
    }
}

impl From<RootError> for TxError {
    fn from(e: RootError) -> TxError {
        TxError::Undone { why: Why::Root(e), undone: 0 }
    }
}

/// Put a plan into effect.
pub fn run<R: Root + ?Sized, S: Source + ?Sized>(
    plan: &Plan,
    root: &mut R,
    source: &S,
    paths: &Paths,
    now: u64,
) -> Result<Outcome, TxError> {
    // Nothing starts on top of something unfinished
    if let Ok(raw) = root.read(&paths.journal) {
        let j = crate::format::journal::Journal::parse(&raw);
        if !j.records.is_empty() && !j.committed() {
            return Err(TxError::Unfinished);
        }
    }

    match crate::plan::space::check(plan, root) {
        Space::Short { .. } => return Err(TxError::NoSpace(crate::plan::space::check(plan, root))),
        _ => {}
    }

    root.make_dir(&paths.journal_dir, 0o755, 0, 0)?;

    let backups = alloc::format!("{}/{}", paths.journal_dir, plan.serial);
    root.make_dir(&backups, 0o700, 0, 0)?;

    let mut tx = Tx {
        records: Vec::with_capacity(plan.ops.len() * 3),
        outcome: Outcome::default(),
        backups,
        next_backup: 0,
    };

    tx.log(
        root,
        paths,
        Record::Begin { serial: plan.serial, time: now, ops: plan.ops.len() as u32 },
    )?;

    for op in &plan.ops {
        if let Err(cause) = tx.one(op, root, source, paths) {
            return Err(tx.unwind(root, paths, cause));
        }
    }

    tx.log(root, paths, Record::Commit { serial: plan.serial })?;

    // Backups done with; removed by the same code a crash after the commit runs
    recover::recover(root, paths)?;
    root.sync()?;

    let mut outcome = tx.outcome;
    outcome.triggers = plan.triggers.clone();
    Ok(outcome)
}

struct Tx {
    records: Vec<Record>,
    outcome: Outcome,
    /// `pkg/journal/<serial>`: everything this transaction might restore, in
    /// one removable directory
    backups: String,
    /// Backups are numbered, since a name from a path would need escaping
    next_backup: u32,
}

impl Tx {
    fn backup_path(&mut self) -> String {
        let p = alloc::format!("{}/{:04}", self.backups, self.next_backup);
        self.next_backup += 1;
        p
    }
}

impl Tx {
    /// Write-ahead: the record reaches the journal first
    fn log<R: Root + ?Sized>(
        &mut self,
        root: &mut R,
        paths: &Paths,
        rec: Record,
    ) -> RootResult<()> {
        root.append(&paths.journal, &rec.encode())?;
        self.records.push(rec);
        Ok(())
    }

    fn one<R: Root + ?Sized, S: Source + ?Sized>(
        &mut self,
        op: &Op,
        root: &mut R,
        source: &S,
        paths: &Paths,
    ) -> Result<(), TxError> {
        match op {
            Op::Dir { path, mode, uid, gid } => {
                match root.stat(path) {
                    Some(st) if st.kind == Kind::Directory => return Ok(()),
                    Some(_) => {
                        return Err(TxError::Undone {
                            why: Why::Impossible {
                                path: path.clone(),
                                why: "a package wants a directory here and something else is",
                            },
                            undone: 0,
                        })
                    }
                    None => {}
                }
                self.log(root, paths, Record::Created { dest: path.clone() })?;
                root.make_dir(path, *mode, *uid, *gid)?;
                self.outcome.directories += 1;
                Ok(())
            }

            Op::File { path, mode, uid, gid, key, size, sha256: want } => {
                let content = source.content(*key)?;
                if content.len() as u64 != *size || sha256(&content) != *want {
                    return Err(TxError::Undone {
                        why: Why::WrongContent { path: path.clone() },
                        undone: 0,
                    });
                }
                let staged = root.staging_path(path);
                self.log(
                    root,
                    paths,
                    Record::Staged { dest: path.clone(), staged: staged.clone() },
                )?;
                root.write_new(&staged, &content, *mode, *uid, *gid)?;
                self.replace_with(&staged, path, root, paths)?;
                self.outcome.placed += 1;
                Ok(())
            }

            Op::Symlink { path, target, uid, gid } => {
                let staged = root.staging_path(path);
                self.log(
                    root,
                    paths,
                    Record::Staged { dest: path.clone(), staged: staged.clone() },
                )?;
                root.symlink(&staged, target, *uid, *gid)?;
                self.replace_with(&staged, path, root, paths)?;
                self.outcome.placed += 1;
                Ok(())
            }

            Op::Remove { path } => {
                let st = match root.stat(path) {
                    None => return Ok(()),
                    Some(st) => st,
                };
                if st.kind == Kind::Directory {
                    // A directory has no contents to keep, and one with
                    // contents is not removable. Records how to make it again
                    self.log(
                        root,
                        paths,
                        Record::RemovedDir {
                            dest: path.clone(),
                            mode: st.mode,
                            uid: st.uid,
                            gid: st.gid,
                        },
                    )?;
                    match root.remove_dir(path) {
                        Ok(()) => {}
                        // Still has contents, so not this package's to remove
                        Err(RootError::DirectoryNotEmpty { .. }) => return Ok(()),
                        Err(e) => return Err(e.into()),
                    }
                } else {
                    let backup = self.backup_path();
                    self.log(
                        root,
                        paths,
                        Record::Removed { dest: path.clone(), backup: backup.clone() },
                    )?;
                    // A rename: the content is not needed twice, and a rename
                    // cannot half-succeed
                    root.rename(path, &backup)?;
                }
                self.outcome.removed += 1;
                Ok(())
            }
        }
    }

    /// Back up whatever is at `dest`, then move `staged` onto it.
    fn replace_with<R: Root + ?Sized>(
        &mut self,
        staged: &str,
        dest: &String,
        root: &mut R,
        paths: &Paths,
    ) -> Result<(), TxError> {
        match root.stat(dest) {
            None => {
                self.log(root, paths, Record::Created { dest: dest.clone() })?;
            }
            Some(st) if st.kind == Kind::Directory => {
                return Err(TxError::Undone {
                    why: Why::Impossible {
                        path: dest.clone(),
                        why: "a package wants a file here and a directory is",
                    },
                    undone: 0,
                })
            }
            Some(st) => {
                let backup = self.backup_path();
                // Before the copy and the rename; after the rename the old
                // content exists nowhere else
                self.log(
                    root,
                    paths,
                    Record::Backup { dest: dest.clone(), backup: backup.clone() },
                )?;
                match st.kind {
                    Kind::File => {
                        let old = root.read(dest)?;
                        root.write_new(&backup, &old, st.mode, st.uid, st.gid)?;
                    }
                    Kind::Symlink => {
                        let target = root.read_link(dest)?;
                        root.symlink(&backup, &target, st.uid, st.gid)?;
                    }
                    Kind::Directory => unreachable!("refused above"),
                }
            }
        }

        self.log(root, paths, Record::Placed { dest: dest.clone() })?;
        root.rename(staged, dest)?;
        Ok(())
    }

    /// Restores the tree with the same code a crash uses and reports which
    /// outcome this is.
    ///
    /// Every failure past the first operation unwinds, not only filesystem
    /// errors: a bad hash on the fourth file follows three placed files
    fn unwind<R: Root + ?Sized>(
        &mut self,
        root: &mut R,
        paths: &Paths,
        cause: TxError,
    ) -> TxError {
        let why = match cause {
            TxError::Undone { why, .. } => why,
            // Refused before the transaction began; nothing to undo
            other => return other,
        };
        match recover::roll_back(root, &self.records) {
            Ok(undone) => {
                // The backups are back, so their directory is empty and goes
                // too
                let _ = recover::recover(root, paths);
                // A failed log cleanup does not make a clean rollback stuck; an
                // uncommitted journal with nothing to undo recovers to a no-op
                let _ = clear(root, paths);
                TxError::Undone { why, undone }
            }
            Err(then) => TxError::Stuck { why, then },
        }
    }
}

fn clear<R: Root + ?Sized>(root: &mut R, paths: &Paths) -> RootResult<()> {
    let raw = root.read(&paths.journal)?;
    let journal = crate::format::journal::Journal::parse(&raw);
    let last = journal.last_transaction().len();
    let keep = journal.records.len() - last;
    let mut bytes = Vec::new();
    for r in &journal.records[..keep] {
        bytes.extend_from_slice(&r.encode());
    }
    root.write_new(&paths.journal, &bytes, 0o600, 0, 0)
}
