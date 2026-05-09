// Directory operations: mkdir, rmdir, readdir, rename

use super::MikuVFS;
use crate::vfs::hash::name_hash;
use crate::vfs::types::*;

impl MikuVFS {
    pub fn mkdir(&mut self, parent: usize, name: &str, mode: FileMode) -> VfsResult<usize> {
        Self::validate_name(name)?;
        let pid = self.effective_node(parent);

        if !self.nodes[pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.is_readonly_fs(pid) {
            return Err(VfsError::ReadOnly);
        }
        self.check_dir_write(pid)?;
        self.ensure_no_duplicate(pid, name)?;

        let id = self.alloc_vnode()?;
        let ts = self.now();
        let applied_mode = mode.apply_umask(self.ctx.umask);

        self.nodes[id].init(
            id as InodeId,
            pid as InodeId,
            name,
            VNodeKind::Directory,
            self.nodes[pid].fs_type,
            applied_mode,
            self.ctx.cred.euid,
            self.ctx.cred.egid,
            ts,
        );

        if self.nodes[pid].is_ext_backed() {
            let parent_ino = self.nodes[pid].ext2_ino;
            let disk_mode = applied_mode.0;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_create_dir(parent_ino, name, disk_mode)
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

        if !self.nodes[pid].children.insert(name, id as InodeId) {
            if self.nodes[id].ext2_ino != 0 {
                let parent_ino = self.nodes[pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_delete_dir(parent_ino, name)
                });
            }
            self.nodes[id].active = false;
            return Err(VfsError::NoSpace);
        }

        self.nodes[pid].nlinks += 1;
        self.nodes[pid].touch_mtime(ts);

        crate::serial_println!(
            "[vfs] mkdir '{}' id={} parent={} ext2_ino={}",
            name,
            id,
            pid,
            self.nodes[id].ext2_ino
        );
        Ok(id)
    }

    pub fn rmdir(&mut self, cwd: usize, path: &str) -> VfsResult<()> {
        let id = self.resolve_path(cwd, path)?;

        if !self.nodes[id].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if id == 0 {
            return Err(VfsError::PermissionDenied);
        }
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        if self.nodes[id].is_ext_backed() && !self.nodes[id].children_loaded {
            self.ext2_ensure_children_loaded(id)?;
        }

        if !self.is_dir_empty(id) {
            return Err(VfsError::NotEmpty);
        }

        let pid = self.nodes[id].parent as usize;
        self.check_dir_write(pid)?;

        if self.nodes[id].is_ext_backed() && self.nodes[pid].ext2_ino != 0 {
            let parent_ino = self.nodes[pid].ext2_ino;
            let dir_name = self.nodes[id].get_name();
            let mut name_buf = [0u8; NAME_LEN];
            let nlen = dir_name.len().min(NAME_LEN);
            name_buf[..nlen].copy_from_slice(dir_name.as_bytes());
            let name_str = unsafe { core::str::from_utf8_unchecked(&name_buf[..nlen]) };
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_delete_dir(parent_ino, name_str)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        let h = name_hash(self.nodes[id].get_name());
        self.nodes[pid].children.remove(h, id as InodeId);

        if self.nodes[pid].nlinks > 0 {
            self.nodes[pid].nlinks -= 1;
        }

        let ts = self.now();
        self.nodes[pid].touch_mtime(ts);
        self.nodes[id].nlinks = 0;
        self.nodes[id].active = false;
        if id < self.vnode_free_hint {
            self.vnode_free_hint = id;
        }

        crate::serial_println!("[vfs] rmdir '{}' id={}", path, id);
        Ok(())
    }

    pub fn readdir(&mut self, dir_id: usize, entries: &mut [DirEntry]) -> VfsResult<usize> {
        if !self.valid_vnode(dir_id) {
            return Err(VfsError::NotFound);
        }
        if !self.nodes[dir_id].is_dir() {
            return Err(VfsError::NotDirectory);
        }

        if self.nodes[dir_id].fs_type.is_ext_family() && !self.nodes[dir_id].children_loaded {
            self.ext2_ensure_children_loaded(dir_id)?;
        }

        let eff = self.effective_node(dir_id);
        let mut count = 0usize;

        if count < entries.len() {
            entries[count] = DirEntry::from_name(".", dir_id as InodeId, VNodeKind::Directory);
            entries[count].offset = 0;
            count += 1;
        }
        if count < entries.len() {
            let parent_id = self.nodes[dir_id].parent as usize;
            let par = if self.valid_vnode(parent_id) {
                parent_id
            } else {
                dir_id
            };
            entries[count] = DirEntry::from_name("..", par as InodeId, VNodeKind::Directory);
            entries[count].offset = 1;
            count += 1;
        }

        for (_, child_id) in self.nodes[eff].children.iter() {
            if count >= entries.len() {
                break;
            }
            let cid = child_id as usize;
            if !self.valid_vnode(cid) {
                continue;
            }
            entries[count] = DirEntry::from_name(
                self.nodes[cid].get_name(),
                cid as InodeId,
                self.nodes[cid].kind,
            );
            entries[count].offset = count as u32;
            count += 1;
        }

        Ok(count)
    }

    pub fn rename(&mut self, cwd: usize, old: &str, new_path: &str) -> VfsResult<()> {
        let id = self.resolve_path(cwd, old)?;
        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if self.nodes[id].flags.immutable {
            return Err(VfsError::PermissionDenied);
        }

        let old_pid = self.nodes[id].parent as usize;

        let (new_pid, new_base) = self.split_path(cwd, new_path)?;
        Self::validate_name(new_base)?;

        if !self.nodes[new_pid].is_dir() {
            return Err(VfsError::NotDirectory);
        }
        if self.nodes[new_pid].fs_type != self.nodes[id].fs_type {
            return Err(VfsError::CrossDevice);
        }

        self.check_dir_write(old_pid)?;
        if new_pid != old_pid {
            self.check_dir_write(new_pid)?;
        }

        // do not allow moving a directory into itself
        if self.nodes[id].is_dir() {
            let mut cur = new_pid;
            loop {
                if cur == id {
                    return Err(VfsError::InvalidArgument);
                }
                let p = self.nodes[cur].parent as usize;
                if p == cur || cur == 0 {
                    break;
                }
                cur = p;
            }
        }

        if let Some(existing) = self.nodes[new_pid].children.find_by_name(new_base) {
            let c = existing as usize;
            if c < MAX_VNODES && self.nodes[c].active && c != id {
                return Err(VfsError::AlreadyExists);
            }
        }

        if self.nodes[id].is_ext_backed()
            && self.nodes[old_pid].ext2_ino != 0
            && self.nodes[id].ext2_ino != 0
        {
            if new_pid != old_pid {
                return Err(VfsError::CrossDevice);
            }
            let parent_ino = self.nodes[old_pid].ext2_ino;
            let old_name = self.nodes[id].get_name();
            let mut old_buf = [0u8; NAME_LEN];
            let olen = old_name.len().min(NAME_LEN);
            old_buf[..olen].copy_from_slice(old_name.as_bytes());
            let old_str = unsafe { core::str::from_utf8_unchecked(&old_buf[..olen]) };
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext3_rename(parent_ino, old_str, new_base)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        let mut old_name_buf = [0u8; NAME_LEN];
        let old_name_len = self.nodes[id].name.len as usize;
        old_name_buf[..old_name_len].copy_from_slice(&self.nodes[id].name.data[..old_name_len]);
        let old_name_str = unsafe { core::str::from_utf8_unchecked(&old_name_buf[..old_name_len]) };

        self.nodes[old_pid].children.remove_by_name(old_name_str);
        self.nodes[id].name = NameBuf::from_str(new_base);
        self.nodes[id].parent = new_pid as InodeId;

        if !self.nodes[new_pid].children.insert(new_base, id as InodeId) {
            // roll back in-memory changes and ext2 rename
            let rollback_name = match core::str::from_utf8(&old_name_buf[..old_name_len]) {
                Ok(s) => s,
                Err(_) => {
                    self.nodes[id].active = false;
                    return Err(VfsError::Corrupted);
                }
            };
            self.nodes[id].name = NameBuf::from_str(rollback_name);
            self.nodes[id].parent = old_pid as InodeId;
            let _ = self.nodes[old_pid]
                .children
                .insert(rollback_name, id as InodeId);
            if self.nodes[id].is_ext_backed() && self.nodes[old_pid].ext2_ino != 0 {
                let parent_ino = self.nodes[old_pid].ext2_ino;
                let _ = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext3_rename(parent_ino, new_base, rollback_name)
                });
            }
            return Err(VfsError::NoSpace);
        }

        let ts = self.now();

        if new_pid != old_pid && self.nodes[id].is_dir() {
            if self.nodes[old_pid].nlinks > 0 {
                self.nodes[old_pid].nlinks -= 1;
            }
            self.nodes[new_pid].nlinks = self.nodes[new_pid].nlinks.saturating_add(1);
            self.nodes[old_pid].touch_mtime(ts);
        }

        self.nodes[id].touch_ctime(ts);
        self.nodes[new_pid].touch_mtime(ts);

        Ok(())
    }
}
