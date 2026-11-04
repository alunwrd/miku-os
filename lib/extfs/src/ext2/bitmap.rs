use crate::{FsError, MikuFS};
use crate::host::ExtHost;
use crate::structs::*;

impl<H: ExtHost> MikuFS<H> {
    /// One free block from the group's hint. A block the open transaction freed
    /// is skipped, as in `alloc_blocks`: a tree node there would sit in the
    /// block cache unpinned and could reach disk before the free commits
    pub fn alloc_block(&mut self, preferred_group: usize) -> Result<u32, FsError> {
        match self.alloc_block_once(preferred_group) {
            Err(FsError::NoSpace) if self.running.as_ref().is_some_and(|t| !t.freed.is_empty()) => {
                self.commit_midway()?;
                self.alloc_block_once(preferred_group)
            }
            r => r,
        }
    }

    fn alloc_block_once(&mut self, preferred_group: usize) -> Result<u32, FsError> {
        // Over the table's length, not `group_count`. They are equal by
        // construction, but this makes the bound checks below unreachable; this
        // loop writes a bitmap bit before the counters, so an unaccounted group
        // would leak the block
        let gc = self.groups.len();
        let bs = self.block_size as usize;
        if gc == 0 { return Err(FsError::NoSpace); }

        for offset in 0..gc {
            let group = (preferred_group + offset) % gc;

            if self.groups[group].free_blocks() == 0 {
                continue;
            }
            self.ensure_group_initialized(group)?;

            let bitmap_block = self.groups[group].block_bitmap();
            let blocks_in_group = self.blocks_per_group;
            let bytes_to_scan = (((blocks_in_group + 7) / 8) as usize).min(bs);

            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;

            let hint = self.alloc_hint[group].min(blocks_in_group);
            let start_byte = (hint / 8) as usize;

            if let Some(result) = self.bitmap_find_free(
                &mut buf, start_byte, bytes_to_scan,
                blocks_in_group, bitmap_block, group, bs,
            )? {
                return Ok(result);
            }

            if start_byte > 0 {
                if let Some(result) = self.bitmap_find_free(
                    &mut buf, 0, start_byte.min(bytes_to_scan),
                    blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(result);
                }
            }
        }

        Err(FsError::NoSpace)
    }

    /// Up to `want` free blocks in a row from the first free one after a
    /// group's hint: one bitmap write, checksum, and count update for all.
    /// Returns the first block and count, at least one, or `NoSpace`.
    ///
    /// A block the open transaction freed is skipped, as Linux skips it: its
    /// data goes straight to the device, and until the free commits a crash
    /// returns it to its old file. When only those are left, the transaction
    /// commits and the search reruns
    pub fn alloc_blocks(&mut self, preferred_group: usize, want: u32) -> Result<(u32, u32), FsError> {
        match self.alloc_blocks_once(preferred_group, want) {
            Err(FsError::NoSpace) if self.running.as_ref().is_some_and(|t| !t.freed.is_empty()) => {
                self.commit_midway()?;
                self.alloc_blocks_once(preferred_group, want)
            }
            r => r,
        }
    }

    fn alloc_blocks_once(&mut self, preferred_group: usize, want: u32) -> Result<(u32, u32), FsError> {
        let want = want.clamp(1, i16::MAX as u32);
        let gc = self.groups.len();
        let bs = self.block_size as usize;
        if gc == 0 { return Err(FsError::NoSpace); }

        for offset in 0..gc {
            let group = (preferred_group + offset) % gc;
            if self.groups[group].free_blocks() == 0 {
                continue;
            }
            self.ensure_group_initialized(group)?;

            let bitmap_block = self.groups[group].block_bitmap();
            let blocks_in_group = self.blocks_per_group.min((bs * 8) as u32);
            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;

            let base = group as u32 * self.blocks_per_group + self.superblock.first_data_block();
            let used = |buf: &[u8], bit: u32| {
                buf[(bit / 8) as usize] & (1 << (bit % 8)) != 0 || self.freed_in_running(base + bit)
            };
            // A byte at a time where the bitmap is full
            let free_from = |buf: &[u8], from: u32, to: u32| {
                let mut bit = from;
                while bit < to {
                    if bit % 8 == 0 && buf[(bit / 8) as usize] == 0xFF {
                        bit += 8;
                    } else if !used(buf, bit) {
                        return Some(bit);
                    } else {
                        bit += 1;
                    }
                }
                None
            };
            let hint = self.alloc_hint[group].min(blocks_in_group);
            let first = free_from(&buf, hint, blocks_in_group).or_else(|| free_from(&buf, 0, hint));
            let Some(first) = first else { continue };
            let mut n = 1;
            while n < want && first + n < blocks_in_group && !used(&buf, first + n) {
                n += 1;
            }
            for bit in first..first + n {
                buf[(bit / 8) as usize] |= 1 << (bit % 8);
            }
            self.write_meta_block(bitmap_block, &buf[..bs])?;
            self.alloc_hint[group] = first + n;
            self.set_block_bitmap_csum(group, &buf[..bs]);
            self.update_group_free_blocks(group, -(n as i16))?;
            self.update_superblock_free_blocks(-(n as i32))?;

            return Ok((base + first, n));
        }
        Err(FsError::NoSpace)
    }

