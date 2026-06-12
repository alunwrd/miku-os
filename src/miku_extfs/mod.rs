pub mod cache;
pub mod error;
pub mod ext2;
pub mod ext3;
pub mod ext4;
pub mod reader;
pub mod structs;
pub mod trim;
pub mod xattr;

use ext3::journal::TxnTag;
use reader::DiskReader;
use structs::*;

pub use error::FsError;

pub struct MikuFS {
    pub superblock: Superblock,
    pub block_size: u32,
    pub inodes_per_group: u32,
    pub blocks_per_group: u32,
    pub group_count: u32,
    pub groups: [GroupDesc; 32],
    pub reader: DiskReader,
    pub journal_seq: u32,
    pub journal_pos: u32,
    pub journal_maxlen: u32,
    pub journal_first: u32,
    pub journal_active: bool,
    pub txn_active: bool,
    pub txn_desc_pos: u32,
    pub txn_tags: [TxnTag; 64],
    pub txn_tag_count: u8,
    pub txn_revokes: [u32; 128],
    pub txn_revoke_count: u8,
    pub block_cache: Option<cache::BlockCache>,
    pub superblock_dirty: bool,
    pub groups_dirty: [bool; 32],
    pub last_sync_ticks: u64,
    pub journal_inode_cached: Option<Inode>,
    pub alloc_hint: [u32; 32],
}

pub const MAX_DIR_ENTRIES: usize = 64;
const SYNC_INTERVAL_TICKS: u64 = 90;

impl MikuFS {
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
    fn is_valid_block(&self, block: u32) -> bool {
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

    pub fn periodic_sync(&mut self) -> Result<bool, FsError> {
        if !self.has_dirty_data() {
            return Ok(false);
        }

        let dirty_count = match self.block_cache {
            Some(ref c) => c.dirty_entries(),
            None => 0,
        };

        if self.journal_active && dirty_count > 0 {
            self.ext3_begin_txn()?;

            let dirty_blocks = match self.block_cache {
                Some(ref c) => c.get_dirty_blocks(),
                None => alloc::vec::Vec::new(),
            };
            for &(block_num, _) in dirty_blocks.iter().take(64) {
                let _ = self.ext3_journal_current_block(block_num);
            }

            self.ext3_commit_txn()?;
        }

        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.reader.flush_drive();

        crate::serial_println!(
            "[pdflush] synced {} dirty blocks",
            dirty_count
        );

        Ok(true)
    }

    pub fn check_periodic_sync(&mut self) {
        let now = crate::vfs::procfs::uptime_ticks();
        if now.wrapping_sub(self.last_sync_ticks) < SYNC_INTERVAL_TICKS {
            return;
        }
        self.last_sync_ticks = now;

        if !self.has_dirty_data() {
            return;
        }

        let _ = self.periodic_sync();
    }

    fn do_write_superblock(&mut self) -> Result<(), FsError> {
        if self.superblock.has_metadata_csum() {
            self.update_superblock_csum();
        }
        let mut s0 = [0u8; 512];
        let mut s1 = [0u8; 512];
        s0.copy_from_slice(&self.superblock.data[0..512]);
        s1.copy_from_slice(&self.superblock.data[512..1024]);
        self.reader.write_sector(2, &s0)?;
        self.reader.write_sector(3, &s1)?;
        self.superblock_dirty = false;
        Ok(())
    }

    pub fn flush_group_desc(&mut self, group: usize) -> Result<(), FsError> {
        if group < 32 {
            self.groups_dirty[group] = true;
        }
        Ok(())
    }

    fn do_write_group_desc(&mut self, group: usize) -> Result<(), FsError> {
        if group >= 32 {
            return Ok(());
        }
        if self.superblock.has_metadata_csum() || self.superblock.has_gdt_csum() {
            self.update_group_desc_csum(group);
        }
        let gdt_block = if self.block_size == 1024 { 2 } else { 1 };
        let gd_size = self.superblock.group_desc_size() as usize;
        let gd_byte_offset = group * gd_size;
        let sector_offset = gd_byte_offset / 512;
        let offset_in_sector = gd_byte_offset % 512;
        let lba = self.block_to_lba(gdt_block) + sector_offset as u32;
        let mut sector = [0u8; 512];
        self.reader.read_sector(lba, &mut sector)?;
        let write_len = gd_size.min(64);
        sector[offset_in_sector..offset_in_sector + write_len]
            .copy_from_slice(&self.groups[group].data[..write_len]);
        self.reader.write_sector(lba, &sector)?;
        self.groups_dirty[group] = false;
        Ok(())
    }

    pub fn flush_all_dirty_metadata(&mut self) -> Result<(), FsError> {
        let had_dirty = self.superblock_dirty || self.groups_dirty.iter().any(|&d| d);
        if self.superblock_dirty {
            self.do_write_superblock()?;
        }
        for group in 0..32 {
            if self.groups_dirty[group] {
                self.do_write_group_desc(group)?;
            }
        }
        // Metadata went through the write-back cache; drain it so callers
        // (unmount, reformat) leave the disk state durable
        if had_dirty {
            self.reader.flush_drive();
        }
        Ok(())
    }

    pub fn sync(&mut self) -> Result<(), FsError> {
        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.reader.flush_drive();
        Ok(())
    }

    pub fn write_inode(&mut self, inode_num: u32, inode: &Inode) -> Result<(), FsError> {
        if inode_num == 0 {
            return Err(FsError::InvalidInode);
        }
        let mut stamped = *inode;
        self.stamp_inode_csum(inode_num, &mut stamped);
        let idx = inode_num - 1;
        let group = (idx / self.inodes_per_group) as usize;
        let local_idx = idx % self.inodes_per_group;
        if group >= self.groups.len() {
            return Err(FsError::InvalidInode);
        }
        let inode_table_block = self.groups[group].inode_table();
        let inode_size = self.superblock.inode_size_val();
        let write_size = (inode_size as usize).min(256);
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
            self.write_block_data(phys_block, &buf[..bs])?;
        } else {
            let first_part = bs - offset_in_block;
            buf[offset_in_block..bs].copy_from_slice(&stamped.data[..first_part]);
            self.write_block_data(phys_block, &buf[..bs])?;

            let next_block = phys_block + 1;
            self.read_block_into(next_block, &mut buf[..bs])?;
            let remaining = write_size - first_part;
            buf[..remaining].copy_from_slice(&stamped.data[first_part..write_size]);
            self.write_block_data(next_block, &buf[..bs])?;
        }
        Ok(())
    }

