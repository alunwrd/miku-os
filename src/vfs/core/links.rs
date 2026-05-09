use super::MikuVFS;
use crate::vfs::types::*;

impl MikuVFS {
    pub fn symlink(
        &mut self,
        parent: usize,
        linkname: &str,
        target: &str,
    ) -> VfsResult<usize> {
        Self::validate_name(linkname)?;
        if target.is_empty() || target.len() > NAME_LEN {
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

        self.nodes[id].init(
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
        self.nodes[id].symlink_target = NameBuf::from_str(target);
        self.nodes[id].size = target.len() as u64;

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_symlink(parent_ino, linkname, target)
            });
            match result {
                Some(Ok(new_ino)) => {
                    self.nodes[id].ext2_ino = new_ino;
                }
                Some(Err(_)) | None => {
                    self.nodes[id].active = false;
                    return Err(VfsError::IoError);
                }
            }
        }

        if !self.nodes[pid].children.insert(linkname, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, linkname)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] symlink '{}' -> '{}' id={} ext2_ino={}",
            linkname,
            target,
            id,
            self.nodes[id].ext2_ino
        );
        Ok(id)
    }

    pub fn readlink(&mut self, cwd: usize, path: &str) -> VfsResult<NameBuf> {
        let id = self.resolve_path_lstat(cwd, path)?;
        if !self.nodes[id].is_symlink() {
            return Err(VfsError::NotSymlink);
        }
        Ok(self.nodes[id].symlink_target)
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

        if self.nodes[pid].is_ext_backed()
            && self.nodes[pid].ext2_ino != 0
            && self.nodes[target_id].ext2_ino != 0
        {
            let parent_ino = self.nodes[pid].ext2_ino;
            let target_ino = self.nodes[target_id].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_hardlink(parent_ino, new_name, target_ino)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        if !self.nodes[pid].children.insert(new_name, target_id as InodeId) {
            if self.nodes[pid].is_ext_backed() && self.nodes[pid].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_file(parent_ino, new_name)
                });
            }
            return Err(VfsError::NoSpace);
        }

        self.nodes[target_id].nlinks += 1;
        let ts = self.now();
        self.nodes[target_id].touch_ctime(ts);
        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] hardlink '{}' id={} nlinks={}",
            new_name,
            target_id,
            self.nodes[target_id].nlinks
        );
        Ok(())
    }
}
