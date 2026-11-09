//! The root half of the installer
//!
//! Reads only:
//!
//! - the plan blob (`plan/encode.rs`: fixed-width lengths and blobs)
//! - each package's metadata and file table, parsed by `format/` with bounds
//!   checks
//! - the keyring, a directory of fixed-size records
//! - staged files, by number, hashed before use
//!
//! Nothing from the unprivileged half is believed. Digests come from the file
//! table, covered by the package's signature, which is checked here against a
//! keyring read here.
//!
//! The database record is one more `Op::File` in the same transaction, so a
//! crash cannot leave files with no owner.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use super::root::{Root, RootError, RootResult};
use super::{tx, Paths};
use crate::db;
use crate::format::db::Installed;
use crate::format::meta as meta_codec;
use crate::format::{manifest_hash, sha256, FileKind, FileTable, FormatError};
use crate::meta::{PackageMeta, Trigger};
use crate::name::Name;
use crate::plan::encode::{Item, StagedPlan, Verb};
use crate::plan::{ContentKey, Op, Plan, Source};
use crate::trust::{self, Keyring, Verdict};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Applied {
    pub installed: Vec<Name>,
    pub removed: Vec<Name>,
    pub placed: usize,
    pub triggers: Vec<Trigger>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StagedError {
    Format { name: Option<Name>, why: FormatError },
    /// Signed by no key known here, or unsigned when that is not allowed
    Untrusted { name: Name, verdict: Verdict },
    /// The plan says one name and the package's own metadata says another
    NameMismatch { said: Name, meta: Name },
    /// Asked to remove something not in the database
    NotInstalled { name: Name },
    Tx(tx::TxError),
    Root(RootError),
}

impl fmt::Display for StagedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StagedError::Format { name: Some(n), why } => write!(f, "{n}: {why}"),
            StagedError::Format { name: None, why } => write!(f, "{why}"),
            StagedError::Untrusted { name, verdict } => write!(f, "{name}: {verdict}"),
            StagedError::NameMismatch { said, meta } => write!(
                f,
                "the plan calls it {said} and the package calls itself {meta} - one of \
                 them is not describing what is on the disk"
            ),
            StagedError::NotInstalled { name } => write!(f, "{name} is not installed"),
            StagedError::Tx(e) => write!(f, "{e}"),
            StagedError::Root(e) => write!(f, "{e}"),
        }
    }
}

impl From<RootError> for StagedError {
    fn from(e: RootError) -> StagedError {
        StagedError::Root(e)
    }
}

/// Bytes to fill an operation: a staged file by number, or a database record
/// computed here
enum Piece {
    Staged { item: usize, file: usize },
    Inline(Vec<u8>),
}

struct StagedSource<'a, Rd: Root + ?Sized> {
    reader: &'a Rd,
    stage_dir: String,
    serial: u64,
    pieces: Vec<Piece>,
}

impl<Rd: Root + ?Sized> Source for StagedSource<'_, Rd> {
    fn content(&self, key: ContentKey) -> RootResult<Vec<u8>> {
        match self.pieces.get(key.file) {
            None => Err(RootError::NotFound { path: alloc::format!("piece {}", key.file) }),
            Some(Piece::Inline(bytes)) => Ok(bytes.clone()),
            Some(Piece::Staged { item, file }) => {
                let path = StagedPlan::staged_at(&self.stage_dir, self.serial, *item, *file);
                self.reader.read(&path)
            }
        }
    }
}

