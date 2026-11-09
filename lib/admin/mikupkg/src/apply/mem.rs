//! A `Root` in memory that can be switched off mid-write. Fails at operation n
//! for every n in the transaction and asserts that recovery restores the tree.
//!
//! `crash_at` performs the operation and then stops: the effect is on disk and
//! nothing recorded it. Sweeping `n` also covers "before operation n", which is
//! "after operation n-1".

use alloc::collections::BTreeMap;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use super::root::{Kind, Root, RootError, RootResult, Stat};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Entry {
    kind: Kind,
    mode: u16,
    uid: u16,
    gid: u16,
    /// A file's contents, or a symlink's target. Empty for a directory
    data: Vec<u8>,
}

/// A filesystem that fits in a `BTreeMap`, and can lose power.
#[derive(Debug, Clone, Default)]
pub struct MemRoot {
    entries: BTreeMap<String, Entry>,
    free: Option<u64>,
    /// Which mutating call to die on, counted from 1
    crash_at: Option<u64>,
    calls: u64,
    dead: bool,
    /// Every mutating call, in order
    pub log: Vec<String>,
}

impl MemRoot {
    pub fn new() -> MemRoot {
        let mut r = MemRoot::default();
        r.free = Some(64 * 1024 * 1024);
        r
    }

    /// Die after the `n`th mutating call, as a power cut would
    pub fn crash_after(&mut self, n: u64) {
        self.crash_at = Some(n);
    }

    /// Restarts: effects that reached the map remain, and nothing fails
    pub fn power_on(&mut self) {
        self.dead = false;
        self.crash_at = None;
        self.calls = 0;
    }

    pub fn did_crash(&self) -> bool {
        self.dead
    }

    /// Mutating calls made by a transaction, as the limit for a crash sweep
    pub fn calls(&self) -> u64 {
        self.calls
    }

    pub fn set_free_space(&mut self, bytes: Option<u64>) {
        self.free = bytes;
    }

    /// Everything, for asserting on the whole tree
    pub fn snapshot(&self) -> Vec<(String, Kind, u16, u16, u16, Vec<u8>)> {
        self.entries
            .iter()
            .map(|(p, e)| (p.clone(), e.kind, e.mode, e.uid, e.gid, e.data.clone()))
            .collect()
    }

    pub fn paths(&self) -> Vec<&str> {
        self.entries.keys().map(|s| s.as_str()).collect()
    }

    /// Places a file without the applier, for a test's starting state
    pub fn preset_file(&mut self, path: &str, content: &[u8], mode: u16, uid: u16, gid: u16) {
        self.entries.insert(
            path.to_string(),
            Entry { kind: Kind::File, mode, uid, gid, data: content.to_vec() },
        );
    }

    pub fn preset_dir(&mut self, path: &str, mode: u16) {
        self.entries.insert(
            path.to_string(),
            Entry { kind: Kind::Directory, mode, uid: 0, gid: 0, data: Vec::new() },
        );
    }

    /// Counts a mutating call and stops if this is the one. Called after the
    /// effect
    fn tick(&mut self, what: &str) -> RootResult<()> {
        self.calls += 1;
        self.log.push(what.to_string());
        if self.dead || self.crash_at == Some(self.calls) {
            self.dead = true;
            return Err(RootError::Io { path: what.to_string(), code: -999 });
        }
        Ok(())
    }

    fn guard(&self) -> RootResult<()> {
        if self.dead {
            return Err(RootError::Io { path: String::new(), code: -999 });
        }
        Ok(())
    }

    /// Creates every missing parent of `path`, like the host backend's
    /// `create_dir_all`
    fn make_parents(&mut self, path: &str, mode: u16, uid: u16, gid: u16) -> RootResult<()> {
        let mut at = 0usize;
        while let Some(slash) = path[at..].find('/') {
            at += slash;
            let parent = &path[..at];
            at += 1;
            match self.entries.get(parent) {
                Some(e) if e.kind == Kind::Directory => continue,
                Some(_) => return Err(RootError::NotADirectory { path: parent.to_string() }),
                None => {
                    self.entries.insert(
                        parent.to_string(),
                        Entry { kind: Kind::Directory, mode, uid, gid, data: Vec::new() },
                    );
                }
            }
        }
        Ok(())
    }

}

impl Root for MemRoot {
    fn stat(&self, path: &str) -> Option<Stat> {
        self.entries.get(path).map(|e| Stat {
            kind: e.kind,
            mode: e.mode,
            uid: e.uid,
            gid: e.gid,
            size: e.data.len() as u64,
        })
    }

