// Filling the vnode tree from a filesystem
//
// A vnode is a cache of what the filesystem knows. This file fills it: one name
// at a time when a path is walked, or a whole directory when it is listed.
// Everything goes through `with_backing` and the `FileSystem` trait. `readdir`
// on the trait is a callback with no cap, and the staging is a `Vec`.
//
// A walk under `core::with_vfs_walk` does not read the disk here: a name or a
// listing the cache does not have becomes a `Deferred`, read with the VFS lock
// released (`fetch`) and put in under it again (`take_deferred`) if the
// directory has not changed meanwhile (`dev/vfs-locking.md`, "4d").

use super::MikuVFS;
use crate::fs::vfs::children::Probe;
use crate::fs::vfs::fsops::{FileSystem, NodeId, NodeMeta};
use crate::fs::vfs::types::*;
use alloc::string::String;
use alloc::vec::Vec;

/// As many children as one directory may put into the cache in a single fill:
/// what a quarter of free memory holds, at about a kilobyte a name and its
/// vnode. Reaching it is logged, and the directory is not marked loaded, so a
/// lookup still asks the disk.
///
/// So a corrupt directory claiming millions of entries cannot exhaust memory
/// before anything notices
fn max_fill() -> usize {
    let (used, total) = crate::mm::pmm::stats();
    (total.saturating_sub(used) * PAGE_SIZE / 4 / 1024).max(4096)
}

/// One child, copied out of the filesystem so the borrow ends before the
/// vnode table is touched. Names are kept at their full on-disk width; what
/// the vnode table can store is its own limit, applied at the point it stores
pub(crate) struct ChildInfo {
    /// Full length: a fill of a large directory holds all of them
    name: alloc::boxed::Box<[u8]>,
    meta: NodeMeta,
    /// A symlink's target, when it was read with the listing
    target: Option<Vec<u8>>,
}

/// A directory's names and what each one is, from its filesystem: the walk,
/// then each inode, then, when asked, each symlink's target. Whether the walk
/// stopped at `max_fill`
fn read_listing(fs: &mut dyn FileSystem, dir: NodeId, targets: bool) -> VfsResult<(Vec<ChildInfo>, bool)> {
    let mut collected: Vec<ChildInfo> = Vec::new();
    let mut hit_cap = false;
    let cap = max_fill();
    fs.readdir(dir, &mut |e| {
        if collected.len() >= cap {
            hit_cap = true;
            return false;
        }
        // `.` and `..` are the tree's own structure, and `lost+found` is the
        // filesystem's, not the user's
        let n = e.name;
        if n == b"." || n == b".." || n == b"lost+found" {
            return true;
        }
        let mut info = ChildInfo { name: n[..n.len().min(255)].into(), meta: NodeMeta::empty(), target: None };
        info.meta.id = e.id;
        info.meta.kind = e.kind;
        collected.push(info);
        true
    })?;
    // The entry gave a name, an id and a kind; the rest is in the inode. Asked
    // after the walk: the walk holds the directory's buffer, and a second read
    // through it would invalidate the borrowed name
    for info in collected.iter_mut() {
        if let Ok(meta) = fs.getattr(info.meta.id) {
            let kind = info.meta.kind;
            info.meta = meta;
            // The directory entry and the inode can disagree on a damaged
            // filesystem. The inode owns the answer, except that a zeroed one
            // leaves the entry's word standing
            if meta.kind == VNodeKind::Regular && kind != VNodeKind::Regular {
                info.meta.kind = kind;
            }
        }
        if targets && info.meta.kind == VNodeKind::Symlink {
            info.target = Some(read_target(fs, info.meta.id));
        }
    }
    Ok((collected, hit_cap))
}

/// A symlink's target, whole: on the heap and sized by `SYMLINK_MAX`, not
/// `NAME_LEN`. A target is a path
fn read_target(fs: &mut dyn FileSystem, id: NodeId) -> Vec<u8> {
    let mut buf = alloc::vec![0u8; SYMLINK_MAX];
    match fs.readlink(id, &mut buf) {
        Ok(n) => {
            buf.truncate(n);
            buf
        }
        Err(_) => Vec::new(),
    }
}

