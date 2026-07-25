use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct MountEntry {
    pub id: u8,
    pub fs_type: FsType,
    pub root_vnode: InodeId,
    pub parent_vnode: InodeId,
    pub flags: u32,
    pub active: bool,
    pub read_only: bool,
}

impl MountEntry {
    pub const fn empty() -> Self {
        Self {
            id: INVALID_U8,
            fs_type: FsType::TmpFS,
            root_vnode: INVALID_ID,
            parent_vnode: INVALID_ID,
            flags: 0,
            active: false,
            read_only: false,
        }
    }
}

pub const MNT_RDONLY: u32 = 0x01;
pub const MNT_NOSUID: u32 = 0x02;
pub const MNT_NODEV: u32 = 0x04;
pub const MNT_NOEXEC: u32 = 0x08;
pub const MNT_NOATIME: u32 = 0x10;

pub struct MountTable {
    pub mounts: [MountEntry; MAX_MOUNTS],
    pub count: u8,
}

impl MountTable {
    pub const fn new() -> Self {
        Self {
            mounts: [MountEntry::empty(); MAX_MOUNTS],
            count: 0,
        }
    }

    pub fn add(&mut self, fs_type: FsType, root: InodeId, parent: InodeId) -> VfsResult<u8> {
        for i in 0..MAX_MOUNTS {
            if !self.mounts[i].active {
                self.mounts[i] = MountEntry {
                    id: i as u8,
                    fs_type,
                    root_vnode: root,
                    parent_vnode: parent,
                    flags: 0,
                    active: true,
                    read_only: matches!(fs_type, FsType::ProcFS),
                };
                self.count += 1;
                return Ok(i as u8);
            }
        }
        Err(VfsError::NoSpace)
    }

    pub fn remove(&mut self, id: u8) -> VfsResult<()> {
        let i = id as usize;
        if i < MAX_MOUNTS && self.mounts[i].active {
            self.mounts[i] = MountEntry::empty();
            if self.count > 0 {
                self.count -= 1;
            }
            Ok(())
        } else {
            Err(VfsError::NotMounted)
        }
    }

    pub fn get(&self, id: u8) -> Option<&MountEntry> {
        let i = id as usize;
        if i < MAX_MOUNTS && self.mounts[i].active {
            Some(&self.mounts[i])
        } else {
            None
        }
    }

    pub fn find_by_mountpoint(&self, vnode_id: InodeId) -> Option<&MountEntry> {
        for i in 0..MAX_MOUNTS {
            if self.mounts[i].active && self.mounts[i].parent_vnode == vnode_id {
                return Some(&self.mounts[i]);
            }
        }
        None
    }

    pub fn find_by_root(&self, vnode_id: InodeId) -> Option<&MountEntry> {
        for i in 0..MAX_MOUNTS {
            if self.mounts[i].active && self.mounts[i].root_vnode == vnode_id {
                return Some(&self.mounts[i]);
            }
        }
        None
    }

    pub fn is_readonly(&self, mount_id: u8) -> bool {
        self.get(mount_id)
            .map(|m| m.read_only || m.flags & MNT_RDONLY != 0)
            .unwrap_or(false)
    }

    pub fn iter(&self) -> MountIter<'_> {
        MountIter {
            table: self,
            pos: 0,
        }
    }
}

pub struct MountIter<'a> {
    table: &'a MountTable,
    pos: usize,
}

impl<'a> Iterator for MountIter<'a> {
    type Item = &'a MountEntry;

    fn next(&mut self) -> Option<Self::Item> {
        while self.pos < MAX_MOUNTS {
            let idx = self.pos;
            self.pos += 1;
            if self.table.mounts[idx].active {
                return Some(&self.table.mounts[idx]);
            }
        }
        None
    }
}
