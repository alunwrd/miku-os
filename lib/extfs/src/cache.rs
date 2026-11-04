// Write-back staging for blocks the journal has not committed
//
// The block layer has its own cache; this adds the pin. A block logged into an
// open transaction may not reach its final location until that transaction's
// commit record has landed. Otherwise a crash leaves half an operation applied
// with nothing in the log to repair it.
//
// Eviction and flush skip pinned blocks. When every slot is pinned, the caller
// commits, which releases the pins.

extern crate alloc;
use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

pub const MAX_CACHE_ENTRIES: usize = 1024;

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

/// The slots, plus which slot holds a block, which hold nothing, and how many
/// are dirty, so a hit is a lookup. Only taking a slot from a full cache scans
/// them all, for the LRU
pub struct BlockCache {
    buffer:         Vec<u8>,
    entries:        Vec<CacheEntry>,
    block_size:     usize,
    count:          usize,
    access_counter: u64,
    /// Blocks logged into an open transaction, sorted for a binary search on
    /// each eviction. By block number, since a block may be pinned before it is
    /// resident
    pinned:         Vec<u32>,
    /// Block number to slot, for every valid slot
    index:          BTreeMap<u32, usize>,
    /// Slots that hold nothing
    free:           Vec<usize>,
    /// Valid slots that are dirty
    dirty:          usize,
    pub hits:       u64,
    pub misses:     u64,
    pub evictions:  u64,
}

impl BlockCache {
    pub fn new(block_size: usize, max_entries: usize) -> Self {
        let count = max_entries.min(MAX_CACHE_ENTRIES);
        let buffer = vec![0u8; count * block_size];
        let entries = vec![CacheEntry::empty(); count];
        crate::extlog!(
            "[cache] {} entries x {} B = {} KB",
            count, block_size, (count * block_size) / 1024
        );
        Self {
            buffer, entries, block_size, count,
            access_counter: 0, pinned: Vec::new(),
            index: BTreeMap::new(),
            free: (0..count).rev().collect(),
            dirty: 0,
            hits: 0, misses: 0, evictions: 0,
        }
    }

    /// Holds this block from its final location until `unpin_all`. Called when
    /// it is logged into the running transaction
    pub fn pin(&mut self, block_num: u32) {
        if let Err(idx) = self.pinned.binary_search(&block_num) {
            self.pinned.insert(idx, block_num);
        }
    }

    /// Releases every pin. Called once the transaction's commit record is on
    /// stable media, when the logged copy can repair the real location
    pub fn unpin_all(&mut self) {
        self.pinned.clear();
    }

    pub fn is_pinned(&self, block_num: u32) -> bool {
        self.pinned.binary_search(&block_num).is_ok()
    }

    pub fn pinned_count(&self) -> usize {
        self.pinned.len()
    }

    /// Whether `block_num` has a slot here
    pub fn contains(&self, block_num: u32) -> bool {
        self.index.contains_key(&block_num)
    }

    /// Whether a new block fits without evicting a pinned one. False means
    /// commit before writing more
    pub fn has_room(&self) -> bool {
        !self.free.is_empty()
            || self
                .entries
                .iter()
                .any(|e| e.valid && !self.is_pinned(e.block_num))
    }

    fn touch(&mut self, slot: usize) {
        self.access_counter += 1;
        self.entries[slot].last_access = self.access_counter;
    }

    fn set_dirty(&mut self, slot: usize, dirty: bool) {
        let was = self.entries[slot].dirty;
        if was != dirty {
            if dirty { self.dirty += 1 } else { self.dirty -= 1 }
            self.entries[slot].dirty = dirty;
        }
    }

    fn copy_in(&mut self, slot: usize, data: &[u8]) {
        let offset = slot * self.block_size;
        let len = data.len().min(self.block_size);
        self.buffer[offset..offset + len].copy_from_slice(&data[..len]);
    }

    pub fn get(&mut self, block_num: u32, buf: &mut [u8]) -> bool {
        let Some(&slot) = self.index.get(&block_num) else {
            self.misses += 1;
            return false;
        };
        let offset = slot * self.block_size;
        let len = buf.len().min(self.block_size);
        buf[..len].copy_from_slice(&self.buffer[offset..offset + len]);
        self.touch(slot);
        self.hits += 1;
        true
    }

    /// `block_num` into `slot`, chosen by `find_slot`; what the slot held
    /// leaves the index
    fn take_slot(&mut self, slot: usize, block_num: u32, dirty: bool) {
        if self.entries[slot].valid {
            self.evictions += 1;
            let old = self.entries[slot].block_num;
            self.index.remove(&old);
            self.set_dirty(slot, false);
        } else if let Some(pos) = self.free.iter().rposition(|&f| f == slot) {
            self.free.swap_remove(pos);
        }
        self.entries[slot] = CacheEntry { block_num, valid: true, dirty: false, last_access: 0 };
        self.set_dirty(slot, dirty);
        self.touch(slot);
        self.index.insert(block_num, slot);
    }