    fn read(&self, path: &str) -> RootResult<Vec<u8>> {
        self.guard()?;
        match self.entries.get(path) {
            Some(e) if e.kind == Kind::File => Ok(e.data.clone()),
            Some(_) => Err(RootError::NotADirectory { path: path.to_string() }),
            None => Err(RootError::NotFound { path: path.to_string() }),
        }
    }

    fn read_link(&self, path: &str) -> RootResult<String> {
        self.guard()?;
        match self.entries.get(path) {
            Some(e) if e.kind == Kind::Symlink => {
                Ok(String::from_utf8_lossy(&e.data).into_owned())
            }
            Some(_) => Err(RootError::NotADirectory { path: path.to_string() }),
            None => Err(RootError::NotFound { path: path.to_string() }),
        }
    }

    fn make_dir(&mut self, path: &str, mode: u16, uid: u16, gid: u16) -> RootResult<()> {
        self.guard()?;
        if let Some(e) = self.entries.get(path) {
            return if e.kind == Kind::Directory {
                Ok(())
            } else {
                Err(RootError::AlreadyExists { path: path.to_string() })
            };
        }
        self.make_parents(path, mode, uid, gid)?;
        self.entries.insert(
            path.to_string(),
            Entry { kind: Kind::Directory, mode, uid, gid, data: Vec::new() },
        );
        self.tick(&alloc::format!("mkdir {path}"))
    }

    fn write_new(
        &mut self,
        path: &str,
        content: &[u8],
        mode: u16,
        uid: u16,
        gid: u16,
    ) -> RootResult<()> {
        self.guard()?;
        self.make_parents(path, 0o755, 0, 0)?;
        if let Some(free) = self.free {
            if content.len() as u64 > free {
                return Err(RootError::NoSpace { path: path.to_string() });
            }
            self.free = Some(free - content.len() as u64);
        }
        self.entries.insert(
            path.to_string(),
            Entry { kind: Kind::File, mode, uid, gid, data: content.to_vec() },
        );
        self.tick(&alloc::format!("write {path}"))
    }

    fn append(&mut self, path: &str, bytes: &[u8]) -> RootResult<()> {
        self.guard()?;
        self.make_parents(path, 0o755, 0, 0)?;
        let e = self.entries.entry(path.to_string()).or_insert(Entry {
            kind: Kind::File,
            mode: 0o600,
            uid: 0,
            gid: 0,
            data: Vec::new(),
        });
        if e.kind != Kind::File {
            return Err(RootError::NotADirectory { path: path.to_string() });
        }
        e.data.extend_from_slice(bytes);
        self.tick(&alloc::format!("append {path}"))
    }

    fn symlink(&mut self, path: &str, target: &str, uid: u16, gid: u16) -> RootResult<()> {
        self.guard()?;
        self.make_parents(path, 0o755, 0, 0)?;
        self.entries.insert(
            path.to_string(),
            Entry {
                kind: Kind::Symlink,
                mode: 0o777,
                uid,
                gid,
                data: target.as_bytes().to_vec(),
            },
        );
        self.tick(&alloc::format!("symlink {path}"))
    }

    fn rename(&mut self, from: &str, to: &str) -> RootResult<()> {
        self.guard()?;
        let e = self
            .entries
            .remove(from)
            .ok_or_else(|| RootError::NotFound { path: from.to_string() })?;
        // Replaces, per the contract
        self.entries.insert(to.to_string(), e);
        self.tick(&alloc::format!("rename {from} -> {to}"))
    }

    fn remove_file(&mut self, path: &str) -> RootResult<()> {
        self.guard()?;
        match self.entries.get(path) {
            None => return Err(RootError::NotFound { path: path.to_string() }),
            Some(e) if e.kind == Kind::Directory => {
                return Err(RootError::NotADirectory { path: path.to_string() })
            }
            Some(_) => {}
        }
        self.entries.remove(path);
        self.tick(&alloc::format!("rm {path}"))
    }

    fn remove_dir(&mut self, path: &str) -> RootResult<()> {
        self.guard()?;
        let prefix = alloc::format!("{path}/");
        if self.entries.keys().any(|k| k.starts_with(&prefix)) {
            return Err(RootError::DirectoryNotEmpty { path: path.to_string() });
        }
        match self.entries.get(path) {
            None => return Err(RootError::NotFound { path: path.to_string() }),
            Some(e) if e.kind != Kind::Directory => {
                return Err(RootError::NotADirectory { path: path.to_string() })
            }
            Some(_) => {}
        }
        self.entries.remove(path);
        self.tick(&alloc::format!("rmdir {path}"))
    }

    fn free_space(&self) -> Option<u64> {
        self.free
    }

    fn sync(&mut self) -> RootResult<()> {
        self.guard()
    }
}
