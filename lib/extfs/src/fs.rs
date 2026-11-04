use alloc::vec::Vec;

use crate::error::FsError;
use crate::ext3::journal::Transaction;
use crate::host::ExtHost;
use crate::reader::DiskReader;
use crate::structs::*;

pub struct MikuFS<H: ExtHost> {
    pub superblock: Superblock,
    pub block_size: u32,
    pub inodes_per_group: u32,
    pub blocks_per_group: u32,
    pub group_count: u32,
    // Grown to group_count at mount time
    pub groups: Vec<GroupDesc>,
    pub reader: DiskReader<H>,
    pub journal_seq: u32,
    pub journal_pos: u32,
    pub journal_maxlen: u32,
    pub journal_first: u32,
    pub journal_active: bool,
    /// The transaction operations are joining now, if any. Heap-held, so any
    /// number of blocks can be logged
    pub running: Option<Transaction>,
    /// The closed transaction being written out. Occupied only during
    /// `ext3_commit_txn`, so the running one can be replaced before the old
    /// one's records are on the medium
    pub committing: Option<Transaction>,
    pub block_cache: Option<crate::cache::BlockCache>,
    pub superblock_dirty: bool,
    pub groups_dirty: Vec<bool>,
    pub last_sync_ticks: u64,
    /// When the last transaction was written, so `commit_due` holds a batch for
    /// a bounded time
    pub last_commit_ticks: u64,
    pub journal_inode_cached: Option<Inode>,
    pub alloc_hint: Vec<u32>,
    /// How many times a directory index said a name was absent and a linear
    /// scan found it. Abnormal: Linux, with no scan fallback, would report it
    /// missing
    pub dx_fallbacks: u32,
    /// What this journal's superblock says about checksums, read once at mount.
    /// `None` when the log has none, as `mke2fs` journals
    pub journal_csum_seed: Option<u32>,
    /// Whole blocks of file data gathered into one device write
    /// (`write_data_blocks`), kept to avoid a megabyte allocation per call
    pub data_buf: Vec<u8>,
}

const SYNC_INTERVAL_TICKS: u64 = 90;

impl<H: ExtHost> MikuFS<H> {
    /// A handle with nothing read: zero geometry, every operation fails.
    /// `MikuFS::open` makes a usable one. For a fixed table of mount slots
    pub const fn with_reader(reader: DiskReader<H>) -> Self {
        Self {
            superblock:       Superblock::zeroed(),
            block_size:       0,
            inodes_per_group: 0,
            blocks_per_group: 0,
            group_count:      0,
            groups:           Vec::new(),
            reader,
            journal_seq:      0,
            journal_pos:      0,
            journal_maxlen:   0,
            journal_first:    0,
            journal_active:   false,
            running:          None,
            committing:       None,
            block_cache:      None,
            superblock_dirty: false,
            groups_dirty:     Vec::new(),
            last_sync_ticks:  0,
            last_commit_ticks: 0,
            journal_inode_cached: None,
            alloc_hint:       Vec::new(),
            dx_fallbacks:     0,
            journal_csum_seed: None,
            data_buf:         Vec::new(),
        }
    }

    /// The host underneath, for a test checking what reached the medium or an
    /// unmount returning the device
    pub fn host(&self) -> &H {
        &self.reader.host
    }

    pub fn host_mut(&mut self) -> &mut H {
        &mut self.reader.host
    }

    #[inline]
    pub fn inode_size(&self) -> u32 {
        self.superblock.inode_size_val()
    }

    #[inline]
    pub fn sectors_per_block(&self) -> u32 {
        if self.block_size == 0 { return 1; }
        self.block_size / 512
    }

    #[inline]
    pub fn block_to_lba(&self, block: u32) -> u32 {
        let spb = self.sectors_per_block();
        (block as u64)
            .saturating_mul(spb as u64)
            .min(u32::MAX as u64) as u32
    }

    #[inline]
    pub(crate) fn is_valid_block(&self, block: u32) -> bool {
        if block == 0 { return false; }
        let max = self.superblock.blocks_count();
        max == 0 || block < max
    }    

    pub fn flush_superblock(&mut self) -> Result<(), FsError> {
        self.superblock_dirty = true;
        Ok(())
    }

    pub fn has_dirty_data(&self) -> bool {
        if self.superblock_dirty { return true; }
        if self.groups_dirty.iter().any(|&d| d) { return true; }
        match self.block_cache {
            Some(ref c) => c.dirty_entries() > 0,
            None => false,
        }
    }

