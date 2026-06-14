use crate::miku_extfs::structs::*;
use crate::miku_extfs::{FsError, MikuFS};

pub const JBD_MAGIC: u32 = 0xC03B3998;

pub const JBD_DESCRIPTOR_BLOCK: u32 = 1;
pub const JBD_COMMIT_BLOCK: u32 = 2;
pub const JBD_SUPERBLOCK_V1: u32 = 3;
pub const JBD_SUPERBLOCK_V2: u32 = 4;
pub const JBD_REVOKE_BLOCK: u32 = 5;

pub const JBD_FLAG_ESCAPE: u32 = 1;
pub const JBD_FLAG_SAME_UUID: u32 = 2;
pub const JBD_FLAG_DELETED: u32 = 4;
pub const JBD_FLAG_LAST_TAG: u32 = 8;

pub const DEFAULT_JOURNAL_BLOCKS: u32 = 256;

#[derive(Clone, Copy)]
pub struct TxnTag {
    pub fs_block: u32,
    pub journal_pos: u32,
}

impl TxnTag {
    pub const fn empty() -> Self {
        Self { fs_block: 0, journal_pos: 0 }
    }
}

#[derive(Clone, Copy)]
pub struct JournalSuperblock {
    pub data: [u8; 1024],
}

impl JournalSuperblock {
    pub const fn zeroed() -> Self { Self { data: [0; 1024] } }

    fn read_be32(&self, offset: usize) -> u32 {
        u32::from_be_bytes([
            self.data[offset], self.data[offset+1],
            self.data[offset+2], self.data[offset+3],
        ])
    }

    pub fn write_be32(&mut self, offset: usize, val: u32) {
        self.data[offset..offset+4].copy_from_slice(&val.to_be_bytes());
    }

    pub fn magic(&self) -> u32 { self.read_be32(0) }
    pub fn blocktype(&self) -> u32 { self.read_be32(4) }
    pub fn blocksize(&self) -> u32 { self.read_be32(12) }
    pub fn maxlen(&self) -> u32 { self.read_be32(16) }
    pub fn first(&self) -> u32 { self.read_be32(20) }
    pub fn start_sequence(&self) -> u32 { self.read_be32(24) }
    pub fn start(&self) -> u32 { self.read_be32(28) }
    pub fn errno_val(&self) -> i32 { self.read_be32(32) as i32 }
    pub fn uuid(&self) -> &[u8] { &self.data[48..64] }
    pub fn is_valid(&self) -> bool { self.magic() == JBD_MAGIC }
    pub fn is_clean(&self) -> bool { self.start() == 0 }
    pub fn is_v2(&self) -> bool { self.blocktype() == JBD_SUPERBLOCK_V2 }
}

#[derive(Clone, Copy)]
pub struct JournalHeader {
    pub magic: u32,
    pub blocktype: u32,
    pub sequence: u32,
}

impl JournalHeader {
    pub fn from_buf(buf: &[u8]) -> Self {
        Self {
            magic:     u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]),
            blocktype: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            sequence:  u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
        }
    }

    pub fn is_valid(&self) -> bool { self.magic == JBD_MAGIC }
    pub fn is_descriptor(&self) -> bool { self.blocktype == JBD_DESCRIPTOR_BLOCK }
    pub fn is_commit(&self) -> bool { self.blocktype == JBD_COMMIT_BLOCK }
}

#[derive(Clone, Copy)]
pub struct JournalBlockTag {
    pub blocknr: u32,
    pub flags: u32,
}

impl JournalBlockTag {
    pub fn from_buf(buf: &[u8], offset: usize) -> Self {
        Self {
            blocknr: u32::from_be_bytes([
                buf[offset], buf[offset+1], buf[offset+2], buf[offset+3],
            ]),
            flags: u32::from_be_bytes([
                buf[offset+4], buf[offset+5], buf[offset+6], buf[offset+7],
            ]),
        }
    }

