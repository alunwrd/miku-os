// A vnode's names (`dev/vfs-dentries-inodes.md`, "Stage 5: dentries and
// inodes")
//
// The vnode is the inode, and a name is an entry in its directory's table
// (`vfs::children`). A directory has one name. A file on a disk can have
// several, and each one cached is an entry pointing at the same vnode:
//
// - its first name is the vnode's `parent` and `name`, the rest are its
//   `aliases`;
// - the icache finds the vnode again when another name of the same inode is
//   looked up or listed, so one inode is one vnode with one size, one set of
//   pages and one set of times;
// - unlink and rename take the entry they were given (`resolve_entry`), and
//   move the vnode's first name to another of its names when they take that
//   one;
// - `free_vnode` takes out every name still in a directory (`forget_names`).
//
// Negative entries are kept here too: a miss on a disk is remembered in the
// directory, up to `NEGATIVE_PER_DIR` of them.

use alloc::vec::Vec;

use super::MikuVFS;
use crate::fs::vfs::hash::name_hash;
use crate::fs::vfs::types::*;
use crate::fs::vfs::vnode::Alias;

/// Negative entries a directory keeps. A program trying names that do not exist
/// (a temporary-name search, a `PATH` walk) would otherwise grow one
/// directory's table without end; at this many they are dropped together
pub(super) const NEGATIVE_PER_DIR: usize = 256;

/// One of `DcacheStats`'s numbers: counted by readers in parallel under the
/// VFS lock held to read (`with_vfs_read`), so an atomic
pub struct Counter(core::sync::atomic::AtomicU64);

impl Counter {
    pub const fn new() -> Self {
        Self(core::sync::atomic::AtomicU64::new(0))
    }
    #[inline]
    pub fn bump(&self) {
        self.0.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    #[inline]
    pub fn get(&self) -> u64 {
        self.0.load(core::sync::atomic::Ordering::Relaxed)
    }
}

/// What lookups cost the filesystems, and what the name cache saved them
pub struct DcacheStats {
    /// Lookups that asked a filesystem for a name
    pub fs_lookups: Counter,
    /// Misses answered by a negative entry or a whiteout
    pub negative_hits: Counter,
    /// Misses answered by a complete directory, with no entry at all
    pub complete_misses: Counter,
    /// Child slots `readdir_at` has looked at, for the selftest: a listing
    /// read in passes looks at each about once
    pub listed_slots: Counter,
    /// Names and listings read from a filesystem with the VFS lock released,
    /// and put in (`fill.rs`, `take_deferred`)
    pub deferred_reads: Counter,
}

impl DcacheStats {
    pub const fn new() -> Self {
        Self {
            fs_lookups: Counter::new(),
            negative_hits: Counter::new(),
            complete_misses: Counter::new(),
            listed_slots: Counter::new(),
            deferred_reads: Counter::new(),
        }
    }
}

impl MikuVFS {
    /// The vnode's key in the icache: its mount and its inode number there
    fn icache_key(&self, v: usize) -> Option<(u16, u32)> {
        let n = &self.nodes[v];
        n.is_ext_backed().then(|| (n.mount_key(), n.ext2_ino))
    }

    /// The vnode already caching inode `ino` of `dir`'s mount
    pub(super) fn icache_find(&self, dir: usize, ino: u32) -> Option<usize> {
        if self.icache.is_empty() {
            return None;
        }
        let key = (self.nodes[dir].mount_key(), ino);
        let v = *self.icache.get(&key)? as usize;
        (self.valid_vnode(v) && self.icache_key(v) == Some(key)).then_some(v)
    }

    /// `v` into the icache: a file on a disk with more than one name
    pub(super) fn icache_add(&mut self, v: usize) {
        if self.nodes[v].is_dir() {
            return;
        }
        if let Some(key) = self.icache_key(v) {
            self.icache.insert(key, v as InodeId);
        }
    }

    /// `v` is named `name` in `dir` too, an entry the caller has made. It
    /// becomes the first name when the first one has gone from its directory
    pub(super) fn add_name(&mut self, v: usize, dir: usize, name: &str) {
        let first = self.nodes[v].parent as usize;
        let first_gone = self.nodes[first].children.find_by_name(self.nodes[v].get_name()) != Some(v as InodeId);
        let n = &mut self.nodes[v];
        if first_gone {
            n.parent = dir as InodeId;
            n.name = NameBuf::from_str(name);
        } else {
            n.aliases.push(Alias { dir: dir as InodeId, name: name.into() });
        }
    }