    // flush a single dirty cache slot to disk before eviction
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
        // The FS cache is consulted only because it may hold a dirty block
        // newer than what is on disk (write-back staging for the journal).
        // Clean read caching is the block layer's job now - populating this
        // cache on read misses would just duplicate the buffer cache
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

    /// Ordered journal write: goes through the block layer's write-through
    /// path so descriptor/data/commit records land on disk in issue order -
    /// the WAL guarantee write-back caching must not break
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
        // Disk now holds the newest content (and the block-layer cache was
        // updated by the write-through); any staged copy here is stale
        if let Some(ref mut c) = self.block_cache {
            c.invalidate(block_num);
        }
        Ok(())
    }

    pub fn write_block_data(&mut self, block_num: u32, data: &[u8]) -> Result<(), FsError> {
        let bs = self.block_size as usize;

        let needs_flush = match self.block_cache {
            Some(ref c) => data.len() >= bs && c.should_flush(),
            None => false,
        };
        if needs_flush {
            self.sync_dirty_blocks()?;
        }

        // flush dirty victim before put_dirty() evicts it
        self.flush_evict_victim()?;

        if let Some(ref mut c) = self.block_cache {
            if data.len() >= bs {
                c.put_dirty(block_num, &data[..bs]);
                return Ok(());
            }
        }
        self.write_block_data_direct(block_num, data)
    }

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

    pub fn zero_block(&mut self, block_num: u32) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let zero = [0u8; 4096];
        self.write_block_data(block_num, &zero[..bs])
    }

    pub fn get_timestamp(&self) -> u32 {
        let wall = crate::vfs::procfs::wall_clock();
        if wall > 0 {
            wall as u32
        } else {
            (crate::vfs::procfs::uptime_ticks() / crate::interrupts::PIT_HZ as u64) as u32
        }
    }

    // orphan inode cleanup - walk the orphan linked list in the superblock
    // and free any inodes with links_count == 0
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
                crate::serial_println!("[orphan] cleaned inode {}", ino);
            } else {
                // truncate to 0 if it's a truncated-but-open file
                if inode.size() > 0 && inode.is_regular() {
                    if inode.uses_extents() {
                        let _ = self.ext4_truncate(ino);
                    } else {
                        let _ = self.ext2_truncate(ino);
                    }
                    cleaned += 1;
                    crate::serial_println!("[orphan] truncated inode {}", ino);
                }
            }
            ino = next_orphan;
        }

        // clear orphan list head in superblock
        self.superblock.write_u32(232, 0);
        self.flush_superblock()?;

        if cleaned > 0 {
            crate::serial_println!("[orphan] cleaned {} orphan inodes", cleaned);
        }
        Ok(cleaned)
    }

    // get physical block for inode_num + logical block (convenience)
    pub fn get_file_block_any(&mut self, inode_num: u32, logical: u32) -> Result<u32, FsError> {
        let inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            self.get_file_block_extent(&inode, logical)
        } else {
            self.get_file_block(&inode, logical)
        }
    }

    pub fn touch_atime(&mut self, inode_num: u32) -> Result<(), FsError> {
        let mut inode = self.read_inode(inode_num)?;
        // respect noatime flag on inode
        if inode.has_flag(structs::EXT4_NOATIME_FL) {
            return Ok(());
        }
        let now = self.get_timestamp();
        if inode.atime() == now {
            return Ok(()); // no-op if already current second
        }
        // relatime: only update if atime < mtime or atime is older than 1 day
        let mtime = inode.mtime();
        let atime = inode.atime();
        if atime >= mtime && now.saturating_sub(atime) < 86400 {
            return Ok(());
        }
        inode.set_atime(now);
        self.write_inode(inode_num, &inode)
    }

    // utimensat - set atime/mtime explicitly
    // atime_val/mtime_val: 0 = leave unchanged, u32::MAX = set to current time
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
        self.superblock.write_u16(58, 2);      // s_state = EXT2_ERROR_FS (not cleanly unmounted)
        self.superblock_dirty = true;
    }

    // mark filesystem as cleanly unmounted
    pub fn mark_clean_unmount(&mut self) {
        let now = self.get_timestamp();
        self.superblock.write_u32(48, now);    // s_wtime (last write time)
        self.superblock.write_u16(58, 1);      // s_state = EXT2_VALID_FS
        self.superblock_dirty = true;
    }

    pub fn init_cache(&mut self) {
        let bs = self.block_size as usize;
        let max_cache_bytes: usize = 512 * 1024;
        let entries = (max_cache_bytes / bs).min(256);
        self.block_cache = Some(cache::BlockCache::new(bs, entries));
    }

    pub fn drop_cache(&mut self) {
        self.block_cache = None;
    }

    pub fn warm_cache(&mut self) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        for g in 0..self.group_count.min(4) as usize {
            if g >= 32 { break; }
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
            crate::serial_println!("[ext3] journal checkpoint: used={}/{}", used, self.journal_maxlen);
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