    #[inline]
    fn bitmap_find_free(
        &mut self,
        buf: &mut [u8],
        from_byte: usize,
        to_byte: usize,
        blocks_in_group: u32,
        bitmap_block: u32,
        group: usize,
        bs: usize,
    ) -> Result<Option<u32>, FsError> {
        let mut byte_idx = from_byte;

        let aligned_start = (byte_idx + 7) & !7;
        while byte_idx < aligned_start && byte_idx < to_byte {
            if buf[byte_idx] != 0xFF {
                if let Some(r) = self.try_alloc_in_byte(
                    buf, byte_idx, blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(Some(r));
                }
            }
            byte_idx += 1;
        }

        while byte_idx + 8 <= to_byte {
            let word = u64::from_le_bytes([
                buf[byte_idx], buf[byte_idx+1], buf[byte_idx+2], buf[byte_idx+3],
                buf[byte_idx+4], buf[byte_idx+5], buf[byte_idx+6], buf[byte_idx+7],
            ]);
            if word != u64::MAX {
                for b in 0..8 {
                    if buf[byte_idx + b] != 0xFF {
                        if let Some(r) = self.try_alloc_in_byte(
                            buf, byte_idx + b, blocks_in_group, bitmap_block, group, bs,
                        )? {
                            return Ok(Some(r));
                        }
                    }
                }
            }
            byte_idx += 8;
        }

        while byte_idx < to_byte {
            if buf[byte_idx] != 0xFF {
                if let Some(r) = self.try_alloc_in_byte(
                    buf, byte_idx, blocks_in_group, bitmap_block, group, bs,
                )? {
                    return Ok(Some(r));
                }
            }
            byte_idx += 1;
        }

        Ok(None)
    }

    #[inline]
    fn try_alloc_in_byte(
        &mut self,
        buf: &mut [u8],
        byte_idx: usize,
        blocks_in_group: u32,
        bitmap_block: u32,
        group: usize,
        bs: usize,
    ) -> Result<Option<u32>, FsError> {
        let b = buf[byte_idx];
        let base = group as u32 * self.blocks_per_group + self.superblock.first_data_block();
        let Some(bit) = (0..8u32).find(|&bit| {
            b & (1 << bit) == 0 && !self.freed_in_running(base + byte_idx as u32 * 8 + bit)
        }) else {
            return Ok(None);
        };

        let bit_index = byte_idx as u32 * 8 + bit;
        if bit_index >= blocks_in_group { return Ok(None); }

        buf[byte_idx] |= 1 << bit;
        self.write_meta_block(bitmap_block, &buf[..bs])?;

        self.alloc_hint[group] = bit_index + 1;

        // From the buffer in hand, not by rereading the bitmap
        self.set_block_bitmap_csum(group, &buf[..bs]);
        self.update_group_free_blocks(group, -1)?;
        self.update_superblock_free_blocks(-1)?;

        let absolute_block = group as u32 * self.blocks_per_group
            + bit_index + self.superblock.first_data_block();

        Ok(Some(absolute_block))
    }

