// vnode management - allocation, validation, name checks, eviction

use super::MikuVFS;
use crate::vfs::devfs;
use crate::vfs::hash::name_hash;
use crate::vfs::procfs;
use crate::vfs::types::*;

impl MikuVFS {
    #[inline]
    pub(super) fn now(&self) -> Timestamp {
        procfs::uptime_ticks()
    }

    pub fn alloc_vnode(&mut self) -> VfsResult<usize> {
        let hint = self.vnode_free_hint;
        for i in hint..MAX_VNODES {
            if !self.nodes[i].active {
                self.vnode_free_hint = i + 1;
                return Ok(i);
            }
        }
        for i in 1..hint {
            if !self.nodes[i].active {
                self.vnode_free_hint = i + 1;
                return Ok(i);
            }
        }
        Err(VfsError::NoSpace)
    }

    #[inline]
    pub fn valid_vnode(&self, id: usize) -> bool {
        id < MAX_VNODES && self.nodes[id].active
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
        matches!(self.nodes[id].fs_type, FsType::DevFS | FsType::ProcFS)
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

    pub(super) fn ensure_no_duplicate(&self, parent: usize, name: &str) -> VfsResult<()> {
        let eff = self.effective_node(parent);
        if let Some(id) = self.nodes[eff].children.find_by_name(name) {
            let c = id as usize;
            if c < MAX_VNODES && self.nodes[c].active {
                return Err(VfsError::AlreadyExists);
            }
        }
        if self.nodes[eff].is_ext_backed() {
            let parent_ino = self.nodes[eff].ext2_ino;
            let exists = crate::commands::ext2_cmds::with_ext2_pub(|fs| {
                fs.ext2_lookup_in_dir(parent_ino, name)
                    .map(|r| r.is_some())
                    .unwrap_or(false)
            });
            if exists == Some(true) {
                return Err(VfsError::AlreadyExists);
            }
        }
        Ok(())
    }

    pub(super) fn is_dir_empty(&self, id: usize) -> bool {
        let eff = self.effective_node(id);
        self.nodes[eff].children.is_empty()
    }

    pub(crate) fn evict_ext2_children(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !self.nodes[cid].active {
                continue;
            }
            if !self.nodes[cid].fs_type.is_ext_family() {
                continue;
            }
            if self.nodes[cid].refcount > 0 {
                continue;
            }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_ext2_children(cid);
                }
                let h = name_hash(self.nodes[cid].get_name());
                self.nodes[dir_id].children.remove(h, cid as InodeId);
                self.nodes[cid].active = false;
            }
        }

        self.nodes[dir_id].children_loaded = false;
    }

    pub(crate) fn evict_children_recursive(&mut self, dir_id: usize) {
        let mut to_evict: alloc::vec::Vec<InodeId> = alloc::vec::Vec::new();

        for (_, child_id) in self.nodes[dir_id].children.iter() {
            let cid = child_id as usize;
            if cid >= MAX_VNODES || !self.nodes[cid].active {
                continue;
            }
            if self.nodes[cid].refcount > 0 {
                continue;
            }
            to_evict.push(child_id);
        }

        for child_id in to_evict {
            let cid = child_id as usize;
            if cid < MAX_VNODES && self.nodes[cid].active {
                if self.nodes[cid].is_dir() {
                    self.evict_children_recursive(cid);
                }
                self.free_file_pages(cid);
                let h = name_hash(self.nodes[cid].get_name());
                self.nodes[dir_id].children.remove(h, cid as InodeId);
                self.nodes[cid].active = false;
            }
        }

        self.nodes[dir_id].children_loaded = false;
    }
}
