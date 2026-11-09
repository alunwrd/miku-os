//! The installed set: what is here, why, and who owns each file
//!
//! A directory of records, one file per package, read and written through
//! `Root`. The same code answers `mikupkg list` on a running machine and writes
//! the database into the staged image, so the first `mikupkg upgrade` has
//! something to upgrade from.

use alloc::string::String;
use alloc::vec::Vec;

use crate::apply::root::{Root, RootError, RootResult};
use crate::format::db::{Installed, Reason};
use crate::format::{meta as meta_codec, FormatError};
use crate::meta::PackageMeta;
use crate::name::Name;
use crate::plan::conflict::Owners;

/// Record file name: the name plus an extension
pub fn record_path(dir: &str, name: &Name) -> String {
    alloc::format!("{dir}/{name}.mpkr")
}

/// A four-byte file beside the records, so a future layout is detected
pub const FORMAT_FILE: &str = "FORMAT";
pub const FORMAT_VERSION: &[u8] = b"1\n";

/// The database, borrowed from whatever filesystem it lives on.
pub struct Db<'a, R: Root + ?Sized> {
    root: &'a R,
    dir: String,
}

impl<'a, R: Root + ?Sized> Db<'a, R> {
    pub fn open(root: &'a R, dir: &str) -> Db<'a, R> {
        Db { root, dir: String::from(dir) }
    }

    /// Every installed package, by name. The caller supplies the listing, since
    /// `Root` has no `read_dir`
    pub fn read(&self, names: &[Name]) -> Vec<(Name, Installed)> {
        let mut out = Vec::new();
        for name in names {
            if let Ok(rec) = self.get(name) {
                out.push((*name, rec));
            }
        }
        out
    }

    pub fn get(&self, name: &Name) -> Result<Installed, DbError> {
        let raw = self
            .root
            .read(&record_path(&self.dir, name))
            .map_err(DbError::Root)?;
        Installed::parse(&raw).map_err(DbError::Format)
    }

    pub fn meta(&self, name: &Name) -> Result<PackageMeta, DbError> {
        let rec = self.get(name)?;
        meta_codec::decode(&rec.meta, |_, _| {}).map_err(DbError::Format)
    }

    pub fn is_installed(&self, name: &Name) -> bool {
        self.root.stat(&record_path(&self.dir, name)).is_some()
    }
}

/// Free function, since writing needs the filesystem mutably
pub fn write<R: Root + ?Sized>(
    root: &mut R,
    dir: &str,
    name: &Name,
    rec: &Installed,
) -> RootResult<()> {
    let path = record_path(dir, name);
    let staged = root.staging_path(&path);
    let bytes = rec.encode();
    // Written beside it, made durable, then renamed over
    root.write_new(&staged, &bytes, 0o644, 0, 0)?;
    root.rename(&staged, &path)
}

pub fn forget<R: Root + ?Sized>(root: &mut R, dir: &str, name: &Name) -> RootResult<()> {
    let path = record_path(dir, name);
    match root.remove_file(&path) {
        Ok(()) => Ok(()),
        Err(RootError::NotFound { .. }) => Ok(()),
        Err(e) => Err(e),
    }
}

/// Lays out an empty database
pub fn create<R: Root + ?Sized>(root: &mut R, dir: &str) -> RootResult<()> {
    // Parent first: `pkg` before `pkg/db`
    if let Some(at) = dir.rfind('/') {
        root.make_dir(&dir[..at], 0o755, 0, 0)?;
    }
    root.make_dir(dir, 0o755, 0, 0)?;
    root.write_new(
        &alloc::format!("{dir}/{FORMAT_FILE}"),
        FORMAT_VERSION,
        0o644,
        0,
        0,
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DbError {
    Root(RootError),
    Format(FormatError),
}

impl core::fmt::Display for DbError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            DbError::Root(e) => write!(f, "{e}"),
            DbError::Format(e) => write!(f, "the record is not one: {e}"),
        }
    }
}

/// Who owns a path, by walking the records. Linear, see the note at the top of
/// `format/db.rs`
pub struct OwnerIndex {
    entries: Vec<(String, Name)>,
}

impl OwnerIndex {
    /// Built once per transaction from records the caller has read
    pub fn build(records: &[(Name, Installed)]) -> OwnerIndex {
        let mut entries = Vec::new();
        for (name, rec) in records {
            for f in &rec.files.entries {
                entries.push((f.path.clone(), *name));
            }
        }
        OwnerIndex { entries }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Owners for OwnerIndex {
    fn owner_of(&self, path: &str) -> Option<Name> {
        self.entries.iter().find(|(p, _)| p == path).map(|(_, n)| *n)
    }
}

pub use crate::format::db::Reason as InstallReason;
const _: () = {
    // `Reason` is re-exported from `format`; this line keeps the names in step
    let _ = Reason::Explicit;
};