/// Puts a staged plan on the machine. `reader` is a second view of the
/// filesystem for the staging area, since the applier holds the tree mutably
pub fn run<R: Root + ?Sized, Rd: Root + ?Sized>(
    staged: &StagedPlan,
    root: &mut R,
    reader: &Rd,
    keyring: &Keyring,
    paths: &Paths,
    stage_dir: &str,
    allow_unsigned: bool,
    now: u64,
) -> Result<Applied, StagedError> {
    let mut plan = Plan::new(staged.serial);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut applied = Applied::default();

    for (index, item) in staged.items.iter().enumerate() {
        match item.verb {
            Verb::Install => {
                let meta = check_and_read(item, keyring, allow_unsigned, now)?;
                let table = FileTable::parse(
                    &item.table,
                    count_of(&item.table),
                    // Staged files are not in a container, so the extents have
                    // no payload. Paths, kinds, and hashes are still checked
                    u64::MAX,
                )
                .map_err(|why| StagedError::Format { name: Some(item.name), why })?;

                for (file, entry) in table.entries.iter().enumerate() {
                    let op = match entry.kind {
                        FileKind::Directory => Op::Dir {
                            path: entry.path.clone(),
                            mode: entry.mode,
                            uid: entry.uid,
                            gid: entry.gid,
                        },
                        FileKind::Symlink => {
                            // A link's target is its content, and is hashed too
                            let raw = read_staged(reader, stage_dir, staged.serial, index, file)?;
                            if sha256(&raw) != entry.sha256 {
                                return Err(StagedError::Format {
                                    name: Some(item.name),
                                    why: FormatError::WrongHash { index: file },
                                });
                            }
                            Op::Symlink {
                                path: entry.path.clone(),
                                target: String::from_utf8_lossy(&raw).into_owned(),
                                uid: entry.uid,
                                gid: entry.gid,
                            }
                        }
                        FileKind::File => {
                            pieces.push(Piece::Staged { item: index, file });
                            Op::File {
                                path: entry.path.clone(),
                                mode: entry.mode,
                                uid: entry.uid,
                                gid: entry.gid,
                                key: ContentKey { package: index, file: pieces.len() - 1 },
                                size: entry.size as u64,
                                sha256: entry.sha256,
                            }
                        }
                    };
                    plan.ops.push(op);
                }

                // In the same transaction as the files, so a crash cannot leave
                // files with no owner
                let record = Installed {
                    meta: item.meta.clone(),
                    files: table,
                    reason: item.reason,
                    install_time: now,
                    container_sha256: item.container_sha256,
                };
                let bytes = record.encode();
                pieces.push(Piece::Inline(bytes.clone()));
                plan.ops.push(Op::File {
                    path: db::record_path(&paths.db, &item.name),
                    mode: 0o644,
                    uid: 0,
                    gid: 0,
                    key: ContentKey { package: index, file: pieces.len() - 1 },
                    size: bytes.len() as u64,
                    sha256: sha256(&bytes),
                });

                plan.triggers.extend(meta.triggers.iter().copied());
                plan.installs.push(meta);
                applied.installed.push(item.name);
            }

            Verb::Remove => {
                // Owned files come from the machine's record, not the plan
                let record_path = db::record_path(&paths.db, &item.name);
                let raw = root
                    .read(&record_path)
                    .map_err(|_| StagedError::NotInstalled { name: item.name })?;
                let record = Installed::parse(&raw)
                    .map_err(|why| StagedError::Format { name: Some(item.name), why })?;

                for entry in &record.files.entries {
                    plan.ops.push(Op::Remove { path: entry.path.clone() });
                }
                plan.ops.push(Op::Remove { path: record_path });
                applied.removed.push(item.name);
            }
        }
    }

    plan.finish();

    let source = StagedSource {
        reader,
        stage_dir: String::from(stage_dir),
        serial: staged.serial,
        pieces,
    };
    let outcome = tx::run(&plan, root, &source, paths, now).map_err(StagedError::Tx)?;

    applied.placed = outcome.placed;
    applied.triggers = super::trigger::owed(
        plan.installs.iter().map(|m| m.triggers.as_slice()),
    );
    Ok(applied)
}

/// Is this package one the machine will take, and what does it say it is?
fn check_and_read(
    item: &Item,
    keyring: &Keyring,
    allow_unsigned: bool,
    now: u64,
) -> Result<PackageMeta, StagedError> {
    let digest = manifest_hash(&item.meta, &item.table);
    let signature = if item.signature.is_empty() { None } else { Some(item.signature.as_slice()) };
    let verdict = trust::check(&digest, signature, keyring, None, now);

    let ok = match &verdict {
        Verdict::Trusted { .. } => true,
        // Unsigned is legal, as a developer's or fresh clone's build.
        // Installing one is a policy the command's caller decides
        Verdict::Unsigned => allow_unsigned,
        _ => false,
    };
    if !ok {
        return Err(StagedError::Untrusted { name: item.name, verdict });
    }

    let meta = meta_codec::decode(&item.meta, |_, _| {})
        .map_err(|why| StagedError::Format { name: Some(item.name), why })?;
    if meta.name != item.name {
        return Err(StagedError::NameMismatch { said: item.name, meta: meta.name });
    }
    Ok(meta)
}

/// Number of records in a table, from its length. No separate count that could
/// disagree
fn count_of(table: &[u8]) -> usize {
    use crate::format::RECORD_LEN;
    // The path region follows the records, so the length bounds the count and
    // the parse checks every offset
    let mut n = table.len() / RECORD_LEN;
    while n > 0 {
        let paths = table.len() - n * RECORD_LEN;
        // Each record names a path of at least one byte
        if paths >= n {
            return n;
        }
        n -= 1;
    }
    0
}

fn read_staged<Rd: Root + ?Sized>(
    reader: &Rd,
    stage_dir: &str,
    serial: u64,
    item: usize,
    file: usize,
) -> RootResult<Vec<u8>> {
    reader.read(&StagedPlan::staged_at(stage_dir, serial, item, file))
}
