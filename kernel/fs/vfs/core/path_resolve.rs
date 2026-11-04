// Path resolution: resolve_path, follow_symlink, lookup_child_or_load,
// split_path

use super::MikuVFS;
use crate::fs::vfs::children::Probe;
use crate::fs::vfs::types::*;

// Search permission on every directory a walk passes through.
//
// The `x` bit decides whether a directory is private: without it the names and
// sizes of its files and the tree beneath it stay hidden. Every one of the four
// walk loops below asks `check_search` before it descends, and before it climbs
// through `..`

// Promote an InodeId stored on disk or in a VNode to a node-array index. None
// if the id is INVALID, and the caller stops the traversal. A corrupted ext2
// parent field reaches this
#[inline]
fn id_to_idx(id: InodeId) -> Option<usize> {
    if id == INVALID_ID { return None; }
    // An id with no vnode behind it reads as an empty one (`vfs::table`), so
    // a corrupt parent field is caught by the `active` check where it is used
    Some(id as usize)
}

impl MikuVFS {
    /// The absolute path of a vnode, written into `out`. Returns how many bytes
    /// were written, or 0 when it does not fit.
    ///
    /// Walking parent links is the only way back to a path: a vnode carries its
    /// own name and its parent. For `/proc/mounts` and the mount listing
    pub fn abs_path_of(&self, id: usize, out: &mut [u8]) -> usize {
        let path = self.abs_path(id);
        if path.len() > out.len() {
            return 0;
        }
        out[..path.len()].copy_from_slice(path.as_bytes());
        path.len()
    }

    /// The same, as a string: the one walk from a vnode back to the root.
    ///
    /// What `realpath` (119) and `getcwd` (8) answer with, and `/proc/mounts`
    /// through `abs_path_of`. The steps are capped for a corrupt parent cycle,
    /// and a walk hitting the cap is an error, not a shorter path
    pub fn abs_path(&self, mut id: usize) -> alloc::string::String {
        use alloc::string::String;
        use alloc::vec::Vec;
        let root = self.ctx.root as usize;
        let mut names: Vec<&str> = Vec::new();
        let mut steps = 0usize;
        while id != 0 && id != root && self.nodes.contains(id) && self.nodes[id].active {
            names.push(self.nodes[id].name.as_str());
            let p = self.nodes[id].parent as usize;
            // A walk longer than there are vnodes is a loop in the parents
            if p == id || steps > self.nodes.len() {
                break;
            }
            id = p;
            steps += 1;
        }
        if names.is_empty() {
            return String::from("/");
        }
        let mut s = String::new();
        for n in names.iter().rev() {
            s.push('/');
            s.push_str(n);
        }
        s
    }

    pub fn resolve_path(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        walk(&mut Writer(self), cwd, path, Links::All)
    }

    pub fn resolve_path_lstat(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        walk(&mut Writer(self), cwd, path, Links::AllButLast)
    }

    pub(super) fn follow_symlink(&mut self, link_id: usize, depth: usize) -> VfsResult<usize> {
        follow(&mut Writer(self), link_id, depth)
    }

    /// One name in one directory: from the cache, or from the filesystem.
    ///
    /// A miss is answered without the filesystem when the directory knows the
    /// name is not there (a negative entry, a whiteout) or knows every name it
    /// has (`children_loaded`); a miss the filesystem answered leaves a
    /// negative entry (`names.rs`)
    pub(super) fn lookup_child_or_load(&mut self, parent: usize, name: &str) -> VfsResult<usize> {
        match self.nodes[parent].children.probe(name) {
            Probe::Node(id) => {
                let cid = id as usize;
                if self.nodes.contains(cid) && self.nodes[cid].active {
                    // Recency for `reclaim`: a name walked through is in use
                    self.nodes[cid].used = self.op;
                    self.lru.push_head(&mut self.nodes, cid);
                    return Ok(cid);
                }
            }
            Probe::Absent => {
                self.dcache.negative_hits.bump();
                return Err(VfsError::NotFound);
            }
            Probe::Unknown => {}
        }

        if self.nodes[parent].fs_type.has_mounted_fs() && self.nodes[parent].ext2_ino != 0 {
            // Every name the directory has is cached. `lost+found` is the
            // exception: a listing leaves it out, a path still reaches it
            if self.nodes[parent].children_loaded && name != "lost+found" {
                self.dcache.complete_misses.bump();
                return Err(VfsError::NotFound);
            }
            return self.lazy_lookup_backed(parent, name);
        }

        Err(VfsError::NotFound)
    }