    /// Makes everything durable, in the one safe order:
    ///
    ///   1. commit the open transaction, so the log describes all changes to be
    ///      checkpointed
    ///   2. checkpoint: write dirty blocks to where they live, allowed since
    ///      step 1 released their pins
    ///   3. superblock and group descriptors
    ///   4. drain the device's cache
    pub fn periodic_sync(&mut self) -> Result<bool, FsError> {
        if !self.has_dirty_data() && !self.txn_active() {
            return Ok(false);
        }

        let dirty_count = match self.block_cache {
            Some(ref c) => c.dirty_entries(),
            None => 0,
        };

        if self.journal_active {
            self.force_commit()?;
        }

        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.reader.flush_drive();

        crate::extlog!("[pdflush] synced {} dirty blocks", dirty_count);

        Ok(true)
    }

    pub fn check_periodic_sync(&mut self) {
        let now = self.reader.host.ticks();
        if now.wrapping_sub(self.last_sync_ticks) < SYNC_INTERVAL_TICKS {
            return;
        }
        self.last_sync_ticks = now;

        if !self.has_dirty_data() {
            return;
        }

        let _ = self.periodic_sync();
    }

    /// Writes the superblock now, for the two fields describing the log's state
    pub(crate) fn write_superblock_now(&mut self) -> Result<(), FsError> {
        self.do_write_superblock()
    }

    fn do_write_superblock(&mut self) -> Result<(), FsError> {
        if self.superblock.has_metadata_csum() {
            self.update_superblock_csum();
        }
        // One I/O. The superblock is 1024 bytes across two sectors, and
        // separate writes risk half old, half new after a power cut, with a
        // checksum matching neither, and an unmountable volume
        let mut sb = [0u8; 1024];
        sb.copy_from_slice(&self.superblock.data);
        self.reader.write_block(2, &sb, 2)?;
        self.superblock_dirty = false;
        Ok(())
    }

    /// Writes the superblock to every group keeping a backup, so
    /// `e2fsck -b 32768` restores current feature bits and counts.
    ///
    /// Not called from `do_write_superblock`, which runs on every metadata
    /// flush. Backups matter when the primary's shape changes: unmount and a
    /// feature-bit change
    pub fn write_superblock_backups(&mut self) -> Result<(), FsError> {
        if self.reader.read_only {
            return Ok(());
        }
        let first = self.superblock.first_data_block();
        let bpg = self.blocks_per_group;
        if bpg == 0 {
            return Err(FsError::CorruptedFs);
        }
        let bs = self.block_size as usize;
        let sectors_per_block = (self.block_size / 512) as u8;
        // Where the primary table lives and how far it runs, as in
        // `do_write_group_desc`: at 1 KiB the superblock owns block 1,
        // otherwise it shares block 0 with the boot area
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let gd_size = self.superblock.group_desc_size() as usize;
        let gdt_bytes = self.group_count as usize * gd_size;
        let gdt_blocks = gdt_bytes.div_ceil(bs) as u32;

        for g in 1..self.group_count {
            if !self.group_has_super(g) {
                continue;
            }
            let mut copy = self.superblock;
            // e2fsck checks this field against the group holding the copy
            copy.write_u16(90, g as u16);
            if copy.has_metadata_csum() {
                let csum = crate::ext4::crc32c::ext4_superblock_csum(&copy.data);
                copy.set_checksum(csum);
            }

            let mut buf = [0u8; 1024];
            buf.copy_from_slice(&copy.data);

            // A backup is at its group's first block. At 1 KiB blocks the
            // superblock is one block; otherwise the 1024-byte copy is at its
            // start
            let sb_block = first + g * bpg;
            let lba = self.block_to_lba(sb_block);
            self.reader.write_block(lba, &buf, 2)?;

            // And the group descriptor table behind it, which `e2fsck -b` reads
            // with the backup superblock. The descriptor checksum uses the
            // group number, not the location, so a verbatim copy is correct
            let mut gd = alloc::vec![0u8; bs];
            for i in 0..gdt_blocks {
                self.read_block_into(gdt_block + i, &mut gd)?;
                let lba = self.block_to_lba(sb_block + 1 + i);
                self.reader.write_block(lba, &gd, sectors_per_block)?;
            }
        }
        Ok(())
    }

    /// Writes a group descriptor now. It carries its group's free counts and
    /// both bitmap checksums, so it goes in the same transaction as the bitmap
    /// it describes. The block cache still batches
    pub fn flush_group_desc(&mut self, group: usize) -> Result<(), FsError> {
        self.do_write_group_desc(group)
    }

