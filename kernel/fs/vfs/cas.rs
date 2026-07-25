use crate::fs::vfs::hash::content_hash;
use crate::fs::vfs::types::*;

#[derive(Clone, Copy)]
pub struct CasObject {
    pub hash: [u8; 32],
    pub page_id: PageId,
    pub refcount: u16,
    pub size: u32,
    pub active: bool,
}

impl CasObject {
    pub const fn empty() -> Self {
        Self {
            hash: [0; 32],
            page_id: INVALID_ID,
            refcount: 0,
            size: 0,
            active: false,
        }
    }
}

pub struct CasStore {
    pub objects: [CasObject; MAX_CAS_OBJECTS],
}

impl CasStore {
    pub const fn new() -> Self {
        Self {
            objects: [CasObject::empty(); MAX_CAS_OBJECTS],
        }
    }

    pub fn find_by_hash(&self, hash: &[u8; 32]) -> Option<usize> {
        for (i, obj) in self.objects.iter().enumerate() {
            if obj.active && obj.hash == *hash {
                return Some(i);
            }
        }
        None
    }

    pub fn store(&mut self, data: &[u8], page_id: PageId) -> VfsResult<usize> {
        let hash = content_hash(data);

        if let Some(idx) = self.find_by_hash(&hash) {
            self.objects[idx].refcount = self.objects[idx].refcount.saturating_add(1);
            return Ok(idx);
        }

        for (i, obj) in self.objects.iter_mut().enumerate() {
            if !obj.active {
                obj.hash = hash;
                obj.page_id = page_id;
                obj.refcount = 1;
                obj.size = data.len() as u32;
                obj.active = true;
                return Ok(i);
            }
        }

        Err(VfsError::NoSpace)
    }

    pub fn release(&mut self, idx: usize) -> Option<PageId> {
        if idx >= MAX_CAS_OBJECTS || !self.objects[idx].active {
            return None;
        }

        self.objects[idx].refcount = self.objects[idx].refcount.saturating_sub(1);
        if self.objects[idx].refcount == 0 {
            let page_id = self.objects[idx].page_id;
            self.objects[idx] = CasObject::empty();
            Some(page_id)
        } else {
            None
        }
    }

    pub fn count(&self) -> usize {
        self.objects.iter().filter(|o| o.active).count()
    }

    pub fn total_refs(&self) -> u64 {
        self.objects
            .iter()
            .filter(|o| o.active)
            .map(|o| o.refcount as u64)
            .sum()
    }
}