    pub fn is_last(&self) -> bool { self.flags & JBD_FLAG_LAST_TAG != 0 }
    pub fn same_uuid(&self) -> bool { self.flags & JBD_FLAG_SAME_UUID != 0 }
}

#[derive(Clone, Copy)]
pub struct JournalTransaction {
    pub sequence: u32,
    pub start_block: u32,
    pub data_blocks: u32,
    pub committed: bool,
    pub active: bool,
}

impl JournalTransaction {
    pub const fn empty() -> Self {
        Self { sequence: 0, start_block: 0, data_blocks: 0, committed: false, active: false }
    }
}

pub struct JournalInfo {
    pub valid: bool,
    pub version: u8,
    pub block_size: u32,
    pub total_blocks: u32,
    pub first_block: u32,
    pub start: u32,
    pub sequence: u32,
    pub clean: bool,
    pub errno: i32,
    pub transactions: [JournalTransaction; 32],
    pub transaction_count: usize,
    pub journal_inode: u32,
    pub journal_size: u64,
}

impl JournalInfo {
    pub const fn empty() -> Self {
        Self {
            valid: false, version: 0, block_size: 0, total_blocks: 0,
            first_block: 0, start: 0, sequence: 0, clean: false, errno: 0,
            transactions: [JournalTransaction::empty(); 32],
            transaction_count: 0, journal_inode: 0, journal_size: 0,
        }
    }
}

impl MikuFS {
    pub fn has_journal(&self) -> bool {
        self.superblock.has_journal()
    }

    pub fn journal_block_to_disk(&mut self, journal_block: u32) -> Result<u32, FsError> {
        let journal_inode = match self.journal_inode_cached {
            Some(ino) => ino,
            None => {
                let ino = self.read_inode(EXT2_JOURNAL_INO)?;
                self.journal_inode_cached = Some(ino);
                ino
            }
        };
        self.get_file_block(&journal_inode, journal_block)
    }