    fn do_write_group_desc(&mut self, group: usize) -> Result<(), FsError> {
        if group >= self.groups.len() {
            return Ok(());
        }
        if self.superblock.has_metadata_csum() || self.superblock.has_gdt_csum() {
            self.update_group_desc_csum(group);
        }
        // Through the block cache and journal, at block granularity, as several
        // descriptors share a block
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let gd_size = self.superblock.group_desc_size() as usize;
        let bs = self.block_size as usize;
        let gd_byte_offset = group * gd_size;
        let block = gdt_block + (gd_byte_offset / bs) as u32;
        let offset_in_block = gd_byte_offset % bs;
        let write_len = gd_size.min(64);
        if offset_in_block + write_len > bs {
            return Err(FsError::CorruptedFs);
        }
        let mut buf = [0u8; 4096];
        self.read_block_into(block, &mut buf[..bs])?;
        buf[offset_in_block..offset_in_block + write_len]
            .copy_from_slice(&self.groups[group].data[..write_len]);
        self.write_meta_block(block, &buf[..bs])?;
        if group < self.groups_dirty.len() {
            self.groups_dirty[group] = false;
        }
        Ok(())
    }

    pub fn flush_all_dirty_metadata(&mut self) -> Result<(), FsError> {
        // The superblock and descriptors carry free counters describing changes
        // still in the open transaction. Written before it commits, they would
        // be ahead of the log, and a replay would make `e2fsck` report wrong
        // free counts
        if self.journal_active && self.running.is_some() {
            self.force_commit()?;
        }
        let had_dirty = self.superblock_dirty || self.groups_dirty.iter().any(|&d| d);
        if self.superblock_dirty {
            self.do_write_superblock()?;
        }
        for group in 0..self.groups_dirty.len() {
            if self.groups_dirty[group] {
                self.do_write_group_desc(group)?;
            }
        }
        // Metadata went through the write-back cache; drain it so unmount and
        // reformat leave the disk durable
        if had_dirty {
            self.reader.flush_drive();
        }
        Ok(())
    }

    /// Everything on the medium. Same order as `periodic_sync`
    pub fn sync(&mut self) -> Result<(), FsError> {
        if self.journal_active {
            self.force_commit()?;
        }
        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.reader.flush_drive();
        // The log's content is now in place, so no recovery is needed, and
        // later mounts do not replay applied records
        if self.journal_active {
            self.mark_journal_checkpointed()?;
            self.reader.flush_drive();
        }
        Ok(())
    }

    /// A blank inode sized for this filesystem. `Inode::zeroed()` is 128 bytes,
    /// with `i_extra_isize` of zero, giving up nanosecond timestamps, dates
    /// past 2038, and the checksum's high half. Thirty-two is what mke2fs
    /// writes
    pub fn fresh_inode(&self) -> Inode {
        let mut inode = Inode::zeroed();
        let size = (self.superblock.inode_size_val() as usize).min(256);
        inode.on_disk_size = size as u16;
        if size >= 160 {
            inode.write_u16(128, 32);
        }
        inode
    }

    pub fn write_inode(&mut self, inode_num: u32, inode: &Inode) -> Result<(), FsError> {
        if inode_num == 0 {
            return Err(FsError::InvalidInode);
        }
        let mut stamped = *inode;
        // The checksum covers the bytes reaching the disk, so the width is the
        // one this write uses. `Inode::zeroed()` says 128, which stamped over
        // 128 bytes and read over 256 would fail
        let write_size = (self.superblock.inode_size_val() as usize).min(256);
        stamped.on_disk_size = write_size as u16;
        self.stamp_inode_csum(inode_num, &mut stamped);
        let idx = inode_num - 1;
        let group = (idx / self.inodes_per_group) as usize;
        let local_idx = idx % self.inodes_per_group;
        if group >= self.groups.len() {
            return Err(FsError::InvalidInode);
        }
        let inode_table_block = self.groups[group].inode_table();
        let inode_size = self.superblock.inode_size_val();
        let byte_offset = local_idx as u64 * inode_size as u64;
        let bs = self.block_size as usize;
        let block_idx = (byte_offset / bs as u64) as u32;
        let offset_in_block = (byte_offset % bs as u64) as usize;
        let phys_block = inode_table_block + block_idx;

        let mut buf = [0u8; 4096];
        self.read_block_into(phys_block, &mut buf[..bs])?;

        if offset_in_block + write_size <= bs {
            buf[offset_in_block..offset_in_block + write_size]
                .copy_from_slice(&stamped.data[..write_size]);
            self.write_meta_block(phys_block, &buf[..bs])?;
        } else {
            let first_part = bs - offset_in_block;
            buf[offset_in_block..bs].copy_from_slice(&stamped.data[..first_part]);
            self.write_meta_block(phys_block, &buf[..bs])?;

            let next_block = phys_block + 1;
            self.read_block_into(next_block, &mut buf[..bs])?;
            let remaining = write_size - first_part;
            buf[..remaining].copy_from_slice(&stamped.data[first_part..write_size]);
            self.write_meta_block(next_block, &buf[..bs])?;
        }
        Ok(())
    }

