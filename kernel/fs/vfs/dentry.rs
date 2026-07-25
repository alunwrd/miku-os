use crate::fs::vfs::hash::dentry_hash;
use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct DentryCacheEntry {
    pub parent_id: InodeId,
    pub child_id: InodeId,
    pub hash: u32,
    pub name: NameBuf,
    pub valid: bool,
    pub negative: bool,
}

impl DentryCacheEntry {
    pub const fn empty() -> Self {
        Self {
            parent_id: INVALID_ID,
            child_id: INVALID_ID,
            hash: 0,
            name: NameBuf::empty(),
            valid: false,
            negative: false,
        }
    }
}

pub struct DentryCache {
    pub entries: [DentryCacheEntry; MAX_DENTRIES],
    pub hits: u64,
    pub misses: u64,
}

impl DentryCache {
    pub const fn new() -> Self {
        Self {
            entries: [DentryCacheEntry::empty(); MAX_DENTRIES],
            hits: 0,
            misses: 0,
        }
    }

    pub fn lookup(&mut self, parent: InodeId, name: &str) -> Option<InodeId> {
        let h = dentry_hash(parent, name);
        let start = (h as usize) % MAX_DENTRIES;

        for i in 0..MAX_DENTRIES {
            let idx = (start + i) % MAX_DENTRIES;
            let entry = &self.entries[idx];
            if !entry.valid {
                if entry.hash == 0 && entry.parent_id == INVALID_ID {
                    break;
                }
                continue;
            }
            if entry.hash == h && entry.parent_id == parent && entry.name.eq_str(name) {
                self.hits += 1;
                if entry.negative {
                    return None;
                }
                return Some(entry.child_id);
            }
        }
        self.misses += 1;
        None
    }

    pub fn insert(&mut self, parent: InodeId, name: &str, child: InodeId) {
        let h = dentry_hash(parent, name);
        let start = (h as usize) % MAX_DENTRIES;

        let mut target = None;
        for i in 0..MAX_DENTRIES {
            let idx = (start + i) % MAX_DENTRIES;
            if !self.entries[idx].valid {
                target = Some(idx);
                break;
            }
            if self.entries[idx].hash == h
                && self.entries[idx].parent_id == parent
                && self.entries[idx].name.eq_str(name)
            {
                target = Some(idx);
                break;
            }
        }

        let idx = target.unwrap_or((h as usize) % MAX_DENTRIES);
        self.entries[idx] = DentryCacheEntry {
            parent_id: parent,
            child_id: child,
            hash: h,
            name: NameBuf::from_str(name),
            valid: true,
            negative: false,
        };
    }

    pub fn insert_negative(&mut self, parent: InodeId, name: &str) {
        let h = dentry_hash(parent, name);
        let idx = (h as usize) % MAX_DENTRIES;
        self.entries[idx] = DentryCacheEntry {
            parent_id: parent,
            child_id: INVALID_ID,
            hash: h,
            name: NameBuf::from_str(name),
            valid: true,
            negative: true,
        };
    }

    pub fn invalidate(&mut self, parent: InodeId, name: &str) {
        let h = dentry_hash(parent, name);
        let start = (h as usize) % MAX_DENTRIES;
        for i in 0..MAX_DENTRIES {
            let idx = (start + i) % MAX_DENTRIES;
            if !self.entries[idx].valid {
                if self.entries[idx].hash == 0 {
                    break;
                }
                continue;
            }
            if self.entries[idx].hash == h
                && self.entries[idx].parent_id == parent
                && self.entries[idx].name.eq_str(name)
            {
                self.entries[idx].valid = false;
                return;
            }
        }
    }

    pub fn invalidate_all_for(&mut self, vnode_id: InodeId) {
        for entry in self.entries.iter_mut() {
            if entry.valid && (entry.parent_id == vnode_id || entry.child_id == vnode_id) {
                entry.valid = false;
            }
        }
    }

    pub fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = DentryCacheEntry::empty();
        }
        self.hits = 0;
        self.misses = 0;
    }

    pub fn hit_rate(&self) -> u64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0
        } else {
            (self.hits * 100) / total
        }
    }
}