/// A read of a directory that a walk under `with_vfs_walk` wanted and left
/// until the VFS lock is released. It holds a reference on the directory until
/// it is put in or thrown away (`take_deferred`)
pub struct Deferred {
    dir: usize,
    /// The node it was planned against: still that node, or nothing goes in
    backing: (FsType, u8, NodeId),
    /// The directory's `Children::gen` then, or after the last part of a
    /// listing this put in: unmoved, or nothing more goes in
    gen: u32,
    got: Got,
    /// Whether any part of it went in yet
    took: bool,
}

/// Names of a listing put in under one hold of the VFS lock. A directory of
/// thousands put in whole holds the lock for milliseconds of `instantiate`
const LISTING_PART: usize = 128;

enum Got {
    /// One name: what it is and, for a symlink, its target
    Lookup(String, VfsResult<(NodeMeta, Vec<u8>)>),
    /// Every name, and whether there were more than fit
    Listing(VfsResult<(Vec<ChildInfo>, bool)>),
}

/// The read a `Deferred` wants, with no VFS lock held: under the
/// filesystem's own slot lock, as `cached::fill` reads pages
pub fn fetch(d: &mut Deferred) {
    let (fs_type, slot, node) = d.backing;
    match &mut d.got {
        Got::Lookup(name, out) => {
            *out = crate::fs::registry::with_mounted(fs_type, slot, |fs| {
                let meta = fs.lookup(node, name)?;
                let target = if meta.kind == VNodeKind::Symlink { read_target(fs, meta.id) } else { Vec::new() };
                Ok((meta, target))
            })
            .unwrap_or(Err(VfsError::NotMounted));
        }
        Got::Listing(out) => {
            *out = crate::fs::registry::with_mounted(fs_type, slot, |fs| read_listing(fs, node, true))
                .unwrap_or(Err(VfsError::NotMounted));
        }
    }
}

impl MikuVFS {
    pub(super) fn lazy_lookup_backed(
        &mut self,
        parent_vnode: usize,
        name: &str,
    ) -> VfsResult<usize> {
        // The gate is ext's and applies only to ext: `is_ext2_ready` says
        // whether ext's active slot holds a mount, nothing about a parent on
        // another filesystem
        if self.nodes[parent_vnode].fs_type.is_ext_family()
            && (!self.ext2_mount_active || !crate::fs::ext::mount::is_ext2_ready())
        {
            return Err(VfsError::NotFound);
        }

        if self.defer && self.deferred.is_none() {
            if let Some(backing) = self.backing_of(parent_vnode) {
                self.nodes[parent_vnode].inc_ref();
                self.deferred = Some(Deferred {
                    dir: parent_vnode,
                    backing,
                    gen: self.nodes[parent_vnode].children.gen(),
                    got: Got::Lookup(name.into(), Err(VfsError::Deferred)),
                    took: false,
                });
                return Err(VfsError::Deferred);
            }
        }

        self.dcache.fs_lookups.bump();
        let meta = match self.with_backing(parent_vnode, |fs, dir| fs.lookup(dir, name)) {
            Some(Ok(meta)) => meta,
            Some(Err(e)) => {
                // The filesystem's word that the name is not there, kept so
                // the next lookup of it does not ask again (`names.rs`)
                if e == VfsError::NotFound {
                    self.add_negative(parent_vnode, name);
                }
                return Err(e);
            }
            None => return Err(VfsError::NotFound),
        };
        self.instantiate(parent_vnode, name, &meta, None)
    }