    /// Writes one dirty block to its real location to free its slot.
    /// `evict_victim` never names a pinned block; when all are pinned it
    /// returns None and the caller commits
    fn flush_evict_victim(&mut self) -> Result<(), FsError> {
        let victim = match self.block_cache {
            Some(ref c) => c.evict_victim(),
            None => None,
        };
        if let Some((blk, slot)) = victim {
            let bs = self.block_size as usize;
            let mut buf = [0u8; 4096];
            if let Some(ref c) = self.block_cache {
                c.get_block_data(slot, &mut buf[..bs]);
            }
            let spb = self.sectors_per_block() as u8;
            let base_lba = self.block_to_lba(blk);
            self.reader.write_block(base_lba, &buf[..bs], spb)?;
            if let Some(ref mut c) = self.block_cache {
                c.mark_clean(slot);
            }
        }
        Ok(())
    }

    pub fn read_block_into(&mut self, block_num: u32, buf: &mut [u8]) -> Result<(), FsError> {
        if !self.is_valid_block(block_num) {
            return Err(FsError::InvalidInode);
        }
        // Consulted only because it may hold a dirty block newer than disk.
        // Clean read caching is the block layer's
        if let Some(ref mut c) = self.block_cache {
            if c.get(block_num, buf) {
                return Ok(());
            }
        }
        let spb = self.sectors_per_block() as u8;
        let base_lba = self.block_to_lba(block_num);
        let bs = self.block_size as usize;
        self.reader.read_block(base_lba, &mut buf[..bs], spb)?;
        Ok(())
    }

    /// Ordered journal write through the block layer's write-through path, so
    /// descriptor, data, and commit records land in issue order
    pub fn write_block_direct_nocache(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        let spb = self.sectors_per_block() as u8;
        let base_lba = self.block_to_lba(block_num);
        let bs = self.block_size as usize;
        let len = data.len().min(bs);
        if len == bs {
            self.reader.write_block_sync(base_lba, &data[..bs], spb)?;
        } else {
            for s in 0..spb as u32 {
                let offset = (s * 512) as usize;
                if offset >= len { break; }
                let mut sector = [0u8; 512];
                let end = (offset + 512).min(len);
                sector[..end - offset].copy_from_slice(&data[offset..end]);
                self.reader.write_sector_sync(base_lba + s, &sector)?;
            }
        }
        Ok(())
    }

    /// Like 'write_block_direct_nocache' with a write barrier (FUA), durable on
    /// return. Whole-block only, for the journal commit record
    pub fn write_block_barrier_nocache(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        let spb = self.sectors_per_block() as u8;
        let base_lba = self.block_to_lba(block_num);
        let bs = self.block_size as usize;
        if data.len() >= bs {
            self.reader.write_block_barrier(base_lba, &data[..bs], spb)
        } else {
            // Short buffer: pads to a full block, then barrier-writes
            let mut blk = [0u8; 4096];
            blk[..data.len()].copy_from_slice(data);
            self.reader.write_block_barrier(base_lba, &blk[..bs], spb)
        }
    }

    pub fn write_block_data_direct(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        let spb = self.sectors_per_block() as u8;
        let base_lba = self.block_to_lba(block_num);
        let bs = self.block_size as usize;
        let len = data.len().min(bs);
        if len == bs {
            self.reader.write_block(base_lba, &data[..bs], spb)?;
        } else {
            for s in 0..spb as u32 {
                let offset = (s * 512) as usize;
                if offset >= len {
                    break;
                }
                let mut sector = [0u8; 512];
                let end = (offset + 512).min(len);
                sector[..end - offset].copy_from_slice(&data[offset..end]);
                self.reader.write_sector(base_lba + s, &sector)?;
            }
        }
        // Disk holds the newest content, and the block-layer cache was updated,
        // so a staged copy here is stale
        if let Some(ref mut c) = self.block_cache {
            c.invalidate(block_num);
        }
        Ok(())
    }