    pub fn resolve_path_follow(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        walk_follow(&mut Writer(self), cwd, path)
    }

    /// `lookup_child_or_load` with what is cached alone, for a walk under
    /// the lock held to read (`Reader`): the vnode, `NotFound` when the cache
    /// knows the name is not there, `Deferred` when only the filesystem knows.
    /// A name found is marked used for reclaim's sweep, with an atomic store,
    /// not moved in the LRU (`VNode::referenced`)
    pub(super) fn cached_child(&self, parent: usize, name: &str) -> VfsResult<usize> {
        match self.nodes[parent].children.probe(name) {
            Probe::Node(id) => {
                let cid = id as usize;
                if self.nodes.contains(cid) && self.nodes[cid].active {
                    self.nodes[cid].referenced.store(true, core::sync::atomic::Ordering::Relaxed);
                    return Ok(cid);
                }
            }
            Probe::Absent => {
                self.dcache.negative_hits.bump();
                return Err(VfsError::NotFound);
            }
            Probe::Unknown => {}
        }
        if self.nodes[parent].fs_type.has_mounted_fs() && self.nodes[parent].ext2_ino != 0 {
            if self.nodes[parent].children_loaded && name != "lost+found" {
                self.dcache.complete_misses.bump();
                return Err(VfsError::NotFound);
            }
            return Err(VfsError::Deferred);
        }
        Err(VfsError::NotFound)
    }

    pub fn split_path<'a>(
        &mut self,
        cwd: usize,
        path: &'a str,
    ) -> VfsResult<(usize, &'a str)> {
        match path.rfind('/') {
            Some(pos) => {
                let name = &path[pos + 1..];
                if name.is_empty() {
                    return Err(VfsError::InvalidPath);
                }
                let dir_part = &path[..pos];
                let parent = if dir_part.is_empty() {
                    self.resolve_path(cwd, "/")?
                } else {
                    self.resolve_path(cwd, dir_part)?
                };
                Ok((parent, name))
            }
            None => Ok((cwd, path)),
        }
    }
}

// The walks, written once. A walk under the lock held to write reads a name the
// cache does not have from the filesystem (`Writer`); one under the lock held
// to read, in parallel with others, has only the cache and its caller's own
// credential, and gives a miss up as `Deferred` (`Reader`,
// `core::with_vfs_read`). Search permission is asked at the same places in
// both: before descending into a directory, and before climbing out through
// `..`.

/// How a walk takes one step
pub(crate) trait Names {
    fn vfs(&self) -> &MikuVFS;
    /// `name` in directory `dir`, already resolved through mounts
    fn child(&mut self, dir: usize, name: &str) -> VfsResult<usize>;
    /// May the caller search `dir`?
    fn search(&self, dir: usize) -> VfsResult<()>;
}

struct Writer<'a>(&'a mut MikuVFS);

impl Names for Writer<'_> {
    fn vfs(&self) -> &MikuVFS {
        self.0
    }
    fn child(&mut self, dir: usize, name: &str) -> VfsResult<usize> {
        self.0.lookup_child_or_load(dir, name)
    }
    fn search(&self, dir: usize) -> VfsResult<()> {
        self.0.check_search(dir)
    }
}

/// A walk under the lock held to read: the cache and `cred`
pub(crate) struct Reader<'a> {
    pub vfs: &'a MikuVFS,
    pub cred: &'a Credentials,
}

impl Names for Reader<'_> {
    fn vfs(&self) -> &MikuVFS {
        self.vfs
    }
    fn child(&mut self, dir: usize, name: &str) -> VfsResult<usize> {
        self.vfs.cached_child(dir, name)
    }
    fn search(&self, dir: usize) -> VfsResult<()> {
        self.vfs.check_search_as(self.cred, dir)
    }
}

/// Which symlinks a walk follows: every one (`stat`), or every one but a
/// link that is the path's last name (`lstat`)
#[derive(Clone, Copy, PartialEq)]
pub enum Links {
    All,
    AllButLast,
}