    pub fn read_journal_superblock(&mut self) -> Result<JournalSuperblock, FsError> {
        if !self.has_journal() {
            return Err(FsError::NoJournal);
        }
        let disk_block = self.journal_block_to_disk(0)?;
        if disk_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut jsb = JournalSuperblock::zeroed();
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_block, &mut buf[..bs])?;
        let copy_size = bs.min(1024);
        jsb.data[..copy_size].copy_from_slice(&buf[..copy_size]);
        if !jsb.is_valid() {
            return Err(FsError::CorruptedFs);
        }
        Ok(jsb)
    }

    pub fn read_journal_block_data(
        &mut self,
        journal_block: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        let disk_block = self.journal_block_to_disk(journal_block)?;
        if disk_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        self.read_block_into(disk_block, buf)
    }

    pub fn init_journal(&mut self) -> Result<(), FsError> {
        if !self.has_journal() {
            self.journal_active = false;
            return Ok(());
        }
        let j_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        self.journal_inode_cached = Some(j_inode);

        let jsb = self.read_journal_superblock()?;
        self.journal_seq = jsb.start_sequence();
        self.journal_maxlen = jsb.maxlen();
        self.journal_first = jsb.first();
        self.journal_active = true;
        self.txn_active = false;
        self.txn_tag_count = 0;
        self.txn_revoke_count = 0;
        if jsb.is_clean() {
            self.journal_pos = jsb.first();
        } else {
            self.journal_pos = jsb.start();
        }
        crate::serial_println!(
            "[ext3] journal init: seq={} pos={} max={} active=true",
            self.journal_seq, self.journal_pos, self.journal_maxlen
        );
        Ok(())
    }

    pub fn advance_journal_pos(&self, pos: u32) -> u32 {
        let next = pos + 1;
        if next >= self.journal_maxlen { self.journal_first } else { next }
    }

    pub fn ext3_begin_txn(&mut self) -> Result<(), FsError> {
        if !self.journal_active { return Ok(()); }
        if self.txn_active { return Ok(()); }
        self.journal_checkpoint_if_needed()?;
        self.txn_active = true;
        self.txn_desc_pos = self.journal_pos;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        self.txn_tag_count = 0;
        self.txn_revoke_count = 0;
        Ok(())
    }

    pub fn ext3_journal_current_block(&mut self, fs_block: u32) -> Result<(), FsError> {
        if !self.journal_active || !self.txn_active { return Ok(()); }
        if self.txn_tag_count >= 64 { return Ok(()); }
        for i in 0..self.txn_tag_count as usize {
            if self.txn_tags[i].fs_block == fs_block {
                return Ok(());
            }
        }
        let idx = self.txn_tag_count as usize;
        self.txn_tags[idx] = TxnTag { fs_block, journal_pos: self.journal_pos };
        self.txn_tag_count += 1;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        Ok(())
    }

    pub fn ext3_commit_txn(&mut self) -> Result<(), FsError> {
        if !self.journal_active || !self.txn_active { return Ok(()); }
        let tag_count = self.txn_tag_count as usize;
        if tag_count == 0 {
            self.txn_active = false;
            self.txn_revoke_count = 0;
            return Ok(());
        }
        let bs = self.block_size as usize;

        let mut desc = [0u8; 4096];
        desc[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        desc[4..8].copy_from_slice(&JBD_DESCRIPTOR_BLOCK.to_be_bytes());
        desc[8..12].copy_from_slice(&self.journal_seq.to_be_bytes());
        let mut offset = 12;
        for i in 0..tag_count {
            let tag_block = self.txn_tags[i].fs_block;
            let mut flags = JBD_FLAG_SAME_UUID;
            if i == tag_count - 1 { flags |= JBD_FLAG_LAST_TAG; }
            desc[offset..offset+4].copy_from_slice(&tag_block.to_be_bytes());
            desc[offset+4..offset+8].copy_from_slice(&flags.to_be_bytes());
            offset += 8;
        }
        let desc_disk_block = self.journal_block_to_disk(self.txn_desc_pos)?;
        self.write_block_direct_nocache(desc_disk_block, &desc[..bs])?;

        for i in 0..tag_count {
            let fs_block = self.txn_tags[i].fs_block;
            let journal_pos = self.txn_tags[i].journal_pos;
            let mut buf = [0u8; 4096];
            self.read_block_into(fs_block, &mut buf[..bs])?;
            let jdb = self.journal_block_to_disk(journal_pos)?;
            if jdb != 0 {
                self.write_block_direct_nocache(jdb, &buf[..bs])?;
            }
        }

        self.ext3_write_revoke_block()?;

        // Journal blocks written direct to disk (no cache pollution).
        // FS data blocks stay in cache - pdflush writes lazily.
        // No sync_dirty_blocks, no flush_drive = fast commit.

        let mut commit = [0u8; 4096];
        commit[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        commit[4..8].copy_from_slice(&JBD_COMMIT_BLOCK.to_be_bytes());
        commit[8..12].copy_from_slice(&self.journal_seq.to_be_bytes());
        let commit_disk_block = self.journal_block_to_disk(self.journal_pos)?;
        // Barrier-write the commit record: with FUA it is on stable media
        // before we advance the journal, so a crash here cannot leave a
        // transaction that replay would treat as committed but isn't durable
        self.write_block_barrier_nocache(commit_disk_block, &commit[..bs])?;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);

        self.mark_journal_dirty_fast()?;

        self.journal_seq += 1;
        self.txn_active = false;
        self.txn_tag_count = 0;
        self.txn_revoke_count = 0;

        Ok(())
    }

	fn mark_journal_dirty_fast(&mut self) -> Result<(), FsError> {
        let disk_blk = self.journal_block_to_disk(0)?;
        if disk_blk == 0 { return Err(FsError::CorruptedFs); }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        buf[24..28].copy_from_slice(&self.journal_seq.to_be_bytes());
        buf[28..32].copy_from_slice(&self.txn_desc_pos.to_be_bytes());
        self.write_block_data_direct(disk_blk, &buf[..bs])?;
        Ok(())
    }

    fn mark_journal_dirty_cached(&mut self) -> Result<(), FsError> {
        let disk_blk = self.journal_block_to_disk(0)?;
        if disk_blk == 0 { return Err(FsError::CorruptedFs); }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        buf[24..28].copy_from_slice(&self.journal_seq.to_be_bytes());
        buf[28..32].copy_from_slice(&self.txn_desc_pos.to_be_bytes());
        self.write_block_data(disk_blk, &buf[..bs])?;
        Ok(())
    }

    pub fn ext3_abort_txn(&mut self) {
        self.txn_active = false;
        self.txn_tag_count = 0;
        self.txn_revoke_count = 0;
    }

    fn mark_journal_dirty(&mut self) -> Result<(), FsError> {
        let disk_blk = {
            let j_inode = match self.journal_inode_cached {
                Some(ino) => ino,
                None => {
                    let ino = self.read_inode(EXT2_JOURNAL_INO)?;
                    self.journal_inode_cached = Some(ino);
                    ino
                }
            };
            self.get_file_block(&j_inode, 0)?
        };
        if disk_blk == 0 { return Err(FsError::CorruptedFs); }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        buf[24..28].copy_from_slice(&self.journal_seq.to_be_bytes());
        buf[28..32].copy_from_slice(&self.txn_desc_pos.to_be_bytes());
        self.write_block_data_direct(disk_blk, &buf[..bs])?;
        Ok(())
    }

    pub fn journal_inode_blocks(&mut self, inode_num: u32) -> Result<(), FsError> {
        let inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            let header = inode.extent_header();
            if header.depth > 0 {
                for i in 0..header.entries as usize {
                    let idx_blk = inode.extent_idx_at(i).leaf_lo;
                    if idx_blk != 0 {
                        self.ext3_journal_current_block(idx_blk)?;
                    }
                }
            }
        } else {
            for i in 0..12 {
                let blk = inode.block(i);
                if blk != 0 { self.ext3_journal_current_block(blk)?; }
            }
            let ind = inode.block(12);
            if ind != 0 { self.ext3_journal_current_block(ind)?; }
            let dind = inode.block(13);
            if dind != 0 { self.ext3_journal_current_block(dind)?; }
            let tind = inode.block(14);
            if tind != 0 { self.ext3_journal_current_block(tind)?; }
        }
        Ok(())
    }

    pub fn journal_inode_metadata(&mut self, inode_num: u32) -> Result<(), FsError> {
        if inode_num == 0 { return Ok(()); }
        let idx = inode_num - 1;
        let group = (idx / self.inodes_per_group) as usize;
        if group >= 32 { return Ok(()); }
        let it_block = self.groups[group].inode_table();
        let inode_size = self.inode_size();
        let local_idx = idx % self.inodes_per_group;
        let byte_off = local_idx as u64 * inode_size as u64;
        let block_off = (byte_off / self.block_size as u64) as u32;
        self.ext3_journal_current_block(it_block + block_off)?;
        self.ext3_journal_current_block(self.groups[group].inode_bitmap())?;
        Ok(())
    }

    pub fn journal_group_metadata(&mut self, group: usize) -> Result<(), FsError> {
        if group >= 32 { return Ok(()); }
        self.ext3_journal_current_block(self.groups[group].block_bitmap())?;
        Ok(())
    }

    pub fn ext3_journal_revoke_block(&mut self, fs_block: u32) -> Result<(), FsError> {
        if !self.journal_active || !self.txn_active { return Ok(()); }
        if self.txn_revoke_count >= 128 { return Ok(()); }
        for i in 0..self.txn_revoke_count as usize {
            if self.txn_revokes[i] == fs_block { return Ok(()); }
        }
        let idx = self.txn_revoke_count as usize;
        self.txn_revokes[idx] = fs_block;
        self.txn_revoke_count += 1;
        Ok(())
    }

    pub fn ext3_journal_revoke_inode_blocks(&mut self, inode_num: u32) -> Result<(), FsError> {
        if !self.journal_active || !self.txn_active { return Ok(()); }
        let inode = self.read_inode(inode_num)?;
        if inode.uses_extents() {
            let header = inode.extent_header();
            if header.depth > 0 {
                for i in 0..header.entries as usize {
                    let idx_blk = inode.extent_idx_at(i).leaf_lo;
                    if idx_blk != 0 {
                        self.ext3_journal_revoke_block(idx_blk)?;
                    }
                }
            }
        } else {
            for i in 0..12 {
                let blk = inode.block(i);
                if blk != 0 { self.ext3_journal_revoke_block(blk)?; }
            }
            let ind = inode.block(12);
            if ind != 0 { self.ext3_journal_revoke_block(ind)?; }
            let dind = inode.block(13);
            if dind != 0 { self.ext3_journal_revoke_block(dind)?; }
            let tind = inode.block(14);
            if tind != 0 { self.ext3_journal_revoke_block(tind)?; }
        }
        Ok(())
    }

    pub fn ext3_write_revoke_block(&mut self) -> Result<(), FsError> {
        if self.txn_revoke_count == 0 { return Ok(()); }
        let bs = self.block_size as usize;
        let mut revoke_data = [0u8; 4096];
        revoke_data[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        revoke_data[4..8].copy_from_slice(&JBD_REVOKE_BLOCK.to_be_bytes());
        revoke_data[8..12].copy_from_slice(&self.journal_seq.to_be_bytes());
        let count = self.txn_revoke_count as usize;
        let record_size = 16 + count * 4;
        revoke_data[12..16].copy_from_slice(&(record_size as u32).to_be_bytes());
        for i in 0..count {
            let offset = 16 + i * 4;
            revoke_data[offset..offset+4].copy_from_slice(&self.txn_revokes[i].to_be_bytes());
        }
        let revoke_disk_block = self.journal_block_to_disk(self.journal_pos)?;
        self.write_block_data_direct(revoke_disk_block, &revoke_data[..bs])?;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        self.txn_revoke_count = 0;
        Ok(())
    }

    pub fn ext3_write_revoke_block_cached(&mut self) -> Result<(), FsError> {
        if self.txn_revoke_count == 0 { return Ok(()); }
        let bs = self.block_size as usize;
        let mut revoke_data = [0u8; 4096];
        revoke_data[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        revoke_data[4..8].copy_from_slice(&JBD_REVOKE_BLOCK.to_be_bytes());
        revoke_data[8..12].copy_from_slice(&self.journal_seq.to_be_bytes());
        let count = self.txn_revoke_count as usize;
        let record_size = 16 + count * 4;
        revoke_data[12..16].copy_from_slice(&(record_size as u32).to_be_bytes());
        for i in 0..count {
            let offset = 16 + i * 4;
            revoke_data[offset..offset+4].copy_from_slice(&self.txn_revokes[i].to_be_bytes());
        }
        let revoke_disk_block = self.journal_block_to_disk(self.journal_pos)?;
        self.write_block_data(revoke_disk_block, &revoke_data[..bs])?;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        self.txn_revoke_count = 0;
        Ok(())
    }

    pub fn ext3_create_journal(&mut self, num_blocks: u32) -> Result<(), FsError> {
        if self.has_journal() {
            return Err(FsError::AlreadyExists);
        }
        if num_blocks < 16 {
            return Err(FsError::NoSpace);
        }
        let free = self.superblock.free_blocks_count();
        if num_blocks + 2 > free {
            return Err(FsError::NoSpace);
        }
        let now = self.get_timestamp();
        let mut j_inode = Inode::zeroed();
        j_inode.data = [0; 256];
        j_inode.set_mode(S_IFREG | 0o600);
        j_inode.set_uid(0);
        j_inode.set_gid(0);
        j_inode.set_atime(now);
        j_inode.set_ctime(now);
        j_inode.set_mtime(now);
        j_inode.set_links_count(1);
        let direct_count = num_blocks.min(12);
        for i in 0..direct_count {
            if i > 0 && i % 64 == 0 { self.sync_dirty_blocks()?; }
            let blk = self.alloc_block(0)?;
            self.zero_block(blk)?;
            j_inode.set_block(i as usize, blk);
        }
        if num_blocks > 12 {
            let ptrs_per_block = self.block_size / 4;
            let indirect_blk = self.alloc_block(0)?;
            self.zero_block(indirect_blk)?;
            j_inode.set_block(12, indirect_blk);
            let remaining = (num_blocks - 12).min(ptrs_per_block);
            for i in 0..remaining {
                if i > 0 && i % 64 == 0 { self.sync_dirty_blocks()?; }
                let blk = self.alloc_block(0)?;
                self.zero_block(blk)?;
                self.write_indirect_entry(indirect_blk, i, blk)?;
            }
            let total_disk_blocks = num_blocks + 1;
            j_inode.set_blocks(total_disk_blocks * (self.block_size / 512));
        } else {
            j_inode.set_blocks(num_blocks * (self.block_size / 512));
        }
        let journal_byte_size = num_blocks * self.block_size;
        j_inode.set_size(journal_byte_size);
        if self.inode_size() >= 128 {
            j_inode.write_u32(108, 0);
        }
        self.write_inode(EXT2_JOURNAL_INO, &j_inode)?;
        self.journal_inode_cached = None;
        self.write_journal_superblock(num_blocks)?;
        let compat = self.superblock.feature_compat();
        self.superblock.write_u32(92, compat | FEATURE_COMPAT_HAS_JOURNAL);
        self.superblock.write_u32(224, EXT2_JOURNAL_INO);
        let fs_uuid = self.superblock.uuid();
        let mut uuid_copy = [0u8; 16];
        uuid_copy.copy_from_slice(fs_uuid);
        self.superblock.data[208..224].copy_from_slice(&uuid_copy);
        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.do_write_superblock_direct()?;
        self.init_journal()?;
        Ok(())
    }

    fn do_write_superblock_direct(&mut self) -> Result<(), FsError> {
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

    fn write_journal_superblock(&mut self, num_blocks: u32) -> Result<(), FsError> {
        let j_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        let first_journal_block = j_inode.block(0);
        if first_journal_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        let bs = self.block_size as usize;
        let mut jsb_data = [0u8; 4096];
        jsb_data[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        jsb_data[4..8].copy_from_slice(&JBD_SUPERBLOCK_V2.to_be_bytes());
        jsb_data[8..12].copy_from_slice(&0u32.to_be_bytes());
        jsb_data[12..16].copy_from_slice(&self.block_size.to_be_bytes());
        jsb_data[16..20].copy_from_slice(&num_blocks.to_be_bytes());
        jsb_data[20..24].copy_from_slice(&1u32.to_be_bytes());
        jsb_data[24..28].copy_from_slice(&1u32.to_be_bytes());
        jsb_data[28..32].copy_from_slice(&0u32.to_be_bytes());
        jsb_data[32..36].copy_from_slice(&0u32.to_be_bytes());
        let fs_uuid = self.superblock.uuid();
        jsb_data[48..64].copy_from_slice(fs_uuid);
        jsb_data[64..68].copy_from_slice(&1u32.to_be_bytes());
        self.write_block_data(first_journal_block, &jsb_data[..bs])?;
        Ok(())
    }
}