    /// Whole blocks of file data from `start`, past the block cache, one call
    /// per run, dropping the cache's copies. `sync` puts them on the device
    /// before returning, as a new block's data must precede its extent;
    /// otherwise they enter the block layer's write-back cache. A block the
    /// open transaction logged or freed goes through the cache, pinned until
    /// the commit, since until then a crash returns it to its previous owner
    pub fn write_data_blocks(&mut self, start: u32, blocks: &[&[u8]], sync: bool) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let spb = self.sectors_per_block();
        let held = |fs: &Self, b: u32| {
            fs.block_cache.as_ref().is_some_and(|c| c.is_pinned(b)) || fs.freed_in_running(b)
        };
        let mut i = 0;
        while i < blocks.len() {
            let first = start + i as u32;
            if held(self, first) && self.block_cache.is_some() {
                if let Some(ref mut c) = self.block_cache {
                    c.pin(first);
                }
                self.write_block_data(first, blocks[i])?;
                i += 1;
                continue;
            }
            let mut n = 1;
            while i + n < blocks.len() && !held(self, first + n as u32) {
                n += 1;
            }
            let lba = self.block_to_lba(first);
            let put = |fs: &mut Self, data: &[u8], sectors: u32| {
                if sync {
                    fs.reader.write_blocks_sync(lba, data, sectors)
                } else {
                    fs.reader.write_blocks(lba, data, sectors)
                }
            };
            if n == 1 {
                put(self, &blocks[i][..bs], spb)?;
            } else {
                let mut buf = core::mem::take(&mut self.data_buf);
                buf.clear();
                for b in &blocks[i..i + n] {
                    buf.extend_from_slice(&b[..bs]);
                }
                let r = put(self, &buf, n as u32 * spb);
                self.data_buf = buf;
                r?;
            }
            if let Some(ref mut c) = self.block_cache {
                for b in first..first + n as u32 {
                    c.invalidate(b);
                }
            }
            i += n;
        }
        Ok(())
    }

    pub fn write_block_data(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        // The reader refuses a read-only mount's writes, but a staged block is
        // what reads see until flushed. Refused here, at the first change
        if self.reader.read_only {
            return Err(FsError::ReadOnlyFs);
        }
        let bs = self.block_size as usize;

        let needs_flush = match self.block_cache {
            Some(ref c) => data.len() >= bs && c.should_flush(),
            None => false,
        };
        if needs_flush {
            self.sync_dirty_blocks()?;
        }

        // Flushes a dirty victim before put_dirty() evicts it. A block with a
        // slot is written in place
        let resident = self.block_cache.as_ref().is_some_and(|c| c.contains(block_num));
        if !resident {
            self.flush_evict_victim()?;
        }

        if self.block_cache.is_some() && data.len() >= bs {
            if self.cache_put_dirty(block_num, &data[..bs]) {
                return Ok(());
            }

            // Every slot holds a logged, uncommitted block. Evicting one would
            // put an uncommitted change at its real location, so commit,
            // releasing the pins, and retry once
            crate::extlog!(
                "[ext3] cache full of pinned blocks - committing to make room"
            );
            self.commit_midway()?;
            self.flush_evict_victim()?;
            if self.cache_put_dirty(block_num, &data[..bs]) {
                return Ok(());
            }

            // Still no room after a commit: the cache is smaller than one
            // transaction. Going to the medium is safe since the commit emptied
            // the pin set
            crate::extlog!("[ext3] cache cannot stage block {} - writing through", block_num);
            return self.write_block_data_direct(block_num, data);
        }
        self.write_block_data_direct(block_num, data)
    }

    /// Writes a block of filesystem metadata, logging it first. A block is
    /// protected because it passes through here.
    ///
    /// File data does not; journaling it would be `data=journal`.
    ///
    /// Zeroing a freshly allocated block needs no record: nothing committed
    /// refers to it, and the write giving its contents does pass here
    pub fn write_meta_block(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        self.ext3_journal_current_block(block_num)?;
        self.write_block_data(block_num, data)
    }

    /// Stages a modified block, reporting whether the cache had room
    fn cache_put_dirty(&mut self, block_num: u32, data: &[u8]) -> bool {
        match self.block_cache {
            Some(ref mut c) => c.put_dirty(block_num, data),
            None => false,
        }
    }

    /// Checkpoint: writes staged blocks to their real locations.
    /// `get_dirty_blocks` omits blocks the open transaction logged, so an
    /// uncommitted caller writes fewer
    pub fn sync_dirty_blocks(&mut self) -> Result<(), FsError> {
        let dirty = match self.block_cache {
            Some(ref c) => c.get_dirty_blocks(),
            None => return Ok(()),
        };
        if dirty.is_empty() { return Ok(()); }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        for (block_num, slot) in dirty {
            if let Some(ref c) = self.block_cache {
                c.get_block_data(slot, &mut buf[..bs]);
            }
            let spb = self.sectors_per_block() as u8;
            let base_lba = self.block_to_lba(block_num);
            self.reader.write_block(base_lba, &buf[..bs], spb)?;
            if let Some(ref mut c) = self.block_cache {
                c.mark_clean(slot);
            }
        }
        Ok(())
    }

    /// Writes a directory block, refreshing its checksum. Every directory
    /// change goes through here, as the checksum covers the contents
    pub fn write_dir_block(
        &mut self,
        dir_ino: u32,
        block_num: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        if self.dirent_tail_len() != 0 {
            let gen = self.read_inode(dir_ino).map(|i| i.generation()).unwrap_or(0);
            self.stamp_dir_block(dir_ino, gen, buf);
        }
        let bs = self.block_size as usize;
        self.write_meta_block(block_num, &buf[..bs])
    }

    /// Writes a directory index block, refreshing its checksum.
    /// `write_dir_block` looks for the 0xDE fake entry and returns if absent,
    /// which would leave an index block's checksum stale
    pub fn write_dx_block(
        &mut self,
        dir_ino: u32,
        block_num: u32,
        entries_off: usize,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        let gen = self.read_inode(dir_ino).map(|i| i.generation()).unwrap_or(0);
        self.stamp_dx_block(dir_ino, gen, entries_off, buf);
        let bs = self.block_size as usize;
        self.write_meta_block(block_num, &buf[..bs])
    }

    /// Writes an extent tree node, refreshing its tail checksum where the block
    /// leaves. Keyed by the owning inode, so a node needs its owner
    pub fn write_extent_node(
        &mut self,
        inode_num: u32,
        gen: u32,
        block_num: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        self.stamp_extent_block(inode_num, gen, buf);
        let bs = self.block_size as usize;
        self.write_meta_block(block_num, &buf[..bs])
    }

    pub fn zero_block(&mut self, block_num: u32) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let zero = [0u8; 4096];
        self.write_block_data(block_num, &zero[..bs])
    }

    /// Time stamped into `i_dtime` when an inode is freed. `i_dtime == 0` on an
    /// inode with no links is what a checker calls "deleted inode has zero
    /// dtime", so a host with no wall clock still gets a nonzero value
    pub fn deletion_timestamp(&self) -> u32 {
        self.get_timestamp().max(1)
    }

    /// Seconds since the Unix epoch for an on-disk timestamp. Tick-derived when
    /// the host has no wall clock, giving a small monotonic number
    pub fn get_timestamp(&self) -> u32 {
        if let Some(wall) = self.reader.host.wall_clock() {
            if wall > 0 {
                return wall;
            }
        }
        let hz = self.reader.host.tick_hz().max(1) as u64;
        (self.reader.host.ticks() / hz) as u32
    }

    // Orphan inode cleanup: walks the orphan list in the superblock and frees
    // inodes with links_count == 0
    /// Offset of `s_last_orphan` in the superblock: head of the list of inodes
    /// being deleted
    const S_LAST_ORPHAN: usize = 232;

    /// Chains an inode into the orphan list. A free is several writes (blocks,
    /// block bitmap, inode bitmap, counters), and this list records it:
    /// `s_last_orphan` names the head and each inode's `i_dtime` the next, so
    /// the next mount can finish
    pub fn orphan_add(&mut self, inode_num: u32) -> Result<(), FsError> {
        if inode_num == 0 {
            return Err(FsError::InvalidInode);
        }
        let head = self.superblock.last_orphan();
        if head == inode_num {
            return Ok(());
        }

        // The dtime is the "next" pointer while on the list, as in ext2; it is
        // a timestamp only once the inode is free
        let mut inode = self.read_inode(inode_num)?;
        inode.set_dtime(head);
        self.write_inode(inode_num, &inode)?;

        self.superblock.write_u32(Self::S_LAST_ORPHAN, inode_num);
        // Write-through: a list head only in cache would be lost in a crash
        self.superblock_dirty = true;
        self.flush_all_dirty_metadata()
    }

    /// Take an inode back off the orphan list, its deletion having completed.
    pub fn orphan_del(&mut self, inode_num: u32) -> Result<(), FsError> {
        if inode_num == 0 {
            return Ok(());
        }
        let head = self.superblock.last_orphan();
        if head == 0 {
            return Ok(());
        }

        let next = self.read_inode(inode_num)?.dtime();

        if head == inode_num {
            self.superblock.write_u32(Self::S_LAST_ORPHAN, next);
            self.superblock_dirty = true;
            return self.flush_all_dirty_metadata();
        }

        // Not the head: walk to the predecessor. Bounded against a corrupted
        // list
        let mut prev = head;
        let max = self.superblock.inodes_count();
        for _ in 0..1024 {
            if prev == 0 || prev > max {
                return Ok(());
            }
            let mut prev_inode = self.read_inode(prev)?;
            let this = prev_inode.dtime();
            if this == inode_num {
                prev_inode.set_dtime(next);
                return self.write_inode(prev, &prev_inode);
            }
            prev = this;
        }
        Ok(())
    }

    pub fn cleanup_orphans(&mut self) -> Result<u32, FsError> {
        let mut ino = self.superblock.last_orphan();
        if ino == 0 {
            return Ok(0);
        }
        let mut cleaned = 0u32;
        let max_inodes = self.superblock.inodes_count();
        let mut iterations = 0u32;

        while ino != 0 && ino <= max_inodes && iterations < 1024 {
            iterations += 1;
            let inode = match self.read_inode(ino) {
                Ok(i) => i,
                Err(_) => break,
            };
            // dtime field stores next orphan inode in the list
            let next_orphan = inode.dtime();

            if inode.links_count() == 0 {
                // free blocks and inode
                if inode.is_directory() || inode.is_regular() {
                    if inode.uses_extents() {
                        let _ = self.ext4_free_extent_blocks(&inode);
                    } else if !inode.is_symlink() || !inode.is_fast_symlink() {
                        let _ = self.free_all_blocks(&inode);
                    }
                }
                let _ = self.free_inode(ino);
                cleaned += 1;
                crate::extlog!("[orphan] cleaned inode {}", ino);
            } else {
                // truncate to 0 if it's a truncated-but-open file
                if inode.size() > 0 && inode.is_regular() {
                    if inode.uses_extents() {
                        let _ = self.ext4_truncate(ino);
                    } else {
                        let _ = self.ext2_truncate(ino);
                    }
                    cleaned += 1;
                    crate::extlog!("[orphan] truncated inode {}", ino);
                }
            }
            ino = next_orphan;
        }

        // clear orphan list head in superblock
        self.superblock.write_u32(Self::S_LAST_ORPHAN, 0);
        self.flush_superblock()?;

        if cleaned > 0 {
            crate::extlog!("[orphan] cleaned {} orphan inodes", cleaned);
        }
        Ok(cleaned)
    }

    // get physical block for inode_num + logical block (convenience)
    pub fn get_file_block_any(&mut self, inode_num: u32, logical: u32) -> Result<u32, FsError> {
        let inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            let owner = Some((inode_num, inode.generation()));
            self.get_file_block_extent_owned(&inode, owner, logical)
        } else {
            self.get_file_block(&inode, logical)
        }
    }

    pub fn touch_atime(&mut self, inode_num: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        // respect noatime flag on inode
        if inode.has_flag(EXT4_NOATIME_FL) {
            return Ok(());
        }
        let now = self.get_timestamp();
        if inode.atime() == now {
            return Ok(()); // no-op if already current second
        }
        // relatime: updates only if atime < mtime or atime is older than 1 day
        let mtime = inode.mtime();
        let atime = inode.atime();
        if atime >= mtime && now.saturating_sub(atime) < 86400 {
            return Ok(());
        }
        inode.set_atime(now);
        self.write_inode(inode_num, &inode)
    }

    // utimensat: sets atime and mtime explicitly. atime_val/mtime_val: 0 =
    // unchanged, u32::MAX = current time
    pub fn utimensat(
        &mut self,
        inode_num: u32,
        atime_val: u32,
        mtime_val: u32,
    ) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        let now = self.get_timestamp();
        if atime_val == u32::MAX {
            inode.set_atime(now);
        } else if atime_val != 0 {
            inode.set_atime(atime_val);
        }
        if mtime_val == u32::MAX {
            inode.set_mtime(now);
        } else if mtime_val != 0 {
            inode.set_mtime(mtime_val);
        }
        inode.set_ctime(now); // ctime always updated on utimensat
        self.write_inode(inode_num, &inode)
    }

    // update superblock mount state
    pub fn update_mount_state(&mut self) {
        let now = self.get_timestamp();
        let count = self.superblock.mnt_count().wrapping_add(1);
        self.superblock.write_u16(52, count);  // s_mnt_count
        self.superblock.write_u32(44, now);    // s_mtime (last mount time)
        // Clears "cleanly unmounted", which is what being mounted means
        let state = self.superblock.state() & !EXT2_VALID_FS;
        self.superblock.write_u16(58, state);
        self.superblock_dirty = true;
    }

    // mark filesystem as cleanly unmounted
    pub fn mark_clean_unmount(&mut self) {
        let now = self.get_timestamp();
        self.superblock.write_u32(48, now);    // s_wtime (last write time)
        // Cleanly unmounted. A recorded fault survives until a check fixes it
        let state = self.superblock.state() | EXT2_VALID_FS;
        self.superblock.write_u16(58, state);
        self.superblock_dirty = true;
    }

    /// Leaves the volume: everything on the medium, the clean-unmount stamp
    /// set, and backup superblocks updated, in that order, as backups copy the
    /// final primary
    pub fn finish_unmount(&mut self) -> Result<(), FsError> {
        self.sync()?;
        self.mark_clean_unmount();
        self.flush_all_dirty_metadata()?;
        self.write_superblock_backups()?;
        self.reader.flush_drive();
        Ok(())
    }

    /// Room for a whole transaction's pinned blocks and surrounding metadata,
    /// since a full cache commits mid-operation. Lookups use a map, so the size
    /// costs no scan
    pub fn init_cache(&mut self) {
        let bs = self.block_size as usize;
        let max_cache_bytes: usize = 2 * 1024 * 1024;
        let entries = (max_cache_bytes / bs).min(crate::cache::MAX_CACHE_ENTRIES);
        self.block_cache = Some(crate::cache::BlockCache::new(bs, entries));
    }

    pub fn drop_cache(&mut self) {
        self.block_cache = None;
    }

    pub fn warm_cache(&mut self) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        for g in 0..self.group_count.min(4) as usize {
            if g >= self.groups.len() { break; }
            let bb = self.groups[g].block_bitmap();
            if bb != 0 { self.read_block_into(bb, &mut buf[..bs])?; }
            let ib = self.groups[g].inode_bitmap();
            if ib != 0 { self.read_block_into(ib, &mut buf[..bs])?; }
            let it = self.groups[g].inode_table();
            if it != 0 {
                self.read_block_into(it, &mut buf[..bs])?;
                self.read_block_into(it + 1, &mut buf[..bs])?;
            }
        }
        if let Ok(root) = self.read_inode(EXT2_ROOT_INO) {
            if root.is_directory() {
                if let Ok(first_block) = self.get_file_block(&root, 0) {
                    if first_block != 0 {
                        let _ = self.read_block_into(first_block, &mut buf[..bs]);
                    }
                }
            }
        }
        if self.has_journal() {
            if let Some(j_inode) = self.journal_inode_cached {
                for b in 0..4u32 {
                    if let Ok(db) = self.get_file_block(&j_inode, b) {
                        if db != 0 { let _ = self.read_block_into(db, &mut buf[..bs]); }
                    }
                }
            }
        }
        Ok(())
    }

    pub fn journal_checkpoint_if_needed(&mut self) -> Result<(), FsError> {
        if !self.journal_active { return Ok(()); }
        let used = if self.journal_pos >= self.journal_first {
            self.journal_pos - self.journal_first
        } else {
            self.journal_maxlen - self.journal_first + self.journal_pos
        };
        if used > self.journal_maxlen * 3 / 4 {
            crate::extlog!("[ext3] journal checkpoint: used={}/{}", used, self.journal_maxlen);
            self.sync()?;
            self.ext3_clean_journal()?;
        }
        Ok(())
    }

    pub fn fs_info(&self) -> FsInfo {
        FsInfo {
            block_size: self.block_size,
            total_blocks: self.superblock.blocks_count(),
            free_blocks: self.superblock.free_blocks_count(),
            total_inodes: self.superblock.inodes_count(),
            free_inodes: self.superblock.free_inodes_count(),
            groups: self.group_count,
            inode_size: self.inode_size(),
            has_journal: self.superblock.has_journal(),
            has_extents: self.superblock.has_extents(),
            version: self.superblock.fs_version_str(),
        }
    }

    pub fn is_ext4(&self) -> bool {
        self.superblock.is_ext4()
    }

    pub fn enable_extents_feature(&mut self) -> Result<(), FsError> {
        if self.superblock.has_extents() {
            return Ok(());
        }
        let incompat = self.superblock.feature_incompat();
        self.superblock
            .write_u32(96, incompat | FEATURE_INCOMPAT_EXTENTS);
        self.flush_superblock()
    }
}

pub struct FsInfo {
    pub block_size: u32,
    pub total_blocks: u32,
    pub free_blocks: u32,
    pub total_inodes: u32,
    pub free_inodes: u32,
    pub groups: u32,
    pub inode_size: u32,
    pub has_journal: bool,
    pub has_extents: bool,
    pub version: &'static str,
}
