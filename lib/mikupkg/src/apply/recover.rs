//! Reads a journal and restores the tree
//!
//! Recovery rolls back, never forward. Rolling forward needs bytes that may
//! have left the cache and a repository still serving the same files; rolling
//! back ends with the state the system last agreed to.
//!
//! Undoing is idempotent, since a restored backup is gone. Recovery can itself
//! be interrupted and rerun.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::root::{Root, RootError, RootResult};
use super::Paths;
use crate::format::journal::{Journal, Record};

/// What recovery found and did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recovery {
    /// No journal, or nothing unfinished
    Nothing,
    /// The last transaction had not committed and was undone
    RolledBack { undone: usize, torn: bool },
    /// The last transaction had committed; its working files were removed
    Cleaned { swept: usize },
}

impl fmt::Display for Recovery {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Recovery::Nothing => write!(f, "nothing to recover"),
            Recovery::RolledBack { undone, torn } => write!(
                f,
                "a transaction was interrupted{} and has been undone - {undone} \
                 path(s) put back",
                if torn { " while it was writing its own log" } else { "" }
            ),
            Recovery::Cleaned { swept } => {
                write!(f, "the last transaction had finished; {swept} working file(s) swept up")
            }
        }
    }
}

/// Reads the journal and undoes the last transaction if it did not commit
pub fn recover<R: Root + ?Sized>(root: &mut R, paths: &Paths) -> RootResult<Recovery> {
    let raw = match root.read(&paths.journal) {
        Ok(b) => b,
        Err(RootError::NotFound { .. }) => return Ok(Recovery::Nothing),
        Err(e) => return Err(e),
    };
    let journal = Journal::parse(&raw);
    if journal.records.is_empty() {
        return Ok(Recovery::Nothing);
    }

    let last = journal.last_transaction().to_vec();
    if journal.committed() {
        let swept = sweep(root, paths, &last)?;
        return Ok(Recovery::Cleaned { swept });
    }

    let undone = roll_back(root, &last)?;
    drop_backup_dir(root, paths, &last)?;
    // Keeps everything before the interrupted transaction
    let keep = journal.records.len() - last.len();
    rewrite(root, paths, &journal.records[..keep])?;
    Ok(Recovery::RolledBack { undone, torn: journal.torn })
}

/// Undoes one transaction's records, latest first. Within a path the records
/// are `Backup` then `Placed`, and the backup returns after anything touching
/// the destination
pub fn roll_back<R: Root + ?Sized>(root: &mut R, records: &[Record]) -> RootResult<usize> {
    let mut undone = 0usize;
    for rec in records.iter().rev() {
        match rec {
            Record::Staged { staged, .. } => {
                // A staged file is the transaction's scratch; nothing outside
                // points at it
                if root.stat(staged).is_some() {
                    root.remove_file(staged)?;
                }
            }
            Record::Backup { dest, backup } | Record::Removed { dest, backup } => {
                if root.stat(backup).is_some() {
                    // A rename, so the destination is never briefly absent
                    root.rename(backup, dest)?;
                    undone += 1;
                }
            }
            Record::Created { dest } => {
                // Nothing was there before, so it is removed. A directory
                // created here stays if something else is now in it
                if let Some(st) = root.stat(dest) {
                    let r = match st.kind {
                        super::Kind::Directory => root.remove_dir(dest),
                        _ => root.remove_file(dest),
                    };
                    match r {
                        Ok(()) => undone += 1,
                        Err(RootError::DirectoryNotEmpty { .. }) => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Record::RemovedDir { dest, mode, uid, gid } => {
                if root.stat(dest).is_none() {
                    root.make_dir(dest, *mode, *uid, *gid)?;
                    undone += 1;
                }
            }
            Record::Placed { .. } | Record::Begin { .. } | Record::Commit { .. } => {}
        }
    }
    Ok(undone)
}

/// A committed transaction leaves backups and unrenamed staging files, both
/// safe to delete
fn sweep<R: Root + ?Sized>(
    root: &mut R,
    paths: &Paths,
    records: &[Record],
) -> RootResult<usize> {
    let mut swept = 0usize;
    for rec in records {
        let path: Option<&String> = match rec {
            Record::Staged { staged, .. } => Some(staged),
            Record::Backup { backup, .. } | Record::Removed { backup, .. } => Some(backup),
            _ => None,
        };
        if let Some(p) = path {
            if root.stat(p).is_some() {
                root.remove_file(p)?;
                swept += 1;
            }
        }
    }
    drop_backup_dir(root, paths, records)?;
    Ok(swept)
}

/// Removes the transaction's backup directory. Derived from the `Begin` record,
/// so it works on a journal from an older run
fn drop_backup_dir<R: Root + ?Sized>(
    root: &mut R,
    paths: &Paths,
    records: &[Record],
) -> RootResult<()> {
    let serial = match records.first() {
        Some(Record::Begin { serial, .. }) => *serial,
        _ => return Ok(()),
    };
    let dir = alloc::format!("{}/{}", paths.journal_dir, serial);
    match root.remove_dir(&dir) {
        Ok(()) => Ok(()),
        Err(RootError::NotFound { .. }) => Ok(()),
        // Still has something in it, so something was not restored; the journal
        // names it
        Err(RootError::DirectoryNotEmpty { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

fn rewrite<R: Root + ?Sized>(
    root: &mut R,
    paths: &Paths,
    keep: &[Record],
) -> RootResult<()> {
    let mut bytes = Vec::new();
    for r in keep {
        bytes.extend_from_slice(&r.encode());
    }
    root.write_new(&paths.journal, &bytes, 0o600, 0, 0)
}
