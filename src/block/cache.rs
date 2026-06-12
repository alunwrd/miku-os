// Shared buffer cache for the block layer - MikuOS's analogue of the Linux
// page cache, sitting below every filesystem and raw consumer.
//
//   Granularity: 4 KiB chunks (8 sectors), keyed by '(device, chunk index)'.
//   Policy: write-through. Writes always reach the device before the call
//   returns, so journal ordering and crash durability are exactly as
//   without the cache; only reads get faster.
//   Organization: 8-way set-associative with per-set LRU - lookup touches
//   at most 8 entries, no global scans.
//
// Coherence is by construction: every disk access in the kernel goes
// through 'crate::block', so there is no second path that could observe  stale data

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use crate::vfs::types::BlockDevId;

/// Sectors per cache chunk (4 KiB)
pub const CHUNK_SECTORS: u64 = 8;
pub const CHUNK_BYTES: usize = CHUNK_SECTORS as usize * 512;

const WAYS: usize = 8;
const SETS: usize = 64;
const ENTRIES: usize = WAYS * SETS; // 512 chunks = 2 MiB of data

#[derive(Clone, Copy)]
struct Entry {
    dev:         BlockDevId,
    chunk:       u64,
    valid:       bool,
    dirty:       bool,
    last_access: u64,
}

impl Entry {
    const fn empty() -> Self {
        Self { dev: 0, chunk: 0, valid: false, dirty: false, last_access: 0 }
    }
}

pub struct BufferCache {
    data:    Vec<u8>, // ENTRIES * CHUNK_BYTES
    entries: Vec<Entry>,
    tick:    u64,
    /// Last chunk touched per device, for sequential-access detection
    last_chunk: [u64; crate::vfs::types::MAX_BLOCK_DEVICES],
    /// Consecutive sequential chunks per device (readahead ramp-up)
    streak:     [u32; crate::vfs::types::MAX_BLOCK_DEVICES],
    pub hits:   u64,
    pub misses: u64,
    pub readaheads: u64,
    /// Dirty chunks that left the cache merged into a neighbour's write
    /// command instead of as their own (writeback request merging)
    pub write_merges: u64,
}

impl BufferCache {
    pub fn new() -> Self {
        crate::serial_println!(
            "[bcache] {} chunks x {} B = {} KB ({}-way x {} sets)",
            ENTRIES, CHUNK_BYTES, ENTRIES * CHUNK_BYTES / 1024, WAYS, SETS
        );
        Self {
            data:    vec![0u8; ENTRIES * CHUNK_BYTES],
            entries: vec![Entry::empty(); ENTRIES],
            tick:    0,
            last_chunk: [u64::MAX; crate::vfs::types::MAX_BLOCK_DEVICES],
            streak:     [0u32; crate::vfs::types::MAX_BLOCK_DEVICES],
            hits:    0,
            misses:  0,
            readaheads: 0,
            write_merges: 0,
        }
    }

    /// Record an access to 'chunk' and report whether it directly follows
    /// the previous access on this device - the readahead trigger
    /// Returns the current sequential-streak length for this device
    /// (0 = random access), which sizes the readahead window
    pub fn advance(&mut self, dev: BlockDevId, chunk: u64) -> u32 {
        let slot = dev as usize % self.last_chunk.len();
        if self.last_chunk[slot] == chunk.wrapping_sub(1) {
            self.streak[slot] = self.streak[slot].saturating_add(1);
        } else if self.last_chunk[slot] != chunk {
            self.streak[slot] = 0;
        }
        self.last_chunk[slot] = chunk;
        self.streak[slot]
    }

    #[inline]
    fn set_base(dev: BlockDevId, chunk: u64) -> usize {
        // Cheap mix of device id and chunk index into a set number
        let h = chunk ^ (chunk >> 13) ^ ((dev as u64) << 7);
        (h as usize % SETS) * WAYS
    }

    /// Copy a cached chunk into 'out'. Returns false on miss
    pub fn get(&mut self, dev: BlockDevId, chunk: u64, out: &mut [u8]) -> bool {
        let base = Self::set_base(dev, chunk);
        for i in base..base + WAYS {
            let e = self.entries[i];
            if e.valid && e.dev == dev && e.chunk == chunk {
                let off = i * CHUNK_BYTES;
                out[..CHUNK_BYTES].copy_from_slice(&self.data[off..off + CHUNK_BYTES]);
                self.tick += 1;
                self.entries[i].last_access = self.tick;
                self.hits += 1;
                return true;
            }
        }
        self.misses += 1;
        false
    }

