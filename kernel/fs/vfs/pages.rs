use crate::fs::vfs::lru::LruList;
use crate::fs::vfs::slab::Slab;
use crate::fs::vfs::types::*;

pub struct CachedPage {
    pub data: [u8; PAGE_SIZE],
    pub inode_id: InodeId,
    pub page_index: u32,
    pub dirty: bool,
    pub valid: bool,
}

impl CachedPage {
    pub const fn empty() -> Self {
        Self {
            data: [0; PAGE_SIZE],
            inode_id: INVALID_ID,
            page_index: 0,
            dirty: false,
            valid: false,
        }
    }

    pub fn clear(&mut self) {
        self.data = [0; PAGE_SIZE];
        self.inode_id = INVALID_ID;
        self.page_index = 0;
        self.dirty = false;
        self.valid = false;
    }
}

pub struct PageCache {
    pub pages: [CachedPage; MAX_DATA_PAGES],
    pub slab: Slab<MAX_DATA_PAGES>,
    pub lru: LruList<MAX_DATA_PAGES>,
    pub total_writes: u64,
    pub total_reads: u64,
    pub evictions: u64,
}

impl PageCache {
    pub const fn new() -> Self {
        Self {
            pages: [const { CachedPage::empty() }; MAX_DATA_PAGES],
            slab: Slab::new(),
            lru: LruList::new(),
            total_writes: 0,
            total_reads: 0,
            evictions: 0,
        }
    }

    pub fn alloc_page(&mut self) -> VfsResult<PageId> {
        match self.slab.alloc() {
            Ok(idx) => {
                let pid = idx as PageId;
                self.pages[idx].clear();
                self.pages[idx].valid = true;
                self.lru.push_front(pid);
                Ok(pid)
            }
            Err(_) => self.evict_and_alloc(),
        }
    }

    fn evict_and_alloc(&mut self) -> VfsResult<PageId> {
        let mut candidate = None;
        let mut current = self.lru.tail;
        while current != INVALID_ID {
            let idx = current as usize;
            if idx < MAX_DATA_PAGES && !self.pages[idx].dirty {
                candidate = Some(current);
                break;
            }
            current = self.lru.nodes[idx].prev;
        }

        let evict_id = match candidate {
            Some(id) => id,
            None => return Err(VfsError::NoSpace),
        };

        let idx = evict_id as usize;
        self.lru.remove(evict_id);
        self.pages[idx].clear();
        self.slab.free(idx);
        self.evictions += 1;

        let new_idx = self.slab.alloc()?;
        let pid = new_idx as PageId;
        self.pages[new_idx].valid = true;
        self.lru.push_front(pid);
        Ok(pid)
    }

    pub fn free_page(&mut self, page_id: PageId) {
        let idx = page_id as usize;
        if idx < MAX_DATA_PAGES && self.slab.is_active(idx) {
            self.lru.remove(page_id);
            self.pages[idx].clear();
            self.slab.free(idx);
        }
    }

    pub fn get_page_data(&mut self, page_id: PageId) -> Option<&[u8; PAGE_SIZE]> {
        let idx = page_id as usize;
        if idx < MAX_DATA_PAGES && self.slab.is_active(idx) {
            self.total_reads += 1;
            self.lru.touch(page_id);
            Some(&self.pages[idx].data)
        } else {
            None
        }
    }

    pub fn get_page_data_mut(&mut self, page_id: PageId) -> Option<&mut [u8; PAGE_SIZE]> {
        let idx = page_id as usize;
        if idx < MAX_DATA_PAGES && self.slab.is_active(idx) {
            self.total_writes += 1;
            self.lru.touch(page_id);
            Some(&mut self.pages[idx].data)
        } else {
            None
        }
    }

    pub fn mark_dirty(&mut self, page_id: PageId) {
        let idx = page_id as usize;
        if idx < MAX_DATA_PAGES && self.slab.is_active(idx) {
            self.pages[idx].dirty = true;
        }
    }

    pub fn mark_clean(&mut self, page_id: PageId) {
        let idx = page_id as usize;
        if idx < MAX_DATA_PAGES && self.slab.is_active(idx) {
            self.pages[idx].dirty = false;
        }
    }

    pub fn used_pages(&self) -> usize {
        self.slab.count()
    }
    pub fn free_pages(&self) -> usize {
        self.slab.free_count()
    }
    pub fn total_capacity(&self) -> usize {
        MAX_DATA_PAGES
    }

    pub fn dirty_count(&self) -> usize {
        let mut count = 0;
        for i in 0..MAX_DATA_PAGES {
            if self.slab.is_active(i) && self.pages[i].dirty {
                count += 1;
            }
        }
        count
    }

    pub fn flush_all(&mut self) {
        for i in 0..MAX_DATA_PAGES {
            if self.slab.is_active(i) {
                self.pages[i].dirty = false;
            }
        }
    }
}