    /// What `fetch` read, put in under the lock: if the directory is still the
    /// node it was planned against and no name has come or gone in it since. A
    /// listing goes in `LISTING_PART` names at a time, the lock released
    /// between parts: `None` while there is more. At the end the plan's
    /// reference goes, and `Some` says whether anything went in or was there
    /// already, so the walk that runs next gets further
    pub(super) fn take_deferred(&mut self, d: &mut Deferred) -> Option<bool> {
        let current = self.valid_vnode(d.dir)
            && self.backing_of(d.dir) == Some(d.backing)
            && self.nodes[d.dir].children.gen() == d.gen;
        if current {
            match &mut d.got {
                Got::Lookup(name, Ok((meta, target))) => {
                    self.dcache.fs_lookups.bump();
                    d.took = match self.nodes[d.dir].children.probe(name) {
                        Probe::Unknown => {
                            let (meta, target) = (*meta, core::mem::take(target));
                            self.instantiate(d.dir, name, &meta, Some(target)).is_ok()
                        }
                        _ => true,
                    };
                }
                Got::Lookup(name, Err(VfsError::NotFound)) => {
                    self.dcache.fs_lookups.bump();
                    if matches!(self.nodes[d.dir].children.probe(name), Probe::Unknown) {
                        self.add_negative(d.dir, name);
                    }
                    d.took = true;
                }
                Got::Listing(Ok((collected, hit_cap))) => {
                    if !self.nodes[d.dir].children_loaded {
                        let part = collected.split_off(collected.len().saturating_sub(LISTING_PART));
                        let full = self.insert_children(d.dir, part);
                        *hit_cap |= full;
                        d.took = true;
                        if !collected.is_empty() && !full {
                            // Its own names moved `gen`: the next part checks
                            // against where this one left it
                            d.gen = self.nodes[d.dir].children.gen();
                            return None;
                        }
                        self.finish_listing(d.dir, *hit_cap);
                    }
                    d.took = true;
                }
                // Anything else is the walk's to meet under the lock
                _ => {}
            }
        }
        if d.took {
            self.dcache.deferred_reads.bump();
        }
        self.drop_ref(d.dir);
        Some(d.took)
    }

    /// `name` in `dir` as the filesystem described it: an entry, and the
    /// vnode behind it. A file with another name already cached is the vnode
    /// that name has (the icache, `names.rs`); anything else is a new one.
    /// A symlink's target is `target`, or read here when it was not read
    /// with the name
    fn instantiate(&mut self, dir: usize, name: &str, meta: &NodeMeta, target: Option<Vec<u8>>) -> VfsResult<usize> {
        if meta.kind != VNodeKind::Directory {
            if let Some(v) = self.icache_find(dir, meta.id.as_ino()) {
                if !self.nodes[dir].children.insert(name, v as InodeId) {
                    return Err(VfsError::NoSpace);
                }
                self.add_name(v, dir, name);
                self.nodes[v].used = self.op;
                self.lru.push_head(&mut self.nodes, v);
                return Ok(v);
            }
        }

        // A symlink's target comes from the filesystem, not from the lookup:
        // the one piece of a node that is contents, not metadata
        let target = match target {
            Some(t) => t,
            None if meta.kind == VNodeKind::Symlink => self.read_symlink_target(dir, meta.id),
            None => Vec::new(),
        };

        let id = self.alloc_vnode()?;
        self.fill_vnode_from(id, dir, name, meta);

        if !target.is_empty() {
            self.nodes[id].symlink_target.set(&target);
        }

        if !self.nodes[dir].children.insert(name, id as InodeId) {
            self.free_vnode(id);
            return Err(VfsError::NoSpace);
        }
        if meta.nlinks > 1 {
            self.icache_add(id);
        }
        Ok(id)
    }

    /// A symlink's target, whole (`read_target`)
    fn read_symlink_target(&self, near: usize, id: NodeId) -> Vec<u8> {
        self.with_backing(near, |fs, _| Ok(read_target(fs, id))).and_then(|r| r.ok()).unwrap_or_default()
    }

    /// Stamp a freshly allocated vnode with what the filesystem said.
    ///
    /// The flavour is the parent's, not asked of any mount table: a child is on
    /// its parent's filesystem, and the mount point's type was settled when it
    /// was grafted. The ext table and the btrfs table both number their slots
    /// from zero, so `ext::mount::fs_type_of(slot)` would stamp every child of
    /// a btrfs directory with ext's slot 0.
    fn fill_vnode_from(
        &mut self,
        id: usize,
        parent_vnode: usize,
        name: &str,
        meta: &NodeMeta,
    ) {
        let ts = self.now();
        let slot = self.nodes[parent_vnode].ext2_slot;
        let fs_type = self.nodes[parent_vnode].fs_type;

        // fold_id, not a cast: an ext4 volume written on another system can
        // name uid 100000, whose low half is a different user here. 65536
        // truncates to 0, which would make a foreign disk's files root-owned
        let uid = crate::process::cred::fold_id(meta.uid);
        let gid = crate::process::cred::fold_id(meta.gid);

        self.init_vnode(id, 
            id as InodeId,
            parent_vnode as InodeId,
            name,
            meta.kind,
            fs_type,
            FileMode::new(meta.mode),
            uid,
            gid,
            ts,
        );
        self.nodes[id].ext2_ino = meta.id.as_ino();
        self.nodes[id].ext2_slot = slot;
        self.nodes[id].size = meta.size;
        self.nodes[id].nlinks = meta.nlinks;
        self.nodes[id].children_loaded = false;
        // The file's times, not the moment it was cached: `make` and `cargo`
        // decide a rebuild by them
        self.set_times_from(id, meta);
        self.nodes[id].addr_space.nr_pages = meta.blocks as u32;
        self.nodes[id].attrs_fresh.store(true, core::sync::atomic::Ordering::Relaxed);
    }

