// vnode management: allocation, validation, name checks, eviction

use super::MikuVFS;
use crate::fs::devfs;
use crate::fs::vfs::children::Probe;
use crate::fs::procfs;
use crate::fs::vfs::types::*;
use crate::fs::vfs::vnode::VNode;

/// What a table that ran dry mid-operation gives back at once
const RECLAIM_BATCH: usize = 512;

impl MikuVFS {
    /// Unix seconds: what `stat`(18) says the three times are, what
    /// `utimensat`(40) stores and what ext keeps on disk. Before any clock
    /// source (no RTC, no NTP yet) the uptime in seconds: small, but one unit
    /// and monotonic
    #[inline]
    pub(super) fn now(&self) -> Timestamp {
        match crate::kcore::clock::now_unix() {
            0 => procfs::uptime_ticks() / procfs::tick_hz(),
            s => s,
        }
    }

    /// `VNode::init` through the table. A method of the VFS so its arguments
    /// may read the VFS (`self.now()`, the parent's `fs_type`), which an
    /// `index_mut` holding the table would not allow
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn init_vnode(
        &mut self,
        id: usize,
        self_id: InodeId,
        parent: InodeId,
        name: &str,
        kind: VNodeKind,
        fs_type: FsType,
        mode: FileMode,
        uid: u16,
        gid: u16,
        now: Timestamp,
    ) {
        // `init` empties the vnode, its LRU links with it
        self.lru.remove(&mut self.nodes, id);
        self.nodes[id].init(self_id, parent, name, kind, fs_type, mode, uid, gid, now);
        self.nodes[id].used = self.op;
        // Every vnode joins the cache's LRU; reclaim takes out what can never
        // go (`reclaim.rs`)
        self.lru.push_head(&mut self.nodes, id);
    }

    /// A new vnode, empty and not yet `active`: the caller's `init` makes it
    /// one. The table grows for it (`vfs::table`); only memory runs out, and
    /// then cached vnodes are given back first
    pub fn alloc_vnode(&mut self) -> VfsResult<usize> {
        if let Some(i) = self.nodes.alloc(VNode::empty()) {
            return Ok(i);
        }
        // No memory in the middle of an operation: make room now. Everything
        // this operation holds is counted, or pinned, or was touched too
        // recently to go (`reclaim.rs`)
        if self.reclaim(RECLAIM_BATCH) > 0 {
            if let Some(i) = self.nodes.alloc(VNode::empty()) {
                return Ok(i);
            }
        }
        Err(VfsError::NoSpace)
    }

    /// A vnode gone, with everything of its: tmpfs's pages and the file
    /// cache's, its pipe, its watches, and the memory. A watch can outlive its
    /// vnode by paths that are not deletions (an unmount's eviction, `rmdir`
    /// freeing at once), so it is dropped here
    pub fn free_vnode(&mut self, id: usize) {
        if !self.nodes.contains(id) {
            return;
        }
        // Its names out of their directories (`names.rs`)
        self.forget_names(id);
        self.lru.remove(&mut self.nodes, id);
        self.free_file_pages(id);
        crate::fs::vfs::filecache::lock().forget(id);
        crate::fs::vfs::pipe::forget(id);
        crate::fs::vfs::notify::forget(id, false, false);
        self.nodes.free(id);
    }

    /// A vnode in no directory: `nlinks` 0, its own parent, never found by a
    /// path. A pipe or an inotify instance, reached only through a descriptor.
    /// `close` frees it with its last descriptor, as it does an unlinked file
    pub fn anon_node(&mut self, kind: VNodeKind, name: &str, mode: FileMode) -> VfsResult<usize> {
        let id = self.alloc_vnode()?;
        let ts = self.now();
        self.init_vnode(id, 
            id as InodeId,
            id as InodeId,
            name,
            kind,
            FsType::TmpFS,
            mode,
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );
        self.nodes[id].nlinks = 0;
        Ok(id)
    }

    #[inline]
    pub fn valid_vnode(&self, id: usize) -> bool {
        self.nodes[id].active
    }

    #[inline]
    pub fn effective_node(&self, id: usize) -> usize {
        id
    }

    #[inline]
    pub fn xm(&self, id: usize) -> usize {
        self.effective_node(id)
    }

    #[inline]
    pub(super) fn is_readonly_fs(&self, id: usize) -> bool {
        if matches!(self.nodes[id].fs_type, FsType::DevFS | FsType::ProcFS) {
            return true;
        }
        // Ext mounts start read-only (see fs::ext::mount::mount_root_disk), so
        // open(O_WRITE), unlink, mkdir and friends fail with "read-only
        // filesystem" instead of reaching the disk.
        //
        // Asked of this node's slot, not the active one: with a writable root
        // and a read-only image mounted, the active slot gives the wrong answer
        // in both directions
        if !self.nodes[id].fs_type.has_mounted_fs() {
            return false;
        }
        // Asked of the filesystem, through the registry, not of ext's mount
        // table: every mount table numbers its slots from zero, so ext's slot 0
        // says nothing about slot 0 of another filesystem.
        //
        // `None` is a vnode stamped with a filesystem that has nothing behind
        // it, and `Err` a mount that is gone. Both refuse
        match self.with_backing(id, |fs, _| Ok(fs.read_only())) {
            Some(Ok(ro)) => ro,
            _ => true,
        }
    }