    /// Stages a clean copy. False when every slot holds a pinned block: nothing
    /// was stored, and the caller must commit
    #[must_use]
    pub fn put(&mut self, block_num: u32, data: &[u8]) -> bool {
        if let Some(&slot) = self.index.get(&block_num) {
            self.copy_in(slot, data);
            self.touch(slot);
            return true;
        }
        let Some(slot) = self.find_slot() else { return false };
        self.copy_in(slot, data);
        self.take_slot(slot, block_num, false);
        true
    }

    /// Stages a modified copy. False when every slot holds a pinned block: the
    /// write did not happen, and the caller commits to release pins and retries
    #[must_use]
    pub fn put_dirty(&mut self, block_num: u32, data: &[u8]) -> bool {
        if let Some(&slot) = self.index.get(&block_num) {
            self.copy_in(slot, data);
            self.touch(slot);
            self.set_dirty(slot, true);
            return true;
        }
        let Some(slot) = self.find_slot() else { return false };
        self.copy_in(slot, data);
        self.take_slot(slot, block_num, true);
        true
    }

    // Returns (block_num, slot_index, data_offset) for a dirty block needing a
    // flush before eviction, or None
    pub fn evict_victim(&self) -> Option<(u32, usize)> {
        // Only when all slots are valid and no clean LRU exists
        if !self.free.is_empty() { return None; }

        // check if there's a clean, unpinned LRU candidate
        let has_clean = self
            .entries
            .iter()
            .any(|e| e.valid && !e.dirty && !self.is_pinned(e.block_num));
        if has_clean { return None; }

        // All unpinned entries are dirty; the LRU of those goes next. A pinned
        // one must not be offered
        let mut lru_idx = usize::MAX;
        let mut lru_val = u64::MAX;
        for i in 0..self.count {
            if self.is_pinned(self.entries[i].block_num) { continue; }
            if self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        if lru_idx == usize::MAX {
            return None;
        }
        Some((self.entries[lru_idx].block_num, lru_idx))
    }

    /// Dirty blocks that may go to their final location now. Pinned blocks are
    /// absent; they stay dirty until the commit releases them
    pub fn get_dirty_blocks(&self) -> Vec<(u32, usize)> {
        let mut out = Vec::new();
        if self.dirty == 0 {
            return out;
        }
        for i in 0..self.count {
            if self.entries[i].valid
                && self.entries[i].dirty
                && !self.is_pinned(self.entries[i].block_num)
            {
                out.push((self.entries[i].block_num, i));
            }
        }
        out
    }

    /// Every dirty block, pinned or not. Only when there is no open
    /// transaction: unmount, or a commit that released its pins
    pub fn all_dirty_blocks(&self) -> Vec<(u32, usize)> {
        let mut out = Vec::new();
        if self.dirty == 0 {
            return out;
        }
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
        self.set_dirty(slot, false);
    }

    /// Picks a slot for a new block: empty, else the clean LRU, else the dirty
    /// LRU. A pinned block is never a candidate, holding the only copy of a
    /// change the journal must replay. `has_room` says if this can succeed; if
    /// not, commit
    fn find_slot(&self) -> Option<usize> {
        if let Some(&slot) = self.free.last() {
            return Some(slot);
        }

        let mut lru_idx = usize::MAX;
        let mut lru_val = u64::MAX;
        for i in 0..self.count {
            if self.is_pinned(self.entries[i].block_num) { continue; }
            if !self.entries[i].dirty && self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        if lru_idx != usize::MAX {
            return Some(lru_idx);
        }

        lru_val = u64::MAX;
        lru_idx = usize::MAX;
        for i in 0..self.count {
            if self.is_pinned(self.entries[i].block_num) { continue; }
            if self.entries[i].last_access < lru_val {
                lru_val = self.entries[i].last_access;
                lru_idx = i;
            }
        }
        if lru_idx == usize::MAX { None } else { Some(lru_idx) }
    }

    pub fn should_flush(&self) -> bool {
        self.dirty > self.count * 3 / 4
    }

    pub fn invalidate(&mut self, block_num: u32) {
        if let Some(slot) = self.index.remove(&block_num) {
            self.set_dirty(slot, false);
            self.entries[slot].valid = false;
            self.free.push(slot);
        }
    }

    pub fn clear(&mut self) {
        for e in self.entries.iter_mut() { *e = CacheEntry::empty(); }
        self.index.clear();
        self.free = (0..self.count).rev().collect();
        self.dirty = 0;
        self.hits = 0; self.misses = 0; self.evictions = 0; self.access_counter = 0;
    }

    pub fn cached_entries(&self) -> usize {
        self.count - self.free.len()
    }

    pub fn dirty_entries(&self) -> usize {
        self.dirty
    }

    pub fn capacity(&self) -> usize { self.count }

    pub fn hit_rate(&self) -> u64 {
        let total = self.hits + self.misses;
        if total == 0 { 0 } else { (self.hits * 100) / total }
    }

    pub fn total_bytes(&self) -> usize { self.count * self.block_size }
}
