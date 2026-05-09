// Attribute requests and modifications: stat/chmod/chown/setattr, statfs, counters

use super::MikuVFS;
use crate::vfs::path::PathWalker;
use crate::vfs::types::*;

impl MikuVFS {
    pub fn stat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path_follow(cwd, path)?;
        self.ext2_refresh_size(id);
        Ok(self.nodes[id].stat())
    }

    pub fn lstat(&mut self, cwd: usize, path: &str) -> VfsResult<VNodeStat> {
        let id = self.resolve_path(cwd, path)?;
        self.ext2_refresh_size(id);
        Ok(self.nodes[id].stat())
    }

    pub fn fstat(&mut self, fd: usize) -> VfsResult<VNodeStat> {
        let vid = self.fd_table.get(fd)?.vnode_id as usize;
        if !self.valid_vnode(vid) {
            return Err(VfsError::BadFd);
        }
        self.ext2_refresh_size(vid);
        Ok(self.nodes[vid].stat())
    }

    pub fn chmod(&mut self, cwd: usize, path: &str, mode: FileMode) -> VfsResult<()> {
        let id = self.resolve_path_follow(cwd, path)?;

        if self.is_readonly_fs(id) {
            return Err(VfsError::ReadOnly);
        }
        if !self.ctx.cred.is_root() && self.ctx.cred.euid != self.nodes[id].uid {
            return Err(VfsError::PermissionDenied);
        }

        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let result =
                crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext2_chmod(ino, mode.0));
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        self.nodes[id].mode = mode;
        self.nodes[id].touch_ctime(self.now());
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
        if !self.ctx.cred.is_root() {
            return Err(VfsError::PermissionDenied);
        }

        let new_uid = uid.unwrap_or(self.nodes[id].uid);
        let new_gid = gid.unwrap_or(self.nodes[id].gid);

        if self.nodes[id].is_ext_backed() {
            let ino = self.nodes[id].ext2_ino;
            let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_chown(ino, new_uid, new_gid)
            });
            match result {
                Some(Ok(())) => {}
                Some(Err(_)) | None => return Err(VfsError::IoError),
            }
        }

        self.nodes[id].uid = new_uid;
        self.nodes[id].gid = new_gid;
        self.nodes[id].touch_ctime(self.now());
        Ok(())
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
            if !self.ctx.cred.is_root() && self.ctx.cred.euid != self.nodes[id].uid {
                return Err(VfsError::PermissionDenied);
            }
            if self.nodes[id].is_ext_backed() {
                let ino = self.nodes[id].ext2_ino;
                let result =
                    crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.ext2_chmod(ino, mode.0));
                if matches!(result, Some(Err(_)) | None) {
                    return Err(VfsError::IoError);
                }
            }
            self.nodes[id].mode = mode;
        }

        if attr.uid.is_some() || attr.gid.is_some() {
            if !self.ctx.cred.is_root() {
                return Err(VfsError::PermissionDenied);
            }
            let new_uid = attr.uid.unwrap_or(self.nodes[id].uid);
            let new_gid = attr.gid.unwrap_or(self.nodes[id].gid);
            if self.nodes[id].is_ext_backed() {
                let ino = self.nodes[id].ext2_ino;
                let result = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                    fs.ext2_chown(ino, new_uid, new_gid)
                });
                if matches!(result, Some(Err(_)) | None) {
                    return Err(VfsError::IoError);
                }
            }
            self.nodes[id].uid = new_uid;
            self.nodes[id].gid = new_gid;
        }

        if let Some(new_size) = attr.size {
            if !self.nodes[id].is_regular() {
                return Err(VfsError::InvalidArgument);
            }
            self.truncate_to(id, new_size);
        }

        if let Some(atime) = attr.atime {
            self.nodes[id].atime = atime;
        }
        if let Some(mtime) = attr.mtime {
            self.nodes[id].mtime = mtime;
        }

        self.nodes[id].touch_ctime(self.now());
        Ok(())
    }

    pub fn statfs(&self, cwd: usize, path: &str) -> VfsResult<StatFs> {
        let id = PathWalker::resolve(&self.nodes, cwd, path)?;
        let fs_type = self.nodes[id].fs_type;

        if fs_type.is_ext_family() {
            let info = crate::commands::ext2_cmds::with_ext2_pub(|fs| fs.fs_info());
            if let Some(info) = info {
                return Ok(StatFs {
                    fs_type,
                    block_size: info.block_size,
                    total_blocks: info.total_blocks as u64,
                    free_blocks: info.free_blocks as u64,
                    total_inodes: info.total_inodes as u64,
                    free_inodes: info.free_inodes as u64,
                    max_name_len: 255,
                    flags: 0,
                });
            }
        }

        let total_inodes = MAX_VNODES as u64;
        let used_inodes = self.total_vnodes() as u64;

        Ok(StatFs {
            fs_type,
            block_size: PAGE_SIZE as u32,
            total_blocks: MAX_DATA_PAGES as u64,
            free_blocks: self.page_cache.slab.free_count() as u64,
            total_inodes,
            free_inodes: total_inodes.saturating_sub(used_inodes),
            max_name_len: NAME_LEN as u32,
            flags: if self.is_readonly_fs(id) { 1 } else { 0 },
        })
    }

    pub fn total_vnodes(&self) -> usize {
        self.nodes.iter().filter(|v| v.active).count()
    }

    pub fn total_mounts(&self) -> usize {
        self.mounts.count as usize
    }

    pub fn is_vnode_open(&self, vid: usize) -> bool {
        for i in 0..MAX_OPEN_FILES {
            if self.fd_table.files[i].active && self.fd_table.files[i].vnode_id as usize == vid {
                return true;
            }
        }
        false
    }
}