    pub fn free_block(&mut self, block_num: u32) -> Result<(), FsError> {
        let first = self.superblock.first_data_block();
        if block_num < first { return Err(FsError::InvalidBlock); }

        let adjusted = block_num - first;
        let group = (adjusted / self.blocks_per_group) as usize;
        let bit = adjusted % self.blocks_per_group;

        if group >= self.groups.len() {
            return Err(FsError::InvalidBlock);
        }

        let bitmap_block = self.groups[group].block_bitmap();
        // Already free: nothing to do
        if !self.set_bitmap_bit(bitmap_block, bit, false)? {
            return Ok(());
        }

        // Every freed block passes here, where a revoke is needed. Without the
        // record, a metadata block freed here and given out as file data would
        // have its old contents replayed over that data after a crash. Revoking
        // a never-logged data block costs eight bytes and suppresses nothing
        self.ext3_journal_revoke_block(block_num)?;
        self.note_freed(block_num, 1);

        if bit < self.alloc_hint[group] {
            self.alloc_hint[group] = bit;
        }

        self.update_block_bitmap_csum(group)?;
        self.update_group_free_blocks(group, 1)?;
        self.update_superblock_free_blocks(1)?;
        Ok(())
    }

    /// `count` blocks from `start` freed a group at a time: one bitmap write,
    /// checksum, and count update per group. `revoke` for blocks that may have
    /// been logged, a directory's or tree's; a regular file's data never was,
    /// and many revokes would overflow the transaction's one record
    pub fn free_blocks(&mut self, start: u32, count: u32, revoke: bool) -> Result<(), FsError> {
        let first = self.superblock.first_data_block();
        if start < first || count == 0 {
            return if count == 0 { Ok(()) } else { Err(FsError::InvalidBlock) };
        }
        let bs = self.block_size as usize;
        let per_group = self.blocks_per_group;
        let mut at = start;
        let end = start.saturating_add(count);
        while at < end {
            let adjusted = at - first;
            let group = (adjusted / per_group) as usize;
            if group >= self.groups.len() {
                return Err(FsError::InvalidBlock);
            }
            let bit0 = adjusted % per_group;
            let n = (end - at).min(per_group - bit0).min((bs * 8) as u32 - bit0.min((bs * 8) as u32));
            if n == 0 {
                return Err(FsError::InvalidBlock);
            }
            let bitmap_block = self.groups[group].block_bitmap();
            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;
            let mut cleared = 0u32;
            for bit in bit0..bit0 + n {
                let (byte, mask) = ((bit / 8) as usize, 1u8 << (bit % 8));
                if buf[byte] & mask != 0 {
                    buf[byte] &= !mask;
                    cleared += 1;
                    if revoke {
                        self.ext3_journal_revoke_block(at + (bit - bit0))?;
                    }
                }
            }
            if cleared > 0 {
                self.write_meta_block(bitmap_block, &buf[..bs])?;
                self.note_freed(at, n);
                if bit0 < self.alloc_hint[group] {
                    self.alloc_hint[group] = bit0;
                }
                self.set_block_bitmap_csum(group, &buf[..bs]);
                let mut left = cleared;
                while left > 0 {
                    let step = left.min(i16::MAX as u32);
                    self.update_group_free_blocks(group, step as i16)?;
                    left -= step;
                }
                self.update_superblock_free_blocks(cleared as i32)?;
            }
            at += n;
        }
        Ok(())
    }

    pub fn alloc_inode(&mut self, preferred_group: usize) -> Result<u32, FsError> {
        // See `alloc_block`: the bit reaches disk before the counters, so every
        // reachable group must be one the counter updates accept, which the
        // table's length guarantees
        let gc = self.groups.len();
        let bs = self.block_size as usize;
        if gc == 0 { return Err(FsError::NoSpace); }

        for offset in 0..gc {
            let group = (preferred_group + offset) % gc;
            if self.groups[group].free_inodes() == 0 { continue; }
            self.ensure_group_initialized(group)?;

            let bitmap_block = self.groups[group].inode_bitmap();
            let inodes_in_group = self.inodes_per_group;
            let bytes_to_scan = (((inodes_in_group + 7) / 8) as usize).min(bs);

            let mut buf = [0u8; 4096];
            self.read_block_into(bitmap_block, &mut buf[..bs])?;

            for byte_idx in 0..bytes_to_scan {
                let b = buf[byte_idx];
                if b == 0xFF { continue; }
                let bit = b.trailing_ones();
                if bit >= 8 { continue; }
                let bit_index = byte_idx as u32 * 8 + bit;
                if bit_index >= inodes_in_group { break; }

                buf[byte_idx] |= 1 << bit;
                self.write_meta_block(bitmap_block, &buf[..bs])?;
                self.set_inode_bitmap_csum(group, &buf[..bs]);
                self.note_inode_used(group, bit_index);
                self.update_group_free_inodes(group, -1)?;
                self.update_superblock_free_inodes(-1)?;
                let inode_num = group as u32 * self.inodes_per_group + bit_index + 1;
                return Ok(inode_num);
            }
        }

        Err(FsError::NoSpace)
    }