    /// Pick the slot a (dev, chunk) insert should use: its own entry if
    /// present, else a free way, else the set's LRU way - preferring clean
    /// victims over dirty ones so inserts rarely force a writeback
    fn pick_slot(&self, dev: BlockDevId, chunk: u64) -> usize {
        let base = Self::set_base(dev, chunk);
        let mut victim = base;
        let mut best = (2u8, u64::MAX); // (class: 0 own/free, 1 clean, 2 dirty; age)
        for i in base..base + WAYS {
            let e = self.entries[i];
            if e.valid && e.dev == dev && e.chunk == chunk {
                return i;
            }
            let cand = if !e.valid {
                (0u8, 0u64)
            } else if !e.dirty {
                (1u8, e.last_access)
            } else {
                (2u8, e.last_access)
            };
            if cand < best {
                best = cand;
                victim = i;
            }
        }
        victim
    }

    fn fill_slot(&mut self, slot: usize, dev: BlockDevId, chunk: u64, data: &[u8], dirty: bool) {
        let off = slot * CHUNK_BYTES;
        self.data[off..off + CHUNK_BYTES].copy_from_slice(&data[..CHUNK_BYTES]);
        self.tick += 1;
        self.entries[slot] = Entry {
            dev,
            chunk,
            valid: true,
            dirty,
            last_access: self.tick,
        };
    }

    /// Insert (or refresh) a clean chunk. If the chosen victim is dirty, its
    /// contents are returned via 'evicted' for the caller to write out first
    pub fn insert(
        &mut self,
        dev: BlockDevId,
        chunk: u64,
        data: &[u8],
        evicted: &mut [u8],
    ) -> Option<(BlockDevId, u64)> {
        let slot = self.pick_slot(dev, chunk);
        let e = self.entries[slot];
        if e.valid && e.dev == dev && e.chunk == chunk && e.dirty {
            // Resident and dirty: the cache holds newer content than the
            // disk data being inserted (e.g. a readahead window overlapping
            // a pending write). Never overwrite it...
            return None;
        }
        let out = self.take_if_dirty_foreign(slot, dev, chunk, evicted);
        self.fill_slot(slot, dev, chunk, data, false);
        out
    }

    /// Insert a chunk as dirty (write-back). Same eviction contract as  'insert'
    pub fn insert_dirty(
        &mut self,
        dev: BlockDevId,
        chunk: u64,
        data: &[u8],
        evicted: &mut [u8],
    ) -> Option<(BlockDevId, u64)> {
        let slot = self.pick_slot(dev, chunk);
        let out = self.take_if_dirty_foreign(slot, dev, chunk, evicted);
        self.fill_slot(slot, dev, chunk, data, true);
        out
    }

    /// If 'slot' holds a dirty entry for a different (dev, chunk), copy it
    /// into 'evicted' and report its identity so the caller can write it out
    fn take_if_dirty_foreign(
        &mut self,
        slot: usize,
        dev: BlockDevId,
        chunk: u64,
        evicted: &mut [u8],
    ) -> Option<(BlockDevId, u64)> {
        let e = self.entries[slot];
        if e.valid && e.dirty && !(e.dev == dev && e.chunk == chunk) {
            let off = slot * CHUNK_BYTES;
            evicted[..CHUNK_BYTES].copy_from_slice(&self.data[off..off + CHUNK_BYTES]);
            return Some((e.dev, e.chunk));
        }
        None
    }

    /// Merge sector-granular data into a cached chunk and mark it dirty.
    /// Returns false when the chunk is not resident
    pub fn merge_dirty(
        &mut self,
        dev: BlockDevId,
        chunk: u64,
        sector_in_chunk: u64,
        data: &[u8],
    ) -> bool {
        let base = Self::set_base(dev, chunk);
        for i in base..base + WAYS {
            let e = self.entries[i];
            if e.valid && e.dev == dev && e.chunk == chunk {
                let off = i * CHUNK_BYTES + sector_in_chunk as usize * 512;
                self.data[off..off + data.len()].copy_from_slice(data);
                self.tick += 1;
                self.entries[i].last_access = self.tick;
                self.entries[i].dirty = true;
                return true;
            }
        }
        false
    }

    /// Look up the cache slot holding a dirty copy of (dev, chunk)
    fn find_dirty(&self, dev: BlockDevId, chunk: u64) -> Option<usize> {
        let base = Self::set_base(dev, chunk);
        (base..base + WAYS).find(|&i| {
            let e = self.entries[i];
            e.valid && e.dirty && e.dev == dev && e.chunk == chunk
        })
    }

