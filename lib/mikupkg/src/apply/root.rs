//! `trait Root`: the I/O seam of the applier
//!
//! The same applier stages the build's tree through `std::fs`, installs through
//! syscalls, and runs in tests against memory that can fail at step n.
//!
//! Contract:
//!
//! **Ownership is always set explicitly.** `ext2_create_file` writes uid 0 on a
//! new inode whoever asked, so `write_new` takes a uid and gid and applies them
//! before returning.
//!
//! **One file is open at a time.** `MAX_VNODES` is shared by every open file,
//! cached directory, device node, and pipe, so contents are passed whole.
//!
//! **`rename` replaces.** An implementation refusing an existing target cannot
//! be a `Root`.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

/// What a filesystem refused. Every variant means stop
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RootError {
    NotFound { path: String },
    AlreadyExists { path: String },
    PermissionDenied { path: String },
    /// The filesystem refuses writes. The message about `rootrw` is written
    /// where the machine is known
    ReadOnly { path: String },
    NoSpace { path: String },
    NotADirectory { path: String },
    DirectoryNotEmpty { path: String },
    /// Anything else from the platform, as a number a report can quote
    Io { path: String, code: i64 },
}

impl RootError {
    pub fn path(&self) -> &str {
        match self {
            RootError::NotFound { path }
            | RootError::AlreadyExists { path }
            | RootError::PermissionDenied { path }
            | RootError::ReadOnly { path }
            | RootError::NoSpace { path }
            | RootError::NotADirectory { path }
            | RootError::DirectoryNotEmpty { path }
            | RootError::Io { path, .. } => path,
        }
    }
}

impl fmt::Display for RootError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RootError::NotFound { path } => write!(f, "{path}: not there"),
            RootError::AlreadyExists { path } => write!(f, "{path}: already there"),
            RootError::PermissionDenied { path } => write!(f, "{path}: not allowed"),
            RootError::ReadOnly { path } => write!(f, "{path}: the filesystem is read-only"),
            RootError::NoSpace { path } => write!(f, "{path}: no room left"),
            RootError::NotADirectory { path } => write!(f, "{path}: not a directory"),
            RootError::DirectoryNotEmpty { path } => write!(f, "{path}: not empty"),
            RootError::Io { path, code } => write!(f, "{path}: the filesystem said {code}"),
        }
    }
}

pub type RootResult<T> = Result<T, RootError>;

/// What something at a path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    File,
    Directory,
    Symlink,
}

/// Enough about an entry to decide what to do with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stat {
    pub kind: Kind,
    pub mode: u16,
    pub uid: u16,
    pub gid: u16,
    pub size: u64,
}

/// The filesystem a transaction applies to. Paths are relative to the root and
/// use `/`, as in a package; an implementation adds any prefix
pub trait Root {
    /// `None` when nothing is there. Does not follow a final symlink, so a link
    /// cannot redirect a write
    fn stat(&self, path: &str) -> Option<Stat>;

    fn read(&self, path: &str) -> RootResult<Vec<u8>>;

    /// The target of a symlink, as bytes
    fn read_link(&self, path: &str) -> RootResult<String>;

    /// Creates it and any parents if absent; "already there" is normal, since
    /// packages share directories. Undoing a `Created` directory removes the
    /// leaf only
    fn make_dir(&mut self, path: &str, mode: u16, uid: u16, gid: u16) -> RootResult<()>;

    /// Writes a new file, durable before returning, since the applier renames
    /// it over a live name next
    fn write_new(
        &mut self,
        path: &str,
        content: &[u8],
        mode: u16,
        uid: u16,
        gid: u16,
    ) -> RootResult<()>;

    fn symlink(&mut self, path: &str, target: &str, uid: u16, gid: u16) -> RootResult<()>;

    /// Replaces `to` with `from`, atomically
    fn rename(&mut self, from: &str, to: &str) -> RootResult<()>;

    /// Appends to a file, creating it if absent, durable before returning. For
    /// the journal, which must survive a crash and cannot be rewritten whole
    /// per record
    fn append(&mut self, path: &str, bytes: &[u8]) -> RootResult<()>;

    fn remove_file(&mut self, path: &str) -> RootResult<()>;

    fn remove_dir(&mut self, path: &str) -> RootResult<()>;

    /// Bytes free, or `None` for "cannot tell", which the planner reports
    fn free_space(&self) -> Option<u64>;

    /// Pushes everything out, once at a transaction's end. `write_new` handles
    /// per-file durability
    fn sync(&mut self) -> RootResult<()>;

    /// Where a file for `path` is written before the rename: beside its
    /// destination, since a rename is atomic only within one filesystem. The
    /// dot keeps it from `ls`, and recovery looks for the fixed prefix.
    ///
    /// Backups go in a transaction directory instead; beside the file they
    /// would keep `bin/old` from being removed after `bin/old/thing`
    fn staging_path(&self, path: &str) -> String {
        decorate(path, ".mikupkg-", ".new")
    }

}

/// `a/b/c` -> `a/b/<pre>c<post>`
fn decorate(path: &str, pre: &str, post: &str) -> String {
    let (dir, base) = match path.rfind('/') {
        Some(at) => (&path[..at + 1], &path[at + 1..]),
        None => ("", path),
    };
    let mut out = String::with_capacity(dir.len() + pre.len() + base.len() + post.len());
    out.push_str(dir);
    out.push_str(pre);
    out.push_str(base);
    out.push_str(post);
    out
}
