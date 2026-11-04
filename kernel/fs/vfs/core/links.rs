use super::MikuVFS;
use crate::fs::vfs::types::*;

impl MikuVFS {
    pub fn symlink(
        &mut self,
        parent: usize,
        linkname: &str,
        target: &str,
    ) -> VfsResult<usize> {
        Self::validate_name(linkname)?;
        // A target is a path, so it is judged against `SYMLINK_MAX` and not
        // against the width of a single name
        if target.is_empty() || target.len() > SYMLINK_MAX {
            return Err(VfsError::NameTooLong);
        }

        let pid = self.effective_node(parent);
        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, linkname)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();

        self.init_vnode(id, 
            id as InodeId,
            pid as InodeId,
            linkname,
            VNodeKind::Symlink,
            self.nodes[pid].fs_type,
            FileMode::default_symlink(),
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );
        self.nodes[id].symlink_target.set(target.as_bytes());
        self.nodes[id].size = target.len() as u64;

        // The owner goes to the filesystem with the creation, in one call, so
        // the vnode and the inode cannot hold different owners
        let owner = (self.ctx.cred.euid as u32, self.ctx.cred.egid as u32);
        match self.with_backing(pid, |fs, dir| {
            fs.create(
                dir,
                linkname,
                VNodeKind::Symlink,
                FileMode::default_symlink().0,
                owner.0,
                owner.1,
                (0, 0),
                Some(target),
            )
        }) {
            Some(Ok(meta)) => {
                self.nodes[id].ext2_ino = meta.id.as_ino();
                self.nodes[id].ext2_slot = self.nodes[pid].ext2_slot;
            }
            Some(Err(e)) => {
                self.free_vnode(id);
                return Err(e);
            }
            None => {}
        }

        if !self.nodes[pid].children.insert(linkname, id as InodeId) {
            // A rollback that fails leaves a name on the disk that the tree
            // does not have. It cannot change what this call returns, but it
            // must not be silent
            if let Some(Err(e)) =
                self.with_backing(pid, |fs, dir| fs.unlink(dir, linkname, false))
            {
                crate::serial_println!(
                    "[vfs] rollback of link {:?} failed: {:?} - disk and tree now disagree",
                    linkname, e
                );
            }
            self.free_vnode(id);
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::trace_println!(
            "[vfs] symlink '{}' -> '{}' id={} ext2_ino={}",
            linkname,
            target,
            id,
            self.nodes[id].ext2_ino
        );
        crate::fs::vfs::notify::entry_event(pid, linkname, false, crate::fs::vfs::notify::IN_CREATE, 0);
        Ok(id)
    }

    /// The target, whole. A `Vec`: a path is not a name, and the caller's
    /// buffer decides how much it wants. `readlink` under the lock held to read
    /// (`with_vfs_read`)
    pub fn readlink_as(&self, cred: &Credentials, cwd: usize, path: &str) -> VfsResult<alloc::vec::Vec<u8>> {
        use super::path_resolve::{walk, Links, Reader};
        let id = walk(&mut Reader { vfs: self, cred }, cwd, path, Links::AllButLast)?;
        if !self.nodes[id].is_symlink() {
            return Err(VfsError::NotSymlink);
        }
        Ok(self.nodes[id].symlink_target.bytes().to_vec())
    }

    pub fn readlink(&mut self, cwd: usize, path: &str) -> VfsResult<alloc::vec::Vec<u8>> {
        let id = self.resolve_path_lstat(cwd, path)?;
        if !self.nodes[id].is_symlink() {
            return Err(VfsError::NotSymlink);
        }
        Ok(self.nodes[id].symlink_target.bytes().to_vec())
    }

    pub fn link(
        &mut self,
        cwd: usize,
        existing_path: &str,
        new_parent: usize,
        new_name: &str,
    ) -> VfsResult<()> {
        Self::validate_name(new_name)?;

        let target_id = self.resolve_path_follow(cwd, existing_path)?;

        if self.nodes[target_id].is_dir() {
            return Err(VfsError::IsDirectory);
        }
        if self.is_readonly_fs(target_id) {
            return Err(VfsError::ReadOnly);
        }

        let pid = self.effective_node(new_parent);
        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[pid].fs_type != self.nodes[target_id].fs_type {
            return Err(VfsError::CrossDevice);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, new_name)?;

        // Both nodes on one mount, checked by `with_backing_pair`, not by
        // comparing `fs_type` above: two ext4 volumes have the same type and
        // are different devices
        if let Some(r) = self.with_backing_pair(pid, target_id, |fs, dir, target| {
            fs.link(dir, new_name, target)
        }) {
            r?;
        }

        if !self.nodes[pid].children.insert(new_name, target_id as InodeId) {
            // The name went onto the disk and the child table has no room
            // for it. Take it back off, or the disk keeps a name the tree
            // cannot see
            let _ = self.with_backing(pid, |fs, dir| fs.unlink(dir, new_name, false));
            return Err(VfsError::NoSpace);
        }

        // One inode, two names: the vnode is both, and the icache finds it
        // again by the other name after this one is evicted (`names.rs`)
        self.add_name(target_id, pid, new_name);
        self.icache_add(target_id);
        self.nodes[target_id].nlinks += 1;
        let ts = self.now();
        self.nodes[target_id].touch_ctime(ts);
        self.nodes[pid].touch_mtime(ts);

        crate::trace_println!(
            "[vfs] hardlink '{}' id={} nlinks={}",
            new_name,
            target_id,
            self.nodes[target_id].nlinks
        );
        // A new name is a create in its directory and a link count change on
        // the file: `IN_ATTRIB` on Linux
        crate::fs::vfs::notify::entry_event(pid, new_name, false, crate::fs::vfs::notify::IN_CREATE, 0);
        crate::fs::vfs::notify::node_event(self, target_id, crate::fs::vfs::notify::IN_ATTRIB);
        Ok(())
    }
}
