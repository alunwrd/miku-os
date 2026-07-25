use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct QuotaEntry {
    pub uid: u16,
    pub bytes_used: u64,
    pub bytes_limit: u64,
    pub inodes_used: u32,
    pub inodes_limit: u32,
    pub active: bool,
}

impl QuotaEntry {
    pub const fn empty() -> Self {
        Self {
            uid: 0,
            bytes_used: 0,
            bytes_limit: 0,
            inodes_used: 0,
            inodes_limit: 0,
            active: false,
        }
    }

    pub fn bytes_available(&self) -> u64 {
        if self.bytes_limit == 0 {
            return u64::MAX;
        }
        self.bytes_limit.saturating_sub(self.bytes_used)
    }

    pub fn inodes_available(&self) -> u32 {
        if self.inodes_limit == 0 {
            return u32::MAX;
        }
        self.inodes_limit.saturating_sub(self.inodes_used)
    }
}

pub struct QuotaManager {
    pub entries: [QuotaEntry; MAX_QUOTA_ENTRIES],
    pub enabled: bool,
}

impl QuotaManager {
    pub const fn new() -> Self {
        Self {
            entries: [QuotaEntry::empty(); MAX_QUOTA_ENTRIES],
            enabled: false,
        }
    }

    pub fn set_quota(&mut self, uid: u16, bytes_limit: u64, inodes_limit: u32) -> VfsResult<()> {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.uid == uid {
                entry.bytes_limit = bytes_limit;
                entry.inodes_limit = inodes_limit;
                return Ok(());
            }
        }

        for entry in self.entries.iter_mut() {
            if !entry.active {
                entry.uid = uid;
                entry.bytes_limit = bytes_limit;
                entry.inodes_limit = inodes_limit;
                entry.active = true;
                return Ok(());
            }
        }

        Err(VfsError::NoSpace)
    }

    pub fn check_bytes(&self, uid: u16, additional: u64) -> VfsResult<()> {
        if !self.enabled {
            return Ok(());
        }
        for entry in &self.entries {
            if entry.active && entry.uid == uid {
                if entry.bytes_limit > 0 && entry.bytes_used + additional > entry.bytes_limit {
                    return Err(VfsError::QuotaExceeded);
                }
            }
        }
        Ok(())
    }

    pub fn check_inodes(&self, uid: u16) -> VfsResult<()> {
        if !self.enabled {
            return Ok(());
        }
        for entry in &self.entries {
            if entry.active && entry.uid == uid {
                if entry.inodes_limit > 0 && entry.inodes_used >= entry.inodes_limit {
                    return Err(VfsError::QuotaExceeded);
                }
            }
        }
        Ok(())
    }

    pub fn add_bytes(&mut self, uid: u16, bytes: u64) {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.uid == uid {
                entry.bytes_used = entry.bytes_used.saturating_add(bytes);
            }
        }
    }

    pub fn sub_bytes(&mut self, uid: u16, bytes: u64) {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.uid == uid {
                entry.bytes_used = entry.bytes_used.saturating_sub(bytes);
            }
        }
    }

    pub fn add_inode(&mut self, uid: u16) {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.uid == uid {
                entry.inodes_used = entry.inodes_used.saturating_add(1);
            }
        }
    }

    pub fn sub_inode(&mut self, uid: u16) {
        for entry in self.entries.iter_mut() {
            if entry.active && entry.uid == uid {
                entry.inodes_used = entry.inodes_used.saturating_sub(1);
            }
        }
    }

    pub fn get(&self, uid: u16) -> Option<&QuotaEntry> {
        self.entries.iter().find(|e| e.active && e.uid == uid)
    }
}