    /// The mount options in force for this vnode.
    ///
    /// Resolved by walking up to the nearest mount root: a filesystem mounted
    /// inside another is governed by its own options. MNT_NOEXEC and MNT_NODEV
    /// keep a plugged-in disk from being a place to run programs or to present
    /// a device node. MNT_NOSUID is absent: exec on this system never raises
    /// anybody
    pub(super) fn mount_flags(&self, id: usize) -> u32 {
        let mut cur = id;
        for _ in 0..MAX_PATH_DEPTH {
            for m in self.mounts.mounts.iter() {
                if m.active && m.root_vnode as usize == cur {
                    return m.flags;
                }
            }
            let p = self.nodes[cur].parent as usize;
            if p == cur || !self.nodes.contains(p) { break; }
            cur = p;
        }
        0
    }

    /// The `/dev/ptmN` node for a slot, made at boot
    ///
    /// Found by its device numbers, among `/dev`'s children: the numbers are
    /// what everything else routes on
    pub(super) fn master_vnode(&self, slot: usize) -> Option<usize> {
        let want = devfs::PTY_MASTER_MINOR + slot as u8;
        let dev = self.nodes[0].children.find_by_name("dev")? as usize;
        self.nodes[dev].children.iter().map(|(_, id)| id as usize).find(|&i| {
            self.nodes[i].active
                && self.nodes[i].is_device()
                && self.nodes[i].dev_major == 5
                && self.nodes[i].dev_minor == want
        })
    }

    pub(super) fn get_dev_type(&self, id: usize) -> Option<devfs::DevType> {
        if self.nodes[id].is_device() {
            devfs::dev_type_from_node(self.nodes[id].dev_major, self.nodes[id].dev_minor)
        } else {
            None
        }
    }

    #[inline]
    pub(super) fn validate_name(name: &str) -> VfsResult<()> {
        if name.is_empty() || name.len() > NAME_LEN {
            return Err(VfsError::NameTooLong);
        }
        if name.contains('/') || name.contains('\0') {
            return Err(VfsError::InvalidArgument);
        }
        if name == "." || name == ".." {
            return Err(VfsError::InvalidArgument);
        }
        Ok(())
    }

    pub(super) fn ensure_no_duplicate(&mut self, parent: usize, name: &str) -> VfsResult<()> {
        let eff = self.effective_node(parent);
        match self.nodes[eff].children.probe(name) {
            Probe::Node(id) if self.valid_vnode(id as usize) => return Err(VfsError::AlreadyExists),
            // A whiteout's name is still on the disk until its file's last
            // close (`names.rs`); a negative entry is the filesystem's word
            // that the name is free
            Probe::Absent if self.nodes[eff].children.is_whiteout(name) => return Err(VfsError::AlreadyExists),
            Probe::Absent => return Ok(()),
            _ => {}
        }
        // A complete directory has every name it holds cached
        if self.nodes[eff].children_loaded && self.has_backing(eff) && name != "lost+found" {
            return Ok(());
        }
        // The tree is a cache and may not have this name yet. Only the
        // filesystem knows whether the directory holds it
        if self.has_backing(eff) {
            self.dcache.fs_lookups.bump();
        }
        if let Some(Ok(_)) = self.with_backing(eff, |fs, dir| fs.lookup(dir, name)) {
            return Err(VfsError::AlreadyExists);
        }
        Ok(())
    }

    pub(super) fn is_dir_empty(&self, id: usize) -> bool {
        let eff = self.effective_node(id);
        self.nodes[eff].children.is_empty()
    }

    pub(crate) fn evict_backed_children(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if !self.nodes.contains(cid) || !self.nodes[cid].active {
                continue;
            }
            if !self.nodes[cid].fs_type.has_mounted_fs() {
                continue;
            }
            if self.nodes[cid].refs() > 0 {
                continue;
            }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if self.nodes.contains(cid) && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_backed_children(cid);
                }
                // Pages not written out go to the disk before the vnode goes
                self.settle(cid);
                crate::fs::vfs::filecache::lock().drop_file(cid);
                // Every name it has goes with it, here and in any other
                // directory a hard link put it in
                self.free_vnode(cid);
            }
        }

        self.nodes[dir_id].children_loaded = false;
        // An unmount follows: the filesystem's misses are not the next one's
        self.nodes[dir_id].children.drop_negatives();
    }

    pub(crate) fn evict_children_recursive(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if !self.nodes.contains(cid) || !self.nodes[cid].active {
                continue;
            }
            if self.nodes[cid].refs() > 0 {
                continue;
            }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if self.nodes.contains(cid) && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_children_recursive(cid);
                }
                // Pages not written out go to the disk before the vnode goes,
                // as an unmount follows
                self.settle(cid);
                self.free_vnode(cid);
            }
        }

        self.nodes[dir_id].children_loaded = false;
        // An unmount follows: the filesystem's misses are not the next one's
        self.nodes[dir_id].children.drop_negatives();
    }
}
