//! A `Root` over `std::fs`, for the build host. The image is produced by
//! installing packages through the same applier that runs on the machine.
//!
//! A build runs as an ordinary user and cannot chown to root, so ownership is
//! recorded, and the step writing the filesystem image reads the table.

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use super::root::{Kind, Root, RootError, RootResult, Stat};

/// What to do about ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ownership {
    /// Tries to apply; fails if the filesystem refuses. For a root process on a
    /// real machine
    Apply,
    /// Records and continues, for a user build; the image-writing step applies
    /// the table
    Record,
}

pub struct HostRoot {
    base: PathBuf,
    policy: Ownership,
    /// Path to what the package asked for. The builder's `sif` script is
    /// generated from this
    wanted: BTreeMap<String, (u16, u16, u16)>,
}

impl HostRoot {
    pub fn new(base: impl Into<PathBuf>, policy: Ownership) -> HostRoot {
        HostRoot { base: base.into(), policy, wanted: BTreeMap::new() }
    }

    pub fn base(&self) -> &Path {
        &self.base
    }

    /// Mode, uid and gid for every path a package placed, in path order.
    pub fn wanted(&self) -> &BTreeMap<String, (u16, u16, u16)> {
        &self.wanted
    }

    fn real(&self, path: &str) -> PathBuf {
        self.base.join(path)
    }

    fn note(&mut self, path: &str, mode: u16, uid: u16, gid: u16) {
        self.wanted.insert(path.to_string(), (mode, uid, gid));
    }

    fn own(&self, path: &str, uid: u16, gid: u16) -> RootResult<()> {
        if self.policy == Ownership::Record {
            return Ok(());
        }
        // `std` has no chown; a host build never reaches this arm, so it is
        // refused
        if (uid, gid) != (0, 0) {
            return Err(RootError::PermissionDenied { path: path.to_string() });
        }
        Ok(())
    }
}

fn io(path: &str, e: std::io::Error) -> RootError {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => RootError::NotFound { path: path.to_string() },
        AlreadyExists => RootError::AlreadyExists { path: path.to_string() },
        PermissionDenied => RootError::PermissionDenied { path: path.to_string() },
        _ => RootError::Io { path: path.to_string(), code: e.raw_os_error().unwrap_or(-1) as i64 },
    }
}

impl Root for HostRoot {
    fn stat(&self, path: &str) -> Option<Stat> {
        // `symlink_metadata`: an installer following a final symlink would
        // write through a link placed by someone else
        let md = fs::symlink_metadata(self.real(path)).ok()?;
        let kind = if md.file_type().is_symlink() {
            Kind::Symlink
        } else if md.is_dir() {
            Kind::Directory
        } else {
            Kind::File
        };
        Some(Stat {
            kind,
            mode: (md.mode() & 0o7777) as u16,
            uid: md.uid() as u16,
            gid: md.gid() as u16,
            size: md.len(),
        })
    }

    fn read(&self, path: &str) -> RootResult<Vec<u8>> {
        fs::read(self.real(path)).map_err(|e| io(path, e))
    }

    fn read_link(&self, path: &str) -> RootResult<String> {
        let t = fs::read_link(self.real(path)).map_err(|e| io(path, e))?;
        Ok(t.to_string_lossy().into_owned())
    }

    fn make_dir(&mut self, path: &str, mode: u16, uid: u16, gid: u16) -> RootResult<()> {
        let real = self.real(path);
        if real.is_dir() {
            self.note(path, mode, uid, gid);
            return Ok(());
        }
        fs::create_dir_all(&real).map_err(|e| io(path, e))?;
        fs::set_permissions(&real, fs::Permissions::from_mode(mode as u32))
            .map_err(|e| io(path, e))?;
        self.own(path, uid, gid)?;
        self.note(path, mode, uid, gid);
        Ok(())
    }

    fn write_new(
        &mut self,
        path: &str,
        content: &[u8],
        mode: u16,
        uid: u16,
        gid: u16,
    ) -> RootResult<()> {
        let real = self.real(path);
        if let Some(parent) = real.parent() {
            fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        }
        let mut f = fs::File::create(&real).map_err(|e| io(path, e))?;
        f.write_all(content).map_err(|e| io(path, e))?;
        // Durable before the rename publishes it, to keep this backend matching
        // the others
        f.sync_all().map_err(|e| io(path, e))?;
        drop(f);
        fs::set_permissions(&real, fs::Permissions::from_mode(mode as u32))
            .map_err(|e| io(path, e))?;
        self.own(path, uid, gid)?;
        self.note(path, mode, uid, gid);
        Ok(())
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> RootResult<()> {
        let real = self.real(path);
        if let Some(parent) = real.parent() {
            fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        }
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&real)
            .map_err(|e| io(path, e))?;
        f.write_all(bytes).map_err(|e| io(path, e))?;
        f.sync_all().map_err(|e| io(path, e))
    }

    fn symlink(&mut self, path: &str, target: &str, uid: u16, gid: u16) -> RootResult<()> {
        let real = self.real(path);
        if let Some(parent) = real.parent() {
            fs::create_dir_all(parent).map_err(|e| io(path, e))?;
        }
        // A link in the way is replaced: the applier stages links under a name
        // of its own, which may survive an interrupted build
        let _ = fs::remove_file(&real);
        std::os::unix::fs::symlink(target, &real).map_err(|e| io(path, e))?;
        self.own(path, uid, gid)?;
        self.note(path, 0o777, uid, gid);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> RootResult<()> {
        fs::rename(self.real(from), self.real(to)).map_err(|e| io(from, e))?;
        if let Some(v) = self.wanted.remove(from) {
            self.wanted.insert(to.to_string(), v);
        }
        Ok(())
    }

    fn remove_file(&mut self, path: &str) -> RootResult<()> {
        fs::remove_file(self.real(path)).map_err(|e| io(path, e))?;
        self.wanted.remove(path);
        Ok(())
    }

    fn remove_dir(&mut self, path: &str) -> RootResult<()> {
        let real = self.real(path);
        match fs::read_dir(&real) {
            Ok(mut d) => {
                if d.next().is_some() {
                    return Err(RootError::DirectoryNotEmpty { path: path.to_string() });
                }
            }
            Err(e) => return Err(io(path, e)),
        }
        fs::remove_dir(&real).map_err(|e| io(path, e))?;
        self.wanted.remove(path);
        Ok(())
    }

    /// Unknown; `std` does not report it. The planner reports "cannot tell"
    fn free_space(&self) -> Option<u64> {
        None
    }

    fn sync(&mut self) -> RootResult<()> {
        Ok(())
    }
}