pub(crate) fn walk<N: Names>(n: &mut N, cwd: usize, path: &str, links: Links) -> VfsResult<usize> {
    let path = path.trim();
    if path.is_empty() {
        return Ok(cwd);
    }
    let total = match links {
        Links::All => usize::MAX,
        Links::AllButLast => path.split('/').filter(|c| !c.is_empty() && *c != "." && *c != "..").count(),
    };
    let mut idx = 0usize;
    let mut current = if path.starts_with('/') { 0 } else { cwd };
    if !n.vfs().nodes.contains(current) { return Err(VfsError::InvalidPath); }
    let mut depth = 0u8;

    for component in path.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            n.search(current)?;
            if let Some(p) = id_to_idx(n.vfs().nodes[current].parent) {
                current = p;
            }
            continue;
        }

        idx += 1;
        depth += 1;
        if depth as usize > MAX_PATH_DEPTH {
            return Err(VfsError::InvalidPath);
        }

        if !n.vfs().nodes[current].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        n.search(current)?;

        let eff = n.vfs().xm(current);
        let next = n.child(eff, component)?;
        if !n.vfs().nodes.contains(next) { return Err(VfsError::NotFound); }
        current = next;

        // `lstat` stops at a link that is the last name; every other walk goes
        // through it
        if idx < total && n.vfs().nodes[current].is_symlink() {
            current = follow(n, current, 0)?;
            if !n.vfs().nodes.contains(current) { return Err(VfsError::NotFound); }
        }
    }
    Ok(current)
}

pub(crate) fn follow<N: Names>(n: &mut N, link_id: usize, depth: usize) -> VfsResult<usize> {
    if depth >= MAX_SYMLINK_DEPTH {
        return Err(VfsError::TooManySymlinks);
    }
    if !n.vfs().nodes.contains(link_id) { return Err(VfsError::InvalidPath); }
    if !n.vfs().nodes[link_id].is_symlink() {
        return Ok(link_id);
    }

    // Cloned, not borrowed: the walk below may take `&mut` of the vnode
    // table the target lives in
    let target_owned = n.vfs().nodes[link_id].symlink_target.bytes().to_vec();
    let target_str = match core::str::from_utf8(&target_owned) {
        Ok(s) => s,
        Err(_) => return Err(VfsError::InvalidPath),
    };
    if target_str.is_empty() {
        return Err(VfsError::InvalidPath);
    }

    let parent = id_to_idx(n.vfs().nodes[link_id].parent).unwrap_or(0);
    let mut current = if target_str.starts_with('/') { 0 } else { parent };
    if !n.vfs().nodes.contains(current) { return Err(VfsError::InvalidPath); }
    for component in target_str.split('/') {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            n.search(current)?;
            if let Some(p) = id_to_idx(n.vfs().nodes[current].parent) {
                current = p;
            }
            continue;
        }
        if !n.vfs().nodes[current].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        n.search(current)?;
        let eff = n.vfs().xm(current);
        let next = n.child(eff, component)?;
        if !n.vfs().nodes.contains(next) { return Err(VfsError::NotFound); }
        current = next;
        if n.vfs().nodes[current].is_symlink() {
            current = follow(n, current, depth + 1)?;
            if !n.vfs().nodes.contains(current) { return Err(VfsError::NotFound); }
        }
    }
    Ok(current)
}

pub(crate) fn walk_follow<N: Names>(n: &mut N, cwd: usize, path: &str) -> VfsResult<usize> {
    let mut id = walk(n, cwd, path, Links::All)?;
    if !n.vfs().nodes.contains(id) { return Err(VfsError::InvalidPath); }
    let mut depth = 0;
    while n.vfs().nodes[id].is_symlink() {
        if depth >= MAX_SYMLINK_DEPTH {
            return Err(VfsError::TooManySymlinks);
        }
        // from_utf8: the bytes come straight from disk, and a non-UTF-8 symlink
        // through from_utf8_unchecked would be an invalid str
        let target_owned = n.vfs().nodes[id].symlink_target.bytes().to_vec();
        let target = match core::str::from_utf8(&target_owned) {
            Ok(s) => s,
            Err(_) => return Err(VfsError::InvalidPath),
        };
        if target.is_empty() {
            return Err(VfsError::InvalidPath);
        }
        let parent = id_to_idx(n.vfs().nodes[id].parent).unwrap_or(0);
        id = walk(n, parent, target, Links::All)?;
        if !n.vfs().nodes.contains(id) { return Err(VfsError::InvalidPath); }
        depth += 1;
    }
    Ok(id)
}
