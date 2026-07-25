use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct FileVersion {
    pub vnode_id: InodeId,
    pub version: u32,
    pub size: u64,
    pub page_id: PageId,
    pub timestamp: Timestamp,
    pub active: bool,
}

impl FileVersion {
    pub const fn empty() -> Self {
        Self {
            vnode_id: INVALID_ID,
            version: 0,
            size: 0,
            page_id: INVALID_ID,
            timestamp: 0,
            active: false,
        }
    }
}

pub struct VersionStore {
    pub versions: [FileVersion; MAX_VERSIONS],
    pub next_version: u32,
}

impl VersionStore {
    pub const fn new() -> Self {
        Self {
            versions: [FileVersion::empty(); MAX_VERSIONS],
            next_version: 1,
        }
    }

    pub fn snapshot(
        &mut self,
        vnode_id: InodeId,
        size: u64,
        page_id: PageId,
        timestamp: Timestamp,
    ) -> VfsResult<u32> {
        let mut target = None;
        let mut oldest_ver = u32::MAX;
        let mut empty_slot = None;

        for (i, ver) in self.versions.iter().enumerate() {
            if !ver.active {
                if empty_slot.is_none() {
                    empty_slot = Some(i);
                }
                continue;
            }
            if ver.vnode_id == vnode_id && ver.version < oldest_ver {
                oldest_ver = ver.version;
                target = Some(i);
            }
        }

        let idx = empty_slot.or(target).ok_or(VfsError::NoSpace)?;
        let ver_num = self.next_version;
        self.next_version += 1;

        self.versions[idx] = FileVersion {
            vnode_id,
            version: ver_num,
            size,
            page_id,
            timestamp,
            active: true,
        };

        Ok(ver_num)
    }

    pub fn get_version(&self, vnode_id: InodeId, version: u32) -> Option<&FileVersion> {
        self.versions
            .iter()
            .find(|v| v.active && v.vnode_id == vnode_id && v.version == version)
    }

    pub fn latest_version(&self, vnode_id: InodeId) -> Option<&FileVersion> {
        let mut best: Option<&FileVersion> = None;
        for ver in &self.versions {
            if ver.active && ver.vnode_id == vnode_id {
                if best.is_none() || ver.version > best.unwrap().version {
                    best = Some(ver);
                }
            }
        }
        best
    }

    pub fn versions_for(&self, vnode_id: InodeId) -> usize {
        self.versions
            .iter()
            .filter(|v| v.active && v.vnode_id == vnode_id)
            .count()
    }

    pub fn remove_all_for(&mut self, vnode_id: InodeId) -> usize {
        let mut count = 0;
        for ver in self.versions.iter_mut() {
            if ver.active && ver.vnode_id == vnode_id {
                *ver = FileVersion::empty();
                count += 1;
            }
        }
        count
    }
}
