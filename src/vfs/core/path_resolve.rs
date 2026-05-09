// Path resolution - resolve_path, follow_symlink, lookup_child_or_load, split_path

use super::MikuVFS;
use crate::vfs::types::*;

impl MikuVFS {
    pub fn resolve_path(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(cwd);
        }

        let mut current = if path.starts_with('/') { 0 } else { cwd };
        let mut depth = 0u8;

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }

            depth += 1;
            if depth as usize > MAX_PATH_DEPTH {
                return Err(VfsError::InvalidPath);
            }

            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }

            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;

            if self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, 0)?;
            }
        }
        Ok(current)
    }

    pub fn resolve_path_lstat(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let path = path.trim();
        if path.is_empty() {
            return Ok(cwd);
        }

        let total = path
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .count();
        let mut idx = 0usize;
        let mut current = if path.starts_with('/') { 0 } else { cwd };
        let mut depth = 0u8;

        for component in path.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }

            idx += 1;
            depth += 1;
            if depth as usize > MAX_PATH_DEPTH {
                return Err(VfsError::InvalidPath);
            }

            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }

            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;

            // intermediate components resolve symbolic links, the final one does not
            if idx < total && self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, 0)?;
            }
        }
        Ok(current)
    }

    pub(super) fn follow_symlink(&mut self, link_id: usize, depth: usize) -> VfsResult<usize> {
        if depth >= MAX_SYMLINK_DEPTH {
            return Err(VfsError::TooManySymlinks);
        }
        if !self.nodes[link_id].is_symlink() {
            return Ok(link_id);
        }

        let mut target_buf = [0u8; NAME_LEN];
        let target_len = self.nodes[link_id].symlink_target.len as usize;
        target_buf[..target_len]
            .copy_from_slice(&self.nodes[link_id].symlink_target.data[..target_len]);

        let target_str = match core::str::from_utf8(&target_buf[..target_len]) {
            Ok(s) => s,
            Err(_) => return Err(VfsError::InvalidPath),
        };

        if target_str.is_empty() {
            return Err(VfsError::InvalidPath);
        }

        let parent = self.nodes[link_id].parent as usize;
        let start = if target_str.starts_with('/') { 0 } else { parent };

        let mut current = start;
        for component in target_str.split('/') {
            if component.is_empty() || component == "." {
                continue;
            }
            if component == ".." {
                let p = self.nodes[current].parent;
                if p != INVALID_ID {
                    current = p as usize;
                }
                continue;
            }
            if !self.nodes[current].is_dir() {
                return Err(VfsError::NotDirectory);
            }
            let eff = self.xm(current);
            current = self.lookup_child_or_load(eff, component)?;
            if self.nodes[current].is_symlink() {
                current = self.follow_symlink(current, depth + 1)?;
            }
        }
        Ok(current)
    }

    pub(super) fn lookup_child_or_load(&mut self, parent: usize, name: &str) -> VfsResult<usize> {
        if let Some(id) = self.nodes[parent].children.find_by_name(name) {
            let cid = id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                return Ok(cid);
            }
        }

        if self.nodes[parent].fs_type.is_ext_family() && self.nodes[parent].ext2_ino != 0 {
            return self.ext2_lazy_lookup(parent, name);
        }

        Err(VfsError::NotFound)
    }

    pub fn resolve_path_follow(&mut self, cwd: usize, path: &str) -> VfsResult<usize> {
        let mut id = self.resolve_path(cwd, path)?;
        let mut depth = 0;
        while self.nodes[id].is_symlink() {
            if depth >= MAX_SYMLINK_DEPTH {
                return Err(VfsError::TooManySymlinks);
            }
            let mut target_buf = [0u8; NAME_LEN];
            let tlen = self.nodes[id].symlink_target.len as usize;
            target_buf[..tlen].copy_from_slice(&self.nodes[id].symlink_target.data[..tlen]);
            let target = unsafe { core::str::from_utf8_unchecked(&target_buf[..tlen]) };
            if target.is_empty() {
                return Err(VfsError::InvalidPath);
            }
            let parent = self.nodes[id].parent as usize;
            id = self.resolve_path(parent, target)?;
            depth += 1;
        }
        Ok(id)
    }

    pub(super) fn split_path<'a>(
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