    pub fn free_inode(&mut self, inode_num: u32) -> Result<(), FsError> {
        if inode_num == 0 { return Err(FsError::InvalidInode); }
        let idx = inode_num - 1;
        let group = (idx / self.inodes_per_group) as usize;
        let bit = idx % self.inodes_per_group;
        if group >= self.groups.len() {
            return Err(FsError::InvalidInode);
        }
        let bitmap_block = self.groups[group].inode_bitmap();
        if !self.set_bitmap_bit(bitmap_block, bit, false)? {
            return Ok(());
        }
        self.update_inode_bitmap_csum(group)?;
        self.update_group_free_inodes(group, 1)?;
        self.update_superblock_free_inodes(1)?;
        Ok(())
    }

    /// Sets or clears one bitmap bit, reporting whether it changed. Counters
    /// may move only if it did: a crash-interrupted delete finished by the
    /// orphan pass may free a free block
    pub fn set_bitmap_bit(
        &mut self, bitmap_block: u32, bit_index: u32, value: bool,
    ) -> Result<bool, FsError> {
        let bs = self.block_size as usize;
        let byte_index = (bit_index / 8) as usize;
        let bit_offset = bit_index % 8;
        if byte_index >= bs {
            return Err(FsError::InvalidBlock);
        }
        let mut buf = [0u8; 4096];
        self.read_block_into(bitmap_block, &mut buf[..bs])?;
        let was_set = buf[byte_index] & (1 << bit_offset) != 0;
        if was_set == value {
            return Ok(false);
        }
        if value {
            buf[byte_index] |= 1 << bit_offset;
        } else {
            buf[byte_index] &= !(1 << bit_offset);
        }
        self.write_meta_block(bitmap_block, &buf[..bs])?;
        Ok(true)
    }

    pub fn update_group_free_blocks(&mut self, group: usize, delta: i16) -> Result<(), FsError> {
        if group >= self.groups.len() { return Err(FsError::InvalidBlock); }
        let current = self.groups[group].free_blocks();
        let new_val = (current as i32 + delta as i32).max(0) as u16;
        self.groups[group].write_u16(12, new_val);
        self.flush_group_desc(group)
    }

    pub fn update_group_free_inodes(&mut self, group: usize, delta: i16) -> Result<(), FsError> {
        if group >= self.groups.len() { return Err(FsError::InvalidInode); }
        let current = self.groups[group].free_inodes();
        let new_val = (current as i32 + delta as i32).max(0) as u16;
        self.groups[group].write_u16(14, new_val);
        self.flush_group_desc(group)
    }

    pub fn update_superblock_free_blocks(&mut self, delta: i32) -> Result<(), FsError> {
        let current = self.superblock.free_blocks_count();
        let new_val = (current as i64 + delta as i64).max(0) as u32;
        self.superblock.write_u32(12, new_val);
        self.flush_superblock()
    }

    pub fn update_superblock_free_inodes(&mut self, delta: i32) -> Result<(), FsError> {
        let current = self.superblock.free_inodes_count();
        let new_val = (current as i64 + delta as i64).max(0) as u32;
        self.superblock.write_u32(16, new_val);
        self.flush_superblock()
    }
}

// Lazily initialized groups
//
// mke2fs does not write the bitmaps of an unused group: it sets BLOCK_UNINIT /
// INODE_UNINIT and leaves old disk contents. Reading those as bitmaps would
// hand out occupied storage; writing back keeps the flag, so a checker still
// reads the group as uninitialized.
//
// `bg_itable_unused` counts never-touched inodes at the end of the table.
// Allocating inside that range without lowering it tells the next checker a
// live inode is in the unused area
impl<H: ExtHost> MikuFS<H> {
    /// Creates a group's bitmaps before any allocation from it
    pub fn ensure_group_initialized(&mut self, group: usize) -> Result<(), FsError> {
        if group >= self.groups.len() {
            return Ok(());
        }
        let flags = self.groups[group].flags_gd();
        if flags & (EXT4_BG_BLOCK_UNINIT | EXT4_BG_INODE_UNINIT) == 0 {
            return Ok(());
        }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];

