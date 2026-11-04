// stat/chmod/chown/setattr, statfs, vnode/mount counters

use super::MikuVFS;
use crate::fs::vfs::fsops::FsSetAttr;
use crate::fs::vfs::path::PathWalker;
use crate::fs::vfs::types::*;
use crate::fs::vfs::core::access::strip_setid;
use crate::process::cred::CAP_CHOWN;

impl MikuVFS {
    pub fn stat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path_follow(cwd, path)?;
        self.refresh_attrs_from_fs(id);
        Ok(self.nodes[id].stat())
    }

    /// `stat` that stops at the link.
    ///
    /// `resolve_path_lstat`, not `resolve_path`: they differ only in the last
    /// component, where `resolve_path` follows a symlink. Following it here
    /// would make `symlink_metadata` report "regular file" about a link, and
    /// `remove_dir_all`, asking this about every entry it walks, would get the
    /// target's kind and miss a dangling link's target
    pub fn lstat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path_lstat(cwd, path)?;
        self.refresh_attrs_from_fs(id);
        Ok(self.nodes[id].stat())
    }

    /// `stat`, or with `links` `AllButLast` `lstat`, under the lock held to
    /// read (`with_vfs_read`): from what is cached, as `cred`. `Deferred`
    /// for a name not cached, and for a disk file whose attributes are not
    /// known to be the filesystem's (`VNode::attrs_fresh`), which `stat`
    /// asks for and keeps
    pub fn stat_as(
        &self,
        cred: &Credentials,
        cwd: usize,
        path: &str,
        links: super::path_resolve::Links,
    ) -> VfsResult<VNodeStat> {
        use super::path_resolve::{walk, walk_follow, Links, Reader};
        let mut r = Reader { vfs: self, cred };
        let id = match links {
            Links::All => walk_follow(&mut r, cwd, path)?,
            Links::AllButLast => walk(&mut r, cwd, path, Links::AllButLast)?,
        };
        if self.has_backing(id) && !self.nodes[id].attrs_fresh.load(core::sync::atomic::Ordering::Relaxed) {
            return Err(VfsError::Deferred);
        }
        Ok(self.nodes[id].stat())
    }

    pub fn fstat(&mut self, fd: usize) -> VfsResult<VNodeStat> {
        let vid = self.fds().get(fd)?.vnode_id as usize;
        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        self.refresh_attrs_from_fs(vid);
        Ok(self.nodes[vid].stat())
    }

    pub fn chmod(&mut self, cwd: usize, path: &str, mode: FileMode) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        self.check_owner(id)?;

        // A caller not in the file's group may not mark it setgid. The bit
        // raises nobody here, but the mode would be false on a disk read where
        // it matters
        let mode = if mode.setgid()
            && !self.ctx.cred.in_group(self.nodes[id].gid)
            && !self.ctx.cred.has(crate::process::cred::CAP_FOWNER)
        {
            FileMode(mode.0 & !0o2000)
        } else {
            mode
        };

        let attr = FsSetAttr { mode: Some(mode.0), ..Default::default() };
        if let Some(r) = self.with_backing(id, |fs, node| fs.setattr(node, &attr)) {
            r?;
        }

        self.nodes[id].mode = mode;
        let now = self.now();
        self.nodes[id].touch_ctime(now);
        crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_ATTRIB);
        Ok(())
    }

    pub fn chown(
        &mut self,
        cwd: usize,
        path: &str,
        uid: Option<u16>,
        gid: Option<u16>,
    ) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        let new_uid = uid.unwrap_or(self.nodes[id].uid);
        let new_gid = gid.unwrap_or(self.nodes[id].gid);
        self.check_chown(id, new_uid, new_gid)?;

        let attr = FsSetAttr {
            uid: Some(new_uid as u32),
            gid: Some(new_gid as u32),
            ..Default::default()
        };
        if let Some(r) = self.with_backing(id, |fs, node| fs.setattr(node, &attr)) {
            r?;
        }

        self.nodes[id].uid = new_uid;
        self.nodes[id].gid = new_gid;
        self.strip_setid_after_chown(id);
        let now = self.now();
        self.nodes[id].touch_ctime(now);
        crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_ATTRIB);
        Ok(())
    }

    /// Who may change a file's owner, and to what.
    ///
    /// Two separate powers:
    ///
    ///   - changing the user gives the file away, and needs CAP_CHOWN. A user
    ///     who could do it freely could plant a file in someone's name, or
    ///     dodge a quota.
    ///   - changing the group to one you are in, on a file you own, gives
    ///     nothing away
    fn check_chown(&self, id: usize, new_uid: u16, new_gid: u16) -> VfsResult<()> {
        let cred = &self.ctx.cred;
        if cred.has(CAP_CHOWN) {
            return Ok(());
        }
        if new_uid != self.nodes[id].uid {
            return Err(VfsError::PermissionDenied);
        }
        if new_gid != self.nodes[id].gid {
            if cred.euid != self.nodes[id].uid || !cred.in_group(new_gid) {
                return Err(VfsError::PermissionDenied);
            }
        }
        Ok(())
    }

    /// A file that has changed hands does not keep its set-id bits, also for a
    /// privileged caller: a mode surviving a change of owner no longer
    /// describes what it did when set
    fn strip_setid_after_chown(&mut self, id: usize) {
        let stripped = strip_setid(self.nodes[id].mode);
        if stripped == self.nodes[id].mode {
            return;
        }
        let attr = FsSetAttr { mode: Some(stripped.0), ..Default::default() };
        let _ = self.with_backing(id, |fs, node| fs.setattr(node, &attr));
        self.nodes[id].mode = stripped;
    }

    pub fn setattr(&mut self, cwd: usize, path: &str, attr: SetAttr) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if let Some(mode) = attr.mode {
            self.check_owner(id)?;
            let fa = FsSetAttr { mode: Some(mode.0), ..Default::default() };
            if let Some(r) = self.with_backing(id, |fs, node| fs.setattr(node, &fa)) {
                r?;
            }
            self.nodes[id].mode = mode;
        }

        if attr.uid.is_some() || attr.gid.is_some() {
            let new_uid = attr.uid.unwrap_or(self.nodes[id].uid);
            let new_gid = attr.gid.unwrap_or(self.nodes[id].gid);
            self.check_chown(id, new_uid, new_gid)?;
            let fa = FsSetAttr {
                uid: Some(new_uid as u32),
                gid: Some(new_gid as u32),
                ..Default::default()
            };
            if let Some(r) = self.with_backing(id, |fs, node| fs.setattr(node, &fa)) {
                r?;
            }
            self.nodes[id].uid = new_uid;
            self.nodes[id].gid = new_gid;
            self.strip_setid_after_chown(id);
        }

        if let Some(new_size) = attr.size {
            if !self.nodes[id].is_regular() {
                return Err(VfsError::InvalidArgument);
            }
            self.truncate_to(id, new_size)?;
        }

        if let Some(atime) = attr.atime {
            self.nodes[id].atime = atime;
        }
        if let Some(mtime) = attr.mtime {
            self.nodes[id].mtime = mtime;
        }

        let now = self.now();
        self.nodes[id].touch_ctime(now);
        // A size change raised its own IN_MODIFY in `truncate_to`; the rest
        // is metadata
        if attr.mode.is_some() || attr.uid.is_some() || attr.gid.is_some()
            || attr.atime.is_some() || attr.mtime.is_some()
        {
            crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_ATTRIB);
        }
        Ok(())
    }

    /// A vnode's access and modification times, `None` leaving one as it is:
    /// `utimensat` (40). The owner may set them, as for `chmod`; a mounted
    /// filesystem is told first and has the last word, as for a mode
    pub fn set_times(
        &mut self,
        id: usize,
        atime: Option<Timestamp>,
        mtime: Option<Timestamp>,
    ) -> VfsResult<()> {
        if !self.valid_vnode(id) {
            return Err(VfsError::NotFound);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }
        self.check_owner(id)?;
        let fa = FsSetAttr { atime, mtime, ..Default::default() };
        if let Some(r) = self.with_backing(id, |fs, node| fs.setattr(node, &fa)) {
            r?;
        }
        if let Some(t) = atime {
            self.nodes[id].atime = t;
        }
        if let Some(t) = mtime {
            self.nodes[id].mtime = t;
        }
        let now = self.now();
        self.nodes[id].touch_ctime(now);
        crate::fs::vfs::notify::node_event(self, id, crate::fs::vfs::notify::IN_ATTRIB);
        Ok(())
    }

    /// Now, in the unit `set_times` stores, for `UTIME_NOW`
    pub fn time_now(&self) -> Timestamp {
        self.now()
    }

    pub fn statfs(&self, cwd: usize, path: &str) -> VfsResult<StatFs> {
        let id = PathWalker::resolve(&self.nodes, cwd, path)?;
        let mut fs_type = self.nodes[id].fs_type;

        // Ask the filesystem, not the stamp given to the vnode at creation:
        // `statfs` answers both questions in one call
        if let Some(Ok(st)) = self.with_backing(id, |fs, _| fs.statfs()) {
            return Ok(st);
        }
        // Everything else is tmpfs: the vnode table and the page cache are the
        // medium, so the numbers come from them. The vnode table has no size:
        // what could still be made is what free memory holds
        let (frames_used, frames_total) = crate::mm::pmm::stats();
        let room = frames_total.saturating_sub(frames_used) * (PAGE_SIZE / core::mem::size_of::<crate::fs::vfs::vnode::VNode>());
        let used_inodes = self.total_vnodes() as u64;
        let total_inodes = used_inodes + room as u64;

        Ok(StatFs {
            fs_type,
            block_size: PAGE_SIZE as u32,
            total_blocks: self.page_cache.total_capacity() as u64,
            free_blocks: self.page_cache.free_pages() as u64,
            total_inodes,
            free_inodes: total_inodes.saturating_sub(used_inodes),
            max_name_len: NAME_LEN as u32,
            flags: if self.is_readonly_fs(id) { 1 } else { 0 },
        })
    }

    pub fn total_vnodes(&self) -> usize {
        self.nodes.len()
    }

    pub fn total_mounts(&self) -> usize {
        self.mounts.count as usize
    }

    pub fn is_vnode_open(&self, vid: usize) -> bool {
        // open == any process holds an active fd on this vnode
        crate::fs::vfs::fd::any_open(vid)
    }
}