    /// `v` is not named `name` in `dir` any more; the caller has taken the
    /// entry out. When that was its first name the next cached one takes its
    /// place. The last name stays as the one the file had: what a deferred
    /// unlink removes from the disk, and what a path to it reads
    pub(super) fn drop_name(&mut self, v: usize, dir: usize, name: &str) {
        let n = &mut self.nodes[v];
        if n.parent as usize == dir && n.name_eq(name) {
            if !n.aliases.is_empty() {
                let a = n.aliases.remove(0);
                n.parent = a.dir;
                n.name = NameBuf::from_str(&a.name);
            }
        } else if let Some(i) = n.aliases.iter().position(|a| a.dir as usize == dir && &*a.name == name) {
            n.aliases.remove(i);
        }
    }

    /// `name` in `old_dir` is `new_name` in `new_dir` now
    pub(super) fn move_name(&mut self, v: usize, old_dir: usize, old_name: &str, new_dir: usize, new_name: &str) {
        let n = &mut self.nodes[v];
        if let Some(a) = n.aliases.iter_mut().find(|a| a.dir as usize == old_dir && &*a.name == old_name) {
            a.dir = new_dir as InodeId;
            a.name = new_name.into();
        } else {
            // Its first name, or a name it was found by before its first one
            // went: either way the one a path to it should read now
            n.parent = new_dir as InodeId;
            n.name = NameBuf::from_str(new_name);
        }
    }

    /// The directories that name `v`
    pub(super) fn dirs_of(&self, v: usize) -> impl Iterator<Item = usize> + '_ {
        let n = &self.nodes[v];
        core::iter::once(n.parent as usize).chain(n.aliases.iter().map(|a| a.dir as usize))
    }

    /// Every name `v` still has, out of its directories, and `v` out of the
    /// icache: `free_vnode` is freeing it. An entry still there at this point
    /// is a name the disk still holds (an eviction, an unmount), so its
    /// directory can no longer answer a miss alone. A name the caller took out
    /// first (unlink, rename) is not there, and changes nothing
    pub(super) fn forget_names(&mut self, v: usize) {
        let names: Vec<(usize, u32)> = {
            let n = &self.nodes[v];
            core::iter::once((n.parent as usize, name_hash(n.get_name())))
                .chain(n.aliases.iter().map(|a| (a.dir as usize, name_hash(&a.name))))
                .collect()
        };
        for (dir, h) in names {
            if dir != v && self.nodes.contains(dir) && self.nodes[dir].children.remove(h, v as InodeId) {
                self.nodes[dir].children_loaded = false;
            }
        }
        if let Some(key) = self.icache_key(v) {
            if self.icache.get(&key) == Some(&(v as InodeId)) {
                self.icache.remove(&key);
            }
        }
    }

    /// `name` is not in `dir`: remembered, so the next lookup of it does not
    /// ask the filesystem
    pub(super) fn add_negative(&mut self, dir: usize, name: &str) {
        let c = &mut self.nodes[dir].children;
        if c.negatives() >= NEGATIVE_PER_DIR {
            c.drop_negatives();
        }
        c.insert_negative(name);
    }

    /// The directory, the name and the vnode a path's last component is,
    /// without following a symlink there. Unlink and rename work on this:
    /// the entry they were given, which for a hard link is not always the
    /// vnode's first name
    pub(super) fn resolve_entry<'a>(&mut self, cwd: usize, path: &'a str) -> VfsResult<(usize, &'a str, usize)> {
        let path = path.trim();
        let bare = path.trim_end_matches('/');
        if bare.is_empty() {
            return Err(VfsError::InvalidArgument);
        }
        let (dir, name) = self.split_path(cwd, bare)?;
        Self::validate_name(name)?;
        let dir = self.xm(dir);
        if !self.nodes[dir].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        self.check_search(dir)?;
        let id = self.lookup_child_or_load(dir, name)?;
        // `a/` names a directory, and only a directory
        if bare.len() != path.len() && !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        Ok((dir, name, id))
    }
}