        if flags & EXT4_BG_BLOCK_UNINIT != 0 {
            buf[..bs].fill(0);
            self.mark_group_metadata(group, &mut buf[..bs]);
            let bitmap_block = self.groups[group].block_bitmap();
            self.write_meta_block(bitmap_block, &buf[..bs])?;
            self.update_block_bitmap_csum(group)?;
        }

        if flags & EXT4_BG_INODE_UNINIT != 0 {
            buf[..bs].fill(0);
            // Everything past this group's last real inode is "in use", so it
            // is never allocated
            mark_tail(&mut buf[..bs], self.inodes_per_group);
            let bitmap_block = self.groups[group].inode_bitmap();
            self.write_meta_block(bitmap_block, &buf[..bs])?;
            self.update_inode_bitmap_csum(group)?;
        }

        self.groups[group].set_flags_gd(flags & !(EXT4_BG_BLOCK_UNINIT | EXT4_BG_INODE_UNINIT));
        self.update_group_desc_csum(group);
        self.flush_group_desc(group)
    }

    /// Sets the bits of a fresh block bitmap for the filesystem's structures in
    /// this group, plus the tail past its last block. Under flex_bg any group's
    /// bitmaps and inode table may live here, so every group's three metadata
    /// blocks are tested against this group's range
    pub(crate) fn mark_group_metadata(&self, group: usize, buf: &mut [u8]) {
        let bpg = self.blocks_per_group;
        let first = self.superblock.first_data_block() + group as u32 * bpg;
        let total = self.superblock.blocks_count();
        let last = (first + bpg).min(total);

        let mut set = |block: u32| {
            if block >= first && block < last {
                let bit = (block - first) as usize;
                if bit / 8 < buf.len() {
                    buf[bit / 8] |= 1 << (bit % 8);
                }
            }
        };

        if self.group_has_super(group as u32) {
            let gd_size = self.superblock.group_desc_size();
            let gdt_blocks = (self.groups.len() as u32 * gd_size).div_ceil(self.block_size);
            let reserved = self.superblock.reserved_gdt_blocks() as u32;
            for b in 0..1 + gdt_blocks + reserved {
                set(first + b);
            }
        }

        let itab_blocks = (self.inodes_per_group as u64
            * self.superblock.inode_size_val() as u64)
            .div_ceil(self.block_size as u64) as u32;
        for g in 0..self.groups.len() {
            set(self.groups[g].block_bitmap());
            set(self.groups[g].inode_bitmap());
            let itab = self.groups[g].inode_table();
            for b in 0..itab_blocks {
                set(itab + b);
            }
        }

        // And the tail, when the last group is short
        if last - first < bpg {
            mark_tail(buf, last - first);
        }
    }

    /// Whether this group has a superblock copy. Without SPARSE_SUPER all do;
    /// with it, only 0, 1, and powers of 3, 5, and 7
    pub(crate) fn group_has_super(&self, group: u32) -> bool {
        if !self.superblock.has_sparse_super() {
            return true;
        }
        crate::mkfs::layout::group_has_sb(group)
    }

    /// Records an inode at `index_in_group` as used, keeping the count of
    /// never-touched inodes correct
    fn note_inode_used(&mut self, group: usize, index_in_group: u32) {
        let unused = self.groups[group].itable_unused();
        let ipg = self.inodes_per_group;
        if unused == 0 || ipg < unused {
            return;
        }
        let initialized = ipg - unused;
        if index_in_group >= initialized {
            self.groups[group].set_itable_unused(ipg - (index_in_group + 1));
        }
    }
}

/// Marks every bit from `first_invalid` to the bitmap block's end as used, so
/// nothing is allocated past what the group holds
pub(crate) fn mark_tail(buf: &mut [u8], first_invalid: u32) {
    let head = first_invalid as usize / 8;
    if head >= buf.len() {
        return;
    }
    let bit = first_invalid as usize % 8;
    if bit != 0 {
        buf[head] |= 0xFFu8 << bit;
        for b in buf[head + 1..].iter_mut() {
            *b = 0xFF;
        }
    } else {
        for b in buf[head..].iter_mut() {
            *b = 0xFF;
        }
    }
}
