extern crate alloc;
use alloc::vec;
use alloc::vec::Vec;

const MAX_CACHE_ENTRIES: usize = 256;

#[derive(Clone, Copy)]
struct CacheEntry {
    block_num:   u32,
    valid:       bool,
    pub dirty:   bool,
    last_access: u64,
}

impl CacheEntry {
    const fn empty() -> Self {
        Self { block_num: 0, valid: false, dirty: false, last_access: 0 }
    }
}

pub struct BlockCache {
    buffer:         Vec<u8>,
    entries:        Vec<CacheEntry>,
    block_size:     usize,
    count:          usize,
    access_counter: u64,
    pub hits:       u64,
    pub misses:     u64,
    pub evictions:  u64,
}

impl BlockCache {
    pub fn new(block_size: usize, max_entries: usize) -> Self {
        let count = max_entries.min(MAX_CACHE_ENTRIES);
        let buffer = vec![0u8; count * block_size];
        let entries = vec![CacheEntry::empty(); count];
        crate::serial_println!(
            "[cache] {} entries x {} B = {} KB",
            count, block_size, (count * block_size) / 1024
        );
        Self { buffer, entries, block_size, count, access_counter: 0, hits: 0, misses: 0, evictions: 0 }
    }

    pub fn get(&mut self, block_num: u32, buf: &mut [u8]) -> bool {
        for i in 0..self.count {
            if self.entries[i].valid && self.entries[i].block_num == block_num {
                let offset = i * self.block_size;
                let len = buf.len().min(self.block_size);
                buf[..len].copy_from_slice(&self.buffer[offset..offset + len]);
                self.access_counter += 1;
                self.entries[i].last_access = self.access_counter;
                self.hits += 1;
                return true;
            }
        }
        self.misses += 1;
        false
    }

    pub fn put(&mut self, block_num: u32, data: &[u8]) {
        for i in 0..self.count {
            if self.entries[i].valid && self.entries[i].block_num == block_num {
                let offset = i * self.block_size;
                let len = data.len().min(self.block_size);
                self.buffer[offset..offset + len].copy_from_slice(&data[..len]);
                self.access_counter += 1;
                self.entries[i].last_access = self.access_counter;
                return;
            }
        }
        let slot = self.find_slot();
        if self.entries[slot].valid { self.evictions += 1; }
        let offset = slot * self.block_size;
        let len = data.len().min(self.block_size);
        self.buffer[offset..offset + len].copy_from_slice(&data[..len]);
        self.access_counter += 1;
        self.entries[slot] = CacheEntry {
            block_num, valid: true, dirty: false, last_access: self.access_counter
        };
    }

    pub fn put_dirty(&mut self, block_num: u32, data: &[u8]) {
        for i in 0..self.count {
            if self.entries[i].valid && self.entries[i].block_num == block_num {
                let offset = i * self.block_size;
                let len = data.len().min(self.block_size);
                self.buffer[offset..offset + len].copy_from_slice(&data[..len]);
                self.access_counter += 1;
                self.entries[i].last_access = self.access_counter;
                self.entries[i].dirty = true;
                return;
            }
        }
        let slot = self.find_slot();
        if self.entries[slot].valid { self.evictions += 1; }
        let offset = slot * self.block_size;
        let len = data.len().min(self.block_size);
        self.buffer[offset..offset + len].copy_from_slice(&data[..len]);
        self.access_counter += 1;
        self.entries[slot] = CacheEntry {
            block_num, valid: true, dirty: true, last_access: self.access_counter
        };
    }

    // Returns (block_num, slot_index, data_offset) for a dirty block
    // that needs to be flushed before eviction. Returns None if no
    // dirty eviction is pending.
    pub fn evict_victim(&self) -> Option<(u32, usize)> {
        // only needed when all slots are valid and no clean LRU exists
        let has_empty = self.entries.iter().any(|e| !e.valid);
        if has_empty { return None; }

        // check if there's a clean LRU candidate
        let has_clean = self.entries.iter().any(|e| e.valid && !e.dirty);
        if has_clean { return None; }

        // all entries dirty - the LRU dirty one will be evicted next
        let mut lru_idx = 0;
        let mut lru_val = u64::MAX;
        for i in 0..self.count {
            if self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        Some((self.entries[lru_idx].block_num, lru_idx))
    }

    pub fn get_dirty_blocks(&self) -> Vec<(u32, usize)> {
        let mut out = Vec::new();
        for i in 0..self.count {
            if self.entries[i].valid && self.entries[i].dirty {
                out.push((self.entries[i].block_num, i));
            }
        }
        out
    }

    pub fn get_block_data(&self, slot: usize, buf: &mut [u8]) {
        let offset = slot * self.block_size;
        let len = buf.len().min(self.block_size);
        buf[..len].copy_from_slice(&self.buffer[offset..offset + len]);
    }

    pub fn mark_clean(&mut self, slot: usize) {
        self.entries[slot].dirty = false;
    }

    fn find_slot(&self) -> usize {
        for i in 0..self.count {
            if !self.entries[i].valid {
                return i;
            }
        }

        let mut lru_idx = usize::MAX;
        let mut lru_val = u64::MAX;
        for i in 0..self.count {
            if !self.entries[i].dirty && self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        if lru_idx != usize::MAX {
            return lru_idx;
        }

        lru_val = u64::MAX;
        lru_idx = 0;
        for i in 0..self.count {
            if self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        lru_idx
    }

    pub fn should_flush(&self) -> bool {
        let dirty = self.entries.iter().filter(|e| e.valid && e.dirty).count();
        dirty > self.count * 3 / 4
    }

    pub fn invalidate(&mut self, block_num: u32) {
        for i in 0..self.count {
            if self.entries[i].valid && self.entries[i].block_num == block_num {
                self.entries[i].valid = false;
            }
        }
    }

    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() { e.valid = false; }
        self.hits = 0; self.misses = 0; self.evictions = 0; self.access_counter = 0;
    }

    pub fn cached_entries(&self) -> usize {
        self.entries.iter().filter(|e| e.valid).count()
    }

    pub fn dirty_entries(&self) -> usize {
        self.entries.iter().filter(|e| e.valid && e.dirty).count()
    }

    pub fn capacity(&self) -> usize { self.count }

    pub fn hit_rate(&self) -> u64 {
        let total = self.hits + self.misses;
        if total == 0 { 0 } else { (self.hits * 100) / total }
    }

    pub fn total_bytes(&self) -> usize { self.count * self.block_size }
}