    /// Pop an ascending run of contiguous dirty chunks for 'dev', starting
    /// at the lowest dirty chunk at or after 'after_chunk': up to 'max'
    /// chunks are copied back-to-back into 'out' and marked clean. Repeated
    /// calls walk the device's dirty set in ascending LBA order - the
    /// elevator sweep used by 'flush' - and contiguous chunks merge into
    /// one driver command (writeback request merging)
    pub fn pop_dirty_run(
        &mut self,
        dev: BlockDevId,
        after_chunk: u64,
        out: &mut [u8],
        max: usize,
    ) -> Option<(u64, usize)> {
        let mut best: Option<(usize, u64)> = None;
        for i in 0..ENTRIES {
            let e = self.entries[i];
            if e.valid && e.dirty && e.dev == dev && e.chunk >= after_chunk {
                if best.map_or(true, |(_, c)| e.chunk < c) {
                    best = Some((i, e.chunk));
                }
            }
        }
        let (slot, first) = best?;
        let off = slot * CHUNK_BYTES;
        out[..CHUNK_BYTES].copy_from_slice(&self.data[off..off + CHUNK_BYTES]);
        self.entries[slot].dirty = false;

        let mut n = 1usize;
        while n < max {
            let Some(slot) = self.find_dirty(dev, first + n as u64) else { break };
            let src = slot * CHUNK_BYTES;
            let dst = n * CHUNK_BYTES;
            out[dst..dst + CHUNK_BYTES].copy_from_slice(&self.data[src..src + CHUNK_BYTES]);
            self.entries[slot].dirty = false;
            self.write_merges += 1;
            n += 1;
        }
        Some((first, n))
    }

    /// Number of dirty chunks (all devices)
    pub fn dirty_count(&self) -> usize {
        self.entries.iter().filter(|e| e.valid && e.dirty).count()
    }

    /// Drop every cached chunk lying fully inside a discarded sector range -
    /// dirty ones included, since their content is dead by definition (this
    /// is what keeps writeback from re-materializing discarded sectors).
    /// Edge chunks only partially covered stay resident: their out-of-range
    /// sectors are still live data
    pub fn invalidate_covered(&mut self, dev: BlockDevId, lba: u64, count: u32) {
        let end = lba + count as u64;
        // First chunk fully inside the range, and one past the last
        let first     = (lba + CHUNK_SECTORS - 1) / CHUNK_SECTORS;
        let last_excl = end / CHUNK_SECTORS;
        if first >= last_excl {
            return;
        }
        if last_excl - first >= ENTRIES as u64 {
            // Range spans more chunks than the cache holds (e.g. a whole-
            // device discard) - one scan over the entries is cheaper than
            // a per-chunk set lookup
            for e in self.entries.iter_mut() {
                if e.valid && e.dev == dev && e.chunk >= first && e.chunk < last_excl {
                    e.valid = false;
                    e.dirty = false;
                }
            }
            return;
        }
        for chunk in first..last_excl {
            let base = Self::set_base(dev, chunk);
            for i in base..base + WAYS {
                let e = self.entries[i];
                if e.valid && e.dev == dev && e.chunk == chunk {
                    self.entries[i].valid = false;
                    self.entries[i].dirty = false;
                }
            }
        }
    }

    /// Drop any cached chunks overlapping the range - used after a failed
    /// write, when the on-disk contents are unknown
    pub fn invalidate_range(&mut self, dev: BlockDevId, lba: u64, count: u32) {
        let end = lba + count as u64;
        let first_chunk = lba / CHUNK_SECTORS;
        let last_chunk  = (end - 1) / CHUNK_SECTORS;
        for chunk in first_chunk..=last_chunk {
            let base = Self::set_base(dev, chunk);
            for i in base..base + WAYS {
                let e = self.entries[i];
                if e.valid && e.dev == dev && e.chunk == chunk {
                    self.entries[i].valid = false;
                }
            }
        }
    }

    /// Fold freshly written sectors into any cached chunk they overlap.
    /// 'lba'/'count' describe the write in sectors; chunks not present are
    /// left absent (a write does not allocate cache space by itself)
    pub fn update_on_write(&mut self, dev: BlockDevId, lba: u64, count: u32, buf: &[u8]) {
        let end = lba + count as u64;
        let first_chunk = lba / CHUNK_SECTORS;
        let last_chunk  = (end - 1) / CHUNK_SECTORS;

        for chunk in first_chunk..=last_chunk {
            let base = Self::set_base(dev, chunk);
            for i in base..base + WAYS {
                let e = self.entries[i];
                if !(e.valid && e.dev == dev && e.chunk == chunk) {
                    continue;
                }
                let chunk_start = chunk * CHUNK_SECTORS;
                let s = lba.max(chunk_start);
                let e_sec = end.min(chunk_start + CHUNK_SECTORS);
                let cache_off = i * CHUNK_BYTES + ((s - chunk_start) as usize) * 512;
                let buf_off = ((s - lba) as usize) * 512;
                let bytes = ((e_sec - s) as usize) * 512;
                self.data[cache_off..cache_off + bytes]
                    .copy_from_slice(&buf[buf_off..buf_off + bytes]);
                break;
            }
        }
    }
}