    fn set_times_from(&mut self, id: usize, meta: &NodeMeta) {
        let n = &mut self.nodes[id];
        n.atime = meta.atime;
        n.mtime = meta.mtime;
        n.ctime = meta.ctime;
        n.btime = meta.ctime;
    }

    pub fn ensure_children_loaded(&mut self, dir_vnode: usize) -> VfsResult<()> {
        if !self.nodes[dir_vnode].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[dir_vnode].children_loaded {
            return Ok(());
        }

        if self.defer && self.deferred.is_none() {
            if let Some(backing) = self.backing_of(dir_vnode) {
                self.nodes[dir_vnode].inc_ref();
                self.deferred = Some(Deferred {
                    dir: dir_vnode,
                    backing,
                    gen: self.nodes[dir_vnode].children.gen(),
                    got: Got::Listing(Err(VfsError::Deferred)),
                    took: false,
                });
                return Err(VfsError::Deferred);
            }
        }

        // Collect first, then touch the vnode table: the filesystem's buffer is
        // borrowed for the length of the walk
        let (collected, hit_cap) = match self.with_backing(dir_vnode, |fs, dir| read_listing(fs, dir, false)) {
            Some(Ok(listing)) => listing,
            Some(Err(e)) => return Err(e),
            // Not backed by a filesystem: the vnode table is the storage, and
            // the children are already there
            None => return Ok(()),
        };
        self.insert_listing(dir_vnode, collected, hit_cap);
        Ok(())
    }

    /// A directory's listing into the tree, and the directory marked complete
    /// unless there were more names than fit
    fn insert_listing(&mut self, dir_vnode: usize, collected: Vec<ChildInfo>, hit_cap: bool) {
        let full = self.insert_children(dir_vnode, collected);
        self.finish_listing(dir_vnode, hit_cap || full);
    }

    /// Names of a listing into the tree: whether the vnodes ran out first
    fn insert_children(&mut self, dir_vnode: usize, collected: Vec<ChildInfo>) -> bool {
        // Reading the directory may have taken a while, and every vnode this
        // loop allocates can reclaim: the directory has to look recently used,
        // or on a first fill (no cached children, so a leaf) it would be the
        // oldest thing in sight
        self.nodes[dir_vnode].used = self.op;

        for info in collected {
            let name_str = match core::str::from_utf8(&info.name) {
                Ok(s) => s,
                Err(_) => continue,
            };

            // Cached already, or a whiteout: a file unlinked while open,
            // whose name the disk keeps until its last close
            let already = match self.nodes[dir_vnode].children.probe(name_str) {
                Probe::Node(id) => self.valid_vnode(id as usize),
                Probe::Absent => self.nodes[dir_vnode].children.is_whiteout(name_str),
                Probe::Unknown => false,
            };
            if already {
                continue;
            }

            match self.instantiate(dir_vnode, name_str, &info.meta, info.target) {
                Ok(_) => {}
                Err(VfsError::NoSpace) => {
                    // Out of vnodes part-way through. Marking the directory
                    // loaded would make the names that did not fit permanently
                    // invisible
                    return true;
                }
                Err(_) => {}
            }
        }
        false
    }

    /// A listing is all in: the directory is complete, unless it had more
    /// names than fit
    fn finish_listing(&mut self, dir_vnode: usize, hit_cap: bool) {
        if hit_cap {
            // Logged, and not marked loaded: a lookup still asks the disk, so a
            // name past the cap is reachable by path though not listed
            crate::serial_println!(
                "[vfs] directory vnode {} has more children than the tree can hold",
                dir_vnode
            );
        } else {
            self.nodes[dir_vnode].children_loaded = true;
            // A complete directory answers a miss without them
            self.nodes[dir_vnode].children.drop_negatives();
        }
    }

    /// What `stat` reports, from the filesystem: size, blocks and the three
    /// times. The filesystem stores the times every write and every change to a
    /// directory sets, so a vnode on a disk holds nothing the disk does not
    /// (`reclaim.rs` evicts it however recently it was written)
    pub(super) fn refresh_attrs_from_fs(&mut self, id: usize) {
        if self.nodes[id].attrs_fresh.load(core::sync::atomic::Ordering::Relaxed) {
            return;
        }
        if let Some(Ok(meta)) = self.with_backing(id, |fs, node| fs.getattr(node)) {
            // Pages not written out yet: the size and the mtime are the
            // VFS's, and the disk's are older (`writeback.rs`)
            let (size, mtime) = (self.nodes[id].size, self.nodes[id].mtime);
            let dirty = self.has_dirty_pages(id);
            self.nodes[id].size = meta.size;
            self.nodes[id].addr_space.nr_pages = meta.blocks as u32;
            self.set_times_from(id, &meta);
            if dirty {
                self.nodes[id].size = size;
                self.nodes[id].mtime = mtime;
            }
            self.nodes[id].attrs_fresh.store(true, core::sync::atomic::Ordering::Relaxed);
        }
    }

    pub(super) fn read_backed_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        buf: &mut [u8],
    ) -> VfsResult<usize> {
        // Every page this covers in the cache: no filesystem asked at all
        if self.cacheable(vid) {
            if let Some(n) = self.cache_copy(vid, offset, buf) {
                self.fds().get_mut(fd)?.offset += n as u64;
                return Ok(n);
            }
        }
        // Around the cache: what it holds dirty goes to the disk first
        self.settle(vid);
        let result = self.with_backing(vid, |fs, node| {
            let size = fs.getattr(node)?.size;
            let n = fs.read_at(node, offset, buf)?;
            // relatime, and never on a read-only mount (the filesystem knows
            // which): reading a file, such as exec of /bin/mikudo, must not
            // write anything back
            fs.touch_atime(node);
            Ok((n, size))
        });

        let (n, disk_size) = match result {
            Some(Ok(pair)) => pair,
            Some(Err(e)) => return Err(e),
            None => return Err(VfsError::NotMounted),
        };

        if disk_size > 0 {
            self.nodes[vid].size = disk_size;
        }

        self.fds().get_mut(fd)?.offset += n as u64;
        Ok(n)
    }

    pub(super) fn write_backed_file(
        &mut self,
        fd: usize,
        vid: usize,
        offset: u64,
        data: &[u8],
    ) -> VfsResult<usize> {
        let is_sync = self
            .fds()
            .get(fd)
            .map(|f| f.flags.has(OpenFlags::SYNC))
            .unwrap_or(false);

        // Into the page cache, written out later (`writeback.rs`), when it
        // can go there
        if !is_sync {
            if let Some(n) = self.cache_write(vid, offset, data) {
                let new_end = offset + n as u64;
                if new_end > self.nodes[vid].size {
                    self.nodes[vid].size = new_end;
                }
                let ts = self.now();
                self.nodes[vid].touch_mtime(ts);
                self.nodes[vid].flags.dirty = true;
                self.fds().get_mut(fd)?.offset = new_end;
                return Ok(n);
            }
        }

        let n = match self.with_backing(vid, |fs, node| fs.write_at(node, offset, data)) {
            Some(Ok(n)) => n,
            Some(Err(e)) => {
                self.cache_written(vid, offset, &[], false);
                return Err(e);
            }
            None => return Err(VfsError::NotMounted),
        };
        self.cache_written(vid, offset, &data[..n], true);

        let new_end = offset + n as u64;
        if new_end > self.nodes[vid].size {
            self.nodes[vid].size = new_end;
        }

        let ts = self.now();
        self.nodes[vid].touch_mtime(ts);
        self.nodes[vid].flags.dirty = true;

        if is_sync {
            // O_SYNC promises the write is on the medium when the call returns,
            // so the result is not discarded
            match self.with_backing(vid, |fs, node| fs.fsync(node)) {
                Some(Ok(())) => {}
                Some(Err(e)) => return Err(e),
                None => return Err(VfsError::NotMounted),
            }
            self.nodes[vid].flags.dirty = false;
        }

        self.fds().get_mut(fd)?.offset = new_end;
        Ok(n)
    }
}
